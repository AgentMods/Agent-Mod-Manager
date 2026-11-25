use super::{events::Events, questions::AnswerShape};
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
    answer_shape: Option<AnswerShape>,
    sender: oneshot::Sender<ApprovalResponse>,
}
#[derive(Clone)]
pub struct Approvals {
    events: Events,
    pending: Arc<Mutex<HashMap<String, Pending>>>,
    browse_install: bool,
}
impl Approvals {
    pub fn new(events: Events) -> Self {
        Self {
            events,
            pending: Arc::new(Mutex::new(HashMap::new())),
            browse_install: false,
        }
    }

    pub(super) fn for_browse_install(&self) -> Self {
        Self {
            browse_install: true,
            ..self.clone()
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
                    answer_shape: (title == "ask_user")
                        .then(|| AnswerShape::from_preview(&payload)),
                    sender: tx,
                },
            );
        let mut event = json!({"approvalId":id,"toolCallId":call,"title":title,"preview":payload});
        if self.browse_install {
            event["reviewSurface"] = json!("browseInstall");
        }
        if let Err(error) = self
            .events
            .emit(turn, conv, Some(message), "approval-request", event)
        {
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
        if !value.approved && title != "ask_user" {
            return Err(AppError::Denied("用户拒绝了此操作".into()));
        }
        Ok(value)
    }
    pub fn respond(&self, conv: &str, id: &str, mut response: ApprovalResponse) -> AppResult<()> {
        let mut map = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let p = map
            .get(id)
            .ok_or_else(|| AppError::NotFound("审批不存在、已处理或已过期".into()))?;
        if p.conversation != conv {
            return Err(AppError::Denied("审批不属于此会话".into()));
        }
        if let Some(shape) = &p.answer_shape {
            response.answer = if response.approved {
                shape.validate(&response.answer)?
            } else {
                Value::Null
            };
        }
        let p = map.remove(id).unwrap();
        p.sender.send(response).map_err(|_| AppError::Cancelled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{history::History, transaction::tests::setup};
    use std::time::Duration;

    #[tokio::test]
    async fn browse_approval_routes_live_and_replayed_requests_without_changing_chat() {
        let (_temp, engine, _) = setup();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let events = Events {
            db: engine.db.clone(),
            sink: Arc::new(move |event| sender.send(event.clone()).unwrap()),
        };
        let approvals = Approvals::new(events.clone());
        for (broker, expected) in [
            (approvals.for_browse_install(), json!("browseInstall")),
            (approvals.clone(), Value::Null),
        ] {
            let conversation = History::new(engine.db.clone())
                .create_conversation("Test", None)
                .unwrap();
            let conv = conversation.id.clone();
            let waiter = tokio::spawn(async move {
                broker
                    .request(
                        "turn",
                        &conv,
                        "msg",
                        "call",
                        "download_mod",
                        json!({"reviewSurface": "from preview"}),
                        &CancellationToken::new(),
                    )
                    .await
            });
            let live = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(live.kind, "approval-request");
            assert_eq!(live.payload["reviewSurface"], expected);
            assert_eq!(
                events.replay(&conversation.id, 0, 10).unwrap()[0].payload,
                live.payload
            );
            approvals
                .respond(
                    &conversation.id,
                    live.payload["approvalId"].as_str().unwrap(),
                    ApprovalResponse {
                        approved: true,
                        answer: Value::Null,
                    },
                )
                .unwrap();
            assert!(waiter.await.unwrap().unwrap().approved);
            assert_eq!(receiver.recv().await.unwrap().kind, "approval-resolved");
        }
    }
}
