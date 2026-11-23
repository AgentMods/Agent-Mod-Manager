use super::*;
use crate::install::{self, tests as legacy};
use std::fs;

fn import(
    engine: &FileEngine,
    game: &games::Game,
    filename: &str,
    source: &Path,
) -> mod_library::LibraryEntry {
    let id = uuid::Uuid::new_v4().to_string();
    let directory = engine.paths.staging().join("imports").join(&id);
    fs::create_dir_all(&directory).unwrap();
    let archive = directory.join(filename);
    fs::copy(source, &archive).unwrap();
    let archived = mod_library::ingest_import(
        engine,
        &mod_library::ApprovedArtifact {
            artifact_id: &id,
            path: &archive,
            filename,
            size: fs::metadata(&archive).unwrap().len(),
            sha256: &util::sha256_file(&archive).unwrap(),
        },
        &game.name,
        Some(&game.id),
    )
    .unwrap();
    fs::remove_file(archive).unwrap();
    archived
}

fn no_adapter(engine: &FileEngine, parent: &Path) -> games::Game {
    let root = parent.join("Other Game");
    fs::create_dir(&root).unwrap();
    let game = games::register_game(&engine.db, &root, None).unwrap();
    assert!(game.adapter_slug.is_none());
    game
}

fn match_request(game: &games::Game, entry: &mod_library::LibraryEntry) -> LibraryMatchRequest {
    LibraryMatchRequest {
        game_id: game.id.clone(),
        library_entry_id: entry.id.clone(),
    }
}

fn auto(
    game: &games::Game,
    entry: &mod_library::LibraryEntry,
    ty: &str,
    fomod: bool,
) -> LibraryInstallRequest {
    LibraryInstallRequest {
        game_id: game.id.clone(),
        library_entry_id: entry.id.clone(),
        mode: LibraryInstallMode::Adapter {
            mod_type: ty.into(),
            folder_name: None,
            fomod_selection: fomod.then_some(FomodSelection::Automatic),
        },
    }
}

fn manual(
    game: &games::Game,
    entry: &mod_library::LibraryEntry,
    source_kind: LibrarySourceKind,
    source: &str,
    destination: &str,
) -> LibraryInstallRequest {
    LibraryInstallRequest {
        game_id: game.id.clone(),
        library_entry_id: entry.id.clone(),
        mode: LibraryInstallMode::ApprovedMappings {
            mod_type: None,
            source_kind,
            mappings: vec![ManualMapping {
                source: source.into(),
                destination: destination.into(),
            }],
        },
    }
}

fn paths_for(engine: &FileEngine, entry: &mod_library::LibraryEntry) -> (PathBuf, PathBuf) {
    (
        paths::safe_join(&engine.paths.root, &entry.archive_relative_path).unwrap(),
        paths::safe_join(&engine.paths.root, &entry.payload_relative_path).unwrap(),
    )
}

#[test]
fn typed_requests_use_artifact_id_and_reject_hidden_legacy_path_fields() {
    let json = serde_json::json!({"gameId":"game-id","artifactId":"artifact-id", "mode":"adapter", "modType":"fomod", "fomodSelection":"automatic"});
    let request: LibraryInstallRequest = serde_json::from_value(json.clone()).unwrap();
    assert!(matches!(
        request.mode,
        LibraryInstallMode::Adapter {
            fomod_selection: Some(FomodSelection::Automatic),
            ..
        }
    ));
    let serialized = serde_json::to_value(request).unwrap();
    assert_eq!(serialized["artifactId"], "artifact-id");
    assert!(serialized.get("libraryEntryId").is_none());
    let mut polluted = json;
    polluted["artifactPath"] = serde_json::json!("not an allowed source");
    assert!(serde_json::from_value::<LibraryInstallRequest>(polluted).is_err());
    let alias: LibraryMatchRequest = serde_json::from_value(
        serde_json::json!({"gameId":"game-id", "libraryEntryId":"artifact-id"}),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(alias).unwrap()["artifactId"],
        "artifact-id"
    );
}

