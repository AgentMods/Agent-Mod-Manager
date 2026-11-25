use super::*;
use crate::transaction::tests::setup;
use tokio_util::sync::CancellationToken;

#[test]
fn library_install_tool_definitions_explain_archive_mapping_and_conflict_workflow() {
    let registry = ToolRegistry::builtin().unwrap();
    register(&registry).unwrap();

    let plan = registry.get("plan_file_ops").unwrap();
    let guidance = plan.definition().description;
    for text in [
        "match_mod_install",
        "install_library_mod",
        "外层 ZIP",
        "冲突审核",
    ] {
        assert!(guidance.contains(text), "missing plan guidance: {text}");
    }
    assert!(!plan
        .inbound_definition()
        .description
        .contains("install_library_mod"));

    let matching = registry.get("match_mod_install").unwrap().definition();
    assert_eq!(matching.effect, Effect::Read);
    for text in [
        "archiveEntries",
        "archiveEntriesTruncated",
        "完整成员路径",
        "activeInstallId",
        "inspect_install",
    ] {
        assert!(
            matching.description.contains(text),
            "missing match guidance: {text}"
        );
    }

    let install = registry.get("install_library_mod").unwrap().definition();
    assert_eq!(install.effect, Effect::ManagerWrite);
    assert!(install.description.contains("sourceKind=archive"));
    assert!(install.description.contains("sourceKind=singleFile"));
    assert!(install.description.contains("inspect_install"));
    let mapping = &install.parameters["properties"]["request"]["properties"]["mappings"]["items"]
        ["properties"];
    assert!(mapping["source"]["description"]
        .as_str()
        .unwrap()
        .contains("完整成员路径"));
    assert!(mapping["destination"]["description"]
        .as_str()
        .unwrap()
        .contains("相对文件路径"));
}

