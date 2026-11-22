use super::{History, Message};
use crate::{agent::Attachment, db::Db, error::AppError};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

fn fixture() -> (tempfile::TempDir, History, String) {
    let dir = tempfile::tempdir().unwrap();
    let history = History::new(Db::open(&dir.path().join("history.sqlite")).unwrap());
    let conv = history.create_conversation("Context", None).unwrap().id;
    (dir, history, conv)
}

fn append(history: &History, conv: &str, role: &str, content: &str) -> Message {
    history
        .append_message(conv, role, content, None, None)
        .unwrap()
}

fn model_messages(messages: &[&Message]) -> Vec<(String, Value)> {
    messages
        .iter()
        .map(|m| (m.id.clone(), m.to_openai()))
        .collect()
}

#[test]
fn checkpoint_survives_reopen_and_does_not_change_transcript() {
    let (dir, history, conv) = fixture();
    let older = append(&history, &conv, "user", "old question");
    let cutoff = append(&history, &conv, "assistant", "old answer");
    let other = history.create_conversation("Other", None).unwrap().id;
    append(&history, &other, "user", "another conversation");
    let user = append(&history, &conv, "user", "new question");
    let calls = json!([{"id":"call_1", "type":"function", "function":{"name":"list_games", "arguments":"{}"}}]);
    let assistant = history
        .append_message(&conv, "assistant", "", Some(calls), None)
        .unwrap();
    let tool = history
        .append_message(&conv, "tool", "result", None, Some("call_1"))
        .unwrap();
    // Neither save nor replay may use wall-clock order or include other conversations.
    history
        .db
        .conn()
        .execute(
            "UPDATE messages SET created_at='2000-01-01' WHERE id=?1",
            [&tool.id],
        )
        .unwrap();
    let transcript = [&older, &cutoff, &user, &assistant, &tool];
    assert_eq!(
        history.agent_context(&conv).unwrap(),
        (None, model_messages(&transcript))
    );
    history
        .save_context_summary(&conv, &cutoff.id, "old turn summary")
        .unwrap();
    drop(history);

    let history = History::new(Db::open(&dir.path().join("history.sqlite")).unwrap());
    assert_eq!(
        history.agent_context(&conv).unwrap(),
        (
            Some("old turn summary".into()),
            model_messages(&[&user, &assistant, &tool]),
        )
    );
    let detail = history.get_conversation(&conv).unwrap();
    assert_eq!(
        detail.conversation.task_summary.as_deref(),
        Some("old turn summary")
    );
    assert_eq!(detail.conversation.message_count, 5);
    assert_eq!(detail.messages.iter().collect::<Vec<_>>(), transcript);
    assert_eq!(
        history.replay_messages(&conv).unwrap(),
        transcript.iter().map(|m| m.to_openai()).collect::<Vec<_>>()
    );
}

#[test]
fn checkpoint_keeps_only_the_latest_covered_prompt_and_its_attachments_if_needed() {
    let (_dir, history, conv) = fixture();
    append(&history, &conv, "user", "previous question");
    let user = history.append_user_message(&conv, "", vec![
        Attachment::Image {
            name: "current.png".into(),
            data_url: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j2ioAAAAASUVORK5CYII=".into(),
        },
        Attachment::Document { name: "current.md".into(), markdown: "# current".into() },
    ]).unwrap();
    let covered_answer = append(&history, &conv, "assistant", "covered answer");
    let tail_answer = append(&history, &conv, "assistant", "latest answer");
    history
        .save_context_summary(&conv, &covered_answer.id, "summary")
        .unwrap();
    assert_eq!(
        history.agent_context(&conv).unwrap(),
        (
            Some("summary".into()),
            model_messages(&[&user, &tail_answer]),
        )
    );
    history
        .save_context_summary(&conv, &user.id, "summary up to user")
        .unwrap();
    assert_eq!(
        history.agent_context(&conv).unwrap(),
        (
            Some("summary up to user".into()),
            model_messages(&[&user, &covered_answer, &tail_answer]),
        )
    );
    history
        .save_context_summary(&conv, &tail_answer.id, "summary through tail")
        .unwrap();
    assert_eq!(
        history.agent_context(&conv).unwrap(),
        (
            Some("summary through tail".into()),
            model_messages(&[&user]),
        )
    );
    let next = append(&history, &conv, "user", "next question");
    assert_eq!(
        history.agent_context(&conv).unwrap(),
        (
            Some("summary through tail".into()),
            model_messages(&[&next]),
        )
    );
}