#[test]
fn matching_is_read_only_bounded_and_cross_game_denied() {
    let (temp, engine, game) = legacy::fixture("stardew-valley");
    let another = no_adapter(&engine, temp.path());
    let readme = "readme instructions\n".repeat(2500);
    let mut entries = vec![
        ("Example/manifest.json", b"fixture".as_slice()),
        ("README.md", readme.as_bytes()),
    ];
    let names = (0..220)
        .map(|index| format!("Example/data-{index:03}.txt"))
        .collect::<Vec<_>>();
    entries.extend(
        names
            .iter()
            .map(|name| (name.as_str(), b"fixture".as_slice())),
    );
    let source = legacy::zip(&engine, &entries);
    let entry = import(&engine, &game, "example.zip", &source);
    let (_, payload) = paths_for(&engine, &entry);
    let result = match_library_install(&engine, &match_request(&game, &entry)).unwrap();
    assert!(result.active_install_id.is_none());
    assert_eq!(result.archive_format.as_deref(), Some("zip"));
    assert_eq!(result.archive_file_count, 222);
    assert_eq!(result.archive_entries.len(), 200);
    assert!(result.archive_entries_truncated);
    assert!(result
        .candidates
        .iter()
        .any(|item| item.mod_type == "smapi-mod" && item.status == "matched"));
    let readme = result.readme.as_ref().unwrap();
    assert!(readme.truncated);
    assert_eq!(readme.text.len(), README_LIMIT);
    assert_eq!(
        serde_json::to_value(&result).unwrap()["artifactId"],
        entry.id
    );
    assert_eq!(fs::read_dir(payload).unwrap().count(), 0);
    let plans: i64 = engine
        .db
        .conn()
        .query_row("SELECT count(*) FROM installs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(plans, 0);
    assert!(matches!(
        match_library_install(&engine, &match_request(&another, &entry)),
        Err(AppError::Denied(_))
    ));
}

#[test]
fn verified_single_file_uses_library_archive_and_manifests_origin_without_download() {
    let (_temp, engine, game) = legacy::fixture("stardew-valley");
    let release = legacy::release(
        &engine,
        &game,
        "project-one",
        "published-v1",
        &[("file.pak", b"inert")],
    );
    let catalog = crate::artifacts::for_path(&engine.db, &release.artifact_path)
        .unwrap()
        .unwrap();
    let entry =
        mod_library::ingest_download(&engine, &catalog, &game.name, Some(&game.id)).unwrap();
    let (archive, payload) = paths_for(&engine, &entry);
    if release.artifact_path.exists() {
        fs::remove_file(&release.artifact_path).unwrap();
    }
    let request = manual(
        &game,
        &entry,
        LibrarySourceKind::SingleFile,
        &entry.filename,
        "Mods/intact.zip",
    );
    let preview = plan_library_install(&engine, &request).unwrap();
    assert!(preview.requires_review && preview.plan.requires_review);
    assert_eq!(
        preview.plan.entries[0].operation.src(),
        Some(archive.as_path())
    );
    assert_eq!(fs::read_dir(payload).unwrap().count(), 0);
    let manifest = install::get_install(&engine, &preview.install_id).unwrap();
    assert_eq!(manifest.source, "modrinth");
    assert_eq!(manifest.version.as_deref(), Some("published-v1"));
    assert_eq!(manifest.download_origin.unwrap().project_id, "project-one");
    assert_eq!(manifest.metadata["libraryEntryId"], entry.id);
    assert_eq!(manifest.install_root, Path::new(&game.root_path));
    let dst = Path::new(&game.root_path).join("Mods/intact.zip");
    assert!(!dst.exists());
    legacy::apply(&engine, &preview.plan);
    assert_eq!(fs::read(&dst).unwrap(), fs::read(&archive).unwrap());
    engine.rollback_transaction(&preview.plan.id).unwrap();
    assert!(!dst.exists());
    assert!(archive.exists());
}

#[test]
fn materialized_payload_is_reused_and_tampering_after_plan_cannot_apply() {
    let (_temp, engine, game) = legacy::fixture("stardew-valley");
    let source = legacy::zip(
        &engine,
        &[
            ("Example/manifest.json", b"one"),
            ("Example/asset.txt", b"two"),
        ],
    );
    let entry = import(&engine, &game, "example.zip", &source);
    let (_, payload) = paths_for(&engine, &entry);
    let request = auto(&game, &entry, "smapi-mod", false);
    let first = plan_library_install(&engine, &request).unwrap();
    let physical = first.plan.entries[0].operation.src().unwrap();
    assert!(physical.starts_with(payload.join(&entry.sha256)));
    let expected_identity = safe_fs::identity(physical).unwrap().unwrap();
    let second = plan_library_install(&engine, &request).unwrap();
    assert_ne!(first.plan.id, second.plan.id);
    assert_eq!(second.plan.entries[0].operation.src().unwrap(), physical);
    assert_eq!(
        safe_fs::identity(physical).unwrap().unwrap(),
        expected_identity
    );
    assert_eq!(fs::read_dir(&payload).unwrap().count(), 1);
    fs::write(physical, b"bad").unwrap();
    engine.approve_plan(&first.plan.id).unwrap();
    assert!(matches!(
        engine.apply_file_ops(&first.plan.id),
        Err(AppError::Conflict(_))
    ));
    assert!(!Path::new(&game.root_path).join("Mods/Example").exists());
    assert!(matches!(
        plan_library_install(&engine, &request),
        Err(AppError::Conflict(_))
    ));
    assert_eq!(fs::read(physical).unwrap(), b"bad");
}

#[test]
fn archive_and_payload_hardlinks_are_denied_even_with_identical_bytes() {
    let (temp, engine, game) = legacy::fixture("stardew-valley");
    let source = legacy::zip(&engine, &[("Example/manifest.json", b"verified")]);
    let entry = import(&engine, &game, "example.zip", &source);
    let (archive, _) = paths_for(&engine, &entry);
    let alias = temp.path().join("duplicate-archive.zip");
    fs::hard_link(&archive, &alias).unwrap();
    assert!(match_library_install(&engine, &match_request(&game, &entry)).is_err());
    fs::remove_file(alias).unwrap();
    let request = auto(&game, &entry, "smapi-mod", false);
    let plan = plan_library_install(&engine, &request).unwrap();
    let src = plan.plan.entries[0].operation.src().unwrap();
    fs::hard_link(src, temp.path().join("duplicate-payload.txt")).unwrap();
    assert!(plan_library_install(&engine, &request).is_err());
    engine.approve_plan(&plan.plan.id).unwrap();
    assert!(engine.apply_file_ops(&plan.plan.id).is_err());
}

#[test]
fn approved_mappings_are_explicit_scoped_and_cannot_write_credentials() {
    let (temp, engine, _) = legacy::fixture("stardew-valley");
    let game = no_adapter(&engine, temp.path());
    let source = legacy::zip(
        &engine,
        &[
            ("assets/mod.txt", b"good"),
            ("assets/readme.txt", b"untrusted"),
        ],
    );
    let entry = import(&engine, &game, "example.zip", &source);
    let (_, payload) = paths_for(&engine, &entry);
    for (source, destination) in [
        ("../asset", "mods/file"),
        ("missing", "mods/file"),
        ("assets/mod.txt", "../other"),
        ("assets/mod.txt", ".env.local"),
        ("assets/mod.txt", ".amm-retained/file"),
    ] {
        assert!(plan_library_install(
            &engine,
            &manual(
                &game,
                &entry,
                LibrarySourceKind::Archive,
                source,
                destination
            )
        )
        .is_err());
    }
    assert_eq!(fs::read_dir(&payload).unwrap().count(), 0);
    assert!(plan_library_install(&engine, &auto(&game, &entry, "smapi-mod", false)).is_err());
    let preview = plan_library_install(
        &engine,
        &manual(
            &game,
            &entry,
            LibrarySourceKind::Archive,
            "assets/mod.txt",
            "author-selected/mod.txt",
        ),
    )
    .unwrap();
    assert_eq!(preview.mod_type, "manual");
    assert_eq!(preview.ignored_files, ["assets/readme.txt"]);
    legacy::apply(&engine, &preview.plan);
    assert_eq!(
        fs::read(Path::new(&game.root_path).join("author-selected/mod.txt")).unwrap(),
        b"good"
    );
}

#[test]
fn deterministic_fomod_and_manual_user_choice_use_their_explicit_roots() {
    let (temp, engine, skyrim) = legacy::fixture("skyrim-se");
    let no_adapter = no_adapter(&engine, temp.path());
    let config = |kind: &str| {
        format!(
            r#"<config><installSteps><installStep><optionalFileGroups><group name="Detail" type="{kind}"><plugins><plugin name="A"><files><file source="files/a.esp" destination="option.esp"/></files></plugin><plugin name="B"><files><file source="files/b.esp" destination="other.esp"/></files></plugin></plugins></group></optionalFileGroups></installStep></installSteps></config>"#
        )
    };
    let make = |mode| {
        legacy::zip(
            &engine,
            &[
                ("Release/fomod/ModuleConfig.xml", config(mode).as_bytes()),
                ("Release/files/a.esp", b"a"),
                ("Release/files/b.esp", b"b"),
            ],
        )
    };
    let auto_entry = import(&engine, &skyrim, "automatic.zip", &make("SelectAll"));
    let match_auto = match_library_install(&engine, &match_request(&skyrim, &auto_entry)).unwrap();
    assert_eq!(match_auto.fomod_status, Some("automatic"));
    let optional = match_auto.fomod.unwrap().automatic_selection;
    assert_eq!(optional.selected_options.len(), 2);
    let auto_plan =
        plan_library_install(&engine, &auto(&skyrim, &auto_entry, "fomod", true)).unwrap();
    assert_eq!(
        auto_plan.install_root,
        Path::new(&skyrim.root_path).join("Data")
    );
    assert_eq!(auto_plan.plan.entries.len(), 2);
    legacy::apply(&engine, &auto_plan.plan);
    assert_eq!(
        fs::read(Path::new(&skyrim.root_path).join("Data/option.esp")).unwrap(),
        b"a"
    );

    let choice = import(
        &engine,
        &no_adapter,
        "choice.zip",
        &make("SelectExactlyOne"),
    );
    let matched = match_library_install(&engine, &match_request(&no_adapter, &choice)).unwrap();
    assert_eq!(matched.fomod_status, Some("requiresUser"));
    let config = matched.fomod.unwrap();
    assert_eq!(config.groups[0].options.len(), 2);
    let choice_plan = plan_library_install(
        &engine,
        &manual(
            &no_adapter,
            &choice,
            LibrarySourceKind::Archive,
            &config.groups[0].options[1].mappings[0].source,
            "Data/user.esp",
        ),
    )
    .unwrap();
    assert_eq!(choice_plan.plan.entries.len(), 1);
    legacy::apply(&engine, &choice_plan.plan);
    assert_eq!(
        fs::read(Path::new(&no_adapter.root_path).join("Data/user.esp")).unwrap(),
        b"b"
    );
}

#[test]
fn generic_and_legacy_paths_cannot_get_library_source_scope() {
    let (_temp, engine, game) = legacy::fixture("stardew-valley");
    let source = legacy::zip(&engine, &[("Example/manifest.json", b"verified")]);
    let entry = import(&engine, &game, "example.zip", &source);
    let (archive, _) = paths_for(&engine, &entry);
    let engine = engine.for_local_agent();
    let dst = Path::new(&game.root_path).join("Mods/example.zip");
    assert!(engine
        .plan_file_ops(
            vec![FileOp::Copy {
                src: archive.clone(),
                dst
            }],
            "generic".into()
        )
        .is_err());
    assert!(engine
        .plan_file_ops(
            vec![FileOp::WriteText {
                dst: archive.clone(),
                content: "changed".into()
            }],
            "generic".into()
        )
        .is_err());
    let legacy = legacy::request(&game, archive, "smapi-mod", &entry.filename);
    assert!(install::plan_install(&engine, &legacy).is_err());
}

#[test]
fn library_trainer_install_uses_the_archive_member_bytes() {
    let (temp, engine, _) = legacy::fixture("stardew-valley");
    let game = no_adapter(&engine, temp.path());
    let member = "scripts/GlossTrainer.asi";
    let bytes = b"inert native plugin fixture";
    let zip = legacy::zip(&engine, &[(member, bytes), ("README.txt", b"other file")]);
    let entry = import(&engine, &game, "GlossTrainer.zip", &zip);
    let (archive, _) = paths_for(&engine, &entry);
    let relative = "platform/steam/scripts/GlossTrainer.asi";
    let target = Path::new(&game.root_path).join(relative);
    let error = engine
        .for_local_agent()
        .plan_file_ops(
            vec![FileOp::Copy {
                src: archive.clone(),
                dst: target.clone(),
            }],
            "restore plugin".into(),
        )
        .unwrap_err();
    assert!(
        matches!(error, AppError::Denied(message) if message.contains("install_library_mod") && message.contains("sourceKind=archive"))
    );
    let wrong = manual(
        &game,
        &entry,
        LibrarySourceKind::SingleFile,
        &entry.filename,
        relative,
    );
    assert!(
        matches!(plan_library_install(&engine, &wrong), Err(AppError::Invalid(message)) if message.contains("sourceKind=archive"))
    );
    assert!(!target.exists());
    assert!(install::list_installs(&engine, &game.id)
        .unwrap()
        .is_empty());

    let matched = match_library_install(&engine, &match_request(&game, &entry)).unwrap();
    assert!(matched.archive_entries.iter().any(|path| path == member));
    let correct = manual(&game, &entry, LibrarySourceKind::Archive, member, relative);
    let preview = plan_library_install(&engine, &correct).unwrap();
    assert_eq!(preview.plan.entries.len(), 1);
    legacy::apply(&engine, &preview.plan);
    assert_eq!(fs::read(&target).unwrap(), bytes);
    assert_eq!(util::sha256_file(&archive).unwrap(), entry.sha256);
    engine.rollback_transaction(&preview.plan.id).unwrap();
    assert!(!target.exists());
}

#[test]
fn pending_whole_archive_plan_cannot_deploy_as_native_plugin_after_upgrade() {
    let (_temp, engine, game) = legacy::fixture("stardew-valley");
    let source = legacy::zip(&engine, &[("module.dll", b"inert plugin")]);
    let entry = import(&engine, &game, "module.zip", &source);
    let request = manual(
        &game,
        &entry,
        LibrarySourceKind::SingleFile,
        &entry.filename,
        "Mods/intact.zip",
    );
    let mut preview = plan_library_install(&engine, &request).unwrap();
    // Recreate the persisted singleFile mapping accepted by older versions.
    let target = Path::new(&game.root_path).join("Mods/module.DLL");
    let FileOp::Copy { dst, .. } = &mut preview.plan.entries[0].operation else {
        unreachable!()
    };
    *dst = target.clone();
    let mut files = install::get_install(&engine, &preview.install_id)
        .unwrap()
        .files;
    files[0].path = target.clone();
    {
        let conn = engine.db.conn();
        conn.execute(
            "UPDATE file_transactions SET plan_json=?1 WHERE id=?2",
            params![
                serde_json::to_string(&preview.plan).unwrap(),
                preview.plan.id
            ],
        )
        .unwrap();
        conn.execute(
            "UPDATE installs SET files_json=?1 WHERE id=?2",
            params![serde_json::to_string(&files).unwrap(), preview.install_id],
        )
        .unwrap();
    }
    engine.approve_plan(&preview.plan.id).unwrap();
    assert!(
        matches!(engine.apply_file_ops(&preview.plan.id), Err(AppError::Invalid(message)) if message.contains("sourceKind=archive"))
    );
    assert!(!target.exists());
}

#[test]
fn archive_apply_rollback_reinstall_reuses_verified_generation() {
    let (_temp, engine, game) = legacy::fixture("stardew-valley");
    let source = legacy::zip(
        &engine,
        &[
            ("Example/manifest.json", b"verified"),
            ("Example/data.txt", b"payload"),
        ],
    );
    let entry = import(&engine, &game, "example.zip", &source);
    let (_, payload) = paths_for(&engine, &entry);
    let request = auto(&game, &entry, "smapi-mod", false);
    let first = plan_library_install(&engine, &request).unwrap();
    let first_src = first.plan.entries[0].operation.src().unwrap();
    let first_identity = safe_fs::identity(first_src).unwrap().unwrap();
    let dst = Path::new(&game.root_path).join("Mods/Example/data.txt");
    legacy::apply(&engine, &first.plan);
    assert_eq!(fs::read(&dst).unwrap(), b"payload");
    assert_eq!(
        install::get_install(&engine, &first.install_id)
            .unwrap()
            .status,
        "enabled"
    );
    engine.rollback_transaction(&first.plan.id).unwrap();
    assert!(!dst.exists());
    assert_eq!(
        install::get_install(&engine, &first.install_id)
            .unwrap()
            .status,
        "rolled_back"
    );
    let second = plan_library_install(&engine, &request).unwrap();
    assert_eq!(
        safe_fs::identity(second.plan.entries[0].operation.src().unwrap())
            .unwrap()
            .unwrap(),
        first_identity
    );
    assert_eq!(fs::read_dir(&payload).unwrap().count(), 1);
    legacy::apply(&engine, &second.plan);
    assert_eq!(fs::read(&dst).unwrap(), b"payload");
    assert_eq!(
        install::get_install(&engine, &second.install_id)
            .unwrap()
            .status,
        "enabled"
    );
}

#[test]
fn cached_archive_rejects_unselected_tampering_and_existing_incomplete_generation() {
    let (_temp, engine, game) = legacy::fixture("stardew-valley");
    let source = legacy::zip(
        &engine,
        &[("source.txt", b"wanted"), ("unused.txt", b"unselected")],
    );
    let entry = import(&engine, &game, "example.zip", &source);
    let (_, payload) = paths_for(&engine, &entry);
    let generation = payload.join(&entry.sha256);
    let request = manual(
        &game,
        &entry,
        LibrarySourceKind::Archive,
        "source.txt",
        "Mod/selected.txt",
    );
    fs::create_dir(&generation).unwrap();
    fs::write(generation.join("source.txt"), b"wanted").unwrap();
    assert!(matches!(
        plan_library_install(&engine, &request),
        Err(AppError::Conflict(_))
    ));
    assert!(!generation.join("unused.txt").exists());
    fs::remove_file(generation.join("source.txt")).unwrap();
    fs::remove_dir(&generation).unwrap();
    let first = plan_library_install(&engine, &request).unwrap();
    fs::write(generation.join("unused.txt"), b"modified!!").unwrap();
    assert!(matches!(
        plan_library_install(&engine, &request),
        Err(AppError::Conflict(_))
    ));
    assert_eq!(first.plan.entries.len(), 1);
}

#[test]
fn cancelled_match_and_archive_plan_are_inert() {
    let (_temp, engine, game) = legacy::fixture("stardew-valley");
    let source = legacy::zip(&engine, &[("Example/manifest.json", b"verified")]);
    let entry = import(&engine, &game, "example.zip", &source);
    let (_, payload) = paths_for(&engine, &entry);
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        match_library_install_cancellable(&engine, &match_request(&game, &entry), &cancel),
        Err(AppError::Cancelled)
    ));
    assert!(matches!(
        plan_library_install_cancellable(
            &engine,
            &auto(&game, &entry, "smapi-mod", false),
            &cancel
        ),
        Err(AppError::Cancelled)
    ));
    assert_eq!(fs::read_dir(payload).unwrap().count(), 0);
}

