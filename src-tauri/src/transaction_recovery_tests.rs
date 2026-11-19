use super::{tests::setup, *};
use std::{
    io::{self, Read},
    panic::AssertUnwindSafe,
    process::{Command, Stdio},
    sync::atomic::Ordering,
    thread,
    time::{Duration, Instant},
};

const CRASH_PLAN_ENV: &str = "AMM_TEST_TRANSACTION_CRASH_PLAN";
const CRASH_MARKER: &str = "replacement-rename-ready";

fn status(engine: &FileEngine, id: &str) -> String {
    engine
        .db
        .conn()
        .query_row(
            "SELECT status FROM file_transactions WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
}

fn reopen(engine: FileEngine) -> FileEngine {
    let paths = engine.paths.as_ref().clone();
    drop(engine);
    FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths)
}

fn load_journal(engine: &FileEngine, id: &str) -> Vec<JournalEntry> {
    let json: String = engine
        .db
        .conn()
        .query_row(
            "SELECT journal_json FROM file_transactions WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str(&json).unwrap()
}

fn wait_for_worker<T>(mut poll: impl FnMut() -> io::Result<Option<T>>) -> io::Result<T> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(value) = poll()? {
            return Ok(value);
        }
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "worker for process_crash_recovers_overwrite_and_implicit_parents"]
fn process_crash_worker() {
    let Ok(plan_id) = std::env::var(CRASH_PLAN_ENV) else {
        return;
    };
    let fixture = std::env::current_dir().unwrap().canonicalize().unwrap();
    let paths = AppPaths {
        root: fixture.join("data"),
    };
    let game = fixture.join("game");
    assert!(paths.db_file().is_file());
    let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
    assert_eq!(engine.roots().unwrap(), std::slice::from_ref(&game));
    *engine.before_rename.lock().unwrap() = Some(Box::new(move |step| {
        if step.to == game.join("installed") {
            // The backup rename has completed and the replacement intent is durable.
            fs::File::create_new(fixture.join(CRASH_MARKER)).unwrap();
            // The parent holds stdin open until the forced exit. EOF cannot resume apply.
            io::stdin().read_exact(&mut [0]).unwrap();
            panic!("crash worker must be killed while paused");
        }
    }));
    panic!(
        "crash worker did not reach the rename: {:?}",
        engine.apply_file_ops(&plan_id)
    );
}

