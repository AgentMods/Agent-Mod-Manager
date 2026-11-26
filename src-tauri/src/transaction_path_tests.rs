use crate::safe_fs::rename_exclusive;
use std::{fs, path::PathBuf};

#[cfg(unix)]
use super::{tests::setup, FileOp};
#[cfg(unix)]
use crate::safe_fs::{create_dir_all, create_new, open_regular};
#[cfg(unix)]
use std::{
    io::Read,
    os::unix::fs::{symlink, MetadataExt},
    process::Command,
    sync::mpsc,
    thread,
    time::Duration,
};

fn canonical_tempdir() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    (temp, root)
}

#[test]
fn ordinary_storage_files_and_registered_games_share_transactional_access() {
    use super::{tests::setup, FileOp};
    let (dir, engine, _) = setup();
    let game = dir.path().join("Another game");
    let source = game.join("config/settings.txt");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, "fixture").unwrap();
    assert!(crate::filesystem::read_file(&engine, &source).is_err());
    crate::games::register_game(&engine.db, &game, None).unwrap();
    assert!(crate::filesystem::read_dir(&engine, &game).is_ok());
    assert!(crate::filesystem::read_file(&engine, &source).is_ok());

    let root = &engine.paths.root;
    for dst in [root.join("notes.txt"), root.join("custom/nested/notes.txt")] {
        let plan = engine
            .plan_file_ops(
                vec![FileOp::Copy {
                    src: source.clone(),
                    dst: dst.clone(),
                }],
                "Copy into storage".into(),
            )
            .unwrap();
        assert!(!dst.exists());
        assert!(!engine.has_external_paths(&plan).unwrap());
        engine.approve_plan(&plan.id).unwrap();
        engine.apply_file_ops(&plan.id).unwrap();
        assert_eq!(
            crate::filesystem::read_file(&engine, &dst).unwrap().lines[0].text,
            "fixture"
        );
        engine.rollback_transaction(&plan.id).unwrap();
        assert!(!dst.exists());
    }
}

#[test]
fn generic_storage_access_preserves_manager_state_and_credentials() {
    use super::{tests::setup, FileOp};
    let (_dir, engine, game) = setup();
    let src = game.join("note.txt");
    fs::write(&src, "fixture").unwrap();
    let root = &engine.paths.root;
    for engine in [&engine, &engine.for_local_agent()] {
        for relative in [
            "",
            "manager.db",
            "manager.db-wal",
            "manager.db-shm",
            "manager.db-journal",
            "backups/saved",
            "trash/item",
            "logs/manager.log",
            "gta-rpf-private/blob",
            "Mods/package",
            "private/credentials",
            "custom/.env.local",
            "downloads",
            "skills",
            "plugins",
            "staging",
        ] {
            let dst = root.join(relative);
            assert!(
                matches!(
                    engine.plan_file_ops(
                        vec![FileOp::Copy {
                            src: src.clone(),
                            dst
                        }],
                        "protected".into()
                    ),
                    Err(crate::error::AppError::Denied(_))
                ),
                "{relative}"
            );
        }
    }
    let folder = root.join("custom");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join(".env"), "secret").unwrap();
    assert!(matches!(
        engine.plan_file_ops(
            vec![FileOp::Move {
                src: folder,
                dst: game.join("moved")
            }],
            "protected descendant".into()
        ),
        Err(crate::error::AppError::Denied(_))
    ));
}