#[test]
fn only_one_of_two_previews_can_commit_the_same_library_artifact() {
    let (_temp, engine, game) = legacy::fixture("stardew-valley");
    let source = legacy::zip(
        &engine,
        &[
            ("Example/manifest.json", b"verified"),
            ("Example/data.txt", b"data"),
        ],
    );
    let entry = import(&engine, &game, "example.zip", &source);
    let request = auto(&game, &entry, "smapi-mod", false);
    let first = plan_library_install(&engine, &request).unwrap();
    let second = plan_library_install(&engine, &request).unwrap();
    // Approve both as though the user accepted two pending dialogs.
    engine.approve_plan(&first.plan.id).unwrap();
    engine.approve_plan(&second.plan.id).unwrap();
    engine.apply_file_ops(&first.plan.id).unwrap();
    assert!(
        matches!(engine.apply_file_ops(&second.plan.id), Err(AppError::Conflict(message)) if message.contains(&first.install_id))
    );
    assert!(
        matches!(plan_library_install(&engine, &request), Err(AppError::Conflict(message)) if message.contains(&first.install_id))
    );
    assert_eq!(
        install::list_installs(&engine, &game.id)
            .unwrap()
            .iter()
            .filter(|item| item.status == "enabled")
            .count(),
        1
    );

    let lookup = match_request(&game, &entry);
    assert_eq!(
        match_library_install(&engine, &lookup)
            .unwrap()
            .active_install_id
            .as_deref(),
        Some(first.install_id.as_str())
    );
    let uninstall = install::plan_uninstall(&engine, &first.install_id).unwrap();
    legacy::apply(&engine, &uninstall);
    assert!(match_library_install(&engine, &lookup)
        .unwrap()
        .active_install_id
        .is_none());
    let reinstall = plan_library_install(&engine, &request).unwrap();
    legacy::apply(&engine, &reinstall.plan);
    assert_eq!(
        install::get_install(&engine, &reinstall.install_id)
            .unwrap()
            .status,
        "enabled"
    );
}

