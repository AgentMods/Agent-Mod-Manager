use crate::{
    db::Db,
    error::{AppError, AppResult},
    util,
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub seq: i64,
    pub turn_id: String,
    pub conversation_id: String,
    pub message_id: Option<String>,
    pub kind: String,
    pub payload: Value,
    pub created_at: String,
}
pub type EventSink = Arc<dyn Fn(&AgentEvent) + Send + Sync>;
#[derive(Clone)]
pub struct Events {
    pub db: Db,
    pub sink: EventSink,
}
impl Events {
    pub fn emit(
        &self,
        turn: &str,
        conv: &str,
        message: Option<&str>,
        kind: &str,
        payload: Value,
    ) -> AppResult<AgentEvent> {
        let event = Self::record(&self.db.conn(), turn, conv, message, kind, payload)?;
        (self.sink)(&event);
        Ok(event)
    }

    pub fn reset_reply(&self, turn: &str, conv: &str, message: &str) -> AppResult<()> {
        let event = {
            let mut conn = self.db.conn();
            let tx = conn.transaction()?;
            let changed = tx.execute(
                "UPDATE messages SET content='', reasoning=NULL, token_count=0 WHERE id=?1 AND conversation_id=?2 AND role='assistant'",
                params![message, conv],
            )?;
            if changed == 0 {
                return Err(AppError::NotFound("助理消息".into()));
            }
            let event = Self::record(
                &tx,
                turn,
                conv,
                Some(message),
                "reply-reset",
                serde_json::json!({}),
            )?;
            tx.commit()?;
            event
        };
        (self.sink)(&event);
        Ok(())
    }

    fn record(
        conn: &Connection,
        turn: &str,
        conv: &str,
        message: Option<&str>,
        kind: &str,
        payload: Value,
    ) -> AppResult<AgentEvent> {
        let created_at = util::now();
        let changed = conn.execute("INSERT INTO agent_events (turn_id,conversation_id,message_id,kind,payload_json,created_at) SELECT ?1,?2,?3,?4,?5,?6 WHERE EXISTS (SELECT 1 FROM conversations WHERE id=?2)",params![turn,conv,message,kind,serde_json::to_string(&payload)?,created_at])?;
        if changed == 0 {
            return Err(AppError::NotFound("会话".into()));
        }
        Ok(AgentEvent {
            seq: conn.last_insert_rowid(),
            turn_id: turn.into(),
            conversation_id: conv.into(),
            message_id: message.map(String::from),
            kind: kind.into(),
            payload,
            created_at,
        })
    }
    pub fn replay(&self, conv: &str, after: i64, limit: usize) -> AppResult<Vec<AgentEvent>> {
        if after < 0 || limit == 0 || limit > 2000 {
            return Err(AppError::Invalid("事件重放参数无效".into()));
        }
        let conn = self.db.conn();
        let mut stmt=conn.prepare("SELECT seq,turn_id,conversation_id,message_id,kind,payload_json,created_at FROM agent_events WHERE conversation_id=?1 AND seq>?2 ORDER BY seq LIMIT ?3")?;
        let rows = stmt.query_map(params![conv, after, limit], |r| {
            let payload: String = r.get(5)?;
            let payload = serde_json::from_str(&payload).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            Ok(AgentEvent {
                seq: r.get(0)?,
                turn_id: r.get(1)?,
                conversation_id: r.get(2)?,
                message_id: r.get(3)?,
                kind: r.get(4)?,
                payload,
                created_at: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}
