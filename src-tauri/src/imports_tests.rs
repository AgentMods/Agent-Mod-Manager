use super::*;
use crate::{db::Db, games, install, plugins, storage::AppPaths};

fn setup() -> (tempfile::TempDir, FileEngine, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    // macOS tempdir may be spelled through /var -> /private/var. The selected
    // test path must itself contain no symlink ancestors.
    let root = temp.path().canonicalize().unwrap();
    let paths = AppPaths::new(root.join("manager")).unwrap();
    let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
    let source = root.join("Example.jar");
    fs::write(&source, b"inert archive bytes").unwrap();
    (temp, engine, source)
}

fn request(game: &games::Game, preview: &ImportPreview) -> install::InstallRequest {
    install::InstallRequest {
        game_id: game.id.clone(),
        artifact_path: preview.artifact_path.clone(),
        filename: preview.filename.clone(),
        name: "Imported example".into(),
        mod_type: "java-mod".into(),
        version: None,
        expected_sha256: Some(preview.sha256.clone()),
        folder_name: None,
        mappings: vec![],
        fomod_selection: None,
        source: "local".into(),
        source_mod_id: None,
        replace_install_id: None,
    }
}

fn copy_source(preview: &ImportPreview) -> &Path {
    match &preview.plan.entries[0].operation {
        FileOp::Copy { src, dst } => {
            assert_eq!(dst, &preview.artifact_path);
            src
        }
        _ => panic!("Import must produce exactly one COPY"),
    }
}

fn scratch_files(engine: &FileEngine) -> Vec<PathBuf> {
    walkdir::WalkDir::new(engine.paths.staging().join("import-scratch"))
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .collect()
}