#[test]
fn competing_library_plugins_cannot_claim_the_same_path_even_if_it_goes_missing() {
    let (temp, engine, _) = legacy::fixture("stardew-valley");
    let game = no_adapter(&engine, temp.path());
    let relative = "platform/steam/scripts/GlossTrainer.asi";
    let plugin = |name, bytes: &[u8]| {
        let zip = legacy::zip(&engine, &[("GlossTrainer.asi", bytes)]);
        let entry = import(&engine, &game, name, &zip);
        manual(
            &game,
            &entry,
            LibrarySourceKind::Archive,
            "GlossTrainer.asi",
            relative,
        )
    };
    let standalone = plugin("standalone.zip", b"standalone");
    let bundled = plugin("bundled.zip", b"bundled");
    let first = plan_library_install(&engine, &standalone).unwrap();
    let second = plan_library_install(&engine, &bundled).unwrap();
    engine.approve_plan(&second.plan.id).unwrap();
    legacy::apply(&engine, &first.plan);
    let target = Path::new(&game.root_path).join(relative);
    let conflict = |error| matches!(error, AppError::Conflict(reason) if reason.contains(&first.install_id) && reason.contains("GlossTrainer.asi"));
    assert!(conflict(
        plan_library_install(&engine, &bundled).unwrap_err()
    ));
    assert_eq!(fs::read(&target).unwrap(), b"standalone");

    // Match the second preview's original absent fingerprint. Ownership alone must block it.
    let moved = temp.path().join("moved.asi");
    fs::rename(&target, &moved).unwrap();
    assert!(conflict(
        engine.apply_file_ops(&second.plan.id).unwrap_err()
    ));
    assert!(conflict(
        plan_library_install(&engine, &bundled).unwrap_err()
    ));
    assert!(!target.exists());
    fs::rename(moved, &target).unwrap();
    let uninstall = install::plan_uninstall(&engine, &first.install_id).unwrap();
    legacy::apply(&engine, &uninstall);
    let selected = plan_library_install(&engine, &bundled).unwrap();
    legacy::apply(&engine, &selected.plan);
    assert_eq!(fs::read(target).unwrap(), b"bundled");
}

