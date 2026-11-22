use super::*;
use crate::{
    db::Db,
    storage::AppPaths,
    transaction::{FileEngine, FileOp},
};

#[test]
fn restart_migrates_data_and_keeps_transactions_usable() {
    let (temp, engine, game) = crate::transaction::tests::setup();
    let source = engine.paths.root.clone();
    let location = StorageLocation::new(source.clone()).unwrap();
    fs::write(source.join("private").join("test-secret"), b"fixture").unwrap();
    let parent = temp.path().join("new location");
    fs::create_dir(&parent).unwrap();
    let parent = parent.canonicalize().unwrap();
    let target = parent.join(DATA_FOLDER);
    let old_file = engine.paths.downloads().join("test.zip");
    fs::write(&old_file, b"archive").unwrap();
    engine
        .db
        .conn()
        .execute(
            "UPDATE games SET cover_path=?1 WHERE id='g'",
            [utf8_path(&old_file).unwrap()],
        )
        .unwrap();

    let installed = game.join("test.zip");
    let plan = engine
        .plan_file_ops(
            vec![FileOp::Copy {
                src: old_file.clone(),
                dst: installed.clone(),
            }],
            "Install fixture".into(),
        )
        .unwrap();
    engine.approve_plan(&plan.id).unwrap();
    engine.apply_file_ops(&plan.id).unwrap();
    let (snapshot, snapshot_plan) = crate::snapshot::plan_create(&engine, "g", "fixture").unwrap();
    engine.approve_plan(&snapshot_plan.id).unwrap();
    engine.apply_file_ops(&snapshot_plan.id).unwrap();
    let trash = crate::trash::plan_trash(
        &engine,
        &crate::trash::TrashRequest {
            paths: vec![utf8_path(&installed).unwrap().into()],
            game_id: Some("g".into()),
            reason: "fixture".into(),
        },
    )
    .unwrap();
    engine.approve_plan(&trash.id).unwrap();
    engine.apply_file_ops(&trash.id).unwrap();

    location.schedule(&engine, &parent).unwrap();
    assert!(!target.exists());
    // Simulate a process restart with all connections and handles closed.
    drop(engine);
    drop(location);
    let location = StorageLocation::new(source.clone()).unwrap();
    assert_eq!(location.startup_root().unwrap(), target);
    assert!(location.failure().unwrap().is_none());
    let paths = AppPaths::new(target.clone())
        .unwrap()
        .with_control_root(&source);
    let migrated = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
    let cover = crate::games::get_game(&migrated.db, "g")
        .unwrap()
        .cover_path
        .unwrap();
    assert_eq!(Path::new(&cover), target.join("downloads/test.zip"));
    assert_eq!(fs::read(&cover).unwrap(), b"archive");
    assert_eq!(migrated.roots().unwrap(), [game]);
    let snapshots = crate::snapshot::list_snapshots(&migrated, "g").unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].id, snapshot.id);
    assert_eq!(
        snapshots[0].path,
        target.join("backups/snapshots").join(snapshot.id)
    );
    assert_eq!(
        fs::read(snapshots[0].path.join("files/test.zip")).unwrap(),
        b"archive"
    );
    let items = crate::trash::list_trash(&migrated.db, false).unwrap();
    assert!(Path::new(&items[0].trash_path).starts_with(&target));
    assert!(items[0].exists);
    migrated.rollback_transaction(&trash.id).unwrap();
    assert_eq!(fs::read(installed).unwrap(), b"archive");
    assert!(old_file.is_file());
    assert!(migrated
        .paths
        .guard(
            &source.join("private/test-secret"),
            std::slice::from_ref(&source)
        )
        .is_err());
    assert_eq!(
        fs::read(source.join("private/test-secret")).unwrap(),
        b"fixture"
    );
    assert!(!target.join("private/test-secret").exists());
    drop(migrated);
    drop(location);
    assert_eq!(
        StorageLocation::new(source)
            .unwrap()
            .startup_root()
            .unwrap(),
        target
    );
}

