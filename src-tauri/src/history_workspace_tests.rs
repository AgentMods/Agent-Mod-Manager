use super::*;
use crate::{db::Db, game_actions::remove_game, storage::AppPaths, transaction::FileEngine};
use serde_json::{json, Value};
use std::path::Path;

fn fixture() -> (tempfile::TempDir, History) {
    let dir = tempfile::tempdir().unwrap();
    let history = History::new(Db::open(&dir.path().join("history.sqlite")).unwrap());
    (dir, history)
}

fn insert_game(history: &History, id: &str, root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    history.db.conn().execute(
        "INSERT INTO games(id,name,slug,root_path,created_at,updated_at) VALUES(?1,?1,?1,?2,'now','now')",
        params![id, root.to_str().unwrap()],
    ).unwrap();
}

fn assert_selection(history: &History, id: &str, archived: bool, expected: Value) {
    let detail = history.get_conversation(id).unwrap().conversation;
    let list = history
        .list_conversations(archived)
        .unwrap()
        .into_iter()
        .find(|conversation| conversation.id == id)
        .unwrap();
    for conversation in [detail, list] {
        let wire = serde_json::to_value(conversation).unwrap();
        assert_eq!(wire.get("workspace_game_ids"), Some(&expected));
        let single = expected
            .as_array()
            .filter(|ids| ids.len() == 1)
            .map(|ids| &ids[0]);
        assert_eq!(&wire["game_id"], single.unwrap_or(&Value::Null));
    }
    assert_eq!(
        serde_json::to_value(history.conversation_workspace(id).unwrap()).unwrap(),
        expected
    );
}

#[test]
fn creates_all_empty_single_and_multi_and_deduplicates_in_selection_order() {
    let (dir, history) = fixture();
    insert_game(&history, "a", &dir.path().join("a"));
    insert_game(&history, "b", &dir.path().join("b"));
    let all = history.create_conversation("All", None).unwrap();
    let empty = history
        .create_conversation_with_workspace("Empty", Some("a"), None, Some(&[]))
        .unwrap();
    let single = history.create_conversation("Legacy", Some("a")).unwrap();
    let deduped = history
        .create_conversation_with_workspace("Repeated", None, None, Some(&["a".into(), "a".into()]))
        .unwrap();
    let multi = history
        .create_conversation_with_workspace(
            "Multi",
            Some("a"),
            None,
            Some(&["b".into(), "a".into(), "b".into()]),
        )
        .unwrap();
    for (conversation, expected) in [
        (all, Value::Null),
        (empty, json!([])),
        (single, json!(["a"])),
        (deduped, json!(["a"])),
        (multi, json!(["b", "a"])),
    ] {
        let wire = serde_json::to_value(&conversation).unwrap();
        assert_eq!(wire.get("workspace_game_ids"), Some(&expected));
        assert_selection(&history, &conversation.id, false, expected);
    }
    for invalid in ["missing", "", " a"] {
        assert!(matches!(
            history.create_conversation("Invalid legacy", Some(invalid)),
            Err(AppError::Invalid(_))
        ));
        assert!(matches!(
            history.create_conversation_with_workspace(
                "Invalid explicit",
                None,
                None,
                Some(&["a".into(), invalid.into()])
            ),
            Err(AppError::Invalid(_))
        ));
    }
    assert_eq!(history.list_conversations(false).unwrap().len(), 5);
}