#[test]
fn a_partially_restored_library_install_still_blocks_duplicate_ownership() {
    let (_temp, engine, game) = legacy::fixture("stardew-valley");
    let source = legacy::zip(
        &engine,
        &[
            ("Example/manifest.json", b"verified"),
            ("Example/data.txt", b"data"),
        ],
    );
    let entry = import(&engine, &game, "example.zip", &source);
    let request = auto(&game, &entry, "smapi-mod", false);
    let first = plan_library_install(&engine, &request).unwrap();
    legacy::apply(&engine, &first.plan);
    let uninstall = install::plan_uninstall(&engine, &first.install_id).unwrap();
    legacy::apply(&engine, &uninstall);
    let trash = crate::trash::list_trash(&engine.db, false).unwrap();
    assert_eq!(trash.len(), 2);
    let restore =
        crate::trash::plan_restore(&engine, std::slice::from_ref(&trash[0].id), false).unwrap();
    legacy::apply(&engine, &restore);
    assert_eq!(
        install::get_install(&engine, &first.install_id)
            .unwrap()
            .status,
        "partially_restored"
    );
    assert!(
        matches!(plan_library_install(&engine, &request), Err(AppError::Conflict(message)) if message.contains(&first.install_id))
    );
}

#[test]
fn management_uninstall_keeps_archive_and_delete_preserves_disk_and_audit_history() {
    use crate::mod_management::{remove, visible_installs, Target};
    let (_tmp, engine, game) = legacy::fixture("stardew-valley");
    let src = legacy::zip(&engine, &[("One/manifest.json", b"contents")]);
    let item = import(&engine, &game, "One.zip", &src);
    let req = auto(&game, &item, "smapi-mod", false);
    let stale = plan_library_install(&engine, &req).unwrap();
    let live = plan_library_install(&engine, &req).unwrap();
    legacy::apply(&engine, &live.plan);
    let archive = Target::Archive(item.id.clone());
    let record = Target::Record(live.install_id.clone());
    assert!(remove(&engine, &game.id, &archive).is_err());
    assert!(remove(&engine, &game.id, &record).is_err());
    assert!(remove(&engine, &game.id, &Target::Plan(live.install_id.clone())).is_err());
    remove(&engine, &game.id, &Target::Plan(stale.install_id.clone())).unwrap();
    assert!(engine.approve_plan(&stale.plan.id).is_err());
    let visible = visible_installs(&engine, &game.id).unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, live.install_id);
    let uninstall = crate::install::plan_uninstall(&engine, &live.install_id).unwrap();
    legacy::apply(&engine, &uninstall);
    assert_eq!(
        visible_installs(&engine, &game.id).unwrap()[0].status,
        "uninstalled"
    );
    assert_eq!(
        mod_library::list_for_game(&engine.db, &game.id)
            .unwrap()
            .len(),
        1
    );
    remove(&engine, &game.id, &record).unwrap();
    assert!(visible_installs(&engine, &game.id).unwrap().is_empty());
    assert!(mod_library::list_for_game(&engine.db, &game.id)
        .unwrap()
        .is_empty());
    assert!(mod_library::verify_for_install(&engine, &game.id, &item.id).is_err());
    assert!(paths_for(&engine, &item).0.is_file());
    assert_eq!(
        install::get_install(&engine, &live.install_id)
            .unwrap()
            .status,
        "uninstalled"
    );
    engine.rollback_transaction(&uninstall.id).unwrap();
    assert_eq!(
        visible_installs(&engine, &game.id).unwrap()[0].status,
        "enabled"
    );
    assert_eq!(
        mod_library::list_for_game(&engine.db, &game.id)
            .unwrap()
            .len(),
        1
    );
    let second_uninstall = install::plan_uninstall(&engine, &live.install_id).unwrap();
    legacy::apply(&engine, &second_uninstall);
    assert_eq!(
        visible_installs(&engine, &game.id).unwrap()[0].status,
        "uninstalled"
    );
}