#[test]
fn process_crash_recovers_overwrite_and_implicit_parents() {
    let (fixture, engine, game) = setup();
    let src = engine.paths.downloads().join("input");
    let parent = game.join("mods");
    let nested = parent.join("new");
    let added = nested.join("package");
    let installed = game.join("installed");
    fs::write(&src, b"payload").unwrap();
    fs::write(&installed, b"original").unwrap();
    let original = require_identity(&installed).unwrap();
    let plan = engine
        .plan_file_ops(
            vec![
                FileOp::Copy {
                    src: src.clone(),
                    dst: added.clone(),
                },
                FileOp::Copy {
                    src: src.clone(),
                    dst: installed.clone(),
                },
            ],
            String::new(),
        )
        .unwrap();
    engine.approve_plan(&plan.id).unwrap();
    let paths = engine.paths.as_ref().clone();
    drop(engine);

    let (_, module) = module_path!().split_once("::").unwrap();
    let mut worker = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--nocapture", "--exact"])
        .arg(format!("{module}::process_crash_worker"))
        .current_dir(fixture.path())
        .env(CRASH_PLAN_ENV, &plan.id)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let marker = fixture.path().join(CRASH_MARKER);
    let ready = wait_for_worker(|| {
        if let Some(exit) = worker.try_wait()? {
            return Err(io::Error::other(format!("worker exited early: {exit}")));
        }
        Ok(marker.try_exists()?.then_some(()))
    });
    // Kill and reap even if waiting for the marker failed.
    let killed = worker.kill();
    let exit = wait_for_worker(|| worker.try_wait());
    ready.expect("crash worker did not reach the replacement rename");
    killed.expect("failed to kill the paused worker");
    let exit = exit.expect("killed worker did not exit");
    assert!(!exit.success());
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(exit.signal(), Some(9)); // SIGKILL
    }

    let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
    assert_eq!(status(&engine, &plan.id), "applying");
    let journal = load_journal(&engine, &plan.id);
    let [addition, overwrite] = journal.as_slice() else {
        panic!("expected exactly two entries");
    };
    assert_eq!(addition.state, "done");
    assert_eq!(overwrite.state, "intent");
    let [parent_step, nested_step, add_step] = addition.renames.as_deref().unwrap() else {
        panic!("expected two parent renames and one addition");
    };
    for (step, expected) in [(parent_step, &parent), (nested_step, &nested)] {
        assert_eq!(&step.to, expected);
        assert!(step.empty_dir_only);
        assert_eq!(require_identity(expected).unwrap(), step.identity);
    }
    assert_eq!(add_step.to, added);
    assert_eq!(fs::read(&added).unwrap(), b"payload");

    let [backup, replacement] = overwrite.renames.as_deref().unwrap() else {
        panic!("expected a backup rename and a replacement intent");
    };
    assert_eq!(backup.from, installed);
    assert_eq!(backup.to, overwrite.backup);
    assert_eq!(fs::read(&backup.to).unwrap(), b"original");
    assert_eq!(require_identity(&backup.to).unwrap(), original);
    assert_eq!(replacement.to, installed);
    assert!(!installed.exists());
    assert_eq!(
        require_identity(&replacement.from).unwrap(),
        replacement.identity
    );

    assert_eq!(
        engine.recover_interrupted().unwrap(),
        std::slice::from_ref(&plan.id)
    );
    assert_eq!(status(&engine, &plan.id), "rolled_back");
    assert_eq!(
        load_journal(&engine, &plan.id)
            .into_iter()
            .map(|entry| entry.state)
            .collect::<Vec<_>>(),
        ["undone", "undone"]
    );
    assert_eq!(fs::read(&installed).unwrap(), b"original");
    assert_eq!(require_identity(&installed).unwrap(), original);
    assert!(!backup.to.exists());
    assert!(!parent.exists());
    assert!(!nested.exists());
    assert!(!added.exists());
    assert_eq!(fs::read(&addition.undo).unwrap(), b"payload");
    assert_eq!(require_identity(&addition.undo).unwrap(), add_step.identity);
    assert_eq!(fs::read(&replacement.from).unwrap(), b"payload");
    assert!(!overwrite.undo.exists());
    assert_eq!(fs::read(&src).unwrap(), b"payload");
    assert!(engine.recover_interrupted().unwrap().is_empty());
}

#[test]
fn unsuccessful_promotion_never_moves_a_competing_object() {
    for (kind, cross_device) in [
        ("copy", false),
        ("move", false),
        ("move", true),
        ("mkdir", false),
        ("parent", false),
    ] {
        let (_d, engine, game) = setup();
        let src = engine.paths.downloads().join("input");
        fs::write(&src, b"payload").unwrap();
        let collision = game.join("collision");
        let is_dir = matches!(kind, "mkdir" | "parent");
        let op = match kind {
            "move" => FileOp::Move {
                src: src.clone(),
                dst: collision.clone(),
            },
            "mkdir" => FileOp::Mkdir {
                dst: collision.clone(),
            },
            "parent" => FileOp::Copy {
                src: src.clone(),
                dst: collision.join("input"),
            },
            _ => FileOp::Copy {
                src: src.clone(),
                dst: collision.clone(),
            },
        };
        let plan = engine.plan_file_ops(vec![op], kind.into()).unwrap();
        engine.approve_plan(&plan.id).unwrap();
        engine.cross_device.store(cross_device, Ordering::Relaxed);
        let dst = collision.clone();
        let competitor = Arc::new(OnceLock::new());
        let captured = competitor.clone();
        *engine.before_rename.lock().unwrap() = Some(Box::new(move |step| {
            if step.to == dst && captured.get().is_none() {
                if is_dir {
                    fs::create_dir(&dst).unwrap();
                } else {
                    // Even byte-identical content must not be inferred as our own.
                    fs::write(&dst, b"payload").unwrap();
                }
                captured.set(require_identity(&dst).unwrap()).unwrap();
            }
        }));
        assert!(
            engine.apply_file_ops(&plan.id).is_err(),
            "{kind}/{cross_device}"
        );
        assert_eq!(
            status(&engine, &plan.id),
            "rolled_back",
            "{kind}/{cross_device}"
        );
        assert_eq!(
            require_identity(&collision).unwrap(),
            *competitor.get().unwrap()
        );
        assert!(!load_journal(&engine, &plan.id)[0].undo.exists());
        assert_eq!(fs::read(src).unwrap(), b"payload");
        if is_dir {
            assert_eq!(fs::read_dir(&collision).unwrap().count(), 0);
        } else {
            assert_eq!(fs::read(&collision).unwrap(), b"payload");
        }
    }
}