#[test]
fn a_conflicting_target_created_before_restart_preserves_active_storage() {
    let (temp, engine, game) = crate::transaction::tests::setup();
    let source = engine.paths.root.clone();
    let location = StorageLocation::new(source.clone()).unwrap();
    assert!(location.schedule(&engine, &game).is_err());
    assert!(location.schedule(&engine, &source).is_err());
    let parent = temp.path().canonicalize().unwrap();
    location.schedule(&engine, &parent).unwrap();
    assert!(location.schedule(&engine, &parent).is_err());
    let target = parent.join(DATA_FOLDER);
    fs::create_dir(&target).unwrap();
    fs::write(target.join("existing"), b"keep").unwrap();
    drop(engine);
    assert_eq!(location.startup_root().unwrap(), source);
    assert!(location.failure().unwrap().is_some());
    assert_eq!(fs::read(target.join("existing")).unwrap(), b"keep");
    assert!(!target.join(MANAGER_DB).exists());
}

#[test]
fn startup_finishes_a_copy_promoted_before_the_location_was_committed() {
    let (temp, engine, _) = crate::transaction::tests::setup();
    let source = engine.paths.root.clone();
    let location = StorageLocation::new(source.clone()).unwrap();
    let parent = temp.path().canonicalize().unwrap();
    location.schedule(&engine, &parent).unwrap();
    drop(engine);
    let stage = parent.join("staged-copy");
    fs::create_dir(&stage).unwrap();
    let mut migration = Migration {
        target: parent.join(DATA_FOLDER),
        parent_identity: identity(&parent).unwrap(),
        ready: Some(ReadyCopy {
            stage: stage.clone(),
            identity: identity(&stage).unwrap(),
        }),
    };
    copy_storage(&source, &stage, &migration.target).unwrap();
    location
        .control
        .lock()
        .unwrap()
        .execute(
            "UPDATE location SET pending=?1 WHERE id=1",
            [serde_json::to_string(&migration).unwrap()],
        )
        .unwrap();
    safe_fs::rename_checked(
        &stage,
        &migration.target,
        &migration.ready.take().unwrap().identity,
    )
    .unwrap();
    assert_eq!(location.startup_root().unwrap(), migration.target);
    assert!(location.failure().unwrap().is_none());
}

#[test]
fn moving_storage_preserves_the_game_library_and_picker_receipt() {
    let (temp, engine, _) = crate::transaction::tests::setup();
    let source = engine.paths.root.clone();
    let original = temp.path().canonicalize().unwrap().join("picked.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&original).unwrap());
    zip.start_file("payload.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"fixture").unwrap();
    zip.finish().unwrap();
    let ready = crate::imports::plan_import_archive(&engine, &original).unwrap();
    engine.approve_plan(&ready.plan.id).unwrap();
    engine.apply_file_ops(&ready.plan.id).unwrap();
    let entry = crate::imports::register_imported_mod(&engine, "g", &ready.plan.id).unwrap();
    let parent = temp.path().join("new parent");
    fs::create_dir(&parent).unwrap();
    let location = StorageLocation::new(source.clone()).unwrap();
    location.schedule(&engine, &parent).unwrap();
    drop(engine);
    drop(location);
    let location = StorageLocation::new(source.clone()).unwrap();
    let target = location.startup_root().unwrap();
    assert_eq!(target, parent.canonicalize().unwrap().join(DATA_FOLDER));
    let paths = AppPaths::new(target.clone())
        .unwrap()
        .with_control_root(&source);
    let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
    let listed = crate::mod_library::list_for_game(&engine.db, "g").unwrap();
    assert_eq!(listed.as_slice(), std::slice::from_ref(&entry));
    let verified = crate::mod_library::verify_for_install(&engine, "g", &entry.id).unwrap();
    assert!(verified.archive_path.starts_with(target.join("Mods")));
    assert_eq!(
        fs::read(verified.archive_path).unwrap(),
        fs::read(original).unwrap()
    );
    let repeated = crate::imports::register_imported_mod(&engine, "g", &ready.plan.id).unwrap();
    assert_eq!(repeated.id, entry.id);
}
