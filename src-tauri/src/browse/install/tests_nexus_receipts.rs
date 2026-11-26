use super::*;
use crate::{
    agent::tools::ToolContext, mcp::client::namespaced_tool_name,
    sources::resolutions::ResolutionScope, transaction::FileEngine,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{io::Write, path::PathBuf};

fn nexus(game_id: &str) -> WorkKey {
    WorkKey {
        game_id: game_id.into(),
        source_id: "nexusmods".into(),
        project_id: "45".into(),
        provider_game_id: "stardewvalley".into(),
        project_page: None,
    }
}

fn context(engine: &FileEngine, conversation: String) -> ToolContext {
    ToolContext {
        engine: engine.clone(),
        cancel: tokio_util::sync::CancellationToken::new(),
        progress: None,
        resolution_scope: ResolutionScope::Agent(conversation),
    }
}

fn archive(engine: &FileEngine, payload: &[u8]) -> (PathBuf, u64, String) {
    let path = crate::storage::incoming_downloads(&engine.paths.root).join("Nexus release.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    // A reconstructed fixture with the same payload must reproduce the exact download digest.
    let options =
        zip::write::SimpleFileOptions::default().last_modified_time(zip::DateTime::default());
    zip.start_file("patch.dat", options).unwrap();
    zip.write_all(payload).unwrap();
    zip.finish().unwrap();
    let archive = std::fs::read(&path).unwrap();
    let hash = hex::encode(Sha256::digest(&archive));
    (path, archive.len() as u64, hash)
}

fn save_call(
    db: &Db,
    ctx: &ToolContext,
    id: &str,
    tool: &str,
    args: &Value,
    status: &str,
    result: &Value,
) {
    let ResolutionScope::Agent(conversation) = &ctx.resolution_scope else {
        unreachable!()
    };
    db.conn()
        .execute(
            "INSERT INTO tool_calls(id, conversation_id, name, arguments_json, status, result_json, started_at)
         VALUES(?1,?2,?3,?4,?5,?6,'now')",
            params![id, conversation, tool, args.to_string(), status, result.to_string()],
        )
        .unwrap();
}

#[tokio::test]
async fn nexus_download_survives_rebuilt_scope_and_commits_real_install_receipt() {
    let (_dir, engine, game_root) = setup();
    let runtime = runtime(engine.clone());
    let db = &engine.db;
    let page = "https://www.nexusmods.com/stardewvalley/mods/45";
    let work = WorkKey {
        project_page: Some(page.into()),
        ..nexus("g")
    };
    let game = GameScope::new(crate::games::get_game(db, "g").unwrap());
    let conversation = create_task(db, &work, "task", "running");
    db.conn()
        .execute(
            "INSERT INTO browse_install_project_pages(task_id,project_page) VALUES('task',?1)",
            [page],
        )
        .unwrap();
    let ctx = context(&engine, conversation.clone());
    let listing = namespaced_tool_name("nexusmods", "list_files");
    save_call(
        db,
        &ctx,
        "listing",
        &listing,
        &json!({}),
        "done",
        &json!({
            "version":"work-current",
            "files":[{"fileId":91,"modId":45,"version":"9.0"},
                     {"fileId":90,"modId":45,"version":"2.1"}]
        }),
    );
    save_call(
        db,
        &ctx,
        "other-project",
        &listing,
        &json!({"mod_id":46}),
        "done",
        &json!({
            "files":[{"fileId":90,"modId":46,"version":"8.1"}]
        }),
    );
    let scope = InstallScope::new("task".into(), work.clone(), game.clone());
    let (path, size, hash) = archive(&engine, b"nexus fixture");
    let mut args = json!({"file_id":90});
    stock::arguments(&work, "download_file", &mut args).unwrap();
    assert_eq!(
        args,
        json!({"game_domain":"stardewvalley","mod_id":45,"file_id":90})
    );
    let download = namespaced_tool_name("nexusmods", "download_file");
    let transfer = json!({"path":path,"bytes":size,"hashes":{"sha256":hash}});
    let mut foreign = transfer.clone();
    foreign["game_domain"] = json!("skyrim");
    assert!(stock::transfer_receipt(
        &work,
        &args,
        &json!({"structuredContent":{"result":foreign}}),
        &ctx,
        &runtime.registry,
    )
    .await
    .is_err());
    let saved = scope
        .finish_tool(
            &runtime.registry,
            &runtime.resolutions,
            &ctx,
            (&download, Some(("nexusmods", "download_file"))),
            &args,
            json!({"structuredContent":{"result":transfer}}),
        )
        .await
        .unwrap();
    assert_eq!(saved["version"], "2.1");
    save_call(db, &ctx, "download", &download, &args, "done", &saved);
    drop(scope);
    let scope = conversation_scope(&db.conn(), &conversation)
        .unwrap()
        .unwrap();
    let receive = runtime.registry.get("file_received_mod").unwrap();
    let bind = |args| scope.arguments(db, receive.as_ref(), "file_received_mod", args);
    for invalid in [
        json!({"filename":"different.zip"}),
        json!({"gameId":"different"}),
        json!({"gameName":"Other Game"}),
        json!({"sourcePath":path}),
    ] {
        assert!(bind(invalid).is_err());
    }
    let args = bind(json!({
        "filename":saved["filename"], "gameName":game.name,
        "modName":"model label", "projectPage":"https://example.invalid/fake",
    }))
    .unwrap();
    assert_eq!(
        args,
        json!({"filename":saved["filename"], "gameId":"g", "projectPage":page})
    );
    let prepared = receive.prepare(&ctx, &args).await.unwrap();
    scope
        .validate_prepared(db, "file_received_mod", receive.as_ref(), &args, &prepared)
        .unwrap();
    let value = receive.execute(&ctx, args.clone(), prepared).await.unwrap();
    let artifact = scope
        .finish_tool(
            &runtime.registry,
            &runtime.resolutions,
            &ctx,
            ("file_received_mod", None),
            &args,
            value,
        )
        .await
        .unwrap();
    assert_eq!(artifact["sha256"], hash);
    assert_eq!(artifact["gameId"], "g");
    assert_eq!(artifact["projectPage"], page);
    assert_eq!(artifact["version"], "2.1");
    assert_eq!(artifact["sourceId"], "local");
    assert!(artifact["projectId"].is_null());
    assert!(artifact["versionId"].is_null());
    {
        let conn = db.conn();
        let entry = conn
            .query_row(
                &format!(
                    "SELECT {} FROM mod_library_entries WHERE id=?1",
                    mod_library::COLUMNS
                ),
                [artifact["artifactId"].as_str().unwrap()],
                mod_library::read_entry,
            )
            .unwrap();
        assert_eq!(entry.version, "2.1");
        assert_eq!(entry.project_page.as_deref(), Some(page));
        assert_eq!(
            stock_archive_metadata(&conn, &entry).unwrap(),
            (Some("2.1".into()), Some(page.into()))
        );
    }
    assert!(!scope.verified(db).unwrap());

    let matcher = runtime.registry.get("match_mod_install").unwrap();
    let args = scope
        .arguments(
            db,
            matcher.as_ref(),
            "match_mod_install",
            json!({"artifact_id":artifact["artifactId"]}),
        )
        .unwrap();
    let prepared = matcher.prepare(&ctx, &args).await.unwrap();
    let matched = matcher.execute(&ctx, args, prepared).await.unwrap();
    assert_eq!(matched["artifactSha256"], hash);
    let install = runtime.registry.get("install_library_mod").unwrap();
    let args = scope.arguments(db, install.as_ref(), "install_library_mod", json!({"request":{
        "artifactId": artifact["artifactId"], "mode":"approvedMappings", "sourceKind":"archive",
        "mappings":[{"source":"patch.dat", "destination":"mods/nexus/patch.dat"}]
    }})).unwrap();
    let preview = install.prepare(&ctx, &args).await.unwrap();
    scope
        .validate_prepared(db, "install_library_mod", install.as_ref(), &args, &preview)
        .unwrap();
    scope
        .record_plan(db, "install_library_mod", &args, &preview)
        .unwrap();
    assert!(!scope.verified(db).unwrap());
    install.execute(&ctx, args, preview).await.unwrap();
    assert!(scope.verified(db).unwrap());
    assert_eq!(
        std::fs::read(game_root.join("mods/nexus/patch.dat")).unwrap(),
        b"nexus fixture"
    );
}

#[tokio::test]
async fn nexus_legacy_receipt_recovers_only_for_matching_work_and_existing_archive() {
    let (_dir, engine, _) = setup();
    let runtime = runtime(engine.clone());
    let db = &engine.db;
    let work = nexus("g");
    let game = GameScope::new(crate::games::get_game(db, "g").unwrap());
    let legacy = WorkKey {
        project_id: "45".into(),
        ..work.clone()
    };
    let old = context(&engine, create_task(db, &legacy, "old", "done"));
    let ctx = context(&engine, create_task(db, &work, "new", "running"));
    let scope = InstallScope::new("new".into(), work.clone(), game.clone());
    let (path, size, hash) = archive(&engine, b"original bytes");
    let saved = json!({"status":"saved","filename":"Nexus release.zip","sha256":hash,
        "size":size,"fileId":"90","gameId":"g"});
    let download = namespaced_tool_name("nexusmods", "download_file");
    let receive = runtime.registry.get("file_received_mod").unwrap();
    let bind = |args| scope.arguments(db, receive.as_ref(), "file_received_mod", args);
    save_call(
        db,
        &old,
        "wrong-tool",
        "ask_user",
        &json!({}),
        "done",
        &saved,
    );
    assert!(bind(json!({"filename":saved["filename"]})).is_err());
    save_call(
        db,
        &old,
        "legacy",
        &download,
        &json!({"file_id":90}),
        "done",
        &saved,
    );
    for foreign in [
        WorkKey {
            project_id: "stardewvalley/46".into(),
            ..work.clone()
        },
        WorkKey {
            provider_game_id: "skyrim".into(),
            ..work.clone()
        },
        WorkKey {
            source_id: "gamebanana".into(),
            ..work.clone()
        },
        WorkKey {
            game_id: "other".into(),
            ..work.clone()
        },
    ] {
        let foreign = InstallScope::new("foreign".into(), foreign, game.clone());
        assert!(foreign
            .arguments(db, receive.as_ref(), "file_received_mod", json!({}))
            .is_err());
    }
    let args = bind(json!({})).unwrap();
    // A same-name replacement cannot use the old saved proof.
    archive(&engine, b"changed bytes!");
    let prepared = receive.prepare(&ctx, &args).await.unwrap();
    assert!(scope
        .validate_prepared(db, "file_received_mod", receive.as_ref(), &args, &prepared)
        .is_err());
    let wrong = receive.execute(&ctx, args.clone(), prepared).await.unwrap();
    archive(&engine, b"original bytes");
    // Simulate an old build that published the archive before saving the work link.
    let display_args =
        json!({"filename":saved["filename"], "gameId":"g", "modName":"Old display name"});
    let prepared = receive.prepare(&ctx, &display_args).await.unwrap();
    let artifact = receive.execute(&ctx, display_args, prepared).await.unwrap();
    std::fs::remove_file(path).unwrap();
    let list = runtime.registry.get("list_mod_library").unwrap();
    let args = scope
        .arguments(db, list.as_ref(), "list_mod_library", json!({}))
        .unwrap();
    let prepared = list.prepare(&ctx, &args).await.unwrap();
    let value = list.execute(&ctx, args.clone(), prepared).await.unwrap();
    assert!(value.as_array().unwrap().contains(&wrong));
    let scoped = scope
        .finish_tool(
            &runtime.registry,
            &runtime.resolutions,
            &ctx,
            ("list_mod_library", None),
            &args,
            value,
        )
        .await
        .unwrap();
    assert_eq!(scoped, json!([artifact]));
    let saved_link: (String, String) = db
        .conn()
        .query_row(
            "SELECT artifact_id, file_id FROM browse_install_downloads WHERE task_id='new'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        saved_link,
        (artifact["artifactId"].as_str().unwrap().into(), "90".into())
    );
    save_call(
        db,
        &ctx,
        "failed-newer",
        &download,
        &json!({"file_id":90}),
        "error",
        &saved,
    );
    assert!(bind(json!({})).is_err());
}