#[test]
fn model_context_never_decodes_covered_payloads_excluded_from_replay() {
    let (_dir, history, conv) = fixture();
    let old_user = append(&history, &conv, "user", "old payload");
    let cutoff = append(&history, &conv, "assistant", "summarized answer");
    let next = append(&history, &conv, "user", "new question");
    history
        .db
        .conn()
        .execute(
            "UPDATE messages SET attachments_json='invalid' WHERE id=?1",
            [&old_user.id],
        )
        .unwrap();
    history
        .db
        .conn()
        .execute(
            "UPDATE messages SET tool_calls_json='invalid' WHERE id=?1",
            [&cutoff.id],
        )
        .unwrap();
    history
        .save_context_summary(&conv, &cutoff.id, "summary")
        .unwrap();
    assert_eq!(
        history.agent_context(&conv).unwrap(),
        (Some("summary".into()), model_messages(&[&next]),)
    );
    assert!(history.replay_messages(&conv).is_err());
}

#[test]
fn invalid_checkpoint_writes_preserve_existing_state_and_manual_summaries_do_not_hide_messages() {
    let (_dir, history, conv) = fixture();
    let cutoff = append(&history, &conv, "user", "covered question");
    let user = append(&history, &conv, "user", "latest question");
    let other = history.create_conversation("Other", None).unwrap().id;
    let other_message = append(&history, &other, "user", "another conversation");
    history
        .save_context_summary(&conv, &cutoff.id, "summary")
        .unwrap();
    for (conversation, message) in [
        (conv.as_str(), "missing"),
        (conv.as_str(), other_message.id.as_str()),
        (other.as_str(), cutoff.id.as_str()),
        ("missing", cutoff.id.as_str()),
    ] {
        assert!(matches!(
            history.save_context_summary(conversation, message, "invalid"),
            Err(AppError::NotFound(_))
        ));
    }
    assert!(matches!(
        history.agent_context("missing"),
        Err(AppError::NotFound(_))
    ));
    assert_eq!(
        history.agent_context(&conv).unwrap(),
        (Some("summary".into()), model_messages(&[&user]))
    );
    assert_eq!(
        history.agent_context(&other).unwrap(),
        (None, model_messages(&[&other_message]))
    );
    history.set_summary(&conv, "manual summary").unwrap();
    assert_eq!(
        history.agent_context(&conv).unwrap(),
        (None, model_messages(&[&cutoff, &user]))
    );
    assert_eq!(
        history
            .get_conversation(&conv)
            .unwrap()
            .conversation
            .task_summary
            .as_deref(),
        Some("manual summary")
    );
}

#[test]
fn only_model_visible_changes_to_the_covered_prefix_invalidate_the_checkpoint() {
    let (_dir, history, conv) = fixture();
    let covered = append(&history, &conv, "user", "covered question");
    let cutoff = append(&history, &conv, "assistant", "covered answer");
    let user = append(&history, &conv, "user", "new question");
    history
        .save_context_summary(&conv, &cutoff.id, "summary")
        .unwrap();
    history
        .update_message(&cutoff.id, "covered answer", Some("local reasoning only"))
        .unwrap();
    history
        .update_message(&user.id, "corrected tail", None)
        .unwrap();
    assert_eq!(
        history.agent_context(&conv).unwrap(),
        (
            Some("summary".into()),
            vec![(user.id, json!({"role":"user","content":"corrected tail"}))],
        )
    );
    history
        .append_message_delta(&cutoff.id, " revised", None)
        .unwrap();
    let (summary, messages) = history.agent_context(&conv).unwrap();
    assert_eq!(summary, None);
    assert_eq!(messages.len(), 3);
    assert_eq!(
        messages[1].1,
        json!({"role":"assistant","content":"covered answer revised"})
    );
    assert_eq!(
        history
            .get_conversation(&conv)
            .unwrap()
            .conversation
            .task_summary,
        None
    );

    history
        .save_context_summary(&conv, &cutoff.id, "summary")
        .unwrap();
    history.update_tool_calls(&cutoff.id, json!([{"id":"call_1", "type":"function", "function":{"name":"list_games", "arguments":"{}"}}])).unwrap();
    assert_eq!(history.agent_context(&conv).unwrap().0, None);

    history
        .save_context_summary(&conv, &cutoff.id, "summary")
        .unwrap();
    // A direct SQL write (as used by the runtime) also invalidates a covered prefix.
    history
        .db
        .conn()
        .execute("DELETE FROM messages WHERE id=?1", [covered.id])
        .unwrap();
    assert_eq!(
        history
            .get_conversation(&conv)
            .unwrap()
            .conversation
            .task_summary,
        None
    );
    assert_eq!(history.agent_context(&conv).unwrap().0, None);
}