#[test]
fn management_record_removal_keeps_other_history_for_the_same_archive() {
    use crate::mod_management::{remove, visible_installs, Target};
    let (_tmp, engine, game) = legacy::fixture("stardew-valley");
    let src = legacy::zip(&engine, &[("One/manifest.json", b"contents")]);
    let item = import(&engine, &game, "One.zip", &src);
    let req = auto(&game, &item, "smapi-mod", false);
    let install_then_uninstall = || {
        let preview = plan_library_install(&engine, &req).unwrap();
        legacy::apply(&engine, &preview.plan);
        let uninstall = install::plan_uninstall(&engine, &preview.install_id).unwrap();
        legacy::apply(&engine, &uninstall);
        (preview.install_id, uninstall.id)
    };
    let (first, _) = install_then_uninstall();
    let (second, second_uninstall) = install_then_uninstall();
    let history = visible_installs(&engine, &game.id).unwrap();
    assert_eq!(history.len(), 2);
    assert!(history.iter().all(|record| record.status == "uninstalled"));

    remove(&engine, &game.id, &Target::Record(first)).unwrap();
    let remaining = visible_installs(&engine, &game.id).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, second);
    let archives = mod_library::list_for_game(&engine.db, &game.id).unwrap();
    assert_eq!(archives.len(), 1);
    assert_eq!(archives[0].id, item.id);

    remove(&engine, &game.id, &Target::Record(second.clone())).unwrap();
    assert!(visible_installs(&engine, &game.id).unwrap().is_empty());
    assert!(mod_library::list_for_game(&engine.db, &game.id)
        .unwrap()
        .is_empty());

    engine.rollback_transaction(&second_uninstall).unwrap();
    let restored = visible_installs(&engine, &game.id).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].id, second);
    assert_eq!(restored[0].status, "enabled");
    assert_eq!(
        mod_library::list_for_game(&engine.db, &game.id).unwrap()[0].id,
        item.id
    );
}

