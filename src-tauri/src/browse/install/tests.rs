use super::*;
use crate::{
    agent::{events::Events, policy::Effect},
    settings::{SecretStore, Settings},
    transaction::tests::setup,
};
use serde_json::json;
use std::sync::Arc;

fn key(game_id: &str) -> WorkKey {
    WorkKey {
        game_id: game_id.into(),
        source_id: "curseforge".into(),
        project_id: "45".into(),
        provider_game_id: "7".into(),
    }
}

fn create_task(db: &Db, key: &WorkKey, id: &str, turn_status: &str) -> String {
    let conversation = History::new(db.clone())
        .create_conversation("test", Some(&key.game_id))
        .unwrap();
    db.conn().execute(
        "INSERT INTO browse_install_tasks(id,conversation_id,game_id,source_id,project_id,provider_game_id) VALUES(?1,?2,?3,?4,?5,?6)",
        params![id, conversation.id, key.game_id, key.source_id, key.project_id, key.provider_game_id],
    ).unwrap();
    db.conn()
        .execute(
            "INSERT INTO agent_turns(id,conversation_id,status,created_at) VALUES(?1,?2,?3,'now')",
            params![format!("turn-{id}"), conversation.id, turn_status],
        )
        .unwrap();
    conversation.id
}

#[test]
fn agent_done_or_a_forged_tool_result_never_confirms_install() {
    let (_dir, engine, _) = setup();
    init(&engine.db).unwrap();
    let key = key("g");
    let conversation = create_task(&engine.db, &key, "task", "done");
    let conn = engine.db.conn();
    drop(conn);
    Events { db: engine.db.clone(), sink: Arc::new(|_| {}) }.emit("turn-task", &conversation, None, "tool-result", json!({
        "name":"install_library_mod", "status":"done", "result": {"status":"committed", "installId":"fake", "transactionId":"fake"}
    })).unwrap();
    let task = task_for(&engine.db.conn(), &key).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Unverified);
    assert_eq!(task.install_id, None);
}

#[test]
fn only_exact_durable_active_install_receipt_is_success_even_after_interruption() {
    let (_dir, engine, _) = setup();
    init(&engine.db).unwrap();
    let key = key("g");
    create_task(&engine.db, &key, "task", "interrupted");
    let conn = engine.db.conn();
    conn.execute("UPDATE browse_install_tasks SET install_id='i',transaction_id='t',artifact_id='a' WHERE id='task'", []).unwrap();
    conn.execute("INSERT INTO installs (id,game_id,name,metadata_json,installed_at,updated_at) VALUES ('i',?1,'Title',?2,'now','now')", params!["g",json!({"downloadOrigin":{"artifactId":"a","sourceId":"curseforge","projectId":"45","versionId":"8"}}).to_string()]).unwrap();
    conn.execute_batch("INSERT INTO download_artifacts(id,path,filename,artifact_json,source_id,project_id,version_id,name,version,created_at) VALUES('a','path','archive.zip','{}','curseforge','45','8','Title','v8','now');\n        INSERT INTO install_transactions(install_id,transaction_id,action,created_at) VALUES ('i','t','install','now');").unwrap();
    conn.execute("INSERT INTO file_transactions(id,kind,status,game_id,plan_json,created_at) VALUES('t','library_install','planned',?1,'{}','now')", ["g"]).unwrap();
    assert_eq!(
        task_for(&conn, &key).unwrap().unwrap().status,
        TaskStatus::Interrupted
    );
    conn.execute(
        "UPDATE file_transactions SET status='committed',game_id='different' WHERE id='t'",
        [],
    )
    .unwrap();
    assert!(verified_install(&conn, "task").unwrap().is_none());
    conn.execute(
        "UPDATE file_transactions SET game_id=?1 WHERE id='t'",
        ["g"],
    )
    .unwrap();
    conn.execute(
        "UPDATE download_artifacts SET project_id='46' WHERE id='a'",
        [],
    )
    .unwrap();
    assert!(verified_install(&conn, "task").unwrap().is_none());
    conn.execute(
        "UPDATE download_artifacts SET project_id='45' WHERE id='a'",
        [],
    )
    .unwrap();
    drop(conn);
    let reopened = Db::open(&engine.paths.db_file()).unwrap();
    let conn = reopened.conn();
    let task = task_for(&conn, &key).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Installed);
    assert_eq!(task.install_id.as_deref(), Some("i"));
    assert_eq!(task.transaction_id.as_deref(), Some("t"));
    conn.execute(
        "UPDATE file_transactions SET status='rolled_back' WHERE id='t'",
        [],
    )
    .unwrap();
    assert!(verified_install(&conn, "task").unwrap().is_none());
}