#[test]
fn competing_destination_after_backup_blocks_until_it_is_moved_aside() {
    for cross_device in [false, true] {
        let (_d, engine, game) = setup();
        let src = engine.paths.downloads().join("input");
        let dst = game.join("installed");
        fs::write(&src, b"payload").unwrap();
        fs::write(&dst, b"original").unwrap();
        let original = require_identity(&dst).unwrap();
        let plan = engine
            .plan_file_ops(
                vec![FileOp::Copy {
                    src,
                    dst: dst.clone(),
                }],
                String::new(),
            )
            .unwrap();
        engine.approve_plan(&plan.id).unwrap();
        engine.cross_device.store(cross_device, Ordering::Relaxed);
        let target = dst.clone();
        *engine.before_rename.lock().unwrap() = Some(Box::new(move |step| {
            if step.to == target {
                fs::write(&target, b"payload").unwrap();
            }
        }));
        assert!(engine.apply_file_ops(&plan.id).is_err());
        assert_eq!(status(&engine, &plan.id), "recovery_failed");
        let j = &load_journal(&engine, &plan.id)[0];
        assert_eq!(fs::read(&j.backup).unwrap(), b"original");
        assert_eq!(
            require_identity(if cross_device {
                j.overwrite_park.as_ref().unwrap()
            } else {
                &j.backup
            })
            .unwrap(),
            original
        );
        assert!(!j.undo.exists());
        let engine = reopen(engine);
        let competitor = require_identity(&dst).unwrap();
        assert!(engine.recover_interrupted().is_err());
        assert_eq!(require_identity(&dst).unwrap(), competitor);
        assert!(engine
            .plan_file_ops(
                vec![FileOp::Mkdir {
                    dst: game.join("other")
                }],
                String::new()
            )
            .is_err());
        let saved = game.join("external");
        fs::rename(&dst, &saved).unwrap();
        assert_eq!(engine.recover_interrupted().unwrap(), [plan.id]);
        assert_eq!(require_identity(&dst).unwrap(), original);
        assert_eq!(fs::read(&dst).unwrap(), b"original");
        assert_eq!(require_identity(&saved).unwrap(), competitor);
        assert_eq!(fs::read(&saved).unwrap(), b"payload");
    }
}

#[test]
fn restart_before_rename_preserves_matching_competitor() {
    let (_d, engine, game) = setup();
    let src = engine.paths.downloads().join("input");
    let dst = game.join("installed");
    fs::write(&src, b"payload").unwrap();
    let plan = engine
        .plan_file_ops(
            vec![FileOp::Copy {
                src,
                dst: dst.clone(),
            }],
            String::new(),
        )
        .unwrap();
    engine.approve_plan(&plan.id).unwrap();
    *engine.before_rename.lock().unwrap() = Some(Box::new(|step| {
        fs::write(&step.to, b"payload").unwrap();
        panic!("stop after recording rename intent, before the syscall");
    }));
    assert!(
        std::panic::catch_unwind(AssertUnwindSafe(|| engine.apply_file_ops(&plan.id))).is_err()
    );
    assert_eq!(status(&engine, &plan.id), "applying");
    let competitor = require_identity(&dst).unwrap();
    let engine = reopen(engine);
    assert_eq!(
        engine.recover_interrupted().unwrap(),
        std::slice::from_ref(&plan.id)
    );
    assert_eq!(require_identity(&dst).unwrap(), competitor);
    let journal = load_journal(&engine, &plan.id);
    assert_eq!(
        fs::read(&journal[0].renames.as_ref().unwrap()[0].from).unwrap(),
        b"payload"
    );
    assert!(!journal[0].undo.exists());
}

