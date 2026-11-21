//! Durable chat history. Message insertion order is independent of clock changes.

use crate::{
    agent::{attachments::user_content, Attachment},
    db::Db,
    error::{AppError, AppResult},
    settings::{conversation_permission_key, validate_permission_mode},
    util::{new_id, now},
};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const DEFAULT_TITLE: &str = "新对话";
const TITLE_LIMIT: usize = 60;

fn first_message_title(content: &str) -> String {
    let line = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let line = line.trim_start_matches(['#', '>', ' ']);
    let topic = [
        "请帮我",
        "麻烦帮我",
        "能不能帮我",
        "帮我",
        "我想要",
        "我想",
        "请",
    ]
    .iter()
    .find_map(|prefix| line.strip_prefix(*prefix))
    .unwrap_or(line);
    let sentence = topic
        .trim_start_matches([' ', '，', ',', '：', ':'])
        .split(['。', '！', '？', '!', '?', '；', ';'])
        .next()
        .unwrap_or_default()
        .trim();
    let subject = if sentence.is_empty() { line } else { sentence };
    let clean = subject
        .split(|c: char| c.is_control() || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if clean.is_empty() {
        return DEFAULT_TITLE.into();
    }
    truncate_title(&clean)
}

fn truncate_title(title: &str) -> String {
    let mut chars = title.chars();
    let mut title: String = chars.by_ref().take(TITLE_LIMIT).collect();
    if chars.next().is_some() {
        title.pop();
        title.push('…');
    }
    title
}

fn message_title(content: &str, attachments: &[Attachment]) -> String {
    if content.trim().is_empty() {
        if let Some(first) = attachments.first() {
            return truncate_title(first.name());
        }
    }
    first_message_title(content)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub game_id: Option<String>,
    pub task_summary: Option<String>,
    pub archived: bool,
    pub pinned: bool,
    pub message_count: i64,
    pub created_at: String,
    pub updated_at: String,
    /// None inherits the global setting. This value lives in settings, not schema.
    pub permission_mode: Option<String>,
    /// List-only checkpoint; the frontend layers more recent live events over it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_status: Option<ConversationAgentStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationAgentStatus {
    turn_id: Option<String>,
    turn_status: Option<String>,
    event_seq: i64,
    pending: Option<PendingAgentInteraction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct PendingAgentInteraction {
    approval_id: String,
    status: String,
}

/// Persistence/UI DTO. Use `to_openai` to omit local metadata from a request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub id: String,
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    pub reasoning: Option<String>,
    pub tool_calls: Option<Value>,
    pub tool_call_id: Option<String>,
}

impl Message {
    /// Reasoning is kept locally; it is not portable across OpenAI providers.
    pub fn to_openai(&self) -> Value {
        let mut value = json!({ "role": self.role });
        value["content"] = if self.role == "user" {
            user_content(&self.content, &self.attachments)
        } else {
            json!(self.content)
        };
        if let Some(calls) = &self.tool_calls {
            value["tool_calls"] = calls.clone();
        }
        if let Some(id) = &self.tool_call_id {
            value["tool_call_id"] = Value::String(id.clone());
        }
        value
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationDetail {
    pub conversation: Conversation,
    pub messages: Vec<Message>,
}

/// Exact OpenAI function-call shape. Arguments remain a string during streaming.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallRecord {
    pub id: String,
    pub conversation_id: String,
    pub message_id: Option<String>,
    pub name: String,
    pub arguments: Value,
    pub result: Option<Value>,
    pub status: String,
    pub approval_status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub duration_ms: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolCallUpdate {
    /// pending/running/awaiting_approval, or terminal done/error/completed/
    /// succeeded/failed/cancelled/denied.
    pub status: String,
    pub approval_status: String,
    pub result: Option<Value>,
    pub error: Option<String>,
}

impl Default for ToolCallUpdate {
    fn default() -> Self {
        Self {
            status: "pending".into(),
            approval_status: "none".into(),
            result: None,
            error: None,
        }
    }
}

#[derive(Clone)]
pub struct History {
    pub db: Db,
}

impl History {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub fn create_conversation(
        &self,
        title: &str,
        game_id: Option<&str>,
    ) -> AppResult<Conversation> {
        let id = new_id("conv");
        let timestamp = now();
        let title = title.trim();
        let auto_title_pending = title.is_empty();
        let title = if auto_title_pending {
            DEFAULT_TITLE
        } else {
            title
        };
        let conn = self.db.conn();
        conn.execute(
            "INSERT INTO conversations(id,title,game_id,created_at,updated_at,auto_title_pending) VALUES(?1,?2,?3,?4,?4,?5)",
            params![id, title, game_id, timestamp, auto_title_pending],
        )?;
        read_conversation(&conn, &id)
    }

    pub fn list_conversations(&self, archived: bool) -> AppResult<Vec<Conversation>> {
        let conn = self.db.conn();
        let mut stmt = conn.prepare(
            "WITH latest_turns AS (
                SELECT conversation_id, MAX(rowid) turn_row FROM agent_turns GROUP BY conversation_id
             )
             SELECT c.id,c.title,c.game_id,c.task_summary,c.archived,c.pinned,c.message_count,c.created_at,c.updated_at,
                    t.id AS turn_id,t.status AS turn_status,
                    COALESCE((SELECT MAX(seq) FROM agent_events WHERE conversation_id=c.id),0) AS event_seq
             FROM conversations c
             LEFT JOIN latest_turns lt ON lt.conversation_id=c.id
             LEFT JOIN agent_turns t ON t.rowid=lt.turn_row
             WHERE c.archived=?1 ORDER BY c.pinned DESC,c.updated_at DESC,c.rowid DESC",
        )?;
        let conversations = stmt.query_map([archived], |row| {
            let mut conversation = conversation_from_row(row)?;
            conversation.agent_status = Some(ConversationAgentStatus {
                turn_id: row.get(9)?,
                turn_status: row.get(10)?,
                event_seq: row.get(11)?,
                pending: None,
            });
            Ok(conversation)
        })?;
        let mut result = Vec::new();
        for item in conversations {
            let mut conversation = item?;
            conversation.permission_mode = read_permission_mode(&conn, &conversation.id)?;
            if let Some(status) = conversation
                .agent_status
                .as_mut()
                .filter(|s| s.turn_status.as_deref() == Some("running"))
            {
                // The runtime waits for one interaction at a time. Require both a
                // published request for this turn and a still-pending approval row.
                status.pending = conn
                    .query_row(
                        "SELECT a.id, CASE
                            WHEN a.title='ask_user' THEN 'waiting-answer'
                            ELSE 'waiting-approval' END
                         FROM agent_events e JOIN approvals a ON a.id=json_extract(e.payload_json,'$.approvalId')
                         WHERE e.conversation_id=?1 AND e.turn_id=?2 AND e.kind='approval-request'
                           AND a.conversation_id=?1 AND a.status='pending'
                         ORDER BY e.seq DESC LIMIT 1",
                        params![conversation.id, status.turn_id],
                        |row| {
                            Ok(PendingAgentInteraction {
                                approval_id: row.get(0)?,
                                status: row.get(1)?,
                            })
                        },
                    )
                    .optional()?;
            }
            result.push(conversation);
        }
        Ok(result)
    }

    pub fn get_conversation(&self, id: &str) -> AppResult<ConversationDetail> {
        let mut conn = self.db.conn();
        let tx = conn.transaction()?;
        let conversation = read_conversation(&tx, id)?;
        let messages = read_messages(&tx, id)?;
        tx.commit()?;
        Ok(ConversationDetail {
            conversation,
            messages,
        })
    }

    /// Snapshot the context before the selected user turn. A retry targets an
    /// assistant message and uses the nearest preceding user message as input.
    pub(crate) fn fork_before_message(
        &self,
        source: &str,
        message_id: &str,
        edited_content: Option<String>,
        edited_attachments: Option<Vec<Attachment>>,
    ) -> AppResult<(String, String, Vec<Attachment>)> {
        let mut conn = self.db.conn();
        let tx = conn.transaction()?;
        let conversation = read_conversation(&tx, source)?;
        if conversation.archived {
            return Err(AppError::Denied("请先恢复归档会话".into()));
        }
        let (target_row, role, original): (i64, String, String) = tx
            .query_row(
                "SELECT rowid,role,content FROM messages WHERE conversation_id=?1 AND id=?2",
                params![source, message_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
            .ok_or_else(|| AppError::NotFound("消息".into()))?;
        let (cutoff, content) = match (role.as_str(), edited_content) {
            ("user", Some(content)) => (target_row, content),
            ("user", None) => (target_row, original),
            ("assistant", None) => tx
                .query_row(
                    "SELECT rowid,content FROM messages
                     WHERE conversation_id=?1 AND role='user' AND rowid<?2 ORDER BY rowid DESC LIMIT 1",
                    params![source, target_row],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
                .ok_or_else(|| AppError::NotFound("原始提问".into()))?,
            _ => return Err(AppError::Invalid("只能编辑用户消息".into())),
        };
        let attachments: Vec<Attachment> = match edited_attachments {
            Some(attachments) => attachments,
            None => tx.query_row(
                "SELECT attachments_json FROM messages WHERE rowid=?1",
                [cutoff],
                |row| Ok(serde_column(row, 0)?.unwrap_or_default()),
            )?,
        };
        let branch = new_id("conv");
        let timestamp = now();
        tx.execute(
            "INSERT INTO conversations(id,title,game_id,created_at,updated_at) VALUES(?1,?2,?3,?4,?4)",
            params![branch, message_title(&content, &attachments), conversation.game_id, timestamp],
        )?;
        let copied = tx.execute(
            "INSERT INTO messages(id,conversation_id,role,content,reasoning,tool_calls_json,tool_call_id,name,token_count,created_at,attachments_json)
             SELECT ?1 || ':' || id,?1,role,content,reasoning,tool_calls_json,tool_call_id,name,token_count,created_at,attachments_json
             FROM messages WHERE conversation_id=?2 AND rowid<?3 ORDER BY rowid",
            params![branch, source, cutoff],
        )?;
        tx.execute(
            "UPDATE conversations SET message_count=?2 WHERE id=?1",
            params![branch, copied],
        )?;
        tx.execute(
            "INSERT INTO settings(key,value) SELECT ?1,value FROM settings WHERE key=?2",
            params![
                conversation_permission_key(&branch),
                conversation_permission_key(source)
            ],
        )?;
        // Compute membership once per conversation. A correlated lookup for
        // the terminal event would rescan thousands of deltas per copied row
        // while holding the app's single database connection.
        tx.execute(
            "WITH prior_turns AS (
                 SELECT e.turn_id FROM agent_events e JOIN messages m ON m.id=e.message_id
                 WHERE e.conversation_id=?2 AND m.conversation_id=?2 AND m.rowid<?3
             ), finished AS (
                 SELECT turn_id FROM agent_events WHERE conversation_id=?2 AND kind='done'
             ), answers AS (
                 SELECT turn_id, json_extract(payload_json,'$.approvalId') FROM agent_events
                 WHERE conversation_id=?2 AND kind='approval-resolved'
             )
             INSERT INTO agent_events(turn_id,conversation_id,message_id,kind,payload_json,created_at)
             SELECT ?1 || ':' || e.turn_id,?1,?1 || ':' || e.message_id,e.kind,e.payload_json,e.created_at
             FROM agent_events e WHERE e.conversation_id=?2
               AND e.turn_id IN prior_turns AND e.turn_id IN finished
               AND (e.kind<>'approval-request' OR (e.turn_id, json_extract(e.payload_json,'$.approvalId')) IN answers)
             ORDER BY e.seq",
            params![branch, source, cutoff],
        )?;
        tx.commit()?;
        Ok((branch, content, attachments))
    }

    pub fn archive_conversation(&self, id: &str, archived: bool) -> AppResult<()> {
        let changed = self.db.conn().execute(
            "UPDATE conversations SET archived=?2,updated_at=?3 WHERE id=?1",
            params![id, archived, now()],
        )?;
        require_changed(changed, "会话")
    }

    pub fn rename_conversation(&self, id: &str, title: &str) -> AppResult<()> {
        let title = title.trim();
        if title.is_empty()
            || title.chars().count() > TITLE_LIMIT
            || title.chars().any(char::is_control)
        {
            return Err(AppError::Invalid(
                "对话名称必须为 1–60 个字符，且不能包含控制字符".into(),
            ));
        }
        let changed = self.db.conn().execute(
            "UPDATE conversations SET title=?2,auto_title_pending=0 WHERE id=?1",
            params![id, title],
        )?;
        require_changed(changed, "会话")
    }

    pub fn set_conversation_pinned(&self, id: &str, pinned: bool) -> AppResult<()> {
        let changed = self.db.conn().execute(
            "UPDATE conversations SET pinned=?2 WHERE id=?1",
            params![id, pinned],
        )?;
        require_changed(changed, "会话")
    }

    pub(crate) fn delete_conversation(&self, id: &str) -> AppResult<()> {
        let mut conn = self.db.conn();
        let tx = conn.transaction()?;
        ensure_conversation(&tx, id)?;
        let busy: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_turns WHERE conversation_id=?1 AND status='running')
                 OR EXISTS(SELECT 1 FROM tool_calls WHERE conversation_id=?1 AND status='running')
                 OR EXISTS(SELECT 1 FROM approvals WHERE conversation_id=?1 AND status='pending')",
            [id],
            |row| row.get(0),
        )?;
        if busy {
            return Err(AppError::Conflict(
                "请先结束此对话中的任务，再删除对话".into(),
            ));
        }
        for sql in [
            "DELETE FROM agent_events WHERE conversation_id=?1",
            "DELETE FROM approvals WHERE conversation_id=?1",
            "DELETE FROM tool_calls WHERE conversation_id=?1",
            "DELETE FROM agent_turns WHERE conversation_id=?1",
        ] {
            tx.execute(sql, [id])?;
        }
        tx.execute(
            "DELETE FROM settings WHERE key=?1",
            [conversation_permission_key(id)],
        )?;
        // Messages cascade. The file transaction ledger remains available for recovery.
        tx.execute("DELETE FROM conversations WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_summary(&self, conversation_id: &str, summary: &str) -> AppResult<()> {
        let changed = self.db.conn().execute(
            "UPDATE conversations SET task_summary=?2,updated_at=?3 WHERE id=?1",
            params![conversation_id, summary, now()],
        )?;
        require_changed(changed, "会话")
    }

    /// None removes just the override and restores inheritance.
    pub fn set_permission_mode(&self, conversation_id: &str, mode: Option<&str>) -> AppResult<()> {
        if let Some(mode) = mode {
            validate_permission_mode(mode)?;
        }
        let mut conn = self.db.conn();
        let tx = conn.transaction()?;
        ensure_conversation(&tx, conversation_id)?;
        let key = conversation_permission_key(conversation_id);
        if let Some(mode) = mode {
            tx.execute(
                "INSERT INTO settings(key,value) VALUES(?1,?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, serde_json::to_string(mode)?],
            )?;
        } else {
            tx.execute("DELETE FROM settings WHERE key=?1", [key])?;
        }
        tx.execute(
            "UPDATE conversations SET updated_at=?2 WHERE id=?1",
            params![conversation_id, now()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn append_message(
        &self,
        conversation_id: &str,
        role: &str,
        content: &str,
        tool_calls: Option<Value>,
        tool_call_id: Option<&str>,
    ) -> AppResult<Message> {
        self.append(
            conversation_id,
            role,
            content,
            tool_calls,
            tool_call_id,
            Vec::new(),
        )
    }

    pub(crate) fn append_user_message(
        &self,
        conversation_id: &str,
        content: &str,
        attachments: Vec<Attachment>,
    ) -> AppResult<Message> {
        self.append(conversation_id, "user", content, None, None, attachments)
    }

    fn append(
        &self,
        conversation_id: &str,
        role: &str,
        content: &str,
        tool_calls: Option<Value>,
        tool_call_id: Option<&str>,
        attachments: Vec<Attachment>,
    ) -> AppResult<Message> {
        validate_message(role, tool_calls.as_ref(), tool_call_id)?;
        let message = Message {
            id: new_id("msg"),
            role: role.into(),
            content: content.into(),
            attachments,
            reasoning: None,
            tool_calls,
            tool_call_id: tool_call_id.map(str::to_owned),
        };
        let attachments_json = (!message.attachments.is_empty())
            .then(|| serde_json::to_string(&message.attachments))
            .transpose()?;
        let timestamp = now();
        let mut conn = self.db.conn();
        let tx = conn.transaction()?;
        ensure_conversation(&tx, conversation_id)?;
        tx.execute(
            "INSERT INTO messages(id,conversation_id,role,content,tool_calls_json,tool_call_id,created_at,attachments_json)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![message.id, conversation_id, role, content,
                message.tool_calls.as_ref().map(serde_json::to_string).transpose()?, tool_call_id, timestamp, attachments_json],
        )?;
        tx.execute(
            "UPDATE conversations SET message_count=message_count+1,updated_at=?2 WHERE id=?1",
            params![conversation_id, timestamp],
        )?;
        if role == "user" && (!content.trim().is_empty() || !message.attachments.is_empty()) {
            tx.execute(
                "UPDATE conversations SET title=?2,auto_title_pending=0 WHERE id=?1 AND auto_title_pending=1",
                params![conversation_id, message_title(content, &message.attachments)],
            )?;
        }
        tx.commit()?;
        Ok(message)
    }

    /// Replace the accumulated stream snapshot. None preserves prior reasoning.
    pub fn update_message(
        &self,
        id: &str,
        content: &str,
        reasoning: Option<&str>,
    ) -> AppResult<()> {
        let mut conn = self.db.conn();
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "UPDATE messages SET content=?2,reasoning=COALESCE(?3,reasoning) WHERE id=?1",
            params![id, content, reasoning],
        )?;
        require_changed(changed, "消息")?;
        touch_message_conversation(&tx, id)?;
        tx.commit()?;
        Ok(())
    }

    /// Append deltas atomically for callers that do not keep an accumulated buffer.
    pub fn append_message_delta(
        &self,
        id: &str,
        content_delta: &str,
        reasoning_delta: Option<&str>,
    ) -> AppResult<()> {
        let mut conn = self.db.conn();
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "UPDATE messages SET content=content || ?2,
             reasoning=CASE WHEN ?3 IS NULL THEN reasoning ELSE COALESCE(reasoning,'') || ?3 END WHERE id=?1",
            params![id, content_delta, reasoning_delta],
        )?;
        require_changed(changed, "消息")?;
        touch_message_conversation(&tx, id)?;
        tx.commit()?;
        Ok(())
    }

    pub fn update_tool_calls(&self, message_id: &str, tool_calls: Value) -> AppResult<()> {
        validate_tool_calls(&tool_calls)?;
        let mut conn = self.db.conn();
        let tx = conn.transaction()?;
        let role: String = tx
            .query_row("SELECT role FROM messages WHERE id=?1", [message_id], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or_else(|| AppError::NotFound("消息".into()))?;
        if role != "assistant" {
            return Err(AppError::Invalid("工具调用只能属于 assistant 消息".into()));
        }
        tx.execute(
            "UPDATE messages SET tool_calls_json=?2 WHERE id=?1",
            params![message_id, serde_json::to_string(&tool_calls)?],
        )?;
        touch_message_conversation(&tx, message_id)?;
        tx.commit()?;
        Ok(())
    }

    pub fn replay_messages(&self, conversation_id: &str) -> AppResult<Vec<Value>> {
        Ok(self
            .get_conversation(conversation_id)?
            .messages
            .iter()
            .map(Message::to_openai)
            .collect())
    }

    /// Upsert execution state; message payloads are set separately by update_tool_calls.
    /// IDs cannot be rebound across conversations/messages. Existing start times persist.
    pub fn set_tool_call(
        &self,
        conversation_id: &str,
        message_id: Option<&str>,
        call: &ToolCall,
        update: ToolCallUpdate,
    ) -> AppResult<ToolCallRecord> {
        validate_call(call)?;
        validate_tool_update(&update)?;
        let timestamp = now();
        // Preserve incomplete streamed arguments as a JSON string until parsable.
        let arguments: Value = serde_json::from_str(&call.function.arguments)
            .unwrap_or_else(|_| Value::String(call.function.arguments.clone()));
        let mut conn = self.db.conn();
        let tx = conn.transaction()?;
        ensure_conversation(&tx, conversation_id)?;
        if let Some(id) = message_id {
            let owner: Option<(String, String)> = tx
                .query_row(
                    "SELECT conversation_id,role FROM messages WHERE id=?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            match owner {
                None => return Err(AppError::NotFound("工具调用消息".into())),
                Some((owner, role)) if owner != conversation_id || role != "assistant" => {
                    return Err(AppError::Invalid(
                        "工具调用必须关联本会话的 assistant 消息".into(),
                    ))
                }
                _ => {}
            }
        }
        let existing = tx.query_row("SELECT id,conversation_id,message_id,name,arguments_json,result_json,status,approval_status,started_at,finished_at,duration_ms,error FROM tool_calls WHERE id=?1", [&call.id], tool_call_from_row).optional()?;
        if let Some(existing) = &existing {
            if existing.conversation_id != conversation_id
                || (message_id.is_some()
                    && existing.message_id.is_some()
                    && existing.message_id.as_deref() != message_id)
                || existing.name != call.function.name
            {
                return Err(AppError::Conflict("工具调用 ID 已被使用".into()));
            }
            if existing.finished_at.is_some() && !is_terminal(&update.status) {
                return Err(AppError::Conflict("已结束的工具调用不能重新运行".into()));
            }
        }
        let started_at = existing
            .as_ref()
            .map(|record| record.started_at.as_str())
            .unwrap_or(&timestamp);
        let finished_at = if is_terminal(&update.status) {
            Some(
                existing
                    .as_ref()
                    .and_then(|record| record.finished_at.as_deref())
                    .unwrap_or(&timestamp),
            )
        } else {
            None
        };
        let duration_ms = finished_at.and_then(|finished| {
            let start = chrono::DateTime::parse_from_rfc3339(started_at).ok()?;
            let finish = chrono::DateTime::parse_from_rfc3339(finished).ok()?;
            Some((finish - start).num_milliseconds().max(0))
        });
        tx.execute(
            "INSERT INTO tool_calls(id,conversation_id,message_id,name,arguments_json,result_json,status,approval_status,started_at,finished_at,duration_ms,error)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
             ON CONFLICT(id) DO UPDATE SET message_id=COALESCE(excluded.message_id,tool_calls.message_id),
             arguments_json=excluded.arguments_json,result_json=excluded.result_json,status=excluded.status,
             approval_status=excluded.approval_status,finished_at=excluded.finished_at,
             duration_ms=excluded.duration_ms,error=excluded.error",
            params![call.id,conversation_id,message_id,call.function.name,serde_json::to_string(&arguments)?,
                update.result.as_ref().map(serde_json::to_string).transpose()?,update.status,update.approval_status,
                started_at,finished_at,duration_ms,update.error],
        )?;
        tx.execute(
            "UPDATE conversations SET updated_at=?2 WHERE id=?1",
            params![conversation_id, timestamp],
        )?;
        let record = tx.query_row("SELECT id,conversation_id,message_id,name,arguments_json,result_json,status,approval_status,started_at,finished_at,duration_ms,error FROM tool_calls WHERE id=?1", [&call.id], tool_call_from_row)?;
        tx.commit()?;
        Ok(record)
    }

    pub fn list_tool_calls(&self, conversation_id: &str) -> AppResult<Vec<ToolCallRecord>> {
        let conn = self.db.conn();
        ensure_conversation(&conn, conversation_id)?;
        let mut stmt = conn.prepare(
            "SELECT id,conversation_id,message_id,name,arguments_json,result_json,status,approval_status,started_at,finished_at,duration_ms,error
             FROM tool_calls WHERE conversation_id=?1 ORDER BY rowid",
        )?;
        let result = stmt
            .query_map([conversation_id], tool_call_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(result)
    }
}

fn conversation_from_row(row: &Row<'_>) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: row.get(0)?,
        title: row.get(1)?,
        game_id: row.get(2)?,
        task_summary: row.get(3)?,
        archived: row.get(4)?,
        pinned: row.get(5)?,
        message_count: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        permission_mode: None,
        agent_status: None,
    })
}

fn read_conversation(conn: &Connection, id: &str) -> AppResult<Conversation> {
    let mut conversation = conn.query_row(
        "SELECT id,title,game_id,task_summary,archived,pinned,message_count,created_at,updated_at FROM conversations WHERE id=?1",
        [id], conversation_from_row,
    ).optional()?.ok_or_else(|| AppError::NotFound("会话".into()))?;
    conversation.permission_mode = read_permission_mode(conn, id)?;
    Ok(conversation)
}

fn read_permission_mode(conn: &Connection, id: &str) -> AppResult<Option<String>> {
    let encoded: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key=?1",
            [conversation_permission_key(id)],
            |r| r.get(0),
        )
        .optional()?;
    encoded
        .map(|encoded| {
            let mode: String = serde_json::from_str(&encoded)?;
            validate_permission_mode(&mode)?;
            Ok(mode)
        })
        .transpose()
}

fn read_messages(conn: &Connection, id: &str) -> AppResult<Vec<Message>> {
    // created_at has millisecond precision; UUIDs are random. Neither is sequence.
    let mut stmt = conn.prepare("SELECT id,role,content,reasoning,tool_calls_json,tool_call_id,attachments_json FROM messages WHERE conversation_id=?1 ORDER BY rowid")?;
    let result = stmt
        .query_map([id], |row| {
            Ok(Message {
                id: row.get(0)?,
                role: row.get(1)?,
                content: row.get(2)?,
                reasoning: row.get(3)?,
                tool_calls: json_column(row, 4)?,
                tool_call_id: row.get(5)?,
                attachments: serde_column(row, 6)?.unwrap_or_default(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(result)
}

fn tool_call_from_row(row: &Row<'_>) -> rusqlite::Result<ToolCallRecord> {
    Ok(ToolCallRecord {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        message_id: row.get(2)?,
        name: row.get(3)?,
        arguments: json_column(row, 4)?.unwrap_or(Value::Null),
        result: json_column(row, 5)?,
        status: row.get(6)?,
        approval_status: row.get(7)?,
        started_at: row.get(8)?,
        finished_at: row.get(9)?,
        duration_ms: row.get(10)?,
        error: row.get(11)?,
    })
}

fn json_column(row: &Row<'_>, index: usize) -> rusqlite::Result<Option<Value>> {
    serde_column(row, index)
}

fn serde_column<T: serde::de::DeserializeOwned>(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<T>> {
    let encoded: Option<String> = row.get(index)?;
    encoded
        .map(|encoded| {
            serde_json::from_str(&encoded).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })
        })
        .transpose()
}

fn ensure_conversation(conn: &Connection, id: &str) -> AppResult<()> {
    if conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversations WHERE id=?1)",
        [id],
        |r| r.get::<_, bool>(0),
    )? {
        Ok(())
    } else {
        Err(AppError::NotFound("会话".into()))
    }
}

fn touch_message_conversation(conn: &Connection, message_id: &str) -> AppResult<()> {
    conn.execute("UPDATE conversations SET updated_at=?2 WHERE id=(SELECT conversation_id FROM messages WHERE id=?1)", params![message_id,now()])?;
    Ok(())
}

fn require_changed(changed: usize, label: &str) -> AppResult<()> {
    if changed == 0 {
        Err(AppError::NotFound(label.into()))
    } else {
        Ok(())
    }
}

fn validate_message(role: &str, calls: Option<&Value>, call_id: Option<&str>) -> AppResult<()> {
    if !matches!(role, "system" | "developer" | "user" | "assistant" | "tool") {
        return Err(AppError::Invalid("无效的消息角色".into()));
    }
    if (role == "tool") != call_id.is_some() || call_id.is_some_and(str::is_empty) {
        return Err(AppError::Invalid(
            "tool 消息必须且仅能携带 tool_call_id".into(),
        ));
    }
    if let Some(calls) = calls {
        if role != "assistant" {
            return Err(AppError::Invalid("工具调用只能属于 assistant 消息".into()));
        }
        validate_tool_calls(calls)?;
    }
    Ok(())
}

fn validate_tool_calls(value: &Value) -> AppResult<()> {
    let calls: Vec<ToolCall> = serde_json::from_value(value.clone())
        .map_err(|_| AppError::Invalid("tool_calls 必须为 OpenAI function 调用数组".into()))?;
    let mut ids = std::collections::HashSet::new();
    for call in &calls {
        validate_call(call)?;
        if !ids.insert(&call.id) {
            return Err(AppError::Invalid("重复的工具调用 ID".into()));
        }
    }
    Ok(())
}

fn validate_call(call: &ToolCall) -> AppResult<()> {
    if call.id.is_empty() || call.function.name.is_empty() || call.kind != "function" {
        return Err(AppError::Invalid("无效的 function 工具调用".into()));
    }
    Ok(())
}

fn is_terminal(status: &str) -> bool {
    matches!(
        status,
        "done" | "error" | "completed" | "succeeded" | "failed" | "cancelled" | "denied"
    )
}

fn validate_tool_update(update: &ToolCallUpdate) -> AppResult<()> {
    if !is_terminal(&update.status)
        && !matches!(
            update.status.as_str(),
            "pending" | "running" | "awaiting_approval"
        )
    {
        return Err(AppError::Invalid("无效的工具调用状态".into()));
    }
    if !matches!(
        update.approval_status.as_str(),
        "none" | "pending" | "approved" | "denied"
    ) {
        return Err(AppError::Invalid("无效的工具审批状态".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, History) {
        let dir = tempfile::tempdir().unwrap();
        let history = History::new(Db::open(&dir.path().join("history.sqlite")).unwrap());
        (dir, history)
    }

    fn call() -> ToolCall {
        ToolCall {
            id: "call_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "list_games".into(),
                arguments: "{}".into(),
            },
        }
    }

    fn checkpoint(history: &History, id: &str) -> ConversationAgentStatus {
        history
            .list_conversations(false)
            .unwrap()
            .into_iter()
            .find(|conversation| conversation.id == id)
            .unwrap()
            .agent_status
            .unwrap()
    }

    fn checkpoint_events(history: &History) -> crate::agent::events::Events {
        crate::agent::events::Events {
            db: history.db.clone(),
            sink: std::sync::Arc::new(|_| {}),
        }
    }

    #[test]
    fn conversation_list_checkpoint_preserves_latest_turn_outcome() {
        let (_dir, history) = fixture();
        let conv = history.create_conversation("Task", None).unwrap().id;
        let empty = checkpoint(&history, &conv);
        assert_eq!(empty.turn_id, None);
        assert_eq!(empty.turn_status, None);
        assert_eq!(empty.event_seq, 0);
        assert_eq!(empty.pending, None);

        let events = checkpoint_events(&history);
        for (turn, status) in [
            ("success", "done"),
            ("cancel", "cancelled"),
            ("failure", "error"),
        ] {
            // Use the same timestamp for each row; only insertion order is reliable.
            history.db.conn().execute(
                "INSERT INTO agent_turns (id,conversation_id,status,created_at) VALUES (?1,?2,?3,'now')",
                params![turn, conv, status],
            ).unwrap();
            let started = events
                .emit(turn, &conv, None, "turn-start", json!({}))
                .unwrap();
            // The terminal DB outcome remains available even without a done event.
            let current = checkpoint(&history, &conv);
            assert_eq!(current.turn_id.as_deref(), Some(turn));
            assert_eq!(current.turn_status.as_deref(), Some(status));
            assert_eq!(current.event_seq, started.seq);
            assert_eq!(current.pending, None);
        }
    }

    #[test]
    fn conversation_list_checkpoint_only_exposes_current_published_pending_requests() {
        let (_dir, history) = fixture();
        let conv = history.create_conversation("Task", None).unwrap().id;
        history
            .db
            .conn()
            .execute(
                "INSERT INTO agent_turns (id,conversation_id,status,created_at)
             VALUES ('old',?1,'done','now'),('current',?1,'running','now')",
                [&conv],
            )
            .unwrap();
        let events = checkpoint_events(&history);
        let requests = [
            (
                "old",
                "old-approval",
                "apply_file_ops",
                json!({}),
                "waiting-approval",
            ),
            (
                "current",
                "legacy",
                "ask_user",
                json!({"question":"Input?"}),
                "waiting-answer",
            ),
            (
                "current",
                "structured",
                "ask_user",
                json!({"questions":[{"question":"Input?"}]}),
                "waiting-answer",
            ),
            (
                "current",
                "permission",
                "apply_file_ops",
                json!({}),
                "waiting-approval",
            ),
        ];
        for (turn, approval, title, preview, expected) in requests {
            history.db.conn().execute(
                "INSERT INTO approvals (id,conversation_id,kind,title,payload_json,status,created_at)
                 VALUES (?1,?2,'tool',?3,?4,'pending','now')",
                params![approval, conv, title, preview.to_string()],
            ).unwrap();
            assert_eq!(
                checkpoint(&history, &conv).pending,
                None,
                "not published yet"
            );

            let request = events
                .emit(
                    turn,
                    &conv,
                    None,
                    "approval-request",
                    json!({
                        "approvalId": approval, "title": title, "preview": preview,
                    }),
                )
                .unwrap();
            let current = checkpoint(&history, &conv);
            assert_eq!(current.turn_id.as_deref(), Some("current"));
            assert_eq!(current.turn_status.as_deref(), Some("running"));
            assert_eq!(current.event_seq, request.seq);
            if turn == "old" {
                // A stray pending row from a previous turn cannot become actionable.
                assert_eq!(current.pending, None);
                continue;
            }
            assert_eq!(
                current.pending,
                Some(PendingAgentInteraction {
                    approval_id: approval.into(),
                    status: expected.into(),
                })
            );

            history
                .db
                .conn()
                .execute(
                    "UPDATE approvals SET status='approved',resolved_at='now' WHERE id=?1",
                    [approval],
                )
                .unwrap();
            // The persisted resolution is authoritative before its event is emitted.
            assert_eq!(checkpoint(&history, &conv).pending, None);
        }
    }

    #[test]
    fn conversation_list_checkpoint_clears_pending_after_runtime_restart() {
        use crate::{
            agent::runtime::AgentRuntime,
            settings::{SecretStore, Settings},
            storage::AppPaths,
            transaction::FileEngine,
        };
        use std::sync::Arc;

        struct NoSecrets;
        impl SecretStore for NoSecrets {
            fn read(&self, _: &str) -> AppResult<Option<String>> {
                Ok(None)
            }
            fn write(&self, _: &str, _: &str) -> AppResult<()> {
                Ok(())
            }
        }

        let (dir, history) = fixture();
        let conv = history.create_conversation("Restart", None).unwrap().id;
        history.db.conn().execute(
            "INSERT INTO agent_turns (id,conversation_id,status,created_at) VALUES ('turn',?1,'running','now')",
            [&conv],
        ).unwrap();
        history
            .db
            .conn()
            .execute(
                "INSERT INTO approvals (id,conversation_id,kind,title,status,created_at)
             VALUES ('approval',?1,'tool','apply_file_ops','pending','now')",
                [&conv],
            )
            .unwrap();
        let request = checkpoint_events(&history)
            .emit(
                "turn",
                &conv,
                None,
                "approval-request",
                json!({
                    "approvalId": "approval", "title": "apply_file_ops"
                }),
            )
            .unwrap();
        assert_eq!(
            checkpoint(&history, &conv).pending.unwrap().status,
            "waiting-approval"
        );
        drop(history);

        let db = Db::open(&dir.path().join("history.sqlite")).unwrap();
        let paths = AppPaths::new(dir.path().join("data")).unwrap();
        let settings = Settings::with_secret_store(db.clone(), Arc::new(NoSecrets));
        let runtime = AgentRuntime::new(
            FileEngine::new(db.clone(), paths),
            settings,
            Arc::new(|_| {}),
        )
        .unwrap();
        let after = checkpoint(&History::new(db.clone()), &conv);
        assert_eq!(after.turn_id.as_deref(), Some("turn"));
        assert_eq!(after.turn_status.as_deref(), Some("interrupted"));
        assert_eq!(after.pending, None);

        let replay = runtime.events.replay(&conv, request.seq, 10).unwrap();
        assert_eq!(replay.len(), 2);
        assert_eq!(replay[0].kind, "approval-resolved");
        assert_eq!(
            replay[0].payload,
            json!({"approvalId": "approval", "status": "cancelled"})
        );
        assert_eq!(replay[1].kind, "done");
        assert_eq!(replay[1].payload["status"], "interrupted");
        assert_eq!(after.event_seq, replay[1].seq);
        let status: String = db
            .conn()
            .query_row(
                "SELECT status FROM approvals WHERE id='approval'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "cancelled");
    }

    #[test]
    fn message_branches_keep_only_the_context_before_the_question() {
        let (_dir, history) = fixture();
        let source = history
            .create_conversation("Source", Some("game_1"))
            .unwrap();
        history
            .set_permission_mode(&source.id, Some("auto_approve"))
            .unwrap();
        let append = |role, text| {
            history
                .append_message(&source.id, role, text, None, None)
                .unwrap()
        };
        append("user", "Earlier question");
        let earlier = append("assistant", "Earlier answer");
        let question = append("user", "Original question");
        let answer = append("assistant", "Original answer");
        append("user", "Later question");
        let events = crate::agent::events::Events {
            db: history.db.clone(),
            sink: std::sync::Arc::new(|_| {}),
        };
        events
            .emit(
                "earlier",
                &source.id,
                Some(&earlier.id),
                "delta",
                json!({"content": "Earlier answer"}),
            )
            .unwrap();
        events
            .emit(
                "earlier",
                &source.id,
                Some(&earlier.id),
                "approval-request",
                json!({"approvalId":"unresolved"}),
            )
            .unwrap();
        for kind in ["approval-request", "approval-resolved"] {
            events
                .emit(
                    "earlier",
                    &source.id,
                    Some(&earlier.id),
                    kind,
                    json!({"approvalId":"answered"}),
                )
                .unwrap();
        }
        events
            .emit(
                "unfinished",
                &source.id,
                Some(&earlier.id),
                "delta",
                json!({"content": "incomplete"}),
            )
            .unwrap();
        events
            .emit(
                "earlier",
                &source.id,
                None,
                "done",
                json!({"status": "done"}),
            )
            .unwrap();
        events
            .emit(
                "target",
                &source.id,
                Some(&answer.id),
                "delta",
                json!({"content": "Original answer"}),
            )
            .unwrap();
        events
            .emit(
                "target",
                &source.id,
                None,
                "done",
                json!({"status": "done"}),
            )
            .unwrap();

        let (retry, prompt, attachments) = history
            .fork_before_message(&source.id, &answer.id, None, None)
            .unwrap();
        assert_eq!(prompt, "Original question");
        assert!(attachments.is_empty());
        let (edit, prompt, attachments) = history
            .fork_before_message(
                &source.id,
                &question.id,
                Some("Edited question".into()),
                None,
            )
            .unwrap();
        assert_eq!(prompt, "Edited question");
        assert!(attachments.is_empty());
        for branch_id in [retry, edit] {
            let branch = history.get_conversation(&branch_id).unwrap();
            assert_eq!(branch.conversation.game_id, source.game_id);
            assert_eq!(
                branch.conversation.permission_mode.as_deref(),
                Some("auto_approve")
            );
            assert_eq!(branch.conversation.message_count, 2);
            assert_eq!(branch.messages[0].content, "Earlier question");
            assert_eq!(branch.messages[1].content, "Earlier answer");
            let replay = events.replay(&branch_id, 0, 10).unwrap();
            assert_eq!(
                replay.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
                ["delta", "approval-request", "approval-resolved", "done"]
            );
            assert_eq!(replay[1].payload["approvalId"], "answered");
            assert_eq!(
                replay[0].message_id.as_deref(),
                Some(branch.messages[1].id.as_str())
            );
        }
        assert_eq!(
            history.get_conversation(&source.id).unwrap().messages.len(),
            5
        );
    }

    #[test]
    fn archive_restore_and_permission_override_survive_reopen() {
        let (dir, history) = fixture();
        let conversation = history
            .create_conversation("A session", Some("game_1"))
            .unwrap();
        history
            .append_message(&conversation.id, "user", "Keep my history", None, None)
            .unwrap();
        history.set_summary(&conversation.id, "A summary").unwrap();
        history
            .set_permission_mode(&conversation.id, Some("auto_approve"))
            .unwrap();
        assert!(history
            .set_permission_mode(&conversation.id, Some("confirm"))
            .is_err());
        history
            .archive_conversation(&conversation.id, true)
            .unwrap();
        assert!(history.list_conversations(false).unwrap().is_empty());
        assert_eq!(history.list_conversations(true).unwrap().len(), 1);
        drop(history);
        let history = History::new(Db::open(&dir.path().join("history.sqlite")).unwrap());
        let detail = history.get_conversation(&conversation.id).unwrap();
        assert!(detail.conversation.archived);
        assert_eq!(detail.messages.len(), 1);
        assert_eq!(detail.conversation.message_count, 1);
        assert_eq!(
            detail.conversation.task_summary.as_deref(),
            Some("A summary")
        );
        assert_eq!(
            detail.conversation.permission_mode.as_deref(),
            Some("auto_approve")
        );
        assert_eq!(detail.conversation.game_id.as_deref(), Some("game_1"));
        history
            .archive_conversation(&conversation.id, false)
            .unwrap();
        history.set_permission_mode(&conversation.id, None).unwrap();
        assert!(history.list_conversations(true).unwrap().is_empty());
        assert_eq!(
            history.list_conversations(false).unwrap()[0].permission_mode,
            None
        );
        assert_eq!(
            history
                .get_conversation(&conversation.id)
                .unwrap()
                .messages
                .len(),
            1
        );
    }

    #[test]
    fn stream_snapshots_deltas_and_tool_payloads_replay_in_insertion_order() {
        let (dir, history) = fixture();
        let conversation = history.create_conversation("", None).unwrap();
        assert_eq!(conversation.title, "新对话");
        let user = history
            .append_message(&conversation.id, "user", "List games", None, None)
            .unwrap();
        let assistant = history
            .append_message(&conversation.id, "assistant", "", None, None)
            .unwrap();
        history
            .update_message(&assistant.id, "Look", Some("thinking"))
            .unwrap();
        history
            .update_message(&assistant.id, "Looking", None)
            .unwrap();
        history
            .append_message_delta(&assistant.id, "…", Some(" done"))
            .unwrap();
        let calls = json!([call()]);
        history
            .update_tool_calls(&assistant.id, calls.clone())
            .unwrap();
        let tool = history
            .append_message(&conversation.id, "tool", "[]", None, Some("call_1"))
            .unwrap();
        // Timestamp ties AND clock reversal must not reorder stored messages.
        history
            .db
            .conn()
            .execute("UPDATE messages SET created_at='2026-01-01T00:00:00Z'", [])
            .unwrap();
        history
            .db
            .conn()
            .execute(
                "UPDATE messages SET created_at='2025-01-01T00:00:00Z' WHERE id=?1",
                [&tool.id],
            )
            .unwrap();
        drop(history);
        let history = History::new(Db::open(&dir.path().join("history.sqlite")).unwrap());
        let detail = history.get_conversation(&conversation.id).unwrap();
        assert_eq!(detail.conversation.message_count, 3);
        assert_eq!(
            detail
                .messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            vec![user.id.as_str(), assistant.id.as_str(), tool.id.as_str()]
        );
        assert_eq!(detail.messages[1].content, "Looking…");
        assert_eq!(
            detail.messages[1].reasoning.as_deref(),
            Some("thinking done")
        );
        assert_eq!(detail.messages[1].tool_calls, Some(calls.clone()));
        let replay = history.replay_messages(&conversation.id).unwrap();
        assert_eq!(
            replay[1],
            json!({"role":"assistant","content":"Looking…","tool_calls":calls})
        );
        assert_eq!(
            replay[2],
            json!({"role":"tool","content":"[]","tool_call_id":"call_1"})
        );
        assert!(replay
            .iter()
            .all(|message| message.get("id").is_none() && message.get("reasoning").is_none()));
    }

    #[test]
    fn tool_execution_state_is_durable_and_cannot_cross_conversations() {
        let (dir, history) = fixture();
        let conversation = history.create_conversation("Tools", None).unwrap();
        let assistant = history
            .append_message(
                &conversation.id,
                "assistant",
                "",
                Some(json!([call()])),
                None,
            )
            .unwrap();
        let pending = history
            .set_tool_call(
                &conversation.id,
                Some(&assistant.id),
                &call(),
                ToolCallUpdate::default(),
            )
            .unwrap();
        let running = history
            .set_tool_call(
                &conversation.id,
                Some(&assistant.id),
                &call(),
                ToolCallUpdate {
                    status: "running".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(running.started_at, pending.started_at);
        assert_eq!(running.finished_at, None);
        let completed = history
            .set_tool_call(
                &conversation.id,
                Some(&assistant.id),
                &call(),
                ToolCallUpdate {
                    status: "completed".into(),
                    result: Some(json!([{"id":"game_1"}])),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(completed.finished_at.is_some());
        assert!(completed.duration_ms.unwrap() >= 0);
        assert_eq!(completed.arguments, json!({}));
        let second = history.create_conversation("Other", None).unwrap();
        assert!(history
            .set_tool_call(
                &second.id,
                Some(&assistant.id),
                &call(),
                ToolCallUpdate::default()
            )
            .is_err());
        assert!(history
            .set_tool_call(&second.id, None, &call(), ToolCallUpdate::default())
            .is_err());
        assert!(history
            .set_tool_call(&conversation.id, None, &call(), ToolCallUpdate::default())
            .is_err());
        assert_eq!(
            history.list_tool_calls(&conversation.id).unwrap(),
            vec![completed.clone()]
        );
        drop(history);
        let history = History::new(Db::open(&dir.path().join("history.sqlite")).unwrap());
        assert_eq!(
            history.list_tool_calls(&conversation.id).unwrap(),
            vec![completed]
        );
        assert!(history.list_tool_calls(&second.id).unwrap().is_empty());
    }

    #[test]
    fn invalid_mutations_leave_no_orphans_and_do_not_change_counts() {
        let (_dir, history) = fixture();
        assert!(history
            .append_message("missing", "user", "x", None, None)
            .is_err());
        assert!(history.update_message("missing", "x", None).is_err());
        assert!(history.archive_conversation("missing", true).is_err());
        assert!(history.set_summary("missing", "x").is_err());
        assert!(history
            .set_permission_mode("missing", Some("full_trust"))
            .is_err());
        let conversation = history.create_conversation("Validation", None).unwrap();
        assert!(history
            .append_message(&conversation.id, "tool", "x", None, None)
            .is_err());
        assert!(history
            .append_message(&conversation.id, "user", "x", Some(json!([call()])), None)
            .is_err());
        assert!(history
            .append_message(&conversation.id, "user", "x", None, Some("call_1"))
            .is_err());
        assert!(history
            .append_message(
                &conversation.id,
                "assistant",
                "x",
                Some(json!([call(), call()])),
                None
            )
            .is_err());
        let user = history
            .append_message(&conversation.id, "user", "x", None, None)
            .unwrap();
        assert!(history
            .update_tool_calls(&user.id, json!([call()]))
            .is_err());
        assert_eq!(
            history
                .get_conversation(&conversation.id)
                .unwrap()
                .conversation
                .message_count,
            1
        );
        assert_eq!(
            history
                .db
                .conn()
                .query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn cloned_histories_serialize_concurrent_deltas_and_message_counts() {
        let (_dir, history) = fixture();
        let conversation = history.create_conversation("Concurrent", None).unwrap();
        let message = history
            .append_message(&conversation.id, "assistant", "", None, None)
            .unwrap();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let history = history.clone();
                let conversation_id = conversation.id.clone();
                let message_id = message.id.clone();
                scope.spawn(move || {
                    history
                        .append_message_delta(&message_id, "x", None)
                        .unwrap();
                    history
                        .append_message(&conversation_id, "user", "concurrent", None, None)
                        .unwrap();
                });
            }
        });
        let detail = history.get_conversation(&conversation.id).unwrap();
        assert_eq!(detail.messages[0].content, "xxxxxxxx");
        assert_eq!(detail.messages.len(), 9);
        assert_eq!(detail.conversation.message_count, 9);
    }
}
