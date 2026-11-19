use super::*;
use crate::{plugins, storage::AppPaths};
use std::io::Write;

fn fixture(slug: &str) -> (tempfile::TempDir, FileEngine, games::Game) {
    let temp = tempfile::tempdir().unwrap();
    let data = AppPaths::new(temp.path().join("data")).unwrap();
    let engine = FileEngine::new(Db::open(&data.db_file()).unwrap(), data);
    plugins::seed_builtins(&engine.db, &engine.paths).unwrap();
    let root = temp.path().join("game");
    std::fs::create_dir(&root).unwrap();
    match slug {
        "minecraft-java" => std::fs::write(root.join("options.txt"), "test").unwrap(),
        "stardew-valley" => std::fs::write(root.join("Stardew Valley.dll"), "game").unwrap(),
        "skyrim-se" => {
            std::fs::write(root.join("SkyrimSE.exe"), "game").unwrap();
            std::fs::create_dir(root.join("Data")).unwrap();
            std::fs::write(root.join("Data/Skyrim.esm"), "game").unwrap();
        }
        _ => unreachable!(),
    }
    let game = games::register_game(&engine.db, &root, Some(slug)).unwrap();
    (temp, engine, game)
}
fn zip(engine: &FileEngine, entries: &[(&str, &[u8])]) -> PathBuf {
    let file = engine
        .paths
        .downloads()
        .join(format!("{}.zip", util::new_id("payload")));
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
    for (name, bytes) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
    file
}
fn request(game: &games::Game, path: PathBuf, ty: &str, filename: &str) -> InstallRequest {
    InstallRequest {
        game_id: game.id.clone(),
        artifact_path: path,
        filename: filename.into(),
        name: "Example".into(),
        mod_type: ty.into(),
        version: Some("1.0".into()),
        expected_sha256: None,
        folder_name: None,
        mappings: Vec::new(),
        fomod_selection: None,
        source: "local".into(),
        source_mod_id: None,
        replace_install_id: None,
    }
}
fn apply(engine: &FileEngine, plan: &FilePlan) {
    engine.approve_plan(&plan.id).unwrap();
    engine.apply_file_ops(&plan.id).unwrap();
}

#[test]
fn install_preview_cannot_use_changed_or_relocated_extracted_payloads() {
    let (_temp, engine, game) = fixture("stardew-valley");
    let artifact = zip(&engine, &[("Example/data.txt", b"verified")]);
    let sha = util::sha256_file(&artifact).unwrap();
    let stage = engine.paths.staging().join("verified-fixture");
    let extraction = archive::extract_verified_archive(&artifact, &stage, &sha).unwrap();
    let file = stage.join("Example/data.txt");
    let plan = |src: &Path| {
        engine
            .plan_file_ops(
                vec![FileOp::Copy {
                    src: src.into(),
                    dst: Path::new(&game.root_path).join("Mods/data.txt"),
                }],
                "Test install source".into(),
            )
            .unwrap()
    };
    let validate =
        |src: &Path| validate_payload_sources(&plan(src), &artifact, &sha, Some(&extraction.files));
    validate(&file).unwrap();

    let alias = engine.paths.staging().join("other.txt");
    std::fs::copy(&file, &alias).unwrap();
    assert!(matches!(validate(&alias), Err(AppError::Conflict(_))));
    std::fs::write(&file, b"modified").unwrap();
    assert!(matches!(validate(&file), Err(AppError::Conflict(_))));
}

fn release(
    engine: &FileEngine,
    game: &games::Game,
    project: &str,
    version: &str,
    entries: &[(&str, &[u8])],
) -> InstallRequest {
    let id = uuid::Uuid::new_v4().to_string();
    let filename = format!("Example-{version}.zip");
    let path = engine.paths.downloads().join(format!("{id}-{filename}"));
    std::fs::rename(zip(engine, entries), &path).unwrap();
    let hash = util::sha256_file(&path).unwrap();
    let artifact = crate::download::DownloadArtifact {
        id,
        path: path.clone(),
        filename: filename.clone(),
        size: std::fs::metadata(&path).unwrap().len(),
        sha256: hash.clone(),
        sha512: String::new(),
        integrity_verified: false,
        requires_review: true,
        auto_install_allowed: false,
        warnings: vec![],
    };
    let account = crate::sources::ResolvedMod {
        source_id: "modrinth".into(),
        project_id: project.into(),
        version_id: version.into(),
        name: "Example".into(),
        version: version.into(),
        download_url: "https://example.org/fixture.zip".into(),
        filename: filename.clone(),
        size: Some(artifact.size),
        hashes: Default::default(),
        homepage_url: None,
        warnings: vec![],
    };
    artifacts::record(engine, &artifact, &account).unwrap();
    let mut req = request(game, path, "smapi-mod", &filename);
    req.version = None;
    req.expected_sha256 = Some(hash);
    req
}

const RELEASE_ONE: &[(&str, &[u8])] = &[
    ("Example/manifest.json", b"one"),
    ("Example/retired.txt", b"retired"),
];
const RELEASE_TWO: &[(&str, &[u8])] = &[
    ("Example/manifest.json", b"two"),
    ("Example/new.txt", b"added"),
];