#[test]
fn latest_attempt_persists_rejection_and_alias_is_window_and_game_bound() {
    let (_dir, engine, _) = setup();
    init(&engine.db).unwrap();
    let key = key("g");
    create_task(&engine.db, &key, "old", "done");
    let conversation = create_task(&engine.db, &key, "new", "done");
    let conn = engine.db.conn();
    conn.execute("INSERT INTO approvals(id,conversation_id,kind,title,status,created_at) VALUES('question',?1,'tool','ask_user','rejected','now')", [&conversation]).unwrap();
    assert_eq!(
        task_for(&conn, &key).unwrap().unwrap().status,
        TaskStatus::Unverified
    );
    conn.execute("INSERT INTO approvals(id,conversation_id,kind,title,status,created_at) VALUES('approval',?1,'tool','download_mod','rejected','now')", [&conversation]).unwrap();
    remember_handle(&conn, "window", "search", "item", &key).unwrap();
    conn.execute(
        "INSERT INTO browse_install_titles(task_id,title) VALUES('new','Current work')",
        [],
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(list_tasks(&conn, "window").unwrap()).unwrap(),
        json!([{ "searchId":"search", "itemId":"item", "gameId":"g", "title":"Current work" }])
    );
    assert!(list_tasks(&conn, "other").unwrap().is_empty());
    let resumed = known_work(&conn, "window", "search", "item", "g")
        .unwrap()
        .unwrap();
    let task = task_for(&conn, &resumed).unwrap().unwrap();
    assert_eq!(task.id, "new");
    assert_eq!(task.status, TaskStatus::Rejected);
    assert!(known_work(&conn, "other", "search", "item", "g")
        .unwrap()
        .is_none());
    assert!(known_work(&conn, "window", "search", "item", "other-game")
        .unwrap()
        .is_none());
}

#[test]
fn scope_injects_catalog_binding_blocks_forged_inputs_and_generic_turns() {
    let (_dir, engine, _) = setup();
    let runtime = runtime(engine);
    let db = &runtime.engine.db;
    let key = key("g");
    let conversation = create_task(db, &key, "task", "done");
    let request = TurnRequest {
        conversation_id: conversation,
        content: "install anything".into(),
        attachments: None,
        model: None,
        permission_mode: Some(PermissionMode::FullTrust),
        reasoning_effort: None,
    };
    assert!(matches!(
        runtime.start_turn(request),
        Err(AppError::Denied(_))
    ));
    let game = GameScope::new(crate::games::get_game(db, "g").unwrap());
    let scope = InstallScope::new("task".into(), key, game);
    let tool = runtime.registry.get("resolve_mod").unwrap();
    assert_eq!(tool.definition().effect, Effect::Read);
    assert_eq!(
        scope
            .arguments(db, tool.as_ref(), "resolve_mod", json!({"version_id":"99"}))
            .unwrap(),
        json!({"version_id":"99","project_id":"45","source_id":"curseforge","game_id":"7"})
    );
    assert!(scope
        .arguments(db, tool.as_ref(), "resolve_mod", json!({"project_id":"46"}))
        .is_err());
    let tool = runtime.registry.get("install_library_mod").unwrap();
    assert!(scope
        .arguments(
            db,
            tool.as_ref(),
            "install_library_mod",
            json!({"request":{"gameId":"elsewhere","artifactId":"forged"}})
        )
        .is_err());
    let tool = runtime.registry.get("plan_file_ops").unwrap();
    assert!(!scope.allows("plan_file_ops", tool.as_ref()));
}