#[tokio::test]
async fn library_install_uses_archive_members_and_preserves_a_changed_target() {
    use crate::agent::mod_library_flow_tests::ingest_library_fixture;

    let (_dir, engine, game) = setup();
    let member = "Release/Patch.pak";
    let destination = "Content/Paks/~mods/Patch.pak";
    let (_, entry) =
        ingest_library_fixture(&engine, "g", "Game", "release.zip", &[(member, b"patch")]);
    let archive = engine.paths.root.join(&entry.archive_relative_path);
    let archive_bytes = std::fs::read(&archive).unwrap();
    let target = game.join(destination);
    let context = ToolContext {
        engine,
        cancel: CancellationToken::new(),
        progress: None,
        resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
    };
    let registry = ToolRegistry::default();
    register(&registry).unwrap();
    let matching = registry.get("match_mod_install").unwrap();
    let match_args = json!({"game_id":"g", "artifact_id":entry.id});
    let prepared = matching.prepare(&context, &match_args).await.unwrap();
    let report = matching
        .execute(&context, match_args, prepared)
        .await
        .unwrap();
    assert_eq!(report["archiveEntries"], json!([member]));
    assert_eq!(report.get("activeInstallId"), Some(&Value::Null));

    let tool = registry.get("install_library_mod").unwrap();
    let invalid = json!({"request":{
        "gameId":"g", "artifactId":entry.id, "mode":"approvedMappings", "sourceKind":"singleFile",
        "mappings":[{"source":"release.zip", "destination":"Patch.asi"}]
    }});
    let Err(AppError::Invalid(reason)) = tool.prepare(&context, &invalid).await else {
        panic!("expected ZIP renamed to a native plug-in to be rejected");
    };
    assert!(reason.contains("sourceKind=archive"), "{reason}");
    assert!(!game.join("Patch.asi").exists());

    let args = json!({"request":{
        "gameId":"g", "artifactId":entry.id, "mode":"approvedMappings", "sourceKind":"archive",
        "mappings":[{"source":report["archiveEntries"][0], "destination":destination}]
    }});
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert!(!target.exists());
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, b"player data").unwrap();

    let Err(AppError::Conflict(reason)) = tool.execute(&context, args, prepared).await else {
        panic!("expected an installation conflict after the target changed");
    };
    assert!(reason.contains("预览之后目标文件已变化"), "{reason}");
    assert!(reason.contains("重新匹配并提交新安装计划"), "{reason}");
    assert!(reason.contains("inspect_install"), "{reason}");
    assert_eq!(std::fs::read(&target).unwrap(), b"player data");
    assert_eq!(std::fs::read(&archive).unwrap(), archive_bytes);
}

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
async fn forget_missing_install_revalidates_after_approval_and_preserves_history_and_archive() {
    use std::io::Write;

    let (temp, engine, _game_root) = setup();
    let source_archive = temp.path().join("forgotten-mod.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&source_archive).unwrap());
    zip.start_file("patch.dat", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"patch").unwrap();
    zip.finish().unwrap();

    let imported = crate::imports::plan_import_archive(&engine, &source_archive).unwrap();
    engine.approve_plan(&imported.plan.id).unwrap();
    engine.apply_file_ops(&imported.plan.id).unwrap();
    let entry = crate::imports::register_imported_mod(&engine, "g", &imported.plan.id).unwrap();
    let archive_path = engine.paths.root.join(&entry.archive_relative_path);
    let archive_before = std::fs::read(&archive_path).unwrap();
    let request = serde_json::from_value(json!({
        "gameId":"g",
        "artifactId":entry.id,
        "mode":"approvedMappings",
        "sourceKind":"archive",
        "mappings":[{"source":"patch.dat", "destination":"mods/forgotten/patch.dat"}]
    }))
    .unwrap();
    let installed = install::plan_library_install(&engine, &request).unwrap();
    engine.approve_plan(&installed.plan.id).unwrap();
    engine.apply_file_ops(&installed.plan.id).unwrap();
    install::reconcile(&engine.db).unwrap();
    let history_count_before_forget: i64 = engine
        .db
        .conn()
        .query_row("SELECT count(*) FROM file_transactions", [], |row| {
            row.get(0)
        })
        .unwrap();

    let record = install::get_install(&engine, &installed.install_id).unwrap();
    assert_eq!(record.status, "enabled");
    assert_eq!(record.files.len(), 1);
    assert!(record.files.iter().all(|file| file.before.is_none()));
    let game_file = record.files[0].path.clone();
    assert_eq!(std::fs::read(&game_file).unwrap(), b"patch");
    std::fs::remove_file(&game_file).unwrap();
    assert!(!game_file.exists());

    let registry = ToolRegistry::default();
    register(&registry).unwrap();
    let context = ToolContext {
        engine,
        cancel: CancellationToken::new(),
        progress: None,
        resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
    };
    let inspect = registry.get("inspect_install").unwrap();
    let inspect_args = json!({"game_id":"g", "install_id":installed.install_id});
    let inspect_prepared = inspect.prepare(&context, &inspect_args).await.unwrap();
    let inspection = inspect
        .execute(&context, inspect_args, inspect_prepared)
        .await
        .unwrap();
    assert_eq!(inspection["status"], "enabled");
    assert_eq!(inspection["canUninstall"], false);
    assert_eq!(inspection["files"][0]["state"], "missing");
    assert_eq!(
        inspection["reconciliation"]["recommendedAction"],
        "forget_missing_files"
    );

    let uninstall = registry.get("uninstall_mod").unwrap();
    assert!(uninstall
        .prepare(
            &context,
            &json!({"game_id":"g", "install_id":installed.install_id})
        )
        .await
        .is_err());
    let rollback = registry.get("rollback_transaction").unwrap();
    assert!(rollback
        .prepare(
            &context,
            &json!({
                "game_id":"g",
                "install_id":installed.install_id,
                "transaction_id":installed.plan.id
            })
        )
        .await
        .is_err());
    let forget = registry.get("forget_uninstalled_mod").unwrap();
    let args = json!({"game_id":"g", "install_id":installed.install_id});
    assert!(forget
        .prepare(
            &context,
            &json!({"game_id":"wrong-game", "install_id":installed.install_id})
        )
        .await
        .is_err());

    // Approval covers only the reviewed missing-file state. An identical file
    // reappearing before execution must invalidate that approval.
    let approved = forget.prepare(&context, &args).await.unwrap();
    assert_eq!(approved.effect, Effect::ManagerWrite);
    std::fs::write(&game_file, b"patch").unwrap();
    assert!(forget
        .execute(&context, args.clone(), approved)
        .await
        .is_err());
    assert_eq!(std::fs::read(&game_file).unwrap(), b"patch");
    assert!(
        crate::mod_management::visible_installs(&context.engine, "g")
            .unwrap()
            .iter()
            .any(|item| item.id == installed.install_id)
    );

    std::fs::remove_file(&game_file).unwrap();
    let approved = forget.prepare(&context, &args).await.unwrap();
    let receipt = forget.execute(&context, args, approved).await.unwrap();
    assert_eq!(receipt["status"], "removed");
    assert_eq!(receipt["gameId"], "g");
    assert_eq!(receipt["installId"], installed.install_id);

    install::reconcile(&context.engine.db).unwrap();
    assert!(
        !crate::mod_management::visible_installs(&context.engine, "g")
            .unwrap()
            .iter()
            .any(|item| item.id == installed.install_id)
    );
    assert_eq!(
        crate::games::get_game(&context.engine.db, "g")
            .unwrap()
            .installed_mod_count,
        0
    );
    assert_eq!(std::fs::read(&archive_path).unwrap(), archive_before);
    assert!(!crate::mod_library::list_for_game(&context.engine.db, "g")
        .unwrap()
        .iter()
        .any(|item| item.id == entry.id));
    let db = context.engine.db.conn();
    let (state, install_state, txn_status, count): (String, String, String, i64) = db
        .query_row(
            "SELECT l.state, i.status, f.status, (SELECT count(*) FROM file_transactions)
         FROM mod_library_entries l, installs i JOIN install_transactions a ON a.install_id=i.id
         JOIN file_transactions f ON f.id=a.transaction_id
         WHERE l.id=?1 AND i.id=?2 AND f.id=?3 AND a.action='install'",
            rusqlite::params![entry.id, installed.install_id, installed.plan.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        (state.as_str(), install_state.as_str(), txn_status.as_str()),
        ("ready", "removed_missing", "committed")
    );
    assert_eq!(count, history_count_before_forget);
    assert!(!game_file.exists());
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
    let matching = registry.get("match_mod_install").unwrap();
    let match_args = json!({"game_id":"g", "artifact_id":entry.id});
    let prepared = matching.prepare(&context, &match_args).await.unwrap();
    let report = matching
        .execute(&context, match_args.clone(), prepared)
        .await
        .unwrap();
    assert_eq!(report["activeInstallId"], installed.install_id);

    let installer = registry.get("install_library_mod").unwrap();
    let Err(AppError::Conflict(reason)) = installer
        .prepare(&context, &json!({"request":request}))
        .await
    else {
        panic!("expected an active installation conflict");
    };
    assert!(reason.contains(&installed.install_id), "{reason}");
    assert!(reason.contains("inspect_install"), "{reason}");

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
    let prepared = matching.prepare(&context, &match_args).await.unwrap();
    let report = matching
        .execute(&context, match_args, prepared)
        .await
        .unwrap();
    assert_eq!(report.get("activeInstallId"), Some(&Value::Null));
    assert!(context
        .engine
        .paths
        .root
        .join(entry.archive_relative_path)
        .is_file());
    assert!(crate::mod_library::list_for_game(&context.engine.db, "g")
        .unwrap()
        .iter()
        .any(|item| item.id == entry.id));
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

#[tokio::test]
async fn workspace_rejects_implicit_uninstall_plan_and_file_verification() {
    let (_dir, engine, _) = setup();
    engine.db.conn().execute("INSERT INTO installs(id,game_id,name,status,install_root,files_json,installed_at,updated_at) VALUES ('workspace-owned','g','Fixture','installed','root','[]','now','now')", []).unwrap();
    let history = crate::history::History::new(engine.db.clone());
    let conversation = history
        .create_conversation("Empty workspace", None)
        .unwrap();
    history
        .set_conversation_workspace(&conversation.id, Some(&[]))
        .unwrap();
    let context = ToolContext {
        engine,
        cancel: CancellationToken::new(),
        progress: None,
        resolution_scope: crate::sources::resolutions::ResolutionScope::Agent(conversation.id),
    };
    for (name, args) in [
        ("uninstall_mod", json!({"install_id":"workspace-owned"})),
        ("plan_install", json!({"request":{"gameId":"g"}})),
    ] {
        let tool = InstallTool {
            name: name.into(),
            tool_available: Arc::new(|_| false),
        };
        assert!(
            matches!(
                tool.prepare(&context, &args).await,
                Err(AppError::Invalid(_))
            ),
            "{name}"
        );
    }
    let args = json!({"game_id":"g","install_id":"workspace-owned"});
    assert!(matches!(
        VerifyInstallFiles.prepare(&context, &args).await,
        Err(AppError::Invalid(_))
    ));
    let prepared = PreparedTool {
        preview: args.clone(),
        effect: Effect::Read,
        plan: None,
    };
    assert!(matches!(
        VerifyInstallFiles.execute(&context, args, prepared).await,
        Err(AppError::Invalid(_))
    ));
}

fn commit_historical_install(
    engine: &crate::transaction::FileEngine,
    game: &std::path::Path,
    install_id: &str,
    name: &str,
    target: &std::path::Path,
    bytes: &[u8],
) -> crate::transaction::FilePlan {
    let source = engine.paths.downloads().join(format!("{install_id}.dat"));
    std::fs::write(&source, bytes).unwrap();
    let plan = engine
        .plan_file_ops(
            vec![crate::transaction::FileOp::Copy {
                src: source,
                dst: target.to_path_buf(),
            }],
            format!("Historical install {name}"),
        )
        .unwrap();
    engine.approve_plan(&plan.id).unwrap();
    engine.apply_file_ops(&plan.id).unwrap();

    let payload = plan.entries[0].source.as_ref().unwrap();
    let files = vec![install::InstalledFile {
        path: target.to_path_buf(),
        sha256: payload.sha256.clone(),
        size: payload.size,
        before: plan.entries[0].before.clone(),
    }];
    let conn = engine.db.conn();
    conn.execute(
        "INSERT INTO installs(id,game_id,name,status,install_root,files_json,installed_at,updated_at)
         VALUES (?1,'g',?2,'enabled',?3,?4,'now','now')",
        rusqlite::params![
            install_id,
            name,
            game.display().to_string(),
            serde_json::to_string(&files).unwrap(),
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO install_transactions(install_id,transaction_id,action,created_at)
         VALUES (?1,?2,'install','now')",
        rusqlite::params![install_id, plan.id],
    )
    .unwrap();
    conn.execute(
        "UPDATE file_transactions SET game_id='g' WHERE id=?1",
        [&plan.id],
    )
    .unwrap();
    drop(conn);
    crate::install::reconcile(&engine.db).unwrap();
    plan
}

#[tokio::test]
async fn scoped_agent_rollback_unwinds_successive_overwrites_in_reverse_order() {
    for (case, previous_payload, successor_payload) in [
        (
            "different contents",
            &b"previous Mod payload"[..],
            &b"successor Mod payload"[..],
        ),
        (
            "identical contents",
            &b"same Mod payload"[..],
            &b"same Mod payload"[..],
        ),
    ] {
        let (_dir, engine, game) = setup();
        let target = game.join("Mods/shared.dat");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"original game file").unwrap();

        let first = commit_historical_install(
            &engine,
            &game,
            "install-first",
            "Previous Mod",
            &target,
            previous_payload,
        );
        let second = commit_historical_install(
            &engine,
            &game,
            "install-second",
            "Successor Mod",
            &target,
            successor_payload,
        );
        let replacement_count: i64 = engine
            .db
            .conn()
            .query_row("SELECT count(*) FROM install_replacements", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            replacement_count, 0,
            "{case}: fixture must be ordinary installs"
        );

        let backup: String = engine
            .db
            .conn()
            .query_row(
                "SELECT backup_path FROM file_transaction_entries WHERE transaction_id=?1 AND seq=0",
                [&second.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(std::fs::read(backup).unwrap(), previous_payload, "{case}");
        assert_eq!(std::fs::read(&target).unwrap(), successor_payload, "{case}");

        let registry = ToolRegistry::default();
        register(&registry).unwrap();
        let context = ToolContext {
            engine,
            cancel: CancellationToken::new(),
            progress: None,
            resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
        };
        let inspect = registry.get("inspect_install").unwrap();
        let rollback = registry.get("rollback_transaction").unwrap();

        let first_args = json!({"game_id":"g", "install_id":"install-first"});
        let first_inspection = inspect.prepare(&context, &first_args).await.unwrap();
        let first_report = inspect
            .execute(&context, first_args.clone(), first_inspection)
            .await
            .unwrap();
        assert_eq!(first_report["status"], "enabled", "{case}");
        assert_eq!(
            first_report["reconciliation"]["recommendedAction"], "review_shared_paths",
            "{case}"
        );
        let first_rollback_args = json!({
            "game_id":"g",
            "install_id":"install-first",
            "transaction_id":first.id,
        });
        assert!(
            matches!(
                rollback.prepare(&context, &first_rollback_args).await,
                Err(AppError::Conflict(_))
            ),
            "{case}: the older transaction must wait for its successor"
        );
        let premature = PreparedTool {
            preview: json!({"action":"rollback"}),
            effect: Effect::ManagerWrite,
            plan: None,
        };
        assert!(
            matches!(
                rollback
                    .execute(&context, first_rollback_args.clone(), premature)
                    .await,
                Err(AppError::Conflict(_))
            ),
            "{case}: execute must recheck order even without a fresh prepare"
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            successor_payload,
            "{case}: an earlier transaction must not replace successor contents"
        );

        let second_args = json!({"game_id":"g", "install_id":"install-second"});
        let second_inspection = inspect.prepare(&context, &second_args).await.unwrap();
        let second_report = inspect
            .execute(&context, second_args, second_inspection)
            .await
            .unwrap();
        assert_eq!(second_report["status"], "enabled", "{case}");
        assert_eq!(
            second_report["reconciliation"]["recommendedAction"], "rollback_install",
            "{case}"
        );
        assert_eq!(
            second_report["reconciliation"]["rollbackTransactionId"], second.id,
            "{case}"
        );
        assert_eq!(
            second_report["reconciliation"]["blockingInstallIds"],
            json!([])
        );
        assert_eq!(
            second_report["files"][0]["otherInstallIds"],
            json!(["install-first"])
        );
        assert_eq!(second_report["files"][0]["state"], "owned", "{case}");

        let second_rollback_args = json!({
            "game_id":"g",
            "install_id":"install-second",
            "transaction_id":second.id,
        });
        let stale_review = rollback
            .prepare(&context, &second_rollback_args)
            .await
            .unwrap();
        std::fs::write(&target, b"player edit after approval").unwrap();
        assert!(
            matches!(
                rollback
                    .execute(&context, second_rollback_args.clone(), stale_review)
                    .await,
                Err(AppError::Conflict(_))
            ),
            "{case}: execution must reject a target changed after prepare"
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"player edit after approval",
            "{case}: preserve the post-approval edit"
        );

        std::fs::write(&target, successor_payload).unwrap();
        let second_review = rollback
            .prepare(&context, &second_rollback_args)
            .await
            .unwrap();
        assert_eq!(
            rollback
                .execute(&context, second_rollback_args, second_review)
                .await
                .unwrap()["status"],
            "rolled_back",
            "{case}"
        );
        assert_eq!(std::fs::read(&target).unwrap(), previous_payload, "{case}");

        let first_inspection = inspect.prepare(&context, &first_args).await.unwrap();
        let first_report = inspect
            .execute(&context, first_args.clone(), first_inspection)
            .await
            .unwrap();
        assert_eq!(
            first_report["reconciliation"]["recommendedAction"], "rollback_install",
            "{case}"
        );
        assert_eq!(
            first_report["reconciliation"]["rollbackTransactionId"], first.id,
            "{case}"
        );
        let first_review = rollback
            .prepare(&context, &first_rollback_args)
            .await
            .unwrap();
        assert_eq!(
            rollback
                .execute(&context, first_rollback_args, first_review)
                .await
                .unwrap()["status"],
            "rolled_back",
            "{case}"
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"original game file",
            "{case}"
        );

        let forget = registry.get("forget_uninstalled_mod").unwrap();
        for install_id in ["install-second", "install-first"] {
            let forget_args = json!({"game_id":"g", "install_id":install_id});
            let forget_review = forget.prepare(&context, &forget_args).await.unwrap();
            let receipt = forget
                .execute(&context, forget_args, forget_review)
                .await
                .unwrap();
            assert_eq!(receipt["status"], "removed", "{case}: {install_id}");
            assert_eq!(receipt["installId"], install_id, "{case}");
        }
        let list = registry.get("list_installs").unwrap();
        let list_args = json!({"game_id":"g"});
        let listed = list
            .execute(
                &context,
                list_args.clone(),
                list.prepare(&context, &list_args).await.unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed, json!([]), "{case}: both records should be removed");
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"original game file",
            "{case}"
        );
    }
}

#[tokio::test]
async fn overlapping_install_rollback_rejects_same_hash_with_different_file_identity() {
    let (_dir, engine, game) = setup();
    let target = game.join("Mods/shared.dat");
    let displaced = game.join("Mods/shared.displaced.dat");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, b"original game file").unwrap();

    let first = commit_historical_install(
        &engine,
        &game,
        "install-first",
        "Mod A",
        &target,
        b"same Mod payload",
    );
    std::fs::rename(&target, &displaced).unwrap();
    std::fs::write(&target, b"same Mod payload").unwrap();
    assert_eq!(std::fs::read(&displaced).unwrap(), b"same Mod payload");
    assert_eq!(std::fs::read(&target).unwrap(), b"same Mod payload");
    let first_file_identity = crate::safe_fs::identity(&displaced).unwrap();
    let recreated_file_identity = crate::safe_fs::identity(&target).unwrap();
    assert_ne!(
        first_file_identity, recreated_file_identity,
        "the recreated path has equal bytes but a different filesystem object"
    );

    let second = commit_historical_install(
        &engine,
        &game,
        "install-second",
        "Mod B",
        &target,
        b"Mod B payload",
    );
    let backup: String = engine
        .db
        .conn()
        .query_row(
            "SELECT backup_path FROM file_transaction_entries WHERE transaction_id=?1 AND seq=0",
            [&second.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(std::fs::read(&backup).unwrap(), b"same Mod payload");
    let backup_identity = crate::safe_fs::identity(std::path::Path::new(&backup)).unwrap();
    assert_eq!(backup_identity, recreated_file_identity);
    assert_ne!(backup_identity, first_file_identity);

    let registry = ToolRegistry::default();
    register(&registry).unwrap();
    let context = ToolContext {
        engine,
        cancel: CancellationToken::new(),
        progress: None,
        resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
    };
    let inspect = registry.get("inspect_install").unwrap();
    let rollback = registry.get("rollback_transaction").unwrap();
    let inspect_args = json!({"game_id":"g", "install_id":"install-second"});
    let inspection = inspect.prepare(&context, &inspect_args).await.unwrap();
    let report = inspect
        .execute(&context, inspect_args, inspection)
        .await
        .unwrap();
    assert_eq!(report["status"], "enabled");
    assert_eq!(
        report["reconciliation"]["recommendedAction"],
        "review_shared_paths"
    );
    assert_eq!(
        report["reconciliation"]["rollbackTransactionId"],
        Value::Null
    );

    let scoped_args = json!({
        "game_id":"g",
        "install_id":"install-second",
        "transaction_id":second.id,
    });
    assert!(matches!(
        rollback.prepare(&context, &scoped_args).await,
        Err(AppError::Conflict(_))
    ));

    let transaction_only_args = json!({"transaction_id":second.id});
    let transaction_only_review = rollback
        .prepare(&context, &transaction_only_args)
        .await
        .unwrap();
    assert!(matches!(
        rollback
            .execute(&context, transaction_only_args, transaction_only_review)
            .await,
        Err(AppError::Conflict(_))
    ));
    assert_eq!(std::fs::read(&target).unwrap(), b"Mod B payload");
    assert_eq!(std::fs::read(&displaced).unwrap(), b"same Mod payload");

    // Keep both committed history records in the fixture: the refusal must be
    // based on file identity, not on a successful or failed earlier rollback.
    let first_status: String = context
        .engine
        .db
        .conn()
        .query_row(
            "SELECT status FROM file_transactions WHERE id=?1",
            [&first.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(first_status, "committed");
}

#[test]
fn three_legacy_owners_with_identical_payloads_only_rollback_from_the_top() {
    let (_dir, engine, game) = setup();
    let path = game.join("shared.dll");
    std::fs::write(&path, b"original").unwrap();
    let ids = ["first", "middle", "last"];
    let plans = ids.map(|id| commit_historical_install(&engine, &game, id, id, &path, b"shared"));
    for top in (0..ids.len()).rev() {
        for earlier in &plans[..top] {
            assert!(engine.rollback_transaction(&earlier.id).is_err());
        }
        let report = install::inspect_install(&engine, ids[top]).unwrap();
        assert_eq!(report.reconciliation.recommended_action, "rollback_install");
        assert_eq!(
            report.reconciliation.rollback_transaction_id.as_ref(),
            Some(&plans[top].id)
        );
        engine.rollback_transaction(&plans[top].id).unwrap();
        let expected = if top == 0 {
            &b"original"[..]
        } else {
            &b"shared"[..]
        };
        assert_eq!(std::fs::read(&path).unwrap(), expected);
    }
}
