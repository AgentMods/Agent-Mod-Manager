//! Dispatch from the committed live event sink only. Replaying a conversation must not notify again.
use crate::agent::events::AgentEvent;

fn notification_body(event: &AgentEvent) -> Option<&'static str> {
    match event.kind.as_str() {
        // An inbound MCP tool call has its own synthetic turn, not a completed chat task.
        "done" if event.payload["status"] == "done" && event.payload["origin"] != "mcp" => {
            Some("Agent 任务已完成。")
        }
        "approval-request" => match event.payload["title"].as_str()? {
            "ask_user" => Some("Agent 正在等待你的回答。"),
            _ => Some("Agent 正在等待你的批准。"),
        },
        _ => None,
    }
}

#[cfg(not(test))]
pub(crate) fn notify<R: tauri::Runtime>(app: &tauri::AppHandle<R>, event: &AgentEvent) {
    use tauri_plugin_notification::NotificationExt;
    let Some(body) = notification_body(event) else {
        return;
    };
    let app = app.clone();
    // OS notification delivery is best-effort and must not delay model streaming or approval.
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = app
            .notification()
            .builder()
            .title("Agent Mod Manager")
            .body(body)
            .show()
        {
            tracing::debug!(%error, "native Agent notification unavailable");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{agent::events::Events, db::Db, history::History};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    #[test]
    fn only_live_success_questions_and_approvals_trigger_notifications() {
        let temp = tempfile::tempdir().unwrap();
        let db = Db::open(&temp.path().join("events.db")).unwrap();
        let conv = History::new(db.clone())
            .create_conversation("test", None)
            .unwrap()
            .id;
        let notices = Arc::new(Mutex::new(Vec::new()));
        let observed = notices.clone();
        let events = Events {
            db,
            sink: Arc::new(move |event| {
                if let Some(body) = notification_body(event) {
                    observed.lock().unwrap().push(body);
                }
            }),
        };
        for (kind, payload) in [
            (
                "approval-request",
                json!({"approvalId":"a", "title":"ask_user", "preview":"private question"}),
            ),
            (
                "approval-resolved",
                json!({"approvalId":"a", "status":"approved"}),
            ),
            (
                "approval-request",
                json!({"approvalId":"b", "title":"file_received_mod", "preview":"private path"}),
            ),
            ("tool-result", json!({"status":"done"})),
            ("done", json!({"status":"done"})),
            ("done", json!({"status":"done", "origin":"mcp"})),
            ("error", json!({})),
            ("done", json!({"status":"error"})),
            ("done", json!({"status":"cancelled"})),
            ("done", json!({"status":"interrupted"})),
        ] {
            events.emit("turn", &conv, None, kind, payload).unwrap();
        }
        assert!(!events.replay(&conv, 0, 100).unwrap().is_empty());
        assert_eq!(
            *notices.lock().unwrap(),
            [
                "Agent 正在等待你的回答。",
                "Agent 正在等待你的批准。",
                "Agent 任务已完成。"
            ]
        );
    }
}
