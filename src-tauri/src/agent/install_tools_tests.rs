use super::*;
use crate::transaction::tests::setup;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn dependency_check_is_read_only_and_returns_the_core_report() {
    let (_dir, engine, _) = setup();
    let registry = ToolRegistry::default();
    register(&registry).unwrap();
    let tool = registry.get("check_modrinth_dependencies").unwrap();
    assert_eq!(tool.definition().effect, Effect::Read);
    let context = ToolContext {
        engine,
        cancel: CancellationToken::new(),
        progress: None,
        resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
    };
    for request in [
        json!({"gameId": "g", "dependencies": null, "replaceInstallId": null}),
        json!({"gameId": "g", "dependencies": []}),
        json!({"gameId": "g", "dependencies": [
            {"dependency_type": "required", "project_id": "P1", "version_id": "V1"},
            {"dependency_type": "optional", "project_id": "P2", "version_id": null},
            {"dependency_type": "embedded", "project_id": null, "file_name": "embedded.jar"},
            {"dependency_type": "incompatible", "project_id": "P3", "file_name": null}
        ]}),
    ] {
        let expected = install_dependencies::check(
            &context.engine,
            &serde_json::from_value(request.clone()).unwrap(),
        )
        .unwrap();
        let args = json!({"request": request});
        let prepared = tool.prepare(&context, &args).await.unwrap();
        assert_eq!(prepared.effect, Effect::Read);
        assert!(prepared.plan.is_none());
        let report = tool.execute(&context, args, prepared).await.unwrap();
        assert_eq!(report, serde_json::to_value(expected).unwrap());
    }
}

#[tokio::test]
async fn dependency_check_rejects_missing_or_oversized_declarations_and_honors_cancellation() {
    let (_dir, engine, _) = setup();
    let context = ToolContext {
        engine,
        cancel: CancellationToken::new(),
        progress: None,
        resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
    };
    let tool = CheckModrinthDependencies;
    for args in [
        json!({}),
        json!({"request": {"dependencies": null}}),
        json!({"request": {"gameId": "g"}}),
        json!({"request": {"gameId": "g", "dependencies": [{"dependency_type": "unknown"}]}}),
    ] {
        let prepared = tool.prepare(&context, &args).await.unwrap();
        assert!(matches!(
            tool.execute(&context, args, prepared).await,
            Err(AppError::Json(_))
        ));
    }

    let oversized = vec![json!({"dependency_type": "required", "project_id": "P1"}); 129];
    let args = json!({"request": {"gameId": "g", "dependencies": oversized}});
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert!(matches!(
        tool.execute(&context, args, prepared).await,
        Err(AppError::Invalid(_))
    ));

    context.cancel.cancel();
    let args = json!({"request": {"gameId": "g", "dependencies": []}});
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert!(matches!(
        tool.execute(&context, args, prepared).await,
        Err(AppError::Cancelled)
    ));
}

#[tokio::test]
async fn verify_install_files_is_scoped_read_only_and_cancellable() {
    let (dir, engine, root) = setup();
    let game = crate::games::register_game(&engine.db, &root, None).unwrap();
    let other_root = dir.path().join("other");
    std::fs::create_dir(&other_root).unwrap();
    let other = crate::games::register_game(&engine.db, &other_root, None).unwrap();
    let jar = root.join("example.jar");
    std::fs::write(&jar, b"before").unwrap();
    let sha = crate::util::sha256_file(&jar).unwrap();
    let manifest = json!([{ "path": jar, "size": 6, "sha256": sha, "before": null }]);
    {
        let conn = engine.db.conn();
        conn.execute("INSERT INTO installs(id,game_id,name,status,install_root,files_json,installed_at,updated_at)
            VALUES ('i_fixture',?1,'Fixture','planned',?2,?3,'now','now')",
            rusqlite::params![game.id, game.root_path, manifest.to_string()]).unwrap();
        conn.execute_batch(
            "INSERT INTO file_transactions(id,kind,status,plan_json,created_at)
            VALUES ('txn_fixture','install','committed','{}','now');
            INSERT INTO install_transactions(install_id,transaction_id,action,created_at)
            VALUES ('i_fixture','txn_fixture','install','now');",
        )
        .unwrap();
    }
    std::fs::write(&jar, b"edited").unwrap();
    let registry = ToolRegistry::default();
    register(&registry).unwrap();
    let tool = registry.get("verify_install_files").unwrap();
    assert_eq!(tool.definition().effect, Effect::Read);
    let context = ToolContext {
        engine,
        cancel: CancellationToken::new(),
        progress: None,
        resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
    };
    let args = json!({"game_id":game.id, "install_id":"i_fixture"});
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert_eq!(prepared.effect, Effect::Read);
    assert!(prepared.plan.is_none());
    let result = tool
        .execute(&context, args.clone(), prepared)
        .await
        .unwrap();
    assert_eq!(result["evidence"]["status"], "mismatch");
    assert_eq!(
        result["evidence"]["issues"][0],
        json!({"path":"example.jar","state":"modified"})
    );
    let stored_status: String = context
        .engine
        .db
        .conn()
        .query_row(
            "SELECT status FROM installs WHERE id='i_fixture'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        stored_status, "planned",
        "this tool reads the journal without reconciliation"
    );
    assert_eq!(std::fs::read(jar).unwrap(), b"edited");

    let wrong_game = json!({"game_id":other.id, "install_id":"i_fixture"});
    let prepared = tool.prepare(&context, &wrong_game).await.unwrap();
    assert!(matches!(
        tool.execute(&context, wrong_game, prepared).await,
        Err(AppError::NotFound(_))
    ));
    let prepared = tool.prepare(&context, &args).await.unwrap();
    context.cancel.cancel();
    assert!(matches!(
        tool.execute(&context, args, prepared).await,
        Err(AppError::Cancelled)
    ));
}

