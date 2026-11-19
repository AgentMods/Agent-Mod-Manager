use super::events::Events;
use crate::{
    error::{AppError, AppResult},
    util,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResponse {
    pub approved: bool,
    #[serde(default)]
    pub answer: Value,
}
struct Pending {
    conversation: String,
    sender: oneshot::Sender<ApprovalResponse>,
}
#[derive(Clone)]
pub struct Approvals {
    events: Events,
    pending: Arc<Mutex<HashMap<String, Pending>>>,
}
impl Approvals {
    pub fn new(events: Events) -> Self {
        Self {
            events,
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn request(
        &self,
        turn: &str,
        conv: &str,
        message: &str,
        call: &str,
        title: &str,
        payload: Value,
        cancel: &CancellationToken,
    ) -> AppResult<ApprovalResponse> {
        let id = util::new_id("approval");
        let (tx, rx) = oneshot::channel();
        self.events.db.conn().execute("INSERT INTO approvals (id,conversation_id,tool_call_id,kind,title,payload_json,status,created_at) VALUES (?1,?2,?3,'tool',?4,?5,'pending',?6)",params![id,conv,call,title,serde_json::to_string(&payload)?,util::now()])?;
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                id.clone(),
                Pending {
                    conversation: conv.into(),
                    sender: tx,
                },
            );
        if let Err(error) = self.events.emit(
            turn,
            conv,
            Some(message),
            "approval-request",
            json!({"approvalId":id,"toolCallId":call,"title":title,"preview":payload}),
        ) {
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            return Err(error);
        }
        let response = tokio::select! {
            biased;
            _=cancel.cancelled()=>Err(AppError::Cancelled),
            value=rx=>value.map_err(|_|AppError::Cancelled),
            _=tokio::time::sleep(std::time::Duration::from_secs(1800))=>Err(AppError::Denied("审批已过期".into())),
        };
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        let status = match &response {
            Ok(r) if r.approved => "approved",
            Ok(_) => "rejected",
            Err(_) => "cancelled",
        };
        self.events.db.conn().execute(
            "UPDATE approvals SET status=?1,resolved_at=?2 WHERE id=?3",
            params![status, util::now(), id],
        )?;
        self.events.emit(
            turn,
            conv,
            Some(message),
            "approval-resolved",
            json!({"approvalId":id,"status":status}),
        )?;
        let value = response?;
        if !value.approved {
            return Err(AppError::Denied("用户拒绝了此操作".into()));
        }
        Ok(value)
    }
    pub fn respond(&self, conv: &str, id: &str, response: ApprovalResponse) -> AppResult<()> {
        let mut map = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let p = map
            .get(id)
            .ok_or_else(|| AppError::NotFound("审批不存在、已处理或已过期".into()))?;
        if p.conversation != conv {
            return Err(AppError::Denied("审批不属于此会话".into()));
        }
        let p = map.remove(id).unwrap();
        p.sender.send(response).map_err(|_| AppError::Cancelled)
    }
}