#[test]
#[cfg(unix)]
fn rollback_refuses_an_in_root_symlink_and_retries_after_restoring_the_file() {
    let (_temp, engine, game) = setup();
    let src = engine.paths.downloads().join("mod.txt");
    let dst = game.join("mod.txt");
    let saved = game.join("saved-installed.txt");
    let other = game.join("other.txt");
    fs::write(&src, b"installed").unwrap();
    fs::write(&dst, b"before install").unwrap();
    // Matching contents make the path substitution, not a hash mismatch, decisive.
    fs::write(&other, b"installed").unwrap();
    let plan = engine
        .plan_file_ops(
            vec![FileOp::Copy {
                src,
                dst: dst.clone(),
            }],
            "Install mod".into(),
        )
        .unwrap();
    engine.approve_plan(&plan.id).unwrap();
    engine.apply_file_ops(&plan.id).unwrap();
    assert_eq!(fs::read(&dst).unwrap(), b"installed");

    let status = || -> String {
        engine
            .db
            .conn()
            .query_row(
                "SELECT status FROM file_transactions WHERE id=?1",
                [&plan.id],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(status(), "committed");

    fs::rename(&dst, &saved).unwrap();
    symlink(&other, &dst).unwrap();
    assert!(engine.rollback_transaction(&plan.id).is_err());
    assert_eq!(status(), "committed");
    assert_eq!(fs::read_link(&dst).unwrap(), other);
    assert_eq!(fs::read(&other).unwrap(), b"installed");

    fs::remove_file(&dst).unwrap();
    fs::rename(&saved, &dst).unwrap();
    engine.rollback_transaction(&plan.id).unwrap();
    assert_eq!(status(), "rolled_back");
    assert_eq!(fs::read(&dst).unwrap(), b"before install");
    assert_eq!(fs::read(&other).unwrap(), b"installed");
}

#[test]
#[cfg(unix)]
fn apply_refuses_to_redirect_an_approved_copy_through_a_linked_parent() {
    let (_temp, engine, game) = setup();
    let src = engine.paths.downloads().join("mod.txt");
    let parent = game.join("mods");
    let saved = game.join("saved-mods");
    let other = game.join("other-mods");
    let dst = parent.join("mod.txt");
    fs::write(&src, b"new").unwrap();
    for dir in [&parent, &other] {
        fs::create_dir(dir).unwrap();
        fs::write(dir.join("mod.txt"), b"old").unwrap();
    }
    let plan = engine
        .plan_file_ops(vec![FileOp::Copy { src, dst }], "Install mod".into())
        .unwrap();
    engine.approve_plan(&plan.id).unwrap();

    fs::rename(&parent, &saved).unwrap();
    symlink(&other, &parent).unwrap();
    assert!(engine.apply_file_ops(&plan.id).is_err());
    assert_eq!(fs::read_link(&parent).unwrap(), other);
    assert_eq!(fs::read(saved.join("mod.txt")).unwrap(), b"old");
    assert_eq!(fs::read(other.join("mod.txt")).unwrap(), b"old");
}

#[test]
#[cfg(unix)]
fn open_regular_rejects_symlinks_substituted_at_the_leaf_or_an_ancestor() {
    let (_temp, root) = canonical_tempdir();
    let ancestor = root.join("logs");
    let path = ancestor.join("nested/log.txt");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"log contents").unwrap();
    let mut contents = Vec::new();
    open_regular(&path)
        .unwrap()
        .read_to_end(&mut contents)
        .unwrap();
    assert_eq!(contents, b"log contents");

    let saved_leaf = path.with_file_name("saved.txt");
    fs::rename(&path, &saved_leaf).unwrap();
    symlink(&saved_leaf, &path).unwrap();
    assert!(open_regular(&path).is_err());
    fs::remove_file(&path).unwrap();
    fs::rename(&saved_leaf, &path).unwrap();

    let saved_ancestor = root.join("saved-logs");
    fs::rename(&ancestor, &saved_ancestor).unwrap();
    symlink(&saved_ancestor, &ancestor).unwrap();
    assert!(open_regular(&path).is_err());
}

#[test]
#[cfg(unix)]
fn open_regular_rejects_a_fifo_without_waiting_for_a_writer() {
    let (_temp, root) = canonical_tempdir();
    let fifo = root.join("pipe");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());

    // A blocking open would otherwise hang the entire test suite.
    let (sender, receiver) = mpsc::channel();
    let worker = thread::spawn(move || sender.send(open_regular(&fifo).is_err()));
    assert!(receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("opening a FIFO without a writer blocked"));
    worker.join().unwrap().unwrap();
}

#[test]
#[cfg(unix)]
fn exclusive_creation_does_not_make_files_or_directories_through_a_linked_parent() {
    let (_temp, root) = canonical_tempdir();
    let elsewhere = root.join("elsewhere");
    let link = root.join("link");
    fs::create_dir(&elsewhere).unwrap();
    symlink(&elsewhere, &link).unwrap();
    assert!(create_dir_all(&link.join("new/nested")).is_err());
    assert!(create_new(&link.join("new/file.txt")).is_err());
    assert!(fs::read_dir(&elsewhere).unwrap().next().is_none());
}

#[test]
fn rename_exclusive_preserves_both_files_on_collision() {
    let (_temp, root) = canonical_tempdir();
    let src = root.join("source.txt");
    let dst = root.join("destination.txt");
    fs::write(&src, b"source").unwrap();
    fs::write(&dst, b"destination").unwrap();

    assert!(rename_exclusive(&src, &dst).is_err());
    assert_eq!(fs::read(&src).unwrap(), b"source");
    assert_eq!(fs::read(&dst).unwrap(), b"destination");
}

#[test]
fn rename_exclusive_preserves_an_existing_empty_destination_directory() {
    let (_temp, root) = canonical_tempdir();
    let src = root.join("source");
    let dst = root.join("destination");
    fs::create_dir(&src).unwrap();
    fs::create_dir(&dst).unwrap();
    fs::write(src.join("payload.txt"), b"source").unwrap();
    #[cfg(unix)]
    let dst_inode = fs::metadata(&dst).unwrap().ino();

    assert!(rename_exclusive(&src, &dst).is_err());
    assert_eq!(fs::read(src.join("payload.txt")).unwrap(), b"source");
    #[cfg(unix)]
    assert_eq!(fs::metadata(&dst).unwrap().ino(), dst_inode);
    assert!(fs::read_dir(&dst).unwrap().next().is_none());
}

#[test]
#[cfg(unix)]
fn rename_exclusive_rejects_linked_source_or_destination_ancestors() {
    let (_temp, root) = canonical_tempdir();
    let src = root.join("source/nested/payload.txt");
    let dst = root.join("destination/nested/payload.txt");
    fs::create_dir_all(src.parent().unwrap()).unwrap();
    fs::create_dir_all(dst.parent().unwrap()).unwrap();
    fs::write(&src, b"source").unwrap();
    symlink(root.join("source"), root.join("source-link")).unwrap();
    symlink(root.join("destination"), root.join("destination-link")).unwrap();

    for (from, to) in [
        (root.join("source-link/nested/payload.txt"), dst.clone()),
        (
            src.clone(),
            root.join("destination-link/nested/payload.txt"),
        ),
    ] {
        assert!(rename_exclusive(&from, &to).is_err());
        assert_eq!(fs::read(&src).unwrap(), b"source");
        assert!(!dst.exists());
    }

    rename_exclusive(&src, &dst).unwrap();
    assert!(!src.exists());
    assert_eq!(fs::read(&dst).unwrap(), b"source");
}