#[test]
fn edit_and_retry_rewinds_invalidate_checkpoints_transactionally() {
    let (_dir, history, conv) = fixture();
    let covered = append(&history, &conv, "user", "older question");
    let user = append(&history, &conv, "user", "original question");
    let assistant = append(&history, &conv, "assistant", "original answer");
    history
        .save_context_summary(&conv, &assistant.id, "original summary")
        .unwrap();
    {
        let mut conn = history.db.conn();
        let tx = conn.transaction().unwrap();
        History::restart_user_message(&tx, &conv, &assistant.id, None).unwrap();
        tx.rollback().unwrap();
    }
    assert_eq!(
        history.agent_context(&conv).unwrap(),
        (Some("original summary".into()), model_messages(&[&user]))
    );
    {
        let mut conn = history.db.conn();
        let tx = conn.transaction().unwrap();
        History::restart_user_message(&tx, &conv, &assistant.id, None).unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(
        history.agent_context(&conv).unwrap(),
        (None, model_messages(&[&covered, &user]))
    );

    history
        .save_context_summary(&conv, &user.id, "summary before edit")
        .unwrap();
    {
        let mut conn = history.db.conn();
        let tx = conn.transaction().unwrap();
        History::restart_user_message(&tx, &conv, &user.id, Some(("edited question", None)))
            .unwrap();
        tx.commit().unwrap();
    }
    let (summary, messages) = history.agent_context(&conv).unwrap();
    assert_eq!(summary, None);
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[1],
        (user.id, json!({"role":"user","content":"edited question"}))
    );
}

#[test]
fn migrates_v12_legacy_summaries_without_treating_them_as_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite");
    let legacy = Connection::open(&path).unwrap();
    legacy
        .execute_batch(
            "CREATE TABLE conversations (
            id TEXT PRIMARY KEY, title TEXT NOT NULL, auto_title_pending INTEGER NOT NULL DEFAULT 0,
            game_id TEXT, task_summary TEXT, archived INTEGER NOT NULL DEFAULT 0,
            pinned INTEGER NOT NULL DEFAULT 0, message_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL, updated_at TEXT NOT NULL
        );
        CREATE TABLE messages (
            id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
            role TEXT NOT NULL, content TEXT NOT NULL DEFAULT '', reasoning TEXT, attachments_json TEXT,
            tool_calls_json TEXT, tool_call_id TEXT, name TEXT, token_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        );
        INSERT INTO conversations (id,title,task_summary,message_count,created_at,updated_at)
            VALUES ('legacy','Legacy','old summary',1,'now','now');
        INSERT INTO messages (id,conversation_id,role,content,created_at)
            VALUES ('legacy-message','legacy','user','original question','now');
        PRAGMA user_version=12;",
        )
        .unwrap();
    drop(legacy);
    let history = History::new(Db::open(&path).unwrap());
    let user = history
        .get_conversation("legacy")
        .unwrap()
        .messages
        .remove(0);
    assert_eq!(user.id, "legacy-message");
    assert_eq!(user.content, "original question");
    assert_eq!(
        history.agent_context("legacy").unwrap(),
        (None, model_messages(&[&user]))
    );
    assert_eq!(
        history
            .get_conversation("legacy")
            .unwrap()
            .conversation
            .task_summary
            .as_deref(),
        Some("old summary")
    );
    history
        .save_context_summary("legacy", &user.id, "migrated checkpoint")
        .unwrap();
    assert_eq!(
        history.agent_context("legacy").unwrap(),
        (Some("migrated checkpoint".into()), model_messages(&[&user]))
    );
    let db = history.db.conn();
    let version = db
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .unwrap();
    assert_eq!(version, crate::db::SCHEMA_VERSION);
    let cutoff_plan: String = db.query_row(
        "EXPLAIN QUERY PLAN SELECT content FROM messages WHERE conversation_id=?1 AND rowid>?2 ORDER BY rowid",
        params!["legacy", 0],
        |row| row.get(3),
    ).unwrap();
    assert!(
        cutoff_plan.contains("conversation_id=? AND rowid>?"),
        "{cutoff_plan}"
    );
}