#[test]
fn unreal_pak_manual_mapping_installs_and_uninstalls_under_nested_content_paks() {
    let (temp, engine, _) = legacy::fixture("stardew-valley");
    let game = no_adapter(&engine, temp.path());
    let pak_bytes = b"UnrealPak fixture payload";
    let pak = temp.path().join("Example.pak");
    fs::write(&pak, pak_bytes).unwrap();
    let entry = import(&engine, &game, "Example.pak", &pak);
    let request = manual(
        &game,
        &entry,
        LibrarySourceKind::SingleFile,
        &entry.filename,
        "Content/Paks/~mods/Example.pak",
    );

    let preview = plan_library_install(&engine, &request).unwrap();
    assert_eq!(preview.mod_type, "manual");
    assert_eq!(preview.plan.entries.len(), 1);
    let destination = Path::new(&game.root_path).join("Content/Paks/~mods/Example.pak");
    legacy::apply(&engine, &preview.plan);

    assert_eq!(fs::read(&destination).unwrap(), pak_bytes);
    let record = install::get_install(&engine, &preview.install_id).unwrap();
    assert_eq!(record.status, "enabled");
    assert_eq!(record.install_root, Path::new(&game.root_path));
    assert_eq!(record.files.len(), 1);
    assert_eq!(record.files[0].path, destination);

    let uninstall = install::plan_uninstall(&engine, &preview.install_id).unwrap();
    legacy::apply(&engine, &uninstall);
    assert!(!destination.exists());
    assert_eq!(
        install::get_install(&engine, &preview.install_id)
            .unwrap()
            .status,
        "uninstalled"
    );

    engine.rollback_transaction(&uninstall.id).unwrap();
    assert_eq!(fs::read(&destination).unwrap(), pak_bytes);
    assert_eq!(
        install::get_install(&engine, &preview.install_id)
            .unwrap()
            .status,
        "enabled"
    );
}

