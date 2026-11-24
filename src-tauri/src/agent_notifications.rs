//! Dispatch from the committed live event sink only. Replaying a conversation must not notify again.
use crate::agent::{chat_strings, events::AgentEvent};

#[derive(Clone, Copy)]
enum Notice {
    Completed,
    AwaitingAnswer,
    AwaitingApproval,
}

impl Notice {
    fn from_event(event: &AgentEvent) -> Option<Self> {
        match event.kind.as_str() {
            // An inbound MCP tool call has its own synthetic turn, not a completed chat task.
            "done" if event.payload["status"] == "done" && event.payload["origin"] != "mcp" => {
                Some(Self::Completed)
            }
            "approval-request" => match event.payload["title"].as_str()? {
                "ask_user" => Some(Self::AwaitingAnswer),
                _ => Some(Self::AwaitingApproval),
            },
            _ => None,
        }
    }

    fn body(self, locale: &str) -> &'static str {
        let strings = chat_strings::for_locale(locale);
        match self {
            Self::Completed => strings.completed,
            Self::AwaitingAnswer => strings.awaiting_answer,
            Self::AwaitingApproval => strings.awaiting_approval,
        }
    }
}

#[cfg(not(test))]
pub(crate) fn notify<R: tauri::Runtime>(app: &tauri::AppHandle<R>, event: &AgentEvent) {
    use tauri::Manager;
    use tauri_plugin_notification::NotificationExt;
    let Some(notice) = Notice::from_event(event) else {
        return;
    };
    let app = app.clone();
    // OS notification delivery is best-effort and must not delay model streaming or approval.
    tauri::async_runtime::spawn_blocking(move || {
        let locale = app
            .try_state::<crate::app::AppServices>()
            .map(|services| crate::app_locale::effective(&services.engine.db));
        if let Err(error) = app
            .notification()
            .builder()
            .title("Agent Mod Manager")
            .body(notice.body(locale.as_deref().unwrap_or("en")))
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
        let locale_db = db.clone();
        crate::app_locale::set(&db, Some("zh-CN".into())).unwrap();
        let events = Events {
            db,
            sink: Arc::new(move |event| {
                if let Some(notice) = Notice::from_event(event) {
                    observed
                        .lock()
                        .unwrap()
                        .push(notice.body(&crate::app_locale::effective(&locale_db)));
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
            if kind == "done" {
                crate::app_locale::set(&events.db, Some("fr".into())).unwrap();
            }
            events.emit("turn", &conv, None, kind, payload).unwrap();
        }
        assert!(!events.replay(&conv, 0, 100).unwrap().is_empty());
        assert_eq!(
            *notices.lock().unwrap(),
            [
                "Agent 正在等待你的回答。",
                "Agent 正在等待你的批准。",
                "L’agent a terminé la tâche."
            ]
        );
    }
}