#[test]
fn protects_history_permissions_and_busy_or_archived_workspaces() {
    let (dir, history) = fixture();
    insert_game(&history, "a", &dir.path().join("a"));
    let chat = history
        .create_conversation_with_category("Task", Some("a"), Some("install"))
        .unwrap();
    history
        .append_message(&chat.id, "user", "Past message", None, None)
        .unwrap();
    history.set_conversation_pinned(&chat.id, true).unwrap();
    history.set_summary(&chat.id, "Past summary").unwrap();
    history
        .set_permission_mode(&chat.id, Some("auto_approve"))
        .unwrap();
    assert!(matches!(
        history.conversation_workspace("missing"),
        Err(AppError::NotFound(_))
    ));
    assert!(matches!(
        history.set_conversation_workspace("missing", None),
        Err(AppError::NotFound(_))
    ));
    assert!(matches!(
        history.set_conversation_workspace(&chat.id, Some(&["missing".into()])),
        Err(AppError::Invalid(_))
    ));
    assert_selection(&history, &chat.id, false, json!(["a"]));

    history.db.conn().execute(
        "INSERT INTO agent_turns(id,conversation_id,status,created_at) VALUES('turn',?1,'running','now')", [&chat.id],
    ).unwrap();
    assert!(matches!(
        history.set_conversation_workspace(&chat.id, None),
        Err(AppError::Conflict(_))
    ));
    assert_selection(&history, &chat.id, false, json!(["a"]));
    history
        .db
        .conn()
        .execute("UPDATE agent_turns SET status='done' WHERE id='turn'", [])
        .unwrap();
    history.archive_conversation(&chat.id, true).unwrap();
    assert!(matches!(
        history.set_conversation_workspace(&chat.id, Some(&[])),
        Err(AppError::Denied(_))
    ));
    assert_selection(&history, &chat.id, true, json!(["a"]));
    history.archive_conversation(&chat.id, false).unwrap();

    let changed = history
        .set_conversation_workspace(&chat.id, Some(&[]))
        .unwrap();
    assert_eq!(changed.workspace_game_ids, Some(vec![]));
    assert!(changed.game_id.is_none());
    assert!(changed.pinned);
    assert_eq!(changed.title, "Task");
    assert_eq!(changed.category.as_deref(), Some("install"));
    assert_eq!(changed.task_summary.as_deref(), Some("Past summary"));
    assert_eq!(changed.permission_mode.as_deref(), Some("auto_approve"));
    assert_eq!(changed.message_count, 1);
    assert_eq!(
        history.get_conversation(&chat.id).unwrap().messages[0].content,
        "Past message"
    );
    assert_eq!(
        history
            .set_conversation_workspace(&chat.id, None)
            .unwrap()
            .workspace_game_ids,
        None
    );
    assert_selection(&history, &chat.id, false, Value::Null);
}

#[test]
fn serializes_workspace_edits_with_turn_start_on_another_connection() {
    let (dir, history) = fixture();
    let chat = history.create_conversation("Race", None).unwrap();
    let other = History::new(Db::open(&dir.path().join("history.sqlite")).unwrap());
    let mut conn = history.db.conn();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    tx.execute(
        "INSERT INTO agent_turns(id,conversation_id,status,created_at) VALUES('race',?1,'running','now')", [&chat.id],
    ).unwrap();
    let id = chat.id.clone();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let modify = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        other.set_conversation_workspace(&id, Some(&[]))
    });
    started_rx.recv().unwrap();
    tx.commit().unwrap();
    drop(conn);
    assert!(matches!(modify.join().unwrap(), Err(AppError::Conflict(_))));
    assert_selection(&history, &chat.id, false, Value::Null);
}

#[test]
fn migrates_legacy_bindings_without_widening_stale_ids_or_altering_messages() {
    let (dir, history) = fixture();
    insert_game(&history, "saved\"game", &dir.path().join("game"));
    history
        .db
        .conn()
        .execute_batch(
            "ALTER TABLE conversations DROP COLUMN workspace_game_ids_json;
         PRAGMA user_version=20;
         INSERT INTO conversations(id,game_id,created_at,updated_at) VALUES
           ('legacy-one','saved\"game','now','now'),
           ('legacy-all',NULL,'now','now'), ('legacy-stale','removed','now','now');",
        )
        .unwrap();
    history
        .append_message("legacy-one", "user", "Before upgrade", None, None)
        .unwrap();
    drop(history);

    let db_path = dir.path().join("history.sqlite");
    let history = History::new(Db::open(&db_path).unwrap());
    assert_selection(&history, "legacy-one", false, json!(["saved\"game"]));
    assert_selection(&history, "legacy-all", false, Value::Null);
    assert_selection(&history, "legacy-stale", false, json!([]));
    assert_eq!(
        history.get_conversation("legacy-one").unwrap().messages[0].content,
        "Before upgrade"
    );
    history
        .set_conversation_workspace("legacy-one", Some(&[]))
        .unwrap();
    drop(history);
    let reopened = History::new(Db::open(&db_path).unwrap());
    assert_selection(&reopened, "legacy-one", false, json!([]));
}