#[test]
fn unreal_pak_adapter_strip_prefix_installs_to_declared_content_paks_root() {
    let (_temp, engine, game) = legacy::fixture("skyrim-se");
    let mut adapter = game_roots::adapter(&engine.db, &game).unwrap();
    let data_root = adapter
        .install_roots
        .iter_mut()
        .find(|root| root.id == "data")
        .unwrap();
    data_root.relative_path = "Content/Paks/~mods".into();
    adapter
        .mod_types
        .iter_mut()
        .find(|mod_type| mod_type.id == "data-mod")
        .unwrap()
        .extensions
        .push("pak".into());
    let strip_rule = adapter
        .archive_rules
        .iter_mut()
        .find(|rule| {
            rule.mod_type == "data-mod" && rule.layout == plugins::ArchiveLayout::StripPrefix
        })
        .unwrap();
    strip_rule.source_prefix = Some("~mods".into());
    adapter.validate().unwrap();
    let adapter_json = serde_json::to_string(&adapter).unwrap();
    engine
        .db
        .conn()
        .execute(
            "UPDATE plugins SET adapter_json=?1 WHERE slug=?2",
            rusqlite::params![adapter_json, adapter.slug],
        )
        .unwrap();

    let archive = legacy::zip(&engine, &[("~mods/Example.pak", b"Unreal package bytes")]);
    let entry = import(&engine, &game, "Example.zip", &archive);

    let matched = match_library_install(&engine, &match_request(&game, &entry)).unwrap();
    assert!(matched
        .candidates
        .iter()
        .any(|candidate| candidate.mod_type == "data-mod" && candidate.status == "matched"));
    let request = auto(&game, &entry, "data-mod", false);
    let preview = plan_library_install(&engine, &request).unwrap();
    let destination = Path::new(&game.root_path).join("Content/Paks/~mods/Example.pak");
    assert_eq!(
        preview.install_root,
        Path::new(&game.root_path).join("Content/Paks/~mods")
    );
    assert_eq!(preview.plan.entries.len(), 1);
    assert_eq!(
        preview.plan.entries[0].operation.dst(),
        destination.as_path()
    );

    legacy::apply(&engine, &preview.plan);
    assert_eq!(fs::read(destination).unwrap(), b"Unreal package bytes");
}

#[test]
fn library_organization_survives_reopen_and_reuses_user_tags() {
    use crate::mod_organization::{
        list_mod_tags, save_mod_tag, set_mod_tags, OrganizationSuggestion,
    };
    use tokio_util::sync::CancellationToken;

    let (_temp, engine, game) = legacy::fixture("stardew-valley");
    let source = legacy::zip(&engine, &[("Example/manifest.json", b"verified")]);
    let entry = import(&engine, &game, "example.zip", &source);
    let request = auto(&game, &entry, "smapi-mod", false);
    let first = crate::install::plan_library_install_with_organization(
        &engine,
        &request,
        Some(OrganizationSuggestion {
            display_name: "Organized name".into(),
            tags: vec!["Alpha".into(), "Beta".into()],
        }),
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(list_mod_tags(&engine.db, &game.id).unwrap().is_empty());
    legacy::apply(&engine, &first.plan);
    let db = crate::db::Db::open(&engine.paths.db_file()).unwrap();
    assert_eq!(list_mod_tags(&db, &game.id).unwrap().len(), 2);
    let installed = install::get_install(&engine, &first.install_id).unwrap();
    assert_eq!(installed.name, "Organized name");
    assert_eq!(installed.tag_ids.len(), 2);
    let custom = save_mod_tag(&db, &game.id, None, "Mine", "#123456").unwrap();
    set_mod_tags(
        &db,
        &game.id,
        &first.install_id,
        std::slice::from_ref(&custom.id),
    )
    .unwrap();
    engine.rollback_transaction(&first.plan.id).unwrap();
    let second = plan_library_install(&engine, &request).unwrap();
    legacy::apply(&engine, &second.plan);
    let installed = install::get_install(&engine, &second.install_id).unwrap();
    assert_eq!(installed.tag_ids, [custom.id]);
    assert_ne!(installed.name, "Organized name");
}
