use super::*;
use crate::{
    install::InstalledFile,
    paths::display_path,
    transaction::{fingerprint, tests::setup},
};

fn record_install(engine: &FileEngine, id: &str, status: &str, paths: &[&Path]) {
    let files: Vec<_> = paths
        .iter()
        .map(|path| {
            let fp = fingerprint(path).unwrap().unwrap();
            InstalledFile {
                path: path.to_path_buf(),
                sha256: fp.sha256,
                size: fp.size,
                before: None,
            }
        })
        .collect();
    engine
        .db
        .conn()
        .execute(
            "INSERT INTO installs (id,game_id,name,status,files_json,installed_at,updated_at)
             VALUES (?1,'g',?1,?2,?3,'now','now')",
            params![id, status, serde_json::to_string(&files).unwrap()],
        )
        .unwrap();
}

fn request(paths: &[&Path]) -> TrashRequest {
    TrashRequest {
        paths: paths.iter().map(|path| display_path(path)).collect(),
        game_id: Some("g".into()),
        reason: String::new(),
    }
}

fn legacy_plan(engine: &FileEngine, paths: &[&Path], record_receipt: bool) -> FilePlan {
    // The old planner persisted the internal plan before recording the receipts.
    let drafts = prepare_trash(engine, &request(paths)).unwrap();
    let plan = engine
        .plan_internal(
            drafts.iter().map(|draft| draft.operation.clone()).collect(),
            "移入回收站".into(),
        )
        .unwrap();
    if record_receipt {
        record_prepared(engine, &engine.db.conn(), &plan.id, drafts).unwrap();
    }
    plan
}

fn assert_ownership_conflict(error: AppError, owners: &[&str], paths: &[&Path]) {
    let AppError::Conflict(message) = error else {
        panic!("expected install ownership conflict, got {error}");
    };
    assert!(
        owners.iter().any(|owner| message.contains(*owner)),
        "{message}"
    );
    assert!(
        paths
            .iter()
            .any(|path| message.contains(&display_path(path))),
        "{message}"
    );
}

