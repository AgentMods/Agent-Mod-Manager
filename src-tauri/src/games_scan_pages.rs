//! In-process pagination for cancellable, read-only discovery workers.
use crate::{
    db::Db,
    error::{AppError, AppResult},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
        Arc, Mutex, Weak,
    },
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const IDLE: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(50);
const MAX_ACTIVE: usize = 16;
static WORKER_SLOTS: Semaphore = Semaphore::const_new(MAX_ACTIVE);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PageRequest {
    /// A continuation from the same operation and client scope.
    pub cursor: Option<Uuid>,
    /// Optional client-generated identifier, allowing cancellation before the first response.
    pub request_id: Option<Uuid>,
    pub cancel: bool,
}

type Message<T> = AppResult<(T, bool)>;

struct Session<T> {
    owner: Weak<Mutex<rusqlite::Connection>>,
    scope: String,
    cancel: CancellationToken,
    receiver: Mutex<Receiver<Message<T>>>,
    resume: SyncSender<()>,
}

pub(crate) struct Sessions<T>(Mutex<Option<HashMap<Uuid, Arc<Session<T>>>>>);

pub(crate) struct Producer<T> {
    sender: SyncSender<Message<T>>,
    resume: Receiver<()>,
    pub cancel: CancellationToken,
}

impl<T> Producer<T> {
    pub fn checkpoint(&self) -> AppResult<()> {
        if self.cancel.is_cancelled() {
            Err(AppError::Cancelled)
        } else {
            Ok(())
        }
    }

    pub fn emit(&self, page: T) -> AppResult<()> {
        self.send(Ok((page, true)))?;
        // Do not traverse or decode the next batch until the client explicitly
        // asks for it. Idle expiry closes files and removes disk spools.
        let started = Instant::now();
        loop {
            self.checkpoint()?;
            match self.resume.recv_timeout(POLL) {
                Ok(()) => return self.checkpoint(),
                Err(RecvTimeoutError::Timeout) if started.elapsed() < IDLE => continue,
                Err(_) => {
                    self.cancel.cancel();
                    return Err(AppError::Cancelled);
                }
            }
        }
    }

    fn send(&self, mut message: Message<T>) -> AppResult<()> {
        let start = Instant::now();
        loop {
            self.checkpoint()?;
            match self.sender.try_send(message) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(_)) => return Err(AppError::Cancelled),
                Err(TrySendError::Full(returned)) if start.elapsed() < IDLE => {
                    message = returned;
                    std::thread::sleep(POLL);
                }
                Err(TrySendError::Full(_)) => {
                    self.cancel.cancel();
                    return Err(AppError::Cancelled);
                }
            }
        }
    }
}

impl<T: Send + Default + 'static> Sessions<T> {
    pub const fn new() -> Self {
        Self(Mutex::new(None))
    }

    pub fn request(
        &'static self,
        db: &Db,
        scope: String,
        request: &PageRequest,
        caller_cancel: &CancellationToken,
        work: impl FnOnce(&Producer<T>) -> AppResult<T> + Send + 'static,
    ) -> AppResult<(T, Option<Uuid>)> {
        let id = request
            .cursor
            .or(request.request_id)
            .unwrap_or_else(Uuid::new_v4);
        let session = {
            let mut state = self.0.lock().unwrap();
            let sessions = state.get_or_insert_with(HashMap::new);
            sessions.retain(|_, item| !item.cancel.is_cancelled());
            if let Some(item) = sessions.get(&id) {
                if !item.owner.ptr_eq(&Arc::downgrade(&db.0)) || item.scope != scope {
                    return Err(AppError::NotFound("分页任务不存在或已结束".into()));
                }
                if request.cancel {
                    item.cancel.cancel();
                    sessions.remove(&id);
                    return Ok((T::default(), None));
                }
                if request.cursor.is_none() {
                    return Err(AppError::Conflict("分页任务已存在".into()));
                }
                item.clone()
            } else {
                if request.cancel {
                    return Ok((T::default(), None));
                }
                if request.cursor.is_some() {
                    return Err(AppError::NotFound(
                        "分页任务不存在或已结束，请重新开始".into(),
                    ));
                }
                if sessions.len() >= MAX_ACTIVE {
                    return Err(AppError::Conflict(
                        "运行中的分页任务过多，请取消旧任务后重试".into(),
                    ));
                }
                let slot = WORKER_SLOTS.try_acquire().map_err(|_| {
                    AppError::Conflict("运行中的分页任务过多，请取消旧任务后重试".into())
                })?;
                let (sender, receiver) = mpsc::sync_channel(0);
                let (resume, permit) = mpsc::sync_channel(1);
                let item = Arc::new(Session {
                    owner: Arc::downgrade(&db.0),
                    scope,
                    cancel: CancellationToken::new(),
                    receiver: Mutex::new(receiver),
                    resume,
                });
                let producer = Producer {
                    sender,
                    resume: permit,
                    cancel: item.cancel.clone(),
                };
                std::thread::Builder::new()
                    .name("diagnostic-page".into())
                    .spawn(move || {
                        let _slot = slot;
                        let result = work(&producer).map(|page| (page, false));
                        let _ = producer.send(result);
                    })?;
                sessions.insert(id, item.clone());
                item
            }
        };
        let result = {
            let receiver = session
                .receiver
                .try_lock()
                .map_err(|_| AppError::Conflict("此分页任务已在读取下一页".into()))?;
            if request.cursor.is_some() && !caller_cancel.is_cancelled() {
                // This thread owns the receive lock, so only one waiter can
                // consume the requested page.
                let _ = session.resume.try_send(());
            }
            loop {
                if caller_cancel.is_cancelled() || session.cancel.is_cancelled() {
                    break Err(AppError::Cancelled);
                }
                match receiver.recv_timeout(POLL) {
                    Ok(result) => break result,
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break Err(AppError::Cancelled),
                }
            }
        };
        if !matches!(result, Ok((_, true))) {
            session.cancel.cancel();
            let mut state = self.0.lock().unwrap();
            if let Some(sessions) = state.as_mut() {
                if sessions
                    .get(&id)
                    .is_some_and(|item| Arc::ptr_eq(item, &session))
                {
                    sessions.remove(&id);
                }
            }
        }
        result.map(|(page, more)| (page, more.then_some(id)))
    }
}