#[tokio::test]
async fn inspect_install_is_read_only_and_checks_game_scope() {
    let (_dir, engine, root) = setup();
    let game = crate::games::register_game(&engine.db, &root, None).unwrap();
    let file = root.join("example.jar");
    std::fs::write(&file, b"owned").unwrap();
    let sha = crate::util::sha256_file(&file).unwrap();
    let manifest = json!([{
        "path": file,
        "size": 5,
        "sha256": sha,
        "before": null
    }]);
    engine
        .db
        .conn()
        .execute(
            "INSERT INTO installs(id,game_id,name,status,install_root,files_json,installed_at,updated_at)
             VALUES ('i_inspect',?1,'Fixture','enabled',?2,?3,'now','now')",
            rusqlite::params![game.id, game.root_path, manifest.to_string()],
        )
        .unwrap();

    let registry = ToolRegistry::default();
    register(&registry).unwrap();
    let tool = registry.get("inspect_install").unwrap();
    assert_eq!(tool.definition().effect, Effect::Read);
    let context = ToolContext {
        engine,
        cancel: CancellationToken::new(),
        progress: None,
        resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
    };
    let args = json!({"game_id": game.id, "install_id": "i_inspect"});
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert!(prepared.plan.is_none());
    let report = tool
        .execute(&context, args.clone(), prepared)
        .await
        .unwrap();
    assert_eq!(report["canUninstall"], true);
    assert_eq!(
        report["reconciliation"]["recommendedAction"],
        "safe_uninstall"
    );
    assert_eq!(report["files"][0]["state"], "owned");
    assert_eq!(report["files"][0]["replaced"], false);
    assert_eq!(std::fs::read(&file).unwrap(), b"owned");
    let status: String = context
        .engine
        .db
        .conn()
        .query_row(
            "SELECT status FROM installs WHERE id='i_inspect'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "enabled");

    std::fs::write(&file, b"edited").unwrap();
    let prepared = tool.prepare(&context, &args).await.unwrap();
    let report = tool.execute(&context, args, prepared).await.unwrap();
    assert_eq!(report["canUninstall"], false);
    assert_eq!(
        report["reconciliation"]["recommendedAction"],
        "review_file_state"
    );
    assert_eq!(report["files"][0]["state"], "modified");
}

#[tokio::test]
async fn scoped_uninstall_restores_the_preinstall_bytes_only_for_the_reviewed_game_and_transaction()
{
    use std::io::Write;
    let (temp, engine, game) = setup();
    let original = temp.path().canonicalize().unwrap().join("release.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&original).unwrap());
    zip.start_file("patch.dat", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"patch").unwrap();
    zip.finish().unwrap();
    let imported = crate::imports::plan_import_archive(&engine, &original).unwrap();
    engine.approve_plan(&imported.plan.id).unwrap();
    engine.apply_file_ops(&imported.plan.id).unwrap();
    let entry = crate::imports::register_imported_mod(&engine, "g", &imported.plan.id).unwrap();
    let target = game.join("original.dat");
    std::fs::write(&target, b"original").unwrap();
    let request = serde_json::from_value(json!({
        "gameId":"g", "artifactId":entry.id, "mode":"approvedMappings", "sourceKind":"archive",
        "mappings":[{"source":"patch.dat", "destination":"original.dat"}]
    }))
    .unwrap();
    let installed = install::plan_library_install(&engine, &request).unwrap();
    engine.approve_plan(&installed.plan.id).unwrap();
    engine.apply_file_ops(&installed.plan.id).unwrap();
    let registry = ToolRegistry::default();
    register(&registry).unwrap();
    let context = ToolContext {
        engine,
        cancel: CancellationToken::new(),
        progress: None,
        resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
    };
    let rollback = registry.get("rollback_transaction").unwrap();
    let args = json!({"game_id":"g", "install_id":installed.install_id, "transaction_id":installed.plan.id});
    assert!(rollback.prepare(&context, &json!({"game_id":"elsewhere", "install_id":installed.install_id,"transaction_id":installed.plan.id})).await.is_err());
    assert!(rollback.prepare(&context, &json!({"game_id":"g", "install_id":installed.install_id,"transaction_id":imported.plan.id})).await.is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"patch");
    let review = rollback.prepare(&context, &args).await.unwrap();
    std::fs::write(&target, b"player edit").unwrap();
    assert!(rollback
        .execute(&context, args.clone(), review)
        .await
        .is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"player edit");
    std::fs::write(&target, b"patch").unwrap();
    let review = rollback.prepare(&context, &args).await.unwrap();
    let outcome = rollback.execute(&context, args, review).await.unwrap();
    assert_eq!(outcome["status"], "rolled_back");
    assert_eq!(std::fs::read(target).unwrap(), b"original");
    assert!(context
        .engine
        .paths
        .root
        .join(entry.archive_relative_path)
        .is_file());
    let forget = registry.get("forget_uninstalled_mod").unwrap();
    assert!(forget
        .prepare(
            &context,
            &json!({"game_id":"wrong-game","install_id":installed.install_id})
        )
        .await
        .is_err());
    let args = json!({"game_id":"g","install_id":installed.install_id});
    let approved = forget.prepare(&context, &args).await.unwrap();
    assert_eq!(approved.effect, Effect::ManagerWrite);
    let receipt = forget.execute(&context, args, approved).await.unwrap();
    assert_eq!(receipt["status"], "removed");
    assert!(
        crate::mod_management::visible_installs(&context.engine, "g")
            .unwrap()
            .is_empty()
    );
}