fn plan_count(engine: &FileEngine) -> i64 {
    engine
        .db
        .conn()
        .query_row("SELECT count(*) FROM file_transactions", [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn preview_is_inert_camel_case_and_native_apply_enables_separate_install() {
    let (_temp, engine, source) = setup();
    let mut jar = zip::ZipWriter::new(File::create(&source).unwrap());
    jar.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    jar.write_all(b"{}").unwrap();
    jar.finish().unwrap();
    plugins::test_fixtures::install(&engine.db, &engine.paths).unwrap();
    let game_root = source.parent().unwrap().join("game");
    fs::create_dir(&game_root).unwrap();
    fs::write(game_root.join("options.txt"), "game").unwrap();
    let game = games::register_game(&engine.db, &game_root, Some("minecraft-java")).unwrap();
    let roots = engine.roots().unwrap();
    let original = fs::read(&source).unwrap();
    let preview = plan_import_archive(&engine, &source).unwrap();
    assert_eq!(preview.filename, "Example.jar");
    assert_eq!(preview.source_path, source);
    assert_eq!(preview.size, original.len() as u64);
    assert_eq!(preview.sha256, hex::encode(Sha256::digest(&original)));
    assert_eq!(preview.plan.entries.len(), 1);
    assert!(preview.plan.entries[0].before.is_none());
    assert!(preview.plan.requires_review);
    assert!(engine.get_plan(&preview.plan.id).unwrap().requires_review);
    assert!(!preview.warnings.is_empty());
    let json = serde_json::to_value(&preview).unwrap();
    let returned_path = PathBuf::from(json["artifactPath"].as_str().unwrap());
    assert_eq!(
        json["artifactPath"],
        paths::display_path(&preview.artifact_path)
    );
    assert_eq!(json["sourcePath"], paths::display_path(&source));
    assert_eq!(
        json["plan"]["entries"][0]["operation"]["dst"],
        json["artifactPath"]
    );
    assert!(json.get("artifact_path").is_none());
    let scratch = copy_source(&preview);
    assert!(scratch.starts_with(engine.paths.staging().join("import-scratch")));
    assert!(preview
        .artifact_path
        .starts_with(engine.paths.staging().join("imports")));
    assert_eq!(scratch.file_name(), source.file_name());
    assert_eq!(
        scratch.parent().unwrap().file_name(),
        preview.artifact_path.parent().unwrap().file_name()
    );
    assert_eq!(fs::read(scratch).unwrap(), original);
    assert!(!preview.artifact_path.exists());
    assert!(!game_root.join("mods").exists());
    assert!(engine
        .paths
        .guard(&source, &engine.roots().unwrap())
        .is_err());
    assert_eq!(engine.roots().unwrap(), roots);
    assert!(engine.apply_file_ops(&preview.plan.id).is_err());
    assert!(!preview.artifact_path.exists());
    assert!(install::plan_install(&engine, &request(&game, &preview)).is_err());

    engine.approve_plan(&preview.plan.id).unwrap();
    engine.apply_file_ops(&preview.plan.id).unwrap();
    assert_eq!(fs::read(&preview.artifact_path).unwrap(), original);
    assert_eq!(fs::read(&source).unwrap(), original);
    assert!(scratch.exists());
    let install_request = install::InstallRequest {
        artifact_path: returned_path,
        ..request(&game, &preview)
    };
    let install = install::plan_install(&engine, &install_request).unwrap();
    assert!(install.plan.requires_review);
    assert!(!game_root.join("mods/Example.jar").exists());
    engine.approve_plan(&install.plan.id).unwrap();
    engine.apply_file_ops(&install.plan.id).unwrap();
    assert_eq!(
        fs::read(game_root.join("mods/Example.jar")).unwrap(),
        original
    );
    assert_eq!(engine.roots().unwrap(), roots);
    engine.rollback_transaction(&preview.plan.id).unwrap();
    assert!(!preview.artifact_path.exists());
    assert!(scratch.exists());
    assert_eq!(fs::read(source).unwrap(), original);
}

#[test]
fn accepts_only_supported_extensions_case_insensitively_without_extraction() {
    let (_temp, engine, source) = setup();
    for name in ["a.jar", "a.ZIP", "a.mrpack", "a.7z", "a.TAR.GZ", "a.tgz"] {
        let path = source.with_file_name(name);
        fs::write(&path, b"not parsed or executed").unwrap();
        let preview = plan_import_archive(&engine, &path).unwrap();
        assert!(!preview.artifact_path.exists());
        assert_eq!(
            fs::read(copy_source(&preview)).unwrap(),
            b"not parsed or executed"
        );
    }
    let count = plan_count(&engine);
    for name in [
        "a",
        "a.gz",
        "a.tar",
        "a.exe",
        "a.zip.part",
        "a.zip.rejected",
        "a.json",
    ] {
        let path = source.with_file_name(name);
        fs::write(&path, b"rejected").unwrap();
        assert!(matches!(
            plan_import_archive(&engine, &path),
            Err(AppError::Unsupported(_))
        ));
    }
    assert_eq!(plan_count(&engine), count);
}

#[test]
fn filenames_are_portable_and_match_the_install_contract() {
    for name in [
        "../a.zip",
        "a/b.zip",
        "a\\b.zip",
        "a.zip:stream",
        "a:stream.zip",
        "a\n.zip",
        "a\0.zip",
        "a?.zip",
        "a*.zip",
        "a|.zip",
        "a<.zip",
        "a>.zip",
        "a\".zip",
        "a.zip.",
        "a.zip ",
        "NUL.zip",
        "con.JAR",
        "AUX .zip",
        "LPT9.zip",
        "COM1.zip",
        "CONIN$.zip",
        "CONOUT$.zip",
        "CLOCK$.zip",
        "COM\u{b9}.zip",
        ".hidden.zip",
        "~a.zip",
        "a$.zip",
        "a%.zip",
    ] {
        let path = Path::new(name);
        // selected_path separately rejects traversal/relative paths. Validate
        // filename with a Unix backslash too, without creating invalid Windows files.
        if name.contains('/') || (cfg!(windows) && name.contains('\\')) {
            let (_temp, engine, _) = setup();
            assert!(plan_import_archive(&engine, path).is_err(), "{name:?}");
        } else {
            // Within a directory, `a:stream.zip` is the selected filename on Windows.
            // In isolation, the same spelling starts with a drive-relative prefix.
            let selected = format!("selection{}{name}", std::path::MAIN_SEPARATOR);
            assert!(filename(Path::new(&selected)).is_err(), "{name:?}");
        }
    }
    assert!(filename(Path::new(&format!("{}.zip", "x".repeat(177)))).is_err());
    for name in [
        "Example Mod-1.2.zip",
        "COM10.jar",
        "mod\u{6a21}\u{7ec4}.tar.gz",
    ] {
        assert_eq!(filename(Path::new(name)).unwrap(), name);
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        assert!(filename(Path::new(std::ffi::OsStr::from_bytes(b"a\xff.zip"))).is_err());
    }
}

#[test]
fn rejects_oversized_sources_before_creating_scratch() {
    let (_temp, engine, source) = setup();
    let file = OpenOptions::new().write(true).open(&source).unwrap();
    file.set_len(MAX_BYTES).unwrap();
    // Validate the inclusive boundary using sparse metadata, not a 4 GiB read.
    assert!(regular(&file.metadata().unwrap()).is_ok());
    file.set_len(MAX_BYTES + 1).unwrap();
    assert!(matches!(
        plan_import_archive(&engine, &source),
        Err(AppError::Invalid(_))
    ));
    assert!(scratch_files(&engine).is_empty());
    assert_eq!(plan_count(&engine), 0);
    assert!(!engine.paths.staging().join("imports").exists());
}

#[test]
fn rejects_internal_metadata_even_renamed_as_archive_and_never_adds_roots() {
    let (_temp, engine, source) = setup();
    for directory in [
        "secrets",
        "logs",
        "backups",
        "staging",
        "downloads",
        "plugins",
        "skills",
    ] {
        let path = engine.paths.root.join(directory).join("metadata.zip");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"private metadata").unwrap();
        assert!(matches!(
            plan_import_archive(&engine, &path),
            Err(AppError::Denied(_))
        ));
        assert_eq!(fs::read(path).unwrap(), b"private metadata");
    }
    assert!(plan_import_archive(&engine, &engine.paths.db_file()).is_err());
    let disguised_db = source.with_file_name("database.zip");
    fs::hard_link(engine.paths.db_file(), &disguised_db).unwrap();
    assert!(plan_import_archive(&engine, &disguised_db).is_err());
    assert!(engine.roots().unwrap().is_empty());
    assert_eq!(plan_count(&engine), 0);
    assert!(scratch_files(&engine).is_empty());
}

#[cfg(unix)]
#[test]
fn rejects_directory_fifo_socket_symlinks_and_hard_links_without_reading() {
    use std::os::unix::{fs::symlink, net::UnixListener};
    let (_temp, engine, source) = setup();
    let root = source.parent().unwrap();
    let directory = root.join("directory.zip");
    fs::create_dir(&directory).unwrap();
    let fifo = root.join("fifo.zip");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let socket = root.join("socket.zip");
    let _listener = UnixListener::bind(&socket).unwrap();
    let link = root.join("link.zip");
    symlink(&source, &link).unwrap();
    let dangling = root.join("dangling.zip");
    symlink(root.join("missing"), &dangling).unwrap();
    let parent_link = root.join("linked-parent");
    symlink(root, &parent_link).unwrap();
    for path in [
        &directory,
        &fifo,
        &socket,
        &link,
        &dangling,
        &parent_link.join("Example.jar"),
    ] {
        assert!(
            plan_import_archive(&engine, path).is_err(),
            "{}",
            path.display()
        );
    }
    let hardlink = root.join("hardlink.zip");
    fs::hard_link(&source, &hardlink).unwrap();
    assert!(plan_import_archive(&engine, &hardlink).is_err());
    assert!(plan_import_archive(&engine, &source).is_err());
    assert_eq!(plan_count(&engine), 0);
    assert!(scratch_files(&engine).is_empty());
    assert_eq!(fs::read(source).unwrap(), b"inert archive bytes");
}

#[cfg(unix)]
#[test]
fn safe_open_does_not_follow_links_or_block_on_a_substituted_fifo() {
    use std::os::unix::fs::symlink;
    let (_temp, _engine, source) = setup();
    let link = source.with_file_name("link.zip");
    symlink(&source, &link).unwrap();
    assert!(open_source(&link).is_err());
    let fifo = source.with_file_name("fifo.zip");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    assert!(open_source(&fifo).is_err());
}

#[cfg(unix)]
#[test]
fn source_growth_mid_stream_is_bounded_and_retains_only_written_bytes() {
    let (_temp, engine, source) = setup();
    fs::write(&source, vec![b'a'; CHUNK_BYTES * 2]).unwrap();
    let mut changed = false;
    let mut hooks = Hooks {
        callback: Some(Box::new(move |point, source, scratch, artifact| {
            if point == TestPoint::Chunk && !changed {
                changed = true;
                assert_eq!(fs::metadata(scratch)?.len(), CHUNK_BYTES as u64);
                OpenOptions::new()
                    .append(true)
                    .open(source)?
                    .write_all(&[b'b'; CHUNK_BYTES])?;
                assert!(!artifact.exists());
            }
            Ok(())
        })),
    };
    assert!(plan_import(&engine, &source, &mut hooks).is_err());
    let retained = scratch_files(&engine);
    assert_eq!(retained.len(), 1);
    assert_eq!(
        fs::metadata(&retained[0]).unwrap().len(),
        (CHUNK_BYTES * 2) as u64
    );
    assert_eq!(
        fs::metadata(source).unwrap().len(),
        (CHUNK_BYTES * 3) as u64
    );
    assert_eq!(plan_count(&engine), 0);
}

#[cfg(unix)]
#[test]
fn source_truncation_and_same_size_rewrite_fail_without_deleting_scratch() {
    for truncate in [true, false] {
        let (_temp, engine, source) = setup();
        fs::write(&source, vec![b'a'; CHUNK_BYTES * 3]).unwrap();
        let mut changed = false;
        let mut hooks = Hooks {
            callback: Some(Box::new(move |point, source, _, _| {
                if point == TestPoint::Chunk && !changed {
                    changed = true;
                    if truncate {
                        OpenOptions::new().write(true).open(source)?.set_len(0)?;
                    } else {
                        OpenOptions::new()
                            .write(true)
                            .open(source)?
                            .write_all(b"changed")?;
                    }
                }
                Ok(())
            })),
        };
        assert!(plan_import(&engine, &source, &mut hooks).is_err());
        let retained = scratch_files(&engine);
        assert_eq!(retained.len(), 1);
        assert!(fs::metadata(&retained[0]).unwrap().len() >= CHUNK_BYTES as u64);
        assert_eq!(plan_count(&engine), 0);
    }
}

#[cfg(unix)]
#[test]
fn source_replacement_or_late_symlink_is_rejected_and_original_is_retained() {
    use std::os::unix::fs::symlink;
    for link in [false, true] {
        let (_temp, engine, source) = setup();
        let parked = source.with_file_name("parked.jar");
        let mut changed = false;
        let mut hooks = Hooks {
            callback: Some(Box::new(move |point, source, _, _| {
                if point == TestPoint::Chunk && !changed {
                    changed = true;
                    fs::rename(source, &parked)?;
                    if link {
                        symlink(&parked, source)?;
                    } else {
                        fs::write(source, b"inert archive bytes")?;
                    }
                }
                Ok(())
            })),
        };
        assert!(plan_import(&engine, &source, &mut hooks).is_err());
        assert_eq!(scratch_files(&engine).len(), 1);
        assert_eq!(
            fs::read(source.with_file_name("parked.jar")).unwrap(),
            b"inert archive bytes"
        );
        assert_eq!(plan_count(&engine), 0);
    }
}

#[cfg(unix)]
#[test]
fn source_changes_during_second_fingerprint_pass_are_rejected() {
    let (_temp, engine, source) = setup();
    let mut hooks = Hooks {
        callback: Some(Box::new(|point, source, _, _| {
            if point == TestPoint::SnapshotReady {
                OpenOptions::new()
                    .write(true)
                    .open(source)?
                    .write_all(b"mutated")?;
            }
            Ok(())
        })),
    };
    assert!(plan_import(&engine, &source, &mut hooks).is_err());
    let retained = scratch_files(&engine);
    assert_eq!(retained.len(), 1);
    assert_eq!(fs::read(&retained[0]).unwrap(), b"inert archive bytes");
    assert_eq!(plan_count(&engine), 0);
}

#[test]
fn read_errors_and_planning_errors_preserve_scratch_and_source() {
    for read_failure in [true, false] {
        let (_temp, engine, source) = setup();
        let mut hooks = Hooks {
            callback: Some(Box::new(move |point, _, _, _| {
                if read_failure && point == TestPoint::Chunk {
                    return Err(std::io::Error::other("injected read failure").into());
                }
                Ok(())
            })),
        };
        if !read_failure {
            engine.db.conn().execute("INSERT INTO file_transactions(id,kind,status,summary,plan_json,created_at) VALUES('interrupted','files','recovery_failed','test','{}','now')", []).unwrap();
        }
        assert!(plan_import(&engine, &source, &mut hooks).is_err());
        let retained = scratch_files(&engine);
        assert_eq!(retained.len(), 1);
        assert_eq!(fs::read(&retained[0]).unwrap(), b"inert archive bytes");
        assert_eq!(fs::read(&source).unwrap(), b"inert archive bytes");
        assert!(!engine.paths.staging().join("imports").exists());
    }
}

#[test]
fn cancellation_retains_the_snapshot_without_a_plan() {
    let (_temp, engine, source) = setup();
    let mut hooks = Hooks {
        callback: Some(Box::new(|point, _, _, _| {
            if point == TestPoint::Chunk {
                return Err(AppError::Cancelled);
            }
            Ok(())
        })),
    };
    assert!(matches!(
        plan_import(&engine, &source, &mut hooks),
        Err(AppError::Cancelled)
    ));
    let retained = scratch_files(&engine);
    assert_eq!(retained.len(), 1);
    assert_eq!(fs::read(&retained[0]).unwrap(), b"inert archive bytes");
    assert_eq!(plan_count(&engine), 0);
    assert_eq!(fs::read(source).unwrap(), b"inert archive bytes");
}

#[test]
fn preexisting_destination_namespace_cannot_be_taken_over() {
    for file_exists in [false, true] {
        let (_temp, engine, source) = setup();
        let mut hooks = Hooks {
            callback: Some(Box::new(move |point, _, _, artifact| {
                if point == TestPoint::BeforePlan {
                    fs::create_dir_all(artifact.parent().unwrap())?;
                    if file_exists {
                        fs::write(artifact, b"owned by someone else")?;
                    }
                }
                Ok(())
            })),
        };
        assert!(matches!(
            plan_import(&engine, &source, &mut hooks),
            Err(AppError::Conflict(_))
        ));
        assert_eq!(scratch_files(&engine).len(), 1);
        assert_eq!(plan_count(&engine), 0);
        if file_exists {
            let artifact = walkdir::WalkDir::new(engine.paths.staging().join("imports"))
                .into_iter()
                .filter_map(Result::ok)
                .find(|e| e.file_type().is_file())
                .unwrap();
            assert_eq!(fs::read(artifact.path()).unwrap(), b"owned by someone else");
        }
    }
}

#[test]
fn raced_overwrite_or_scratch_tampering_invalidates_persisted_plan() {
    for change_source in [true, false] {
        let (_temp, engine, source) = setup();
        let mut hooks = Hooks {
            callback: Some(Box::new(move |point, _, scratch, artifact| {
                if point == TestPoint::BeforeEnginePlan {
                    if change_source {
                        fs::write(scratch, b"tampered")?;
                    } else {
                        fs::create_dir_all(artifact.parent().unwrap())?;
                        fs::write(artifact, b"owned by someone else")?;
                    }
                }
                Ok(())
            })),
        };
        assert!(matches!(
            plan_import(&engine, &source, &mut hooks),
            Err(AppError::Conflict(_))
        ));
        let (id, status): (String, String) = engine
            .db
            .conn()
            .query_row("SELECT id,status FROM file_transactions", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(status, "failed");
        assert!(engine.approve_plan(&id).is_err());
        assert!(engine.apply_file_ops(&id).is_err());
        assert_eq!(scratch_files(&engine).len(), 1);
        assert_eq!(fs::read(source).unwrap(), b"inert archive bytes");
    }
}

#[cfg(windows)]
#[test]
fn source_writers_are_denied_while_snapshot_is_read() {
    let (_temp, engine, source) = setup();
    let mut hooks = Hooks {
        callback: Some(Box::new(|point, source, _, _| {
            if point == TestPoint::Chunk {
                assert!(OpenOptions::new().write(true).open(source).is_err());
            }
            Ok(())
        })),
    };
    let preview = plan_import(&engine, &source, &mut hooks).unwrap();
    assert_eq!(
        fs::read(copy_source(&preview)).unwrap(),
        b"inert archive bytes"
    );
}

#[test]
fn destination_or_scratch_changes_after_preview_block_apply_without_overwrite() {
    for change_source in [true, false] {
        let (_temp, engine, source) = setup();
        let preview = plan_import_archive(&engine, &source).unwrap();
        if change_source {
            fs::write(copy_source(&preview), b"tampered").unwrap();
        } else {
            fs::create_dir_all(preview.artifact_path.parent().unwrap()).unwrap();
            fs::write(&preview.artifact_path, b"owned by someone else").unwrap();
        }
        engine.approve_plan(&preview.plan.id).unwrap();
        assert!(engine.apply_file_ops(&preview.plan.id).is_err());
        if change_source {
            assert!(!preview.artifact_path.exists());
        } else {
            assert_eq!(
                fs::read(&preview.artifact_path).unwrap(),
                b"owned by someone else"
            );
        }
        assert!(copy_source(&preview).exists());
        assert_eq!(fs::read(source).unwrap(), b"inert archive bytes");
    }
}

#[cfg(unix)]
#[test]
fn redirected_staging_or_import_subdirectory_is_rejected_without_external_writes() {
    use std::os::unix::fs::symlink;
    for directory in ["import-scratch", "imports"] {
        let (_temp, engine, source) = setup();
        let external = source.parent().unwrap().join("outside");
        fs::create_dir(&external).unwrap();
        symlink(&external, engine.paths.staging().join(directory)).unwrap();
        assert!(plan_import_archive(&engine, &source).is_err());
        assert_eq!(fs::read_dir(external).unwrap().count(), 0);
        assert_eq!(plan_count(&engine), 0);
    }
}

#[test]
fn parallel_and_reentrant_previews_use_independent_exclusive_namespaces() {
    let (_temp, engine, source) = setup();
    let previews = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..3)
            .map(|_| scope.spawn(|| plan_import_archive(&engine, &source).unwrap()))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    let final_preview = plan_import_archive(&engine, &source).unwrap();
    for (index, preview) in previews.iter().enumerate() {
        assert_ne!(preview.artifact_path, final_preview.artifact_path);
        for other in &previews[..index] {
            assert_ne!(copy_source(preview), copy_source(other));
        }
        assert!(copy_source(preview).exists());
        assert!(!preview.artifact_path.exists());
    }
    assert_eq!(scratch_files(&engine).len(), 4);
    assert_eq!(plan_count(&engine), 4);
}

#[tokio::test]
async fn archive_selection_is_not_an_agent_tool_and_import_plans_need_review() {
    use crate::agent::{
        policy::{Effect, PermissionMode},
        tools::{ToolContext, ToolRegistry},
    };
    let (_temp, engine, source) = setup();
    let registry = ToolRegistry::builtin().unwrap();
    assert!(registry.get("plan_import_archive").is_err());
    let preview = plan_import_archive(&engine, &source).unwrap();
    let context = ToolContext {
        engine,
        cancel: tokio_util::sync::CancellationToken::new(),
        progress: None,
        resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
    };
    let prepared = registry
        .get("apply_file_ops")
        .unwrap()
        .prepare(&context, &serde_json::json!({"plan_id": preview.plan.id}))
        .await
        .unwrap();
    assert_eq!(prepared.effect, Effect::ManagerWrite);
    assert!(PermissionMode::AutoApprove.needs_approval(prepared.effect));
    assert!(!preview.artifact_path.exists());
}