#[test]
fn dependency_audit_reads_journal_without_reconciliation_and_tracks_replacement() {
    use crate::{
        install_dependencies::{check, CheckRequest, DependencyStatus},
        sources::{DependencyType, ModDependency},
    };
    let (temp, engine, game) = fixture("stardew-valley");
    let other_root = temp.path().join("other-game");
    std::fs::create_dir(&other_root).unwrap();
    std::fs::write(other_root.join("Stardew Valley.dll"), "game").unwrap();
    let other_game = games::register_game(&engine.db, &other_root, Some("stardew-valley")).unwrap();
    let first = plan_install(
        &engine,
        &release(&engine, &game, "project", "v1", RELEASE_ONE),
    )
    .unwrap();
    let mut request = CheckRequest {
        game_id: game.id.clone(),
        dependencies: Some(vec![ModDependency {
            dependency_type: DependencyType::Required,
            project_id: Some("project".into()),
            version_id: Some("v1".into()),
            file_name: None,
        }]),
        replace_install_id: None,
    };
    let status = |req: &CheckRequest| {
        check(&engine, req)
            .unwrap()
            .checks
            .unwrap()
            .remove(0)
            .status
    };
    assert_eq!(status(&request), DependencyStatus::NotRecorded);
    apply(&engine, &first.plan);
    assert_eq!(status(&request), DependencyStatus::RecordedExact);
    let stored: String = engine
        .db
        .conn()
        .query_row(
            "SELECT status FROM installs WHERE id=?1",
            [&first.install_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, "planned");

    request.game_id = other_game.id;
    assert_eq!(status(&request), DependencyStatus::NotRecorded);
    request.replace_install_id = Some(first.install_id.clone());
    assert!(check(&engine, &request).is_err());
    request.game_id = game.id.clone();
    let projected = check(&engine, &request).unwrap();
    assert_eq!(
        projected.excluded_install_id.as_deref(),
        Some(first.install_id.as_str())
    );
    assert_eq!(
        projected.checks.unwrap()[0].status,
        DependencyStatus::NotRecorded
    );

    let mut next = release(&engine, &game, "project", "v2", RELEASE_TWO);
    next.replace_install_id = request.replace_install_id.take();
    let second = plan_install(&engine, &next).unwrap();
    apply(&engine, &second.plan);
    assert_eq!(status(&request), DependencyStatus::DifferentVersion);
    engine.rollback_transaction(&second.plan.id).unwrap();
    assert_eq!(status(&request), DependencyStatus::RecordedExact);
}

#[test]
fn dependency_audit_ignores_source_hints_without_catalog_provenance() {
    use crate::{
        install_dependencies::{check, CheckRequest, DependencyStatus},
        sources::{DependencyType, ModDependency},
    };
    let (_temp, engine, game) = fixture("stardew-valley");
    let mut local = request(&game, zip(&engine, RELEASE_ONE), "smapi-mod", "Example.zip");
    local.source = "modrinth".into();
    local.source_mod_id = Some("project".into());
    local.version = Some("v1".into());
    let preview = plan_install(&engine, &local).unwrap();
    assert!(preview.download_origin.is_none());
    apply(&engine, &preview.plan);

    let mut request = CheckRequest {
        game_id: game.id,
        dependencies: None,
        replace_install_id: None,
    };
    assert!(check(&engine, &request).unwrap().checks.is_none());
    request.dependencies = Some(vec![]);
    assert!(check(&engine, &request).unwrap().checks.unwrap().is_empty());
    request.dependencies = Some(vec![ModDependency {
        dependency_type: DependencyType::Required,
        project_id: Some("project".into()),
        version_id: Some("v1".into()),
        file_name: None,
    }]);
    assert_eq!(
        check(&engine, &request).unwrap().checks.unwrap()[0].status,
        DependencyStatus::NotRecorded
    );
    request.dependencies.as_mut().unwrap()[0].file_name = Some("../private/file".into());
    assert!(matches!(
        check(&engine, &request),
        Err(AppError::Invalid(_))
    ));
}

#[test]
fn replacement_commits_and_rolls_back_move_overwrite_and_add_as_one_version_change() {
    let (_temp, engine, game) = fixture("stardew-valley");
    let v1 = plan_install(
        &engine,
        &release(&engine, &game, "project", "v1", RELEASE_ONE),
    )
    .unwrap();
    apply(&engine, &v1.plan);
    let folder = Path::new(&game.root_path).join("Mods/Example");
    std::fs::write(folder.join("user.cfg"), b"preserve").unwrap();
    let stale_uninstall = plan_uninstall(&engine, &v1.install_id).unwrap();
    let mut req = release(&engine, &game, "project", "v2", RELEASE_TWO);
    req.replace_install_id = Some(v1.install_id.clone());
    let v2 = plan_install(&engine, &req).unwrap();
    assert_eq!(v2.plan.entries.len(), 3);
    for impact in ["move", "overwrite", "add"] {
        assert!(v2.plan.entries.iter().any(|entry| entry.impact == impact));
    }
    assert!(v2.plan.requires_review);
    assert!(v2.plan.summary.contains("v1 → v2"));
    assert!(engine.apply_file_ops(&v2.plan.id).is_err());
    assert_eq!(std::fs::read(folder.join("manifest.json")).unwrap(), b"one");
    assert!(trash::list_trash(&engine.db, false).unwrap().is_empty());
    apply(&engine, &v2.plan);
    let reopened = FileEngine::new(
        Db::open(&engine.paths.db_file()).unwrap(),
        (*engine.paths).clone(),
    );
    let old = get_install(&reopened, &v1.install_id).unwrap();
    let new = get_install(&reopened, &v2.install_id).unwrap();
    assert_eq!(
        (old.status.as_str(), new.status.as_str()),
        ("superseded", "enabled")
    );
    assert_eq!(
        games::get_game(&reopened.db, &game.id)
            .unwrap()
            .installed_mod_count,
        1
    );
    assert_eq!(new.metadata["replacesInstallId"], v1.install_id);
    assert_eq!(new.files.len(), 2);
    assert!(new.files.iter().all(|file| file.before.is_none()));
    let replacement_review = inspect_install(&reopened, &v2.install_id).unwrap();
    assert_eq!(
        replacement_review.reconciliation.recommended_action,
        "rollback_replacement"
    );
    assert_eq!(
        replacement_review
            .reconciliation
            .rollback_transaction_id
            .as_deref(),
        Some(v2.plan.id.as_str())
    );
    assert!(!folder.join("retired.txt").exists());
    assert_eq!(std::fs::read(folder.join("manifest.json")).unwrap(), b"two");
    assert_eq!(std::fs::read(folder.join("new.txt")).unwrap(), b"added");
    let trash = trash::list_trash(&reopened.db, false).unwrap();
    assert_eq!(trash.len(), 1);
    assert!(trash[0].restore_requires_rollback);
    assert_eq!(std::fs::read(&trash[0].trash_path).unwrap(), b"retired");
    assert!(trash::plan_restore(&reopened, &[trash[0].id.clone()], false).is_err());
    assert!(reopened.rollback_transaction(&v1.plan.id).is_err());
    reopened.approve_plan(&stale_uninstall.id).unwrap();
    assert!(reopened.apply_file_ops(&stale_uninstall.id).is_err());

    std::fs::write(folder.join("manifest.json"), b"user edit").unwrap();
    assert!(reopened.rollback_transaction(&v2.plan.id).is_err());
    assert_eq!(
        std::fs::read(folder.join("manifest.json")).unwrap(),
        b"user edit"
    );
    std::fs::write(folder.join("manifest.json"), b"two").unwrap();
    let overwrite = v2
        .plan
        .entries
        .iter()
        .position(|entry| entry.impact == "overwrite")
        .unwrap();
    let backup = engine
        .paths
        .backup_dir(&v2.plan.id)
        .join(format!("{overwrite}.original"));
    assert_eq!(std::fs::read(&backup).unwrap(), b"one");
    let saved_backup = backup.with_extension("saved");
    std::fs::rename(&backup, &saved_backup).unwrap();
    assert!(reopened.rollback_transaction(&v2.plan.id).is_err());
    assert_eq!(std::fs::read(folder.join("new.txt")).unwrap(), b"added");
    std::fs::write(&backup, b"one").unwrap();
    assert!(reopened.rollback_transaction(&v2.plan.id).is_err());
    assert_eq!(std::fs::read(folder.join("new.txt")).unwrap(), b"added");
    std::fs::rename(&backup, backup.with_extension("unrelated")).unwrap();
    std::fs::rename(&saved_backup, &backup).unwrap();
    reopened.rollback_transaction(&v2.plan.id).unwrap();
    assert_eq!(
        get_install(&reopened, &v1.install_id).unwrap().status,
        "enabled"
    );
    assert_eq!(
        get_install(&reopened, &v2.install_id).unwrap().status,
        "rolled_back"
    );
    assert_eq!(std::fs::read(folder.join("manifest.json")).unwrap(), b"one");
    assert_eq!(
        std::fs::read(folder.join("retired.txt")).unwrap(),
        b"retired"
    );
    assert_eq!(std::fs::read(folder.join("user.cfg")).unwrap(), b"preserve");
    assert!(!folder.join("new.txt").exists());
    assert!(trash::list_trash(&reopened.db, false).unwrap().is_empty());
}

#[test]
fn replacement_rejects_wrong_release_edited_and_untracked_files_without_approvable_plan() {
    let (_temp, engine, game) = fixture("stardew-valley");
    let req = release(&engine, &game, "project", "v1", RELEASE_ONE);
    let v1 = plan_install(&engine, &req).unwrap();
    apply(&engine, &v1.plan);
    for (project, version) in [("other", "v2"), ("project", "v1")] {
        let mut req = release(&engine, &game, project, version, RELEASE_TWO);
        req.replace_install_id = Some(v1.install_id.clone());
        assert!(matches!(
            plan_install(&engine, &req),
            Err(AppError::Conflict(_))
        ));
    }
    let folder = Path::new(&game.root_path).join("Mods/Example");
    let mut req = release(&engine, &game, "project", "v2", RELEASE_TWO);
    req.replace_install_id = Some(v1.install_id);
    std::fs::write(folder.join("new.txt"), b"unowned").unwrap();
    assert!(matches!(
        plan_install(&engine, &req),
        Err(AppError::Conflict(_))
    ));
    std::fs::remove_file(folder.join("new.txt")).unwrap();
    std::fs::write(folder.join("retired.txt"), b"edited").unwrap();
    assert!(matches!(
        plan_install(&engine, &req),
        Err(AppError::Conflict(_))
    ));
    assert_eq!(
        std::fs::read(folder.join("retired.txt")).unwrap(),
        b"edited"
    );
    let count: u32 = engine
        .db
        .conn()
        .query_row("SELECT count(*) FROM file_transactions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn replacement_rechecks_file_ownership_after_preview_even_when_bytes_match() {
    let (_temp, engine, game) = fixture("stardew-valley");
    let v1 = plan_install(
        &engine,
        &release(&engine, &game, "project", "v1", RELEASE_ONE),
    )
    .unwrap();
    apply(&engine, &v1.plan);
    let mut req = release(&engine, &game, "project", "v2", RELEASE_TWO);
    req.replace_install_id = Some(v1.install_id.clone());
    let preview = plan_install(&engine, &req).unwrap();
    let shared = zip(&engine, &[("Example/manifest.json", b"one")]);
    let other = plan_install(&engine, &request(&game, shared, "smapi-mod", "other.zip")).unwrap();
    apply(&engine, &other.plan);
    assert!(matches!(
        plan_install(&engine, &req),
        Err(AppError::Conflict(_))
    ));
    engine.approve_plan(&preview.plan.id).unwrap();
    assert!(matches!(
        engine.apply_file_ops(&preview.plan.id),
        Err(AppError::Conflict(_))
    ));
    assert_eq!(
        get_install(&engine, &v1.install_id).unwrap().status,
        "enabled"
    );
    assert!(trash::list_trash(&engine.db, false).unwrap().is_empty());
}

#[test]
fn replacement_failure_after_trashing_and_overwriting_recovers_prior_install() {
    let (_temp, engine, game) = fixture("stardew-valley");
    let v1 = plan_install(
        &engine,
        &release(&engine, &game, "project", "v1", RELEASE_ONE),
    )
    .unwrap();
    apply(&engine, &v1.plan);
    let mut req = release(&engine, &game, "project", "v2", RELEASE_TWO);
    req.replace_install_id = Some(v1.install_id.clone());
    let v2 = plan_install(&engine, &req).unwrap();
    assert_eq!(v2.plan.entries[0].impact, "move");
    assert_eq!(v2.plan.entries[1].impact, "overwrite");
    engine.inject_failure_before(2);
    engine.approve_plan(&v2.plan.id).unwrap();
    assert!(engine.apply_file_ops(&v2.plan.id).is_err());
    assert_eq!(
        get_install(&engine, &v2.install_id).unwrap().status,
        "rolled_back"
    );
    assert_eq!(
        get_install(&engine, &v1.install_id).unwrap().status,
        "enabled"
    );
    let folder = Path::new(&game.root_path).join("Mods/Example");
    assert_eq!(std::fs::read(folder.join("manifest.json")).unwrap(), b"one");
    assert_eq!(
        std::fs::read(folder.join("retired.txt")).unwrap(),
        b"retired"
    );
    assert!(!folder.join("new.txt").exists());
    assert!(trash::list_trash(&engine.db, false).unwrap().is_empty());
}

#[test]
fn replacement_recovery_after_last_write_before_commit_restores_previous_record() {
    let (_temp, engine, game) = fixture("stardew-valley");
    let v1 = plan_install(
        &engine,
        &release(&engine, &game, "project", "v1", RELEASE_ONE),
    )
    .unwrap();
    apply(&engine, &v1.plan);
    let mut req = release(&engine, &game, "project", "v2", RELEASE_TWO);
    req.replace_install_id = Some(v1.install_id.clone());
    let v2 = plan_install(&engine, &req).unwrap();
    apply(&engine, &v2.plan);
    // All entries and filesystem effects are durable; the final status write
    // is the only step that would be missing at this interruption point.
    engine
        .db
        .conn()
        .execute(
            "UPDATE file_transactions SET status='applying' WHERE id=?1",
            [&v2.plan.id],
        )
        .unwrap();
    let reopened = FileEngine::new(
        Db::open(&engine.paths.db_file()).unwrap(),
        (*engine.paths).clone(),
    );
    assert_eq!(reopened.recover_interrupted().unwrap(), [v2.plan.id]);
    assert_eq!(
        get_install(&reopened, &v1.install_id).unwrap().status,
        "enabled"
    );
    assert_eq!(
        get_install(&reopened, &v2.install_id).unwrap().status,
        "rolled_back"
    );
    let folder = Path::new(&game.root_path).join("Mods/Example");
    assert_eq!(std::fs::read(folder.join("manifest.json")).unwrap(), b"one");
    assert_eq!(
        std::fs::read(folder.join("retired.txt")).unwrap(),
        b"retired"
    );
    assert!(!folder.join("new.txt").exists());
    assert!(trash::list_trash(&reopened.db, false).unwrap().is_empty());
}

#[test]
fn replacement_rejects_concurrent_successor_and_rolls_back_chain_in_order() {
    let (_temp, engine, game) = fixture("stardew-valley");
    let v1 = plan_install(
        &engine,
        &release(&engine, &game, "project", "v1", RELEASE_ONE),
    )
    .unwrap();
    apply(&engine, &v1.plan);
    let mut req2 = release(&engine, &game, "project", "v2", RELEASE_ONE);
    let mut req3 = release(&engine, &game, "project", "v3", RELEASE_ONE);
    req2.replace_install_id = Some(v1.install_id.clone());
    req3.replace_install_id = Some(v1.install_id.clone());
    let v2 = plan_install(&engine, &req2).unwrap();
    let stale = plan_install(&engine, &req3).unwrap();
    assert!(v2
        .plan
        .entries
        .iter()
        .all(|entry| entry.impact == "overwrite"));
    apply(&engine, &v2.plan);
    engine.approve_plan(&stale.plan.id).unwrap();
    assert!(matches!(
        engine.apply_file_ops(&stale.plan.id),
        Err(AppError::Conflict(_))
    ));
    req3.replace_install_id = Some(v2.install_id.clone());
    let v3 = plan_install(&engine, &req3).unwrap();
    apply(&engine, &v3.plan);
    assert!(engine.rollback_transaction(&v2.plan.id).is_err());
    engine.rollback_transaction(&v3.plan.id).unwrap();
    engine.rollback_transaction(&v2.plan.id).unwrap();
    assert_eq!(
        get_install(&engine, &v1.install_id).unwrap().status,
        "enabled"
    );
    assert_eq!(
        get_install(&engine, &v2.install_id).unwrap().status,
        "rolled_back"
    );
    assert_eq!(
        get_install(&engine, &v3.install_id).unwrap().status,
        "rolled_back"
    );
}

#[test]
fn jar_is_installed_intact_and_complete_trash_restore_lifecycle() {
    let (_temp, engine, game) = fixture("minecraft-java");
    let archive = zip(
        &engine,
        &[("fabric.mod.json", b"{}"), ("assets/test.txt", b"asset")],
    );
    let preview = plan_install(
        &engine,
        &request(&game, archive.clone(), "java-mod", "Example.jar"),
    )
    .unwrap();
    let target = Path::new(&game.root_path).join("mods/Example.jar");
    assert!(!target.exists());
    assert!(engine.apply_file_ops(&preview.plan.id).is_err());
    assert_eq!(
        list_installs(&engine, &game.id).unwrap()[0].status,
        "planned"
    );
    apply(&engine, &preview.plan);
    assert_eq!(
        std::fs::read(&archive).unwrap(),
        std::fs::read(&target).unwrap()
    );
    assert_eq!(
        list_installs(&engine, &game.id).unwrap()[0].status,
        "enabled"
    );
    assert_eq!(
        games::get_game(&engine.db, &game.id)
            .unwrap()
            .installed_mod_count,
        1
    );
    let uninstall = plan_uninstall(&engine, &preview.install_id).unwrap();
    apply(&engine, &uninstall);
    assert!(!target.exists());
    assert_eq!(
        list_installs(&engine, &game.id).unwrap()[0].status,
        "uninstalled"
    );
    let ids: Vec<_> = trash::list_trash(&engine.db, false)
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    let restore = trash::plan_restore(&engine, &ids, false).unwrap();
    apply(&engine, &restore);
    assert_eq!(
        list_installs(&engine, &game.id).unwrap()[0].status,
        "enabled"
    );
    assert_eq!(
        std::fs::read(&archive).unwrap(),
        std::fs::read(&target).unwrap()
    );
}

#[test]
fn overwrite_backup_and_rollback_reconcile_records_after_restart() {
    let (_temp, engine, game) = fixture("minecraft-java");
    let target = Path::new(&game.root_path).join("mods/Example.jar");
    std::fs::create_dir(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "original").unwrap();
    let archive = zip(&engine, &[("fabric.mod.json", b"{}")]);
    let preview =
        plan_install(&engine, &request(&game, archive, "java-mod", "Example.jar")).unwrap();
    assert_eq!(preview.plan.entries[0].impact, "overwrite");
    apply(&engine, &preview.plan);
    assert!(plan_uninstall(&engine, &preview.install_id).is_err());
    engine.rollback_transaction(&preview.plan.id).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"original");
    let reopened = FileEngine::new(
        Db::open(&engine.paths.db_file()).unwrap(),
        (*engine.paths).clone(),
    );
    assert_eq!(
        list_installs(&reopened, &game.id).unwrap()[0].status,
        "rolled_back"
    );
    assert_eq!(
        games::get_game(&engine.db, &game.id)
            .unwrap()
            .installed_mod_count,
        0
    );
}

#[test]
fn smapi_preserves_real_mod_directory_and_strips_only_release_wrapper() {
    let (_temp, engine, game) = fixture("stardew-valley");
    let archive = zip(
        &engine,
        &[
            ("Release/Example/manifest.json", b"{\"Name\":\"Example\"}"),
            ("Release/Example/assets/a.png", b"bitmap"),
        ],
    );
    let preview = plan_install(
        &engine,
        &request(&game, archive, "smapi-mod", "example.zip"),
    )
    .unwrap();
    assert!(preview.plan.entries.iter().all(|e| e
        .operation
        .dst()
        .starts_with(Path::new(&game.root_path).join("Mods/Example"))));
    apply(&engine, &preview.plan);
    assert!(Path::new(&game.root_path)
        .join("Mods/Example/manifest.json")
        .exists());
    assert!(!Path::new(&game.root_path)
        .join("Mods/manifest.json")
        .exists());
}

#[test]
fn data_root_not_doubled_and_fomod_requires_explicit_mapping() {
    let (_temp, engine, game) = fixture("skyrim-se");
    let archive = zip(&engine, &[("Release/Data/test.esp", b"plugin")]);
    let preview =
        plan_install(&engine, &request(&game, archive, "data-mod", "example.zip")).unwrap();
    assert_eq!(
        preview.plan.entries[0].operation.dst(),
        Path::new(&game.root_path).join("Data/test.esp")
    );
    let archive = zip(
        &engine,
        &[
            ("Data/test.esp", b"plugin"),
            ("fomod/ModuleConfig.xml", b"<config/>"),
        ],
    );
    let mut req = request(&game, archive, "data-mod", "fomod.zip");
    assert!(plan_install(&engine, &req).is_err());
    req.mappings = vec![ManualMapping {
        source: "Data/test.esp".into(),
        destination: "test.esp".into(),
    }];
    let preview = plan_install(&engine, &req).unwrap();
    assert_eq!(preview.plan.entries.len(), 1);
    assert_eq!(preview.ignored_files, vec!["fomod/ModuleConfig.xml"]);
}

#[test]
fn deterministic_fomod_mappings_survive_approval_and_rollback() {
    let (_temp, engine, game) = fixture("skyrim-se");
    let xml = br#"<config><requiredInstallFiles><file source="Data/base.esp" destination="base.esp"/></requiredInstallFiles>
        <installSteps><installStep><optionalFileGroups><group name="Textures" type="SelectAll"><plugins>
        <plugin name="Core"><files>
        <folder source="Core" destination="textures"/>
        <folder source="RootFiles" destination=""/>
        </files></plugin>
        </plugins></group></optionalFileGroups></installStep></installSteps></config>"#;
    let archive = zip(
        &engine,
        &[
            ("Release/fomod/ModuleConfig.xml", xml),
            ("Release/Data/base.esp", b"replacement"),
            ("Release/Core/base.dds", b"texture"),
            ("Release/Core/landscape/caves/detail.dds", b"nested texture"),
            ("Release/RootFiles/root.ini", b"root configuration"),
            ("Release/CoreExtra/extra.dds", b"sibling texture"),
            ("Release/Other/extra.dds", b"unselected"),
        ],
    );
    let install_root = Path::new(&game.root_path).join("Data");
    let base = install_root.join("base.esp");
    let textures = install_root.join("textures");
    let root_file = install_root.join("root.ini");
    std::fs::write(&base, "original").unwrap();
    let assert_original_files = || {
        assert_eq!(std::fs::read(&base).unwrap(), b"original");
        assert!(!textures.exists());
        assert!(!root_file.exists());
    };

    let selection = crate::fomod::preview(&engine, &archive).unwrap();
    assert_eq!(selection.config_path, "Release/fomod/ModuleConfig.xml");
    assert_eq!(selection.automatic_selection.status, "safe");
    let expected_mappings = [
        ("Release/Core/base.dds", "textures/base.dds"),
        (
            "Release/Core/landscape/caves/detail.dds",
            "textures/landscape/caves/detail.dds",
        ),
        ("Release/Data/base.esp", "base.esp"),
        ("Release/RootFiles/root.ini", "root.ini"),
    ];
    let mut mappings: Vec<_> = selection
        .automatic_selection
        .mappings
        .iter()
        .map(|mapping| (mapping.source.as_str(), mapping.destination.as_str()))
        .collect();
    mappings.sort_unstable();
    assert_eq!(mappings, expected_mappings);

    let mut req = request(&game, archive, "fomod", "fixture.zip");
    req.expected_sha256 = Some(selection.artifact_sha256);
    req.fomod_selection = Some(FomodSelection::Automatic);
    let install = plan_install(&engine, &req).unwrap();
    assert_eq!(install.install_root, install_root);
    assert!(install.plan.requires_review);
    assert_eq!(
        install.ignored_files,
        [
            "Release/CoreExtra/extra.dds",
            "Release/Other/extra.dds",
            "Release/fomod/ModuleConfig.xml",
        ]
    );
    assert_eq!(install.plan.entries.len(), expected_mappings.len());
    let staging = engine.paths.staging();
    for (source, destination) in expected_mappings {
        let destination = install_root.join(destination);
        let entry = install
            .plan
            .entries
            .iter()
            .find(|entry| entry.operation.dst() == destination)
            .unwrap_or_else(|| panic!("missing planned operation for {}", destination.display()));
        let FileOp::Copy { src, .. } = &entry.operation else {
            panic!("expected a copy for {}", destination.display());
        };
        assert!(src.starts_with(&staging) && src.ends_with(source));
        assert_eq!(
            entry.impact,
            if destination == base {
                "overwrite"
            } else {
                "add"
            }
        );
    }

    assert!(matches!(
        engine.apply_file_ops(&install.plan.id),
        Err(AppError::Denied(_))
    ));
    assert_original_files();
    engine.approve_plan(&install.plan.id).unwrap();
    assert_original_files();
    engine.apply_file_ops(&install.plan.id).unwrap();
    assert_eq!(std::fs::read(&base).unwrap(), b"replacement");
    assert_eq!(
        std::fs::read(textures.join("base.dds")).unwrap(),
        b"texture"
    );
    assert_eq!(
        std::fs::read(textures.join("landscape/caves/detail.dds")).unwrap(),
        b"nested texture"
    );
    assert_eq!(std::fs::read(&root_file).unwrap(), b"root configuration");
    engine.rollback_transaction(&install.plan.id).unwrap();
    assert_original_files();
}

#[test]
fn automatic_fomod_rejects_ambiguous_inputs_before_creating_a_plan() {
    let (_temp, engine, game) = fixture("skyrim-se");
    let deterministic = br#"<config><requiredInstallFiles>
        <file source="base.esp" destination="base.esp"/>
        </requiredInstallFiles></config>"#;
    let conditional = br#"<config><requiredInstallFiles>
        <file source="base.esp" destination="base.esp"/>
        </requiredInstallFiles><moduleDependencies>
        <fileDependency file="AnotherMod.esp" state="Active"/>
        </moduleDependencies></config>"#;
    let archive = zip(
        &engine,
        &[
            ("fomod/ModuleConfig.xml", deterministic),
            ("base.esp", b"plugin"),
        ],
    );
    let mut req = request(&game, archive, "fomod", "fixture.zip");
    req.fomod_selection = Some(FomodSelection::Automatic);
    req.mappings.push(ManualMapping {
        source: "base.esp".into(),
        destination: "alternate.esp".into(),
    });
    assert!(
        matches!(plan_install(&engine, &req), Err(AppError::Invalid(message)) if message.contains("不能同时使用"))
    );
    req.mappings.clear();

    req.mod_type = "data-mod".into();
    assert!(
        matches!(plan_install(&engine, &req), Err(AppError::Invalid(message)) if message.contains("fomod 模组类型"))
    );
    req.mod_type = "fomod".into();

    req.expected_sha256 = Some("0".repeat(64));
    assert!(
        matches!(plan_install(&engine, &req), Err(AppError::Conflict(message)) if message.contains("SHA-256"))
    );
    req.expected_sha256 = None;

    for entries in [
        vec![
            ("fomod/ModuleConfig.xml", &conditional[..]),
            ("base.esp", b"plugin"),
        ],
        vec![
            ("fomod/ModuleConfig.xml", &deterministic[..]),
            ("base.esp", b"plugin"),
            ("fomod/Script.cs", b"inert script fixture"),
        ],
    ] {
        req.artifact_path = zip(&engine, &entries);
        assert!(
            matches!(plan_install(&engine, &req), Err(AppError::Unsupported(message)) if message.contains("需要用户选择"))
        );
    }

    req.artifact_path = zip(&engine, &[("Data/base.esp", b"plugin")]);
    assert!(
        matches!(plan_install(&engine, &req), Err(AppError::NotFound(message)) if message.contains("ModuleConfig.xml"))
    );
    assert!(list_installs(&engine, &game.id).unwrap().is_empty());
    let transaction_count: usize = engine
        .db
        .conn()
        .query_row("SELECT COUNT(*) FROM file_transactions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(transaction_count, 0);
    assert!(!Path::new(&game.root_path).join("Data/base.esp").exists());
}

#[test]
fn unsafe_hash_paths_and_changed_payloads_never_modify_game() {
    let (temp, engine, game) = fixture("minecraft-java");
    let archive = zip(&engine, &[("fabric.mod.json", b"{}")]);
    let mut req = request(&game, archive, "java-mod", "Example.jar");
    req.expected_sha256 = Some("0".repeat(64));
    assert!(plan_install(&engine, &req).is_err());
    req.expected_sha256 = None;
    req.filename = "../bad.jar".into();
    assert!(plan_install(&engine, &req).is_err());
    req.filename = "Example.jar".into();
    req.artifact_path = temp.path().join("outside.jar");
    std::fs::write(&req.artifact_path, "unsafe").unwrap();
    assert!(plan_install(&engine, &req).is_err());
    assert!(!Path::new(&game.root_path).join("mods").exists());
}

#[test]
fn modified_installed_file_is_not_silently_uninstalled() {
    let (_temp, engine, game) = fixture("minecraft-java");
    let archive = zip(&engine, &[("fabric.mod.json", b"{}")]);
    let preview =
        plan_install(&engine, &request(&game, archive, "java-mod", "Example.jar")).unwrap();
    apply(&engine, &preview.plan);
    let target = Path::new(&game.root_path).join("mods/Example.jar");
    std::fs::write(&target, "user modified").unwrap();
    assert!(plan_uninstall(&engine, &preview.install_id).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"user modified");
}

#[test]
fn inspect_install_reports_a_safe_install_without_reconciling() {
    let (_temp, engine, game) = fixture("minecraft-java");
    let preview = plan_install(
        &engine,
        &request(
            &game,
            zip(&engine, &[("fabric.mod.json", b"{}")]),
            "java-mod",
            "Example.jar",
        ),
    )
    .unwrap();
    apply(&engine, &preview.plan);

    let report = inspect_install(&engine, &preview.install_id).unwrap();
    assert_eq!(report.status, "enabled");
    assert!(report.can_uninstall);
    assert_eq!(report.reconciliation.recommended_action, "safe_uninstall");
    assert!(report.reconciliation.rollback_transaction_id.is_none());
    assert_eq!(report.files.len(), 1);
    let file = &report.files[0];
    assert_eq!(file.state, "owned");
    assert_eq!(
        file.current_fingerprint,
        Some(file.expected_fingerprint.clone())
    );
    assert!(!file.replaced);
    assert!(file.other_install_ids.is_empty());
    assert!(file.reasons.is_empty());

    let stored_status: String = engine
        .db
        .conn()
        .query_row(
            "SELECT status FROM installs WHERE id=?1",
            [&preview.install_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_status, "planned");
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["installId"], preview.install_id);
    assert!(json["files"][0]["expectedFingerprint"].is_object());
    assert!(json["files"][0]["currentFingerprint"].is_object());
}

#[test]
fn inspect_install_distinguishes_modified_and_missing_files() {
    let (_temp, engine, game) = fixture("minecraft-java");
    let preview = plan_install(
        &engine,
        &request(
            &game,
            zip(&engine, &[("fabric.mod.json", b"{}")]),
            "java-mod",
            "Example.jar",
        ),
    )
    .unwrap();
    apply(&engine, &preview.plan);
    let target = Path::new(&game.root_path).join("mods/Example.jar");

    std::fs::write(&target, b"user modified").unwrap();
    let modified = inspect_install(&engine, &preview.install_id).unwrap();
    assert!(!modified.can_uninstall);
    assert_eq!(
        modified.reconciliation.recommended_action,
        "review_file_state"
    );
    assert_eq!(modified.files[0].state, "modified");
    assert!(modified.files[0]
        .reasons
        .iter()
        .any(|reason| reason.contains("指纹")));

    std::fs::remove_file(&target).unwrap();
    let missing = inspect_install(&engine, &preview.install_id).unwrap();
    assert!(!missing.can_uninstall);
    assert_eq!(missing.files[0].state, "missing");
    assert!(missing.files[0].current_fingerprint.is_none());
    assert!(missing.files[0]
        .reasons
        .iter()
        .any(|reason| reason.contains("不存在")));
}

#[test]
fn inspect_install_reports_shared_paths_between_active_installs() {
    let (_temp, engine, game) = fixture("minecraft-java");
    let artifact = zip(&engine, &[("fabric.mod.json", b"{}")]);
    let first = plan_install(
        &engine,
        &request(&game, artifact.clone(), "java-mod", "Example.jar"),
    )
    .unwrap();
    apply(&engine, &first.plan);
    let second = plan_install(
        &engine,
        &request(&game, artifact, "java-mod", "Example.jar"),
    )
    .unwrap();
    apply(&engine, &second.plan);

    let report = inspect_install(&engine, &first.install_id).unwrap();
    assert!(!report.can_uninstall);
    assert_eq!(
        report.reconciliation.recommended_action,
        "review_shared_paths"
    );
    assert!(report.reconciliation.rollback_transaction_id.is_none());
    assert!(report.reconciliation.successor_install_id.is_none());
    assert_eq!(report.files[0].state, "owned");
    assert!(!report.files[0].replaced);
    assert_eq!(report.files[0].other_install_ids, vec![second.install_id]);
    assert!(report.files[0]
        .reasons
        .iter()
        .any(|reason| reason.contains("共享路径")));
}

#[test]
fn inspect_install_reports_files_that_overwrote_existing_content() {
    let (_temp, engine, game) = fixture("minecraft-java");
    let target = Path::new(&game.root_path).join("mods/Example.jar");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, b"original").unwrap();
    let preview = plan_install(
        &engine,
        &request(
            &game,
            zip(&engine, &[("fabric.mod.json", b"{}")]),
            "java-mod",
            "Example.jar",
        ),
    )
    .unwrap();
    apply(&engine, &preview.plan);

    let report = inspect_install(&engine, &preview.install_id).unwrap();
    assert!(!report.can_uninstall);
    assert_eq!(report.files[0].state, "owned");
    assert!(report.files[0].replaced);
    assert!(report.files[0]
        .reasons
        .iter()
        .any(|reason| reason.contains("覆盖了既有文件")));
}

#[cfg(unix)]
#[test]
fn inspect_install_marks_link_replaced_files_as_unreadable() {
    use std::os::unix::fs::symlink;

    let (temp, engine, game) = fixture("minecraft-java");
    let preview = plan_install(
        &engine,
        &request(
            &game,
            zip(&engine, &[("fabric.mod.json", b"{}")]),
            "java-mod",
            "Example.jar",
        ),
    )
    .unwrap();
    apply(&engine, &preview.plan);
    let target = Path::new(&game.root_path).join("mods/Example.jar");
    let outside = temp.path().join("outside.jar");
    std::fs::write(&outside, b"outside").unwrap();
    std::fs::remove_file(&target).unwrap();
    symlink(&outside, &target).unwrap();

    let report = inspect_install(&engine, &preview.install_id).unwrap();
    assert!(!report.can_uninstall);
    assert_eq!(report.files[0].state, "unreadable");
    assert!(report.files[0].current_fingerprint.is_none());
}

#[test]
fn explicit_purge_keeps_uninstall_tombstone_and_does_not_create_phantom_install() {
    let (_temp, engine, game) = fixture("minecraft-java");
    let path = zip(&engine, &[("fabric.mod.json", b"{}")]);
    let preview = plan_install(&engine, &request(&game, path, "java-mod", "Example.jar")).unwrap();
    apply(&engine, &preview.plan);
    let uninstall = plan_uninstall(&engine, &preview.install_id).unwrap();
    apply(&engine, &uninstall);
    let ids: Vec<_> = trash::list_trash(&engine.db, false)
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert!(trash::purge_trash(&engine.db, &engine.paths, &ids, "").is_err());
    assert!(trash::purge_trash(&engine.db, &engine.paths, &ids, "PURGE_TRASH").unwrap() > 0);
    assert!(trash::list_trash(&engine.db, true).unwrap().is_empty());
    assert_eq!(
        list_installs(&engine, &game.id).unwrap()[0].status,
        "uninstalled"
    );
    assert_eq!(
        games::get_game(&engine.db, &game.id)
            .unwrap()
            .installed_mod_count,
        0
    );
    assert!(engine.rollback_transaction(&uninstall.id).is_err());
    let status: String = engine
        .db
        .conn()
        .query_row(
            "SELECT status FROM file_transactions WHERE id=?1",
            [uninstall.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "committed");
}

#[test]
fn partial_restore_does_not_claim_a_complete_enabled_install() {
    let (_temp, engine, game) = fixture("stardew-valley");
    let archive = zip(
        &engine,
        &[
            ("Example/manifest.json", b"{}"),
            ("Example/content.json", b"{}"),
        ],
    );
    let preview = plan_install(
        &engine,
        &request(&game, archive, "smapi-mod", "Example.zip"),
    )
    .unwrap();
    apply(&engine, &preview.plan);
    let uninstall = plan_uninstall(&engine, &preview.install_id).unwrap();
    apply(&engine, &uninstall);
    let items = trash::list_trash(&engine.db, false).unwrap();
    assert_eq!(items.len(), 2);
    let restore = trash::plan_restore(&engine, std::slice::from_ref(&items[0].id), false).unwrap();
    apply(&engine, &restore);
    assert_eq!(
        get_install(&engine, &preview.install_id).unwrap().status,
        "partially_restored"
    );
    let game_state = games::get_game(&engine.db, &game.id).unwrap();
    assert_eq!(game_state.installed_mod_count, 1);
    assert_eq!(game_state.enabled_mod_count, 0);
    let remaining = trash::list_trash(&engine.db, false).unwrap();
    assert_eq!(remaining.len(), 1);
    let restore =
        trash::plan_restore(&engine, std::slice::from_ref(&remaining[0].id), false).unwrap();
    apply(&engine, &restore);
    assert_eq!(
        get_install(&engine, &preview.install_id).unwrap().status,
        "enabled"
    );
    assert_eq!(
        games::get_game(&engine.db, &game.id)
            .unwrap()
            .enabled_mod_count,
        1
    );
}

#[test]
fn user_roots_require_host_registration_before_datapack_plan() {
    let (_temp, engine, game) = fixture("minecraft-java");
    let world = Path::new(&game.root_path).join("saves/world");
    std::fs::create_dir_all(&world).unwrap();
    let archive = zip(&engine, &[("pack.mcmeta", b"{}")]);
    let req = request(&game, archive, "datapack", "pack.zip");
    assert!(plan_install(&engine, &req).is_err());
    game_roots::register_user_root(&engine, &game.id, "world-datapacks", &world).unwrap();
    let preview = plan_install(&engine, &req).unwrap();
    assert_eq!(
        preview.plan.entries[0].operation.dst(),
        world.join("datapacks/pack.zip")
    );
}

#[tokio::test]
async fn generic_apply_cannot_bypass_install_review_under_auto_approve() {
    use crate::agent::{
        policy::{Effect, PermissionMode},
        tools::{ToolContext, ToolRegistry},
    };
    let (_temp, engine, game) = fixture("minecraft-java");
    let archive = zip(&engine, &[("fabric.mod.json", b"{}")]);
    let preview =
        plan_install(&engine, &request(&game, archive, "java-mod", "Example.jar")).unwrap();
    assert!(preview.plan.requires_review);
    let registry = ToolRegistry::builtin().unwrap();
    let FileOp::Copy { src, .. } = &preview.plan.entries[0].operation else {
        unreachable!()
    };
    let mut raw = engine
        .plan_file_ops(
            vec![FileOp::Copy {
                src: src.clone(),
                dst: Path::new(&game.root_path).join("raw.jar"),
            }],
            "raw generic deployment".into(),
        )
        .unwrap();
    assert!(raw.requires_review);
    // Existing persisted previews from older versions must also be classified
    // from their real sources, not only the optional serialized review flag.
    raw.requires_review = false;
    engine
        .db
        .conn()
        .execute(
            "UPDATE file_transactions SET plan_json=?1 WHERE id=?2",
            rusqlite::params![serde_json::to_string(&raw).unwrap(), raw.id],
        )
        .unwrap();
    let context = ToolContext {
        engine,
        cancel: tokio_util::sync::CancellationToken::new(),
        progress: None,
        resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
    };
    let prepared = registry
        .get("apply_file_ops")
        .unwrap()
        .prepare(&context, &serde_json::json!({"plan_id":preview.plan.id}))
        .await
        .unwrap();
    assert_eq!(prepared.effect, Effect::ManagerWrite);
    assert!(PermissionMode::AutoApprove.needs_approval(prepared.effect));
    let raw_prepared = registry
        .get("apply_file_ops")
        .unwrap()
        .prepare(&context, &serde_json::json!({"plan_id":raw.id}))
        .await
        .unwrap();
    assert_eq!(raw_prepared.effect, Effect::ManagerWrite);
}

#[test]
fn download_catalog_binds_original_name_and_bytes_without_persisting_urls() {
    let (_temp, engine, game) = fixture("minecraft-java");
    let artifact_id = uuid::Uuid::new_v4().to_string();
    let path = engine
        .paths
        .downloads()
        .join(format!("{artifact_id}-Example.jar"));
    std::fs::rename(zip(&engine, &[("fabric.mod.json", b"{}")]), &path).unwrap();
    let hash = util::sha256_file(&path).unwrap();
    let artifact = crate::download::DownloadArtifact {
        id: artifact_id,
        path: path.clone(),
        filename: "Example.jar".into(),
        size: std::fs::metadata(&path).unwrap().len(),
        sha256: hash,
        sha512: String::new(),
        integrity_verified: true,
        requires_review: true,
        auto_install_allowed: false,
        warnings: vec![],
    };
    let resolved = crate::sources::ResolvedMod {
        source_id: "modrinth".into(),
        project_id: "project".into(),
        version_id: "v1".into(),
        name: "Example".into(),
        version: "1".into(),
        download_url: "https://example.com/file?token=private-value".into(),
        filename: "Example.jar".into(),
        size: None,
        hashes: Default::default(),
        homepage_url: None,
        warnings: vec![],
    };
    crate::artifacts::record(&engine, &artifact, &resolved).unwrap();
    let serialized = serde_json::to_string(&crate::artifacts::list(&engine.db).unwrap()).unwrap();
    assert!(!serialized.contains("private-value"));
    assert!(!serialized.contains("download_url"));
    let mut req = request(&game, path.clone(), "java-mod", "Renamed.jar");
    assert!(plan_install(&engine, &req).is_err());
    req.filename = "Example.jar".into();
    assert!(matches!(
        plan_install(&engine, &req),
        Err(AppError::Conflict(_))
    ));
    req.version = None;
    req.source = "nexus".into();
    assert!(matches!(
        plan_install(&engine, &req),
        Err(AppError::Conflict(_))
    ));
    req.source = "local".into();
    req.source_mod_id = Some("another-project".into());
    assert!(matches!(
        plan_install(&engine, &req),
        Err(AppError::Conflict(_))
    ));
    req.source_mod_id = None;
    let preview = plan_install(&engine, &req).unwrap();
    let origin = preview.download_origin.as_ref().unwrap();
    assert_eq!(origin.artifact_id, artifact.id);
    assert_eq!(origin.source_id, "modrinth");
    assert_eq!(origin.project_id, "project");
    assert_eq!(origin.version_id, "v1");
    assert!(preview.plan.summary.contains("版本 ID：v1"));
    assert_eq!(preview.artifact_sha256, artifact.sha256);
    assert!(preview.plan.requires_review);
    let target = Path::new(&game.root_path).join("mods/Example.jar");
    assert!(!target.exists());
    apply(&engine, &preview.plan);
    let reopened = FileEngine::new(
        Db::open(&engine.paths.db_file()).unwrap(),
        (*engine.paths).clone(),
    );
    let installed = get_install(&reopened, &preview.install_id).unwrap();
    assert_eq!(installed.download_origin.as_ref(), Some(origin));
    assert_eq!(installed.version.as_deref(), Some("1"));
    assert_eq!(installed.source, "modrinth");
    assert_eq!(installed.metadata["sourceModId"], "project");
    assert_eq!(installed.status, "enabled");
    assert_eq!(util::sha256_file(&target).unwrap(), artifact.sha256);
    let serialized = serde_json::to_string(&installed).unwrap();
    assert!(!serialized.contains("private-value"));
    assert!(!serialized.contains("download_url"));
    std::fs::write(path, b"changed payload").unwrap();
    assert!(plan_install(&engine, &req).is_err());
    assert_eq!(util::sha256_file(&target).unwrap(), artifact.sha256);
}

#[test]
fn web_download_provenance_is_catalog_bound_and_install_still_needs_approval() {
    let (_temp, engine, game) = fixture("minecraft-java");
    let id = uuid::Uuid::new_v4().to_string();
    let filename = "Example.jar";
    let path = engine.paths.downloads().join(format!("{id}-{filename}"));
    std::fs::rename(zip(&engine, &[("fabric.mod.json", b"{}")]), &path).unwrap();
    let artifact = crate::download::DownloadArtifact {
        id,
        path: path.clone(),
        filename: filename.into(),
        size: std::fs::metadata(&path).unwrap().len(),
        sha256: util::sha256_file(&path).unwrap(),
        sha512: String::new(),
        integrity_verified: false,
        requires_review: true,
        auto_install_allowed: false,
        warnings: vec![],
    };
    let mut req: InstallRequest = serde_json::from_value(serde_json::json!({
        "gameId": game.id,
        "artifactPath": path,
        "filename": filename,
        "name": filename,
        "modType": "java-mod",
    }))
    .unwrap();

    let omitted = req.clone();
    req.source = "web".into();
    assert!(matches!(
        plan_install(&engine, &req),
        Err(AppError::Conflict(_))
    ));
    artifacts::record_web(&engine, &artifact).unwrap();
    for (source, project, version) in [
        ("modrinth", None, None),
        ("local", Some("fake-project"), None),
        ("web", None, Some("fake-version")),
    ] {
        req.source = source.into();
        req.source_mod_id = project.map(str::to_owned);
        req.version = version.map(str::to_owned);
        assert!(matches!(
            plan_install(&engine, &req),
            Err(AppError::Conflict(_))
        ));
    }
    assert!(list_installs(&engine, &game.id).unwrap().is_empty());

    let with_omitted_hints = plan_install(&engine, &omitted).unwrap();
    let planned = get_install(&engine, &with_omitted_hints.install_id).unwrap();
    assert_eq!(planned.source, "web");
    assert!(planned.version.is_none());
    assert!(planned.download_origin.is_none());
    assert!(planned.metadata["sourceModId"].is_null());

    req.source = "web".into();
    req.source_mod_id = Some(String::new());
    req.version = Some(String::new());
    req.expected_sha256 = Some(artifact.sha256.clone());
    let preview = plan_install(&engine, &req).unwrap();
    assert_eq!(preview.artifact_sha256, artifact.sha256);
    assert!(preview.download_origin.is_none());
    assert!(preview.requires_review && preview.plan.requires_review);
    let target = Path::new(&game.root_path).join("mods/Example.jar");
    assert_eq!(preview.plan.entries.len(), 1);
    assert_eq!(preview.plan.entries[0].operation.dst(), target);
    assert!(matches!(
        engine.apply_file_ops(&preview.plan.id),
        Err(AppError::Denied(_))
    ));
    assert!(!target.exists());
    apply(&engine, &preview.plan);

    let installed = get_install(&engine, &preview.install_id).unwrap();
    assert_eq!(installed.source, "web");
    assert!(installed.version.is_none());
    assert!(installed.download_origin.is_none());
    assert!(installed.metadata["sourceModId"].is_null());
    assert_eq!(installed.status, "enabled");
    assert_eq!(util::sha256_file(&target).unwrap(), artifact.sha256);
    req.replace_install_id = Some(installed.id);
    assert!(matches!(
        plan_install(&engine, &req),
        Err(AppError::Conflict(_))
    ));
}

#[test]
fn local_and_legacy_labels_never_manufacture_exact_download_origin() {
    let (_temp, engine, game) = fixture("minecraft-java");
    let archive = zip(&engine, &[("fabric.mod.json", b"{}")]);
    let mut req = request(&game, archive, "java-mod", "Example.jar");
    req.source = "modrinth".into();
    req.source_mod_id = Some("project".into());
    req.version = Some("v1".into());
    let preview = plan_install(&engine, &req).unwrap();
    assert!(preview.download_origin.is_none());
    apply(&engine, &preview.plan);
    let installed = get_install(&engine, &preview.install_id).unwrap();
    assert!(installed.download_origin.is_none());
    assert!(serde_json::to_value(&installed).unwrap()["downloadOrigin"].is_null());

    let mut legacy_metadata = installed.metadata;
    legacy_metadata
        .as_object_mut()
        .unwrap()
        .remove("downloadOrigin");
    engine
        .db
        .conn()
        .execute(
            "UPDATE installs SET metadata_json=?1 WHERE id=?2",
            params![legacy_metadata.to_string(), preview.install_id],
        )
        .unwrap();
    let legacy = get_install(&engine, &preview.install_id).unwrap();
    assert!(legacy.download_origin.is_none());
    assert_eq!(legacy.metadata["sourceModId"], "project");
}

#[cfg(unix)]
#[test]
fn replacing_registered_or_staging_roots_with_links_does_not_expand_authority() {
    use std::os::unix::fs::symlink;
    let (temp, engine, game) = fixture("minecraft-java");
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let archive = zip(&engine, &[("fabric.mod.json", b"{}")]);
    std::fs::rename(&game.root_path, temp.path().join("game-old")).unwrap();
    symlink(&outside, &game.root_path).unwrap();
    let req = request(&game, archive, "java-mod", "Example.jar");
    assert!(plan_install(&engine, &req).is_err());
    assert!(engine
        .plan_file_ops(
            vec![FileOp::Copy {
                src: req.artifact_path,
                dst: Path::new(&game.root_path).join("mods/Example.jar")
            }],
            "blocked".into()
        )
        .is_err());
    assert!(!outside.join("mods").exists());
    // Even an unrelated offline game root must not stop safe manager reads.
    assert!(engine
        .paths
        .guard(&engine.paths.downloads(), &engine.roots().unwrap())
        .is_ok());
    std::fs::rename(
        engine.paths.staging(),
        engine.paths.root.join("staging-old"),
    )
    .unwrap();
    symlink(&outside, engine.paths.staging()).unwrap();
    let req = request(
        &game,
        outside.join("payload.jar"),
        "java-mod",
        "Example.jar",
    );
    std::fs::write(&req.artifact_path, b"outside").unwrap();
    assert!(plan_install(&engine, &req).is_err());
}

#[cfg(unix)]
#[test]
fn root_identity_is_bound_to_selected_game_even_when_target_is_other_authorized_root() {
    use std::os::unix::fs::symlink;
    for to_manager in [false, true] {
        let (temp, engine, game) = fixture("minecraft-java");
        let other = temp.path().join("other-game");
        std::fs::create_dir(&other).unwrap();
        std::fs::write(other.join("options.txt"), b"other game").unwrap();
        let other_game = games::register_game(&engine.db, &other, Some("minecraft-java")).unwrap();
        let path = zip(&engine, &[("fabric.mod.json", b"{}")]);
        std::fs::rename(&game.root_path, temp.path().join("original-game")).unwrap();
        let target = if to_manager {
            engine.paths.downloads()
        } else {
            PathBuf::from(&other_game.root_path)
        };
        symlink(&target, &game.root_path).unwrap();
        let before: i64 = engine
            .db
            .conn()
            .query_row("SELECT count(*) FROM file_transactions", [], |r| r.get(0))
            .unwrap();
        assert!(plan_install(&engine, &request(&game, path, "java-mod", "Example.jar")).is_err());
        let after: i64 = engine
            .db
            .conn()
            .query_row("SELECT count(*) FROM file_transactions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, after);
        assert!(!target.join("mods").exists());
        assert!(list_installs(&engine, &game.id).unwrap().is_empty());
    }
}

#[test]
fn valid_and_malformed_hashes_and_post_preview_payload_changes() {
    let (_temp, engine, game) = fixture("minecraft-java");
    let path = zip(&engine, &[("fabric.mod.json", b"{}")]);
    let mut req = request(&game, path.clone(), "java-mod", "Example.jar");
    req.expected_sha256 = Some("z".repeat(64));
    assert!(plan_install(&engine, &req).is_err());
    req.expected_sha256 = Some(util::sha256_file(&path).unwrap().to_uppercase());
    let preview = plan_install(&engine, &req).unwrap();
    std::fs::write(&path, b"changed after preview").unwrap();
    engine.approve_plan(&preview.plan.id).unwrap();
    assert!(engine.apply_file_ops(&preview.plan.id).is_err());
    assert!(!Path::new(&game.root_path).join("mods").exists());
}

#[test]
fn partial_downloads_cannot_be_deployed_through_raw_file_tools() {
    let (_temp, engine, game) = fixture("minecraft-java");
    let partial = engine.paths.downloads().join("payload.part");
    std::fs::write(&partial, b"partial payload").unwrap();
    assert!(engine
        .plan_file_ops(
            vec![FileOp::Copy {
                src: partial.clone(),
                dst: Path::new(&game.root_path).join("mods/payload.jar")
            }],
            "blocked partial".into()
        )
        .is_err());
    let trash = trash::plan_trash(
        &engine,
        &trash::TrashRequest {
            paths: vec![partial.to_string_lossy().into()],
            game_id: None,
            reason: "discard partial".into(),
        },
    )
    .unwrap();
    apply(&engine, &trash);
    assert!(!partial.exists());
    let rejected = engine.paths.staging().join("payload.rejected");
    std::fs::write(&rejected, b"rejected staged payload").unwrap();
    for op in [
        FileOp::Copy {
            src: rejected.clone(),
            dst: Path::new(&game.root_path).join("mods/staged.jar"),
        },
        FileOp::Move {
            src: rejected.clone(),
            dst: Path::new(&game.root_path).join("mods/staged.jar"),
        },
    ] {
        assert!(engine
            .plan_file_ops(vec![op], "blocked staged quarantine".into())
            .is_err());
    }
    let trash = trash::plan_trash(
        &engine,
        &trash::TrashRequest {
            paths: vec![rejected.to_string_lossy().into()],
            game_id: None,
            reason: "discard staged quarantine".into(),
        },
    )
    .unwrap();
    apply(&engine, &trash);
    assert!(!rejected.exists());
}