#[test]
fn restart_after_cross_volume_promotion_before_source_park() {
    let (_d, engine, game) = setup();
    let src = engine.paths.downloads().join("input");
    let dst = game.join("installed");
    fs::write(&src, b"payload").unwrap();
    let original = require_identity(&src).unwrap();
    let plan = engine
        .plan_file_ops(
            vec![FileOp::Move {
                src: src.clone(),
                dst: dst.clone(),
            }],
            String::new(),
        )
        .unwrap();
    engine.approve_plan(&plan.id).unwrap();
    engine.cross_device.store(true, Ordering::Relaxed);
    let original_path = src.clone();
    *engine.before_rename.lock().unwrap() = Some(Box::new(move |step| {
        if step.from == original_path && step.to.file_name().unwrap() == "0-source" {
            panic!("stop after promotion but before parking the original");
        }
    }));
    assert!(
        std::panic::catch_unwind(AssertUnwindSafe(|| engine.apply_file_ops(&plan.id))).is_err()
    );
    assert_eq!(require_identity(&src).unwrap(), original);
    let copy = require_identity(&dst).unwrap();
    assert_ne!(copy, original);
    let engine = reopen(engine);
    assert_eq!(
        engine.recover_interrupted().unwrap(),
        std::slice::from_ref(&plan.id)
    );
    let journal = load_journal(&engine, &plan.id);
    assert!(!dst.exists());
    assert_eq!(require_identity(&journal[0].undo).unwrap(), copy);
    assert_eq!(require_identity(&src).unwrap(), original);
}

#[test]
fn competing_cross_volume_park_preserves_the_original_and_retains_the_copy() {
    let (_d, engine, game) = setup();
    let src = engine.paths.downloads().join("input");
    let dst = game.join("installed");
    fs::write(&src, b"payload").unwrap();
    let original = require_identity(&src).unwrap();
    let plan = engine
        .plan_file_ops(
            vec![FileOp::Move {
                src: src.clone(),
                dst: dst.clone(),
            }],
            String::new(),
        )
        .unwrap();
    engine.approve_plan(&plan.id).unwrap();
    engine.cross_device.store(true, Ordering::Relaxed);
    *engine.before_rename.lock().unwrap() = Some(Box::new(|step| {
        if step.to.file_name().unwrap() == "0-source" {
            fs::write(&step.to, b"foreign").unwrap();
        }
    }));
    assert!(engine.apply_file_ops(&plan.id).is_err());
    assert_eq!(status(&engine, &plan.id), "rolled_back");
    let journal = load_journal(&engine, &plan.id);
    assert_eq!(
        fs::read(journal[0].source_park.as_ref().unwrap()).unwrap(),
        b"foreign"
    );
    assert_eq!(fs::read(&journal[0].undo).unwrap(), b"payload");
    assert_eq!(require_identity(&src).unwrap(), original);
    assert!(!dst.exists());
}