#[test]
fn removing_games_prunes_explicit_workspaces_without_widening_them() {
    let (dir, history) = fixture();
    for id in ["a", "b", "c"] {
        insert_game(&history, id, &dir.path().join(id));
    }
    let engine = FileEngine::new(
        history.db.clone(),
        AppPaths::new(dir.path().join("data")).unwrap(),
    );
    let selected = history
        .create_conversation_with_workspace(
            "Multi",
            None,
            None,
            Some(&["c".into(), "a".into(), "b".into()]),
        )
        .unwrap();
    let single = history.create_conversation("Single", Some("a")).unwrap();
    let global = history.create_conversation("Global", None).unwrap();
    history
        .append_message(&single.id, "user", "Keep this", None, None)
        .unwrap();
    history.archive_conversation(&single.id, true).unwrap();

    remove_game(&engine, "a").unwrap();
    assert!(dir.path().join("a").is_dir());
    assert_selection(&history, &single.id, true, json!([]));
    assert_eq!(
        history.get_conversation(&single.id).unwrap().messages[0].content,
        "Keep this"
    );
    assert_selection(&history, &selected.id, false, json!(["c", "b"]));
    assert!(matches!(
        history.set_conversation_workspace(&selected.id, Some(&["a".into()])),
        Err(AppError::Invalid(_))
    ));
    remove_game(&engine, "b").unwrap();
    assert_selection(&history, &selected.id, false, json!(["c"]));
    remove_game(&engine, "c").unwrap();
    assert_selection(&history, &selected.id, false, json!([]));
    assert_selection(&history, &global.id, false, Value::Null);
}

#[test]
fn workspace_commands_accept_legacy_and_explicit_camel_case_arguments() {
    use crate::{app::AppServices, commands};
    use tauri::{test, webview::InvokeRequest};

    let dir = tempfile::tempdir().unwrap();
    let state = AppServices::initialize(dir.path().join("data")).unwrap();
    let history = History::new(state.engine.db.clone());
    insert_game(&history, "g", &dir.path().join("game"));
    let app = test::mock_builder()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::create_conversation,
            commands::get_conversation,
            commands::list_conversations,
            commands::set_conversation_workspace,
        ])
        .build(test::mock_context(test::noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let invoke = |command: &str, args: Value| {
        test::get_ipc_response(
            &window,
            InvokeRequest {
                cmd: command.into(),
                callback: tauri::ipc::CallbackFn(1),
                error: tauri::ipc::CallbackFn(2),
                url: (if cfg!(windows) {
                    "http://tauri.localhost"
                } else {
                    "tauri://localhost"
                })
                .parse()
                .unwrap(),
                body: tauri::ipc::InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: test::INVOKE_KEY.into(),
            },
        )
        .map(|response| response.deserialize::<Value>().unwrap())
    };
    let legacy = invoke(
        "create_conversation",
        json!({"title":"Legacy","gameId":"g"}),
    )
    .unwrap();
    assert_eq!(legacy["workspace_game_ids"], json!(["g"]));
    assert_eq!(legacy["game_id"], "g");
    let empty = invoke(
        "create_conversation",
        json!({"title":"Empty","gameId":"g","workspaceGameIds":[]}),
    )
    .unwrap();
    assert_eq!(empty["workspace_game_ids"], json!([]));
    let id = &empty["id"];
    assert!(invoke("set_conversation_workspace", json!({"id":id})).is_err());
    assert!(invoke("set_conversation_workspace", json!({"id":id,"gameIds":[1]})).is_err());
    let unchanged = invoke("get_conversation", json!({"id":id})).unwrap();
    assert_eq!(unchanged["conversation"]["workspace_game_ids"], json!([]));
    let switched = invoke(
        "set_conversation_workspace",
        json!({"id":id,"gameIds":null}),
    )
    .unwrap();
    assert_eq!(switched.get("workspace_game_ids"), Some(&Value::Null));
    let list = invoke("list_conversations", json!({"archived":false})).unwrap();
    let listed = list
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == *id)
        .unwrap();
    assert_eq!(listed.get("workspace_game_ids"), Some(&Value::Null));
}