#[test]
fn planning_blocks_shared_dll_and_asi_files_and_their_parent() {
    let (_temp, engine, game) = setup();
    let bin = game.join("bin");
    fs::create_dir(&bin).unwrap();
    let dll = bin.join("loader.dll");
    let asi = bin.join("plugin.asi");
    let untracked = game.join("notes.txt");
    for path in [&dll, &asi, &untracked] {
        fs::write(path, b"fixture").unwrap();
    }
    let owners = ["install-one", "install-two"];
    for owner in owners {
        record_install(&engine, owner, "enabled", &[&dll, &asi]);
    }

    for tracked in [&dll, &asi] {
        let error = plan_trash(&engine, &request(&[&untracked, tracked])).unwrap_err();
        assert_ownership_conflict(error, &owners, &[tracked]);
    }
    let error = plan_trash(&engine, &request(&[&bin])).unwrap_err();
    assert_ownership_conflict(error, &owners, &[&dll, &asi]);
    for path in [&dll, &asi, &untracked] {
        assert_eq!(fs::read(path).unwrap(), b"fixture");
    }
    let count: i64 = engine
        .db
        .conn()
        .query_row(
            "SELECT (SELECT count(*) FROM file_transactions) + (SELECT count(*) FROM trash_items)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    assert!(fs::read_dir(engine.paths.trash()).unwrap().next().is_none());
}

#[test]
fn legacy_planned_and_approved_trash_moves_recheck_live_install_ownership() {
    for (already_approved, record_receipt, whole_dir) in [
        (false, true, false),
        (true, true, true),
        (true, false, false),
    ] {
        let (_temp, engine, game) = setup();
        let bin = game.join("bin");
        fs::create_dir(&bin).unwrap();
        let dll = bin.join("loader.dll");
        let untracked = game.join("notes.txt");
        fs::write(&dll, b"unchanged file").unwrap();
        fs::write(&untracked, b"unowned file").unwrap();

        let selected = if whole_dir { &bin } else { &dll };
        let plan = legacy_plan(&engine, &[&untracked, selected], record_receipt);
        if already_approved {
            engine.approve_plan(&plan.id).unwrap();
        }
        record_install(&engine, "late-owner", "enabled", &[&dll]);
        if !already_approved {
            engine.approve_plan(&plan.id).unwrap();
        }

        let error = engine.apply_file_ops(&plan.id).unwrap_err();
        assert_ownership_conflict(error, &["late-owner"], &[&dll]);
        assert_eq!(fs::read(&dll).unwrap(), b"unchanged file");
        assert_eq!(fs::read(&untracked).unwrap(), b"unowned file");
        assert!(plan
            .entries
            .iter()
            .all(|entry| !entry.operation.dst().exists()));
        assert!(!engine.paths.backup_dir(&plan.id).exists());
        assert!(list_trash(&engine.db, false).unwrap().is_empty());
        let status: String = engine
            .db
            .conn()
            .query_row(
                "SELECT status FROM file_transactions WHERE id=?1",
                [&plan.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "approved");
    }
}

#[test]
fn ordinary_trash_still_moves_a_similarly_named_directory_owned_only_by_an_inactive_install() {
    let (_temp, engine, game) = setup();
    let bin = game.join("bin");
    let old_bin = game.join("bin-old");
    fs::create_dir(&bin).unwrap();
    fs::create_dir(&old_bin).unwrap();
    let active = bin.join("loader.dll");
    let inactive = old_bin.join("loader.dll");
    fs::write(&active, b"active dll").unwrap();
    fs::write(&inactive, b"inactive dll").unwrap();
    record_install(&engine, "active", "enabled", &[&active]);
    record_install(&engine, "inactive", "uninstalled", &[&inactive]);

    let plan = plan_trash(&engine, &request(&[&old_bin])).unwrap();
    engine.approve_plan(&plan.id).unwrap();
    engine.apply_file_ops(&plan.id).unwrap();
    assert!(!old_bin.exists());
    assert_eq!(fs::read(active).unwrap(), b"active dll");
    let trash = list_trash(&engine.db, false).unwrap();
    assert_eq!(trash.len(), 1);
    assert_eq!(
        fs::read(Path::new(&trash[0].trash_path).join("loader.dll")).unwrap(),
        b"inactive dll"
    );
}

fn assert_library_conflict(error: AppError, path: &Path) {
    assert!(
        matches!(error, AppError::Denied(ref message)
            if message.contains("Mods 库") && message.contains(&display_path(path))),
        "{error}"
    );
}

#[test]
fn planning_blocks_library_archives_payloads_and_directories() {
    let (_temp, engine, _game) = setup();
    let library = engine.paths.mod_library();
    let category = library.join("Game");
    let payload = category.join("package/payload/loader.dll");
    fs::create_dir_all(payload.parent().unwrap()).unwrap();
    let archive = category.join("package.zip");
    fs::write(&archive, b"library archive").unwrap();
    fs::write(&payload, b"library payload").unwrap();

    for path in [&library, &category, &archive, &payload] {
        assert_library_conflict(plan_trash(&engine, &request(&[path])).unwrap_err(), path);
    }
    assert_eq!(fs::read(&archive).unwrap(), b"library archive");
    assert_eq!(fs::read(&payload).unwrap(), b"library payload");
    assert!(fs::read_dir(engine.paths.trash()).unwrap().next().is_none());
}

#[test]
fn legacy_planned_and_approved_trash_moves_cannot_remove_the_mod_library() {
    let (_temp, engine, _game) = setup();
    let library = engine.paths.mod_library();
    let archive = library.join("package.zip");
    fs::write(&archive, b"library archive").unwrap();

    let planned = legacy_plan(&engine, &[&archive], true);
    let approved = legacy_plan(&engine, &[&library], false);
    engine.approve_plan(&approved.id).unwrap();
    engine.approve_plan(&planned.id).unwrap();
    for (plan, path) in [(&planned, &archive), (&approved, &library)] {
        assert_library_conflict(engine.apply_file_ops(&plan.id).unwrap_err(), path);
        assert!(!plan.entries[0].operation.dst().exists());
    }
    assert_eq!(fs::read(&archive).unwrap(), b"library archive");
    assert!(list_trash(&engine.db, false).unwrap().is_empty());
}

#[test]
fn ordinary_trash_restore_and_rollback_protect_a_later_owner_at_the_original_path() {
    for whole_dir in [false, true] {
        let (_temp, engine, game) = setup();
        let bin = game.join("bin");
        fs::create_dir(&bin).unwrap();
        let dll = bin.join("loader.dll");
        fs::write(&dll, b"trashed file").unwrap();
        let original = if whole_dir { &bin } else { &dll };
        let trash = plan_trash(&engine, &request(&[original])).unwrap();
        engine.approve_plan(&trash.id).unwrap();
        engine.apply_file_ops(&trash.id).unwrap();
        let items = list_trash(&engine.db, false).unwrap();
        let item = &items[0];
        let ids = std::slice::from_ref(&item.id);
        let stale = plan_restore(&engine, ids, false).unwrap();
        engine.approve_plan(&stale.id).unwrap();

        fs::create_dir_all(&bin).unwrap();
        fs::write(&dll, b"new install").unwrap();
        record_install(&engine, "later-owner", "enabled", &[&dll]);
        assert_ownership_conflict(
            plan_restore(&engine, ids, true).unwrap_err(),
            &["later-owner"],
            &[&dll],
        );
        assert_eq!(fs::read(&dll).unwrap(), b"new install");
        fs::remove_file(&dll).unwrap();
        if whole_dir {
            fs::remove_dir(&bin).unwrap();
        }
        assert_eq!(fingerprint(original).unwrap(), stale.entries[0].before);
        assert_ownership_conflict(
            engine.apply_file_ops(&stale.id).unwrap_err(),
            &["later-owner"],
            &[&dll],
        );
        assert_ownership_conflict(
            engine.rollback_transaction(&trash.id).unwrap_err(),
            &["later-owner"],
            &[&dll],
        );
        assert!(!original.exists());
        assert!(!engine.paths.backup_dir(&stale.id).exists());
        let restored_items = list_trash(&engine.db, false).unwrap();
        assert_eq!(restored_items.len(), 1);
        assert!(restored_items[0].exists);
    }
}