#[test]
fn rollback_leaves_an_implicit_directory_if_it_now_contains_unrelated_files() {
    let (_d, engine, game) = setup();
    let src = engine.paths.downloads().join("input");
    let dst = game.join("mods/new/package");
    let foreign = game.join("mods/new/readme");
    fs::write(&src, b"payload").unwrap();
    let plan = engine
        .plan_file_ops(
            vec![FileOp::Copy {
                src,
                dst: dst.clone(),
            }],
            String::new(),
        )
        .unwrap();
    engine.approve_plan(&plan.id).unwrap();
    engine.apply_file_ops(&plan.id).unwrap();
    fs::write(&foreign, b"foreign").unwrap();
    engine.rollback_transaction(&plan.id).unwrap();
    assert_eq!(fs::read(foreign).unwrap(), b"foreign");
    assert!(!dst.exists());
}

#[test]
fn replay_after_inverses_ran_before_progress_was_saved() {
    let (_d, engine, game) = setup();
    let src = engine.paths.downloads().join("input");
    let dst = game.join("installed");
    // Identical bytes exercise both the backup and retained-payload receipts.
    fs::write(&src, b"payload").unwrap();
    fs::write(&dst, b"payload").unwrap();
    let original = require_identity(&dst).unwrap();
    let plan = engine
        .plan_file_ops(
            vec![FileOp::Copy {
                src,
                dst: dst.clone(),
            }],
            String::new(),
        )
        .unwrap();
    engine.approve_plan(&plan.id).unwrap();
    engine.apply_file_ops(&plan.id).unwrap();
    let installed = require_identity(&dst).unwrap();
    assert_ne!(installed, original);
    let journal = load_journal(&engine, &plan.id);
    engine
        .save_journal(&plan.id, "rolling_back", &journal)
        .unwrap();
    engine.undo_entry(&journal[0]).unwrap();
    let engine = reopen(engine);
    assert_eq!(engine.recover_interrupted().unwrap(), [plan.id]);
    assert_eq!(require_identity(&dst).unwrap(), original);
    assert_eq!(require_identity(&journal[0].undo).unwrap(), installed);
}

#[test]
fn same_content_replacement_after_success_is_not_owned_by_the_transaction() {
    let (_d, engine, game) = setup();
    let src = engine.paths.downloads().join("input");
    let dst = game.join("installed");
    fs::write(&src, b"payload").unwrap();
    let plan = engine
        .plan_file_ops(
            vec![FileOp::Copy {
                src,
                dst: dst.clone(),
            }],
            String::new(),
        )
        .unwrap();
    engine.approve_plan(&plan.id).unwrap();
    engine.apply_file_ops(&plan.id).unwrap();
    let saved = game.join("saved");
    fs::rename(&dst, &saved).unwrap();
    fs::write(&dst, b"payload").unwrap();
    let competitor = require_identity(&dst).unwrap();
    let engine = reopen(engine);
    assert!(engine.rollback_transaction(&plan.id).is_err());
    assert_eq!(status(&engine, &plan.id), "committed");
    assert_eq!(require_identity(&dst).unwrap(), competitor);
    assert!(!load_journal(&engine, &plan.id)[0].undo.exists());
    assert_eq!(fs::read(&saved).unwrap(), b"payload");
}

#[test]
fn older_journal_history_is_readable_but_an_ambiguous_intent_is_not_undone() {
    let (_d, engine, game) = setup();
    let dst = game.join("installed");
    let plan = engine
        .plan_file_ops(vec![FileOp::Mkdir { dst: dst.clone() }], String::new())
        .unwrap();
    let mut journal = engine.prepare_journal(&plan).unwrap();
    journal[0].state = "intent".into();
    let mut old_json = serde_json::to_value(&journal).unwrap();
    old_json[0].as_object_mut().unwrap().remove("renames");
    engine
        .db
        .conn()
        .execute(
            "UPDATE file_transactions SET journal_json=?1,status='applying' WHERE id=?2",
            params![old_json.to_string(), plan.id],
        )
        .unwrap();
    fs::create_dir(&dst).unwrap();
    let engine = reopen(engine);
    assert!(engine.retained_paths().unwrap().contains(&journal[0].undo));
    assert!(engine.recover_interrupted().is_err());
    assert_eq!(status(&engine, &plan.id), "recovery_failed");
    assert!(dst.is_dir());
    assert!(!journal[0].undo.exists());
}