fn runtime(engine: crate::transaction::FileEngine) -> AgentRuntime {
    struct NoSecrets;
    impl SecretStore for NoSecrets {
        fn read(&self, _: &str) -> AppResult<Option<String>> {
            Ok(None)
        }
        fn write(&self, _: &str, _: &str) -> AppResult<()> {
            Ok(())
        }
    }
    let db = engine.db.clone();
    AgentRuntime::new(
        engine,
        Settings::with_secret_store(db.clone(), Arc::new(NoSecrets)),
        Arc::new(|_| {}),
    )
    .unwrap()
}

#[tokio::test]
async fn stock_receipt_installs_only_the_exact_archive_and_survives_an_attempt_retry() {
    use crate::{agent::tools::ToolContext, sources::resolutions::ResolutionScope};
    use sha2::{Digest, Sha256};
    use std::io::Write;
    let (_dir, engine, game_root) = setup();
    let runtime = runtime(engine.clone());
    let db = &engine.db;
    let mut work = key("g");
    work.source_id = "gamebanana".into();
    let conversation = create_task(db, &work, "task-first", "done");
    let game = GameScope::new(crate::games::get_game(db, "g").unwrap());
    let scope = InstallScope::new("task-first".into(), work.clone(), game.clone());
    let context = ToolContext {
        engine: engine.clone(),
        cancel: tokio_util::sync::CancellationToken::new(),
        progress: None,
        resolution_scope: ResolutionScope::Agent(conversation),
    };
    let path = crate::storage::incoming_downloads(&engine.paths.root).join("release.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    zip.start_file("patch.dat", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"exact downloaded bytes").unwrap();
    zip.finish().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let hash = hex::encode(Sha256::digest(&bytes));
    let mut args = json!({"file_id": 90});
    stock::arguments(&work, "download_file", &mut args).unwrap();
    assert_eq!(args, json!({"mod_id":45,"file_id":90}));
    let receipt = json!({"file_id":90,"path":path,"bytes":bytes.len(),"sha256":hash});
    for (name, replacement) in [
        ("file_id", json!(91)),
        ("bytes", json!(bytes.len() + 1)),
        ("sha256", json!("0".repeat(64))),
        ("path", json!(game_root.join("release.zip"))),
    ] {
        let mut bad = receipt.clone();
        bad[name] = replacement;
        assert!(stock::transfer_receipt(
            &work,
            &args,
            &json!({"structuredContent":bad}),
            &context,
            &runtime.registry
        )
        .await
        .is_err());
    }
    let saved = scope
        .finish_tool(
            &runtime.registry,
            &runtime.resolutions,
            &context,
            ("stock-download", Some(("gamebanana", "download_file"))),
            &args,
            json!({"structuredContent": receipt}),
        )
        .await
        .unwrap();
    assert_eq!(saved["filename"], "release.zip");
    assert_eq!(saved["status"], "saved");
    assert!(!scope.verified(db).unwrap());
    let receive = runtime.registry.get("file_received_mod").unwrap();
    let bind = |arguments| scope.arguments(db, receive.as_ref(), "file_received_mod", arguments);
    assert!(bind(json!({"filename":"wrong.zip"})).is_err());
    let args = bind(json!({})).unwrap();
    let prepared = receive.prepare(&context, &args).await.unwrap();
    scope
        .validate_prepared(db, "file_received_mod", receive.as_ref(), &args, &prepared)
        .unwrap();
    let value = receive
        .execute(&context, args.clone(), prepared)
        .await
        .unwrap();
    let artifact = scope
        .finish_tool(
            &runtime.registry,
            &runtime.resolutions,
            &context,
            ("file_received_mod", None),
            &args,
            value,
        )
        .await
        .unwrap();
    assert!(
        !scope.verified(db).unwrap(),
        "a real download and import are not an install"
    );

    let next = create_task(db, &work, "task-second", "running");
    let context = ToolContext {
        resolution_scope: ResolutionScope::Agent(next),
        ..context
    };
    let scope = InstallScope::new("task-second".into(), work.clone(), game);
    let install = runtime.registry.get("install_library_mod").unwrap();
    let args = scope.arguments(db, install.as_ref(), "install_library_mod", json!({"request":{
        "artifactId": artifact["artifactId"], "mode":"approvedMappings", "sourceKind":"archive",
        "mappings":[{"source":"patch.dat", "destination":"mods/fixture/patch.dat"}]
    }})).unwrap();
    let preview = install.prepare(&context, &args).await.unwrap();
    scope
        .validate_prepared(db, "install_library_mod", install.as_ref(), &args, &preview)
        .unwrap();
    scope
        .record_plan(db, "install_library_mod", &args, &preview)
        .unwrap();
    assert!(
        !scope.verified(db).unwrap(),
        "approval/plan alone is not an installation"
    );
    install.execute(&context, args, preview).await.unwrap();
    assert!(
        scope.verified(db).unwrap(),
        "the earlier verified archive can be used after retry"
    );
    assert_eq!(
        std::fs::read(game_root.join("mods/fixture/patch.dat")).unwrap(),
        b"exact downloaded bytes"
    );
    db.conn()
        .execute(
            "UPDATE browse_install_tasks SET project_id='999' WHERE id='task-first'",
            [],
        )
        .unwrap();
    assert!(
        !scope.verified(db).unwrap(),
        "a different work cannot claim the same archive"
    );
}

#[test]
fn stock_arguments_and_3dm_contract_fail_closed() {
    let mut work = key("g");
    work.source_id = "modio".into();
    for input in [
        json!({"mod_id":46,"file_id":8}),
        json!({"game_id":6,"file_id":8}),
        json!({"file_id":8,"url":"https://fake.invalid"}),
    ] {
        assert!(stock::arguments(&work, "download_file", &mut input.clone()).is_err());
    }
    let mut args = json!({"file_id":8});
    stock::arguments(&work, "download_file", &mut args).unwrap();
    assert_eq!(args, json!({"game_id":7,"mod_id":45,"file_id":8}));
    assert!(stock::result(&json!({"isError":true,"structuredContent":{"path":"fake"}})).is_err());

    work.source_id = "glossmod_3dm".into();
    let fixture = json!({"structuredContent":{"result":{"success":true,"data":{
        "id":45,"game_id":7,"mods_title":"Work",
        "mods_resource":[{"id":8,"mods_id":45,"mods_resource_name":"patch.zip","mods_resource_version":"1.2",
            "mods_resource_latest_version":true,"mods_resource_url":"https://dmod.3dmgame.com/static/upload/resource/patch.zip"}]
    }}}});
    let resolved = three_dm::resolution(&work, &fixture)
        .unwrap()
        .download
        .unwrap();
    assert_eq!(
        (resolved.project_id.as_str(), resolved.version_id.as_str()),
        ("45", "8")
    );
    for (pointer, value) in [
        ("/structuredContent/result/data/game_id", json!(8)),
        (
            "/structuredContent/result/data/mods_resource/0/mods_id",
            json!(46),
        ),
        (
            "/structuredContent/result/data/mods_resource/0/mods_resource_url",
            json!("https://drive.google.com/example.zip"),
        ),
    ] {
        let mut bad = fixture.clone();
        *bad.pointer_mut(pointer).unwrap() = value;
        assert!(three_dm::resolution(&work, &bad).is_err());
    }
}

#[test]
fn redacted_text_only_mcp_result_preserves_windows_paths_and_escaped_labels() {
    let expected = json!({"path":r"C:\users\me\downloads\archive.zip", "label":"示例"});
    let remote = r#"{"path":"C:\\users\\me\\downloads\\archive.zip","label":"\u793a\u4f8b"}"#;
    let valid = json!({"content":[{"type":"text","text":remote}]});
    assert_eq!(stock::result(&valid).unwrap(), expected);
    let multi = json!({"content":[{"type":"text","text":remote},{"type":"text","text":remote}]});
    assert!(stock::result(&multi).is_err());
}
