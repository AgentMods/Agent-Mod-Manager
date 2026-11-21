use super::{
    GameChange::{Remove, SelectAdapter},
    *,
};
use crate::{
    db::Db,
    paths::user_path,
    transaction::{FileOp, FilePlan},
};
use serde_json::json;

struct Fixture {
    _temp: tempfile::TempDir,
    engine: FileEngine,
    root: PathBuf,
    outside: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("data")).unwrap();
        let db = Db::open(&paths.db_file()).unwrap();
        let engine = FileEngine::new(db, paths);
        let root = make_dir(&temp.path().join("game"));
        let outside = make_dir(&temp.path().join("outside"));
        insert_game(&engine, "g", &root);
        Self {
            _temp: temp,
            engine,
            root,
            outside,
        }
    }

    fn game(&self) -> Game {
        games::get_game(&self.engine.db, "g").unwrap()
    }
}

fn make_dir(path: &Path) -> PathBuf {
    fs::create_dir_all(path).unwrap();
    path.canonicalize().unwrap()
}

fn insert_game(engine: &FileEngine, id: &str, root: &Path) {
    engine.db.conn().execute(
        "INSERT INTO games (id,name,slug,root_path,platform,metadata_json,created_at,updated_at)
         VALUES (?1,'Original','minecraft-java',?2,'windows','{\"keep\": {\"value\": 1}}','old','old')",
        params![id, root.to_str().unwrap()],
    ).unwrap();
}

fn executable(path: &Path) -> PathBuf {
    fs::write(path, b"inert test fixture").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    path.canonicalize().unwrap()
}

#[test]
fn validates_trimmed_names_and_unicode_character_count() {
    assert_eq!(validate_name("  游戏 名称　").unwrap(), "游戏 名称");
    assert!(validate_name(&"界".repeat(80)).is_ok());
    for name in ["　", "改\n名", "a\0b", "a\u{7f}b", &"界".repeat(81)] {
        assert!(matches!(validate_name(name), Err(AppError::Invalid(_))));
    }
}

#[test]
fn update_preserves_identity_and_other_metadata_and_clears_launch_path() {
    let f = Fixture::new();
    let program = executable(&f.root.join("game.EXE"));
    let original = f.game();
    let changed = update_game(&f.engine, "g", " 新名称 ", program.to_str()).unwrap();
    assert_eq!(changed.name, "新名称");
    assert_eq!(changed.slug, original.slug);
    assert_eq!(changed.root_path, original.root_path);
    assert_ne!(changed.updated_at, original.updated_at);
    assert_eq!(changed.metadata["keep"], original.metadata["keep"]);
    assert_eq!(changed.metadata["launchPath"], program.to_str().unwrap());
    let serialized = serde_json::to_value(&changed).unwrap();
    assert_eq!(
        serialized["metadata"]["launchPath"],
        json!(user_path(&program))
    );
    assert_eq!(serialized["rootPath"], json!(user_path(&f.root)));

    let outside = executable(&f.outside.join("game.exe"));
    assert!(update_game(&f.engine, "g", "拒绝的修改", outside.to_str()).is_err());
    assert_eq!(f.game().name, changed.name);
    assert_eq!(f.game().metadata, changed.metadata);
    for clear in [None, Some(""), Some("   ")] {
        update_game(&f.engine, "g", "最终名称", program.to_str()).unwrap();
        update_game(&f.engine, "g", "最终名称", clear).unwrap();
        assert_eq!(f.game().metadata, original.metadata);
    }
}

#[test]
fn launch_path_requires_existing_regular_file_inside_game() {
    let f = Fixture::new();
    let paths = &f.engine.paths;
    let outside = executable(&f.outside.join("outside.exe"));
    assert!(matches!(
        validate_launch_path(paths, &f.root, &outside),
        Err(AppError::PathEscape(_))
    ));
    assert!(matches!(
        validate_launch_path(paths, &f.root, Path::new("relative.exe")),
        Err(AppError::Invalid(_))
    ));
    assert!(matches!(
        validate_launch_path(paths, &f.root, &f.root.join("absent.exe")),
        Err(AppError::NotFound(_))
    ));
    assert!(matches!(
        validate_launch_path(paths, &f.root, &f.root),
        Err(AppError::Invalid(_))
    ));
    let protected = executable(&f.root.join(".env.exe"));
    assert!(matches!(
        validate_launch_path(paths, &f.root, &protected),
        Err(AppError::Denied(_))
    ));
}

#[cfg(windows)]
#[test]
fn windows_extension_is_checked_case_insensitively() {
    let f = Fixture::new();
    let paths = &f.engine.paths;
    let invalid = executable(&f.root.join("game.cmd"));
    let valid = executable(&f.root.join("game.EXE"));
    assert_eq!(
        validate_launch_path(paths, &f.root, &valid)
            .unwrap()
            .program,
        valid
    );
    assert!(matches!(
        validate_launch_path(paths, &f.root, &invalid),
        Err(AppError::Invalid(_))
    ));
}

#[cfg(unix)]
#[test]
fn unix_requires_executable_permission_and_rejects_file_symlink_escape() {
    let f = Fixture::new();
    let paths = &f.engine.paths;
    let not_executable = f.root.join("data");
    fs::write(&not_executable, b"data").unwrap();
    assert!(matches!(
        validate_launch_path(paths, &f.root, &not_executable),
        Err(AppError::Invalid(_))
    ));
    let outside = executable(&f.outside.join("program"));
    let link = f.root.join("app-link");
    std::os::unix::fs::symlink(outside, &link).unwrap();
    assert!(matches!(
        validate_launch_path(paths, &f.root, &link),
        Err(AppError::PathEscape(_))
    ));
}

#[cfg(windows)]
#[test]
fn windows_rejects_junction_escape() {
    let f = Fixture::new();
    executable(&f.outside.join("program.exe"));
    let target = f.outside.to_str().unwrap().strip_prefix(r"\\?\").unwrap();
    let output = Command::new("cmd.exe")
        .current_dir(&f.root)
        .args(["/D", "/C", "mklink", "/J", "outside-link", target])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let link = f.root.join("outside-link/program.exe");
    assert!(matches!(
        validate_launch_path(&f.engine.paths, &f.root, &link),
        Err(AppError::PathEscape(_))
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_resolves_bundle_executable_and_rejects_escape() {
    let f = Fixture::new();
    let bundle = make_dir(&f.root.join("Example.app"));
    let macos = make_dir(&bundle.join("Contents/MacOS"));
    let info = bundle.join("Contents/Info.plist");
    let mut dict = plist::Dictionary::new();
    dict.insert("CFBundleExecutable".into(), "Example".into());
    plist::Value::Dictionary(dict)
        .to_file_binary(&info)
        .unwrap();
    let program = executable(&macos.join("Example"));
    let valid = validate_launch_path(&f.engine.paths, &f.root, &bundle).unwrap();
    assert_eq!(valid.selected, bundle);
    assert_eq!(valid.program, program);
    fs::remove_file(&program).unwrap();
    std::os::unix::fs::symlink(executable(&f.outside.join("Example")), program).unwrap();
    assert!(matches!(
        validate_launch_path(&f.engine.paths, &f.root, &bundle),
        Err(AppError::PathEscape(_))
    ));
}

#[test]
fn launch_prefers_custom_and_revalidates_without_falling_back() {
    let f = Fixture::new();
    let paths = &f.engine.paths;
    let program = executable(&f.root.join("game.exe"));
    let mut game = f.game();
    game.steam_app_id = Some("123456".into());
    game.metadata["launchPath"] = json!(program);
    assert_eq!(
        decide_launch(paths, &game).unwrap(),
        LaunchAction::Program(program.clone())
    );
    fs::remove_file(&program).unwrap();
    assert!(matches!(
        decide_launch(paths, &game),
        Err(AppError::NotFound(_))
    ));
    game.metadata["launchPath"] = json!(42);
    assert!(matches!(
        decide_launch(paths, &game),
        Err(AppError::Invalid(_))
    ));
    game.metadata["launchPath"] = json!("");
    assert_eq!(
        decide_launch(paths, &game).unwrap(),
        LaunchAction::Steam("steam://rungameid/123456".into())
    );
    for invalid in ["", " 1", "1/2", "１２３"] {
        game.steam_app_id = Some(invalid.into());
        assert!(matches!(
            decide_launch(paths, &game),
            Err(AppError::Invalid(_))
        ));
    }
    game.steam_app_id = None;
    game.metadata["LaunchCommand"] = json!("ignored");
    assert_eq!(
        decide_launch(paths, &game).unwrap_err().to_string(),
        "尚未设置启动方式，请在“编辑”中选择游戏可执行文件"
    );
}

#[test]
fn remove_refuses_active_installs_without_touching_data() {
    let f = Fixture::new();
    let file = executable(&f.root.join("Mod.jar"));
    let db = &f.engine.db;
    for status in ["enabled", "installed", "partially_restored", "uninstalled"] {
        db.conn()
            .execute(
                "INSERT INTO installs(id,game_id,name,status,installed_at,updated_at)
                          VALUES (?1,'g','Mod',?1,'now','now')",
                [status],
            )
            .unwrap();
    }
    assert!(remove_game(&f.engine, "g")
        .unwrap_err()
        .to_string()
        .contains("该游戏仍有 3 个已安装的 Mod"));
    assert_eq!(f.game().name, "Original");
    assert_eq!(fs::read(file).unwrap(), b"inert test fixture");
}

#[test]
fn remove_checks_committed_install_before_reconciliation() {
    let f = Fixture::new();
    f.engine
        .db
        .conn()
        .execute_batch(
            "INSERT INTO installs(id,game_id,name,status,installed_at,updated_at)
           VALUES ('i','g','Mod','planned','now','now');
         INSERT INTO file_transactions(id,kind,status,game_id,plan_json,created_at)
           VALUES ('committed','files','committed','g','{}','now');
         INSERT INTO install_transactions VALUES ('i','committed','install','now');",
        )
        .unwrap();
    assert!(remove_game(&f.engine, "g")
        .unwrap_err()
        .to_string()
        .contains("该游戏仍有 1 个已安装的 Mod"));
    assert_eq!(f.game().id, "g");
}

#[test]
fn remove_preserves_history_files_and_shared_authorizations() {
    let f = Fixture::new();
    let other = make_dir(&f.outside.join("other"));
    let shared = make_dir(&f.outside.join("shared"));
    let exclusive = make_dir(&f.outside.join("exclusive"));
    let file = executable(&f.root.join("Mod.jar"));
    let snapshot_file = executable(&f.engine.paths.backups().join("snapshot"));
    let trash_file = executable(&f.engine.paths.trash().join("payload"));
    let download_file = executable(&f.engine.paths.downloads().join("archive"));
    let conn = &f.engine.db;
    insert_game(&f.engine, "other", &other);
    conn.conn().execute(
        "INSERT INTO game_user_roots VALUES ('g','one',?1,'now'),('g','two',?2,'now'),('other','one',?1,'now')",
        params![shared.to_str().unwrap(), exclusive.to_str().unwrap()],
    ).unwrap();
    conn.conn().execute_batch(
        "INSERT INTO conversations(id,game_id,created_at,updated_at) VALUES ('c','g','now','now');
         INSERT INTO messages(id,conversation_id,role,content,created_at) VALUES ('m','c','user','history','now');
         INSERT INTO file_transactions(id,kind,status,game_id,plan_json,created_at)
           VALUES ('old','files','committed','g','{}','now');
         INSERT INTO installs(id,game_id,name,status,installed_at,updated_at)
           VALUES ('old-install','g','Mod','uninstalled','now','now');
         INSERT INTO install_transactions VALUES ('old-install','old','uninstall','now');
         INSERT INTO snapshots(id,game_id,label,root_path,path,created_at) VALUES ('s','g','Backup','root','path','now');
         INSERT INTO snapshot_transactions VALUES ('s','old');
         INSERT INTO trash_items(id,game_id,original_path,trash_path,transaction_id,created_at)
           VALUES ('t','g','root','path','old','now');
         INSERT INTO download_artifacts(id,path,filename,artifact_json,source_id,project_id,version_id,name,version,created_at)
           VALUES ('d','path','archive','{}','web','p','v','archive','1','now');"
    ).unwrap();
    assert!(f
        .engine
        .paths
        .guard(&exclusive, &f.engine.roots().unwrap())
        .is_ok());
    remove_game(&f.engine, "g").unwrap();
    assert!(matches!(
        games::get_game(conn, "g"),
        Err(AppError::NotFound(_))
    ));
    assert!(matches!(
        remove_game(&f.engine, "g"),
        Err(AppError::NotFound(_))
    ));
    let roots = f.engine.roots().unwrap();
    assert!(f.engine.paths.guard(&shared, &roots).is_ok());
    assert!(f.engine.paths.guard(&other, &roots).is_ok());
    assert!(f.engine.paths.guard(&exclusive, &roots).is_err());
    assert!(f.engine.paths.guard(&f.root, &roots).is_err());
    for path in [file, snapshot_file, trash_file, download_file] {
        assert_eq!(fs::read(path).unwrap(), b"inert test fixture");
    }
    let conn = conn.conn();
    let (game_id, content): (Option<String>, String) = conn.query_row(
        "SELECT c.game_id,m.content FROM conversations c JOIN messages m ON m.conversation_id=c.id WHERE c.id='c'",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!((game_id, content), (None, "history".into()));
    for table in [
        "installs",
        "install_transactions",
        "file_transactions",
        "snapshots",
        "snapshot_transactions",
        "trash_items",
        "download_artifacts",
    ] {
        let count: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1, "{table}");
    }
}

#[test]
fn remove_rolls_back_root_and_conversation_changes_on_sql_failure() {
    let f = Fixture::new();
    let db = &f.engine.db;
    db.conn().execute_batch(
        "INSERT INTO conversations(id,game_id,created_at,updated_at) VALUES ('c','g','now','now');
         INSERT INTO game_user_roots VALUES ('g','modroot','unchanged','now');
         CREATE TRIGGER block_delete BEFORE DELETE ON games BEGIN SELECT RAISE(ABORT, 'test'); END;"
    ).unwrap();
    assert!(matches!(remove_game(&f.engine, "g"), Err(AppError::Db(_))));
    assert_eq!(f.game().id, "g");
    let counts: (i64, i64) = db
        .conn()
        .query_row(
            "SELECT (SELECT count(*) FROM conversations WHERE game_id='g'),
                (SELECT count(*) FROM game_user_roots WHERE game_id='g')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(counts, (1, 1));
}

#[test]
fn remove_blocks_recovery_and_plans_with_or_without_an_explicit_game_id() {
    let f = Fixture::new();
    let plan = f
        .engine
        .plan_file_ops(
            vec![FileOp::Mkdir {
                dst: f.root.join("Mods"),
            }],
            "test".into(),
        )
        .unwrap();
    assert!(remove_game(&f.engine, "g")
        .unwrap_err()
        .to_string()
        .contains("待审批或待执行"));
    f.engine.approve_plan(&plan.id).unwrap();
    assert!(remove_game(&f.engine, "g")
        .unwrap_err()
        .to_string()
        .contains("待审批或待执行"));
    let db = &f.engine.db;
    db.conn()
        .execute(
            "UPDATE file_transactions SET status='failed' WHERE id=?1",
            [&plan.id],
        )
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO file_transactions(id,kind,status,game_id,plan_json,created_at)
                       VALUES ('explicit','files','planned','g','{}','now')",
            [],
        )
        .unwrap();
    assert!(remove_game(&f.engine, "g")
        .unwrap_err()
        .to_string()
        .contains("待审批或待执行"));
    db.conn().execute("UPDATE file_transactions SET status='recovery_failed',game_id=NULL WHERE id='explicit'", []).unwrap();
    assert!(remove_game(&f.engine, "g")
        .unwrap_err()
        .to_string()
        .contains("待恢复"));
    assert!(games::get_game(db, "g").is_ok());
    assert!(!f.root.join("Mods").exists());
}

#[test]
fn remove_checks_snapshot_and_agent_approval_bindings() {
    let f = Fixture::new();
    let db = &f.engine.db;
    db.conn().execute_batch(
        "INSERT INTO snapshots(id,game_id,label,root_path,path,created_at) VALUES ('s','g','Backup','root','path','now');
         INSERT INTO snapshot_transactions VALUES ('s','snapshot-plan');
         INSERT INTO file_transactions(id,kind,status,plan_json,created_at) VALUES ('snapshot-plan','internal','planned','{}','now');"
    ).unwrap();
    assert!(remove_game(&f.engine, "g")
        .unwrap_err()
        .to_string()
        .contains("待审批或待执行"));
    db.conn()
        .execute_batch(
            "UPDATE file_transactions SET status='committed' WHERE id='snapshot-plan';
         INSERT INTO tool_calls (id,conversation_id,name,arguments_json,started_at)
           VALUES ('tool','c','install_mod','{\"request\":{\"gameId\":\"g\"}}','now');
         INSERT INTO approvals (id,conversation_id,tool_call_id,kind,title,created_at)
           VALUES ('a','c','tool','tool','Approval','now');",
        )
        .unwrap();
    assert!(remove_game(&f.engine, "g")
        .unwrap_err()
        .to_string()
        .contains("待审批或待执行"));
    db.conn()
        .execute("UPDATE approvals SET status='rejected'", [])
        .unwrap();
    remove_game(&f.engine, "g").unwrap();
}

#[test]
fn pending_rollback_approval_is_scoped_by_its_original_plan() {
    let f = Fixture::new();
    let plan = f
        .engine
        .plan_file_ops(
            vec![FileOp::Mkdir {
                dst: f.root.join("Mods"),
            }],
            "test".into(),
        )
        .unwrap();
    let db = &f.engine.db;
    db.conn()
        .execute(
            "UPDATE file_transactions SET status='committed' WHERE id=?1",
            [&plan.id],
        )
        .unwrap();
    db.conn().execute(
        "INSERT INTO approvals(id,kind,title,payload_json,created_at) VALUES ('a','tool','Rollback',?1,'now')",
        [json!({"originalPlan": plan}).to_string()],
    ).unwrap();
    assert!(remove_game(&f.engine, "g")
        .unwrap_err()
        .to_string()
        .contains("待审批或待执行"));
    db.conn()
        .execute("UPDATE approvals SET status='rejected'", [])
        .unwrap();
    remove_game(&f.engine, "g").unwrap();
}

fn minecraft_adapter() -> GameAdapter {
    serde_json::from_str(include_str!(
        "../../plugins/games/minecraft-java/adapter.json"
    ))
    .unwrap()
}

#[test]
fn root_location_is_registered_and_mods_requires_adapter() {
    let f = Fixture::new();
    assert_eq!(resolve_location(&f.engine, "g", "root").unwrap(), f.root);
    assert!(resolve_location(&f.engine, "g", "outside")
        .unwrap_err()
        .to_string()
        .contains("root 或 mods"));
    assert!(resolve_location(&f.engine, "g", "mods")
        .unwrap_err()
        .to_string()
        .contains("请先为游戏确认适配包"));
    fs::remove_dir(&f.root).unwrap();
    assert!(matches!(
        resolve_location(&f.engine, "g", "root"),
        Err(AppError::NotFound(_))
    ));
}

#[test]
fn mods_uses_first_existing_root_in_declared_type_order() {
    let f = Fixture::new();
    let mut adapter = minecraft_adapter();
    adapter.mod_types.truncate(2);
    adapter.archive_rules.truncate(2);
    let location = || {
        resolve_mods_directory(
            &f.engine.paths,
            &f.root,
            &adapter,
            Platform::Windows,
            &BTreeMap::new(),
        )
    };
    let err = location().unwrap_err().to_string();
    assert!(err.contains("Mod 目录不存在或不是目录"));
    assert!(!f.root.join("mods").exists());
    let resource_packs = make_dir(&f.root.join("resourcepacks"));
    assert_eq!(location().unwrap(), resource_packs);
    let mods = make_dir(&f.root.join("mods"));
    assert_eq!(location().unwrap(), mods);
}

#[test]
fn mods_reports_unregistered_user_root_and_uses_explicit_registration() {
    let f = Fixture::new();
    let mut adapter = minecraft_adapter();
    adapter.mod_types.retain(|kind| kind.id == "datapack");
    adapter
        .archive_rules
        .retain(|rule| rule.mod_type == "datapack");
    f.engine.db.conn().execute(
        "INSERT INTO plugins(slug,name,game_name,version,install_path,adapter_json,created_at,updated_at)
         VALUES (?1,'Minecraft','Minecraft',?2,'unused',?3,'now','now')",
        params![adapter.slug, adapter.version, serde_json::to_string(&adapter).unwrap()],
    ).unwrap();
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE games SET adapter_slug=?1 WHERE id='g'",
            [&adapter.slug],
        )
        .unwrap();
    let world = make_dir(&f.outside.join("world"));
    let mods = make_dir(&world.join("datapacks"));
    let error = resolve_location(&f.engine, "g", "mods").unwrap_err();
    assert!(error
        .to_string()
        .contains("请先添加用户管理目录 world-datapacks"));
    let registered =
        game_roots::register_user_root(&f.engine, "g", "world-datapacks", &world).unwrap();
    assert_eq!(registered, world);
    let again =
        game_roots::register_user_root(&f.engine, "g", "world-datapacks", &user_path(&world))
            .unwrap();
    assert_eq!(again, world);
    assert_eq!(
        game_roots::registered_user_roots(&f.engine.db, "g").unwrap()["world-datapacks"],
        world
    );
    assert_eq!(resolve_location(&f.engine, "g", "mods").unwrap(), mods);
}

#[test]
fn selecting_a_package_checks_markers_and_keeps_the_game_registration() {
    let f = Fixture::new();
    crate::plugins::seed_builtins(&f.engine.db, &f.engine.paths).unwrap();
    assert!(set_game_adapter(&f.engine, "g", "minecraft-java").is_err());
    assert_eq!(f.game().adapter_slug, None);
    let adapter = minecraft_adapter();
    let marker = adapter
        .markers
        .iter()
        .find(|marker| matches!(marker.kind, crate::plugins::MarkerKind::Directory))
        .unwrap();
    make_dir(&f.root.join(&marker.path));
    let updated = set_game_adapter(&f.engine, "g", "minecraft-java").unwrap();
    assert_eq!(updated.id, "g");
    assert_eq!(updated.name, "Original");
    assert_eq!(updated.adapter_slug.as_deref(), Some("minecraft-java"));
    assert_eq!(
        updated.adapter_version.as_deref(),
        Some(adapter.version.as_str())
    );
    assert_eq!(updated.metadata["keep"], json!({"value":1}));
    assert_eq!(games::list_games(&f.engine.db).unwrap().len(), 1);
}

#[test]
fn adapter_binding_preview_round_trips_and_preserves_unrelated_edits() {
    let f = Fixture::new();
    make_minecraft_selectable(&f);
    f.engine.db.conn().execute_batch(
        "UPDATE games SET slug='stardew-valley',adapter_slug='stardew-valley',adapter_version='0.1.0',
                         installed_mod_count=2,enabled_mod_count=1 WHERE id='g';",
    ).unwrap();
    let before = serde_json::to_value(f.game()).unwrap();
    let before_files = transaction::fingerprint(&f.root).unwrap();
    let binding = prepare_game_adapter_binding(&f.engine, "g", "minecraft-java").unwrap();
    let preview = serde_json::to_value(&binding).unwrap();
    assert_eq!(preview["gameId"], "g");
    assert_eq!(preview["gameName"], "Original");
    assert_eq!(preview["rootPath"], paths::display_path(&f.root));
    #[cfg(windows)]
    assert_ne!(preview["rootPath"], f.root.to_str().unwrap());
    assert_eq!(preview["currentAdapterSlug"], "stardew-valley");
    assert_eq!(preview["currentAdapterVersion"], "0.1.0");
    assert_eq!(preview["targetAdapterSlug"], "minecraft-java");
    assert_eq!(preview["targetAdapterName"], minecraft_adapter().name);
    assert_eq!(preview["targetAdapterVersion"], minecraft_adapter().version);
    assert_eq!(preview["matchedMarkers"], json!(["versions"]));
    assert_eq!(preview["missingRequiredMarkers"], json!([]));
    assert_eq!(preview.as_object().unwrap().len(), 11);
    assert_eq!(serde_json::to_value(f.game()).unwrap(), before);

    assert_eq!(transaction::fingerprint(&f.root).unwrap(), before_files);
    let world = make_dir(&f.outside.join("world"));
    let launch = executable(&f.root.join("game.exe"));
    let before_files_with_launch = transaction::fingerprint(&f.root).unwrap();
    update_game(&f.engine, "g", "Original", launch.to_str()).unwrap();
    f.engine
        .db
        .conn()
        .execute(
            "INSERT INTO game_user_roots(game_id,root_id,path,created_at)
         VALUES ('g','world-datapacks',?1,'now')",
            [world.to_str().unwrap()],
        )
        .unwrap();
    assert_eq!(
        serde_json::from_value::<GameAdapterBinding>(preview.clone()).unwrap(),
        binding
    );
    let mut tampered = preview.clone();
    tampered["targetAdapterVersion"] = json!("999.0.0");
    let tampered = serde_json::from_value(tampered).unwrap();
    assert!(matches!(
        apply_game_adapter_binding(&f.engine, &tampered),
        Err(AppError::Conflict(_))
    ));
    let updated =
        apply_game_adapter_binding(&f.engine, &serde_json::from_value(preview).unwrap()).unwrap();
    assert_eq!(updated.adapter_slug.as_deref(), Some("minecraft-java"));
    assert_eq!(updated.installed_mod_count, 2);
    assert_eq!(updated.enabled_mod_count, 1);
    assert_eq!(updated.metadata["keep"], json!({"value":1}));
    assert_eq!(updated.metadata["launchPath"], launch.to_str().unwrap());
    assert_eq!(
        game_roots::registered_user_roots(&f.engine.db, "g").unwrap()["world-datapacks"],
        world
    );
    assert_eq!(
        transaction::fingerprint(&f.root).unwrap(),
        before_files_with_launch
    );
    assert!(matches!(
        apply_game_adapter_binding(&f.engine, &binding),
        Err(AppError::Conflict(_))
    ));

    let repeat = prepare_game_adapter_binding(&f.engine, "g", "minecraft-java").unwrap();
    let repeated = apply_game_adapter_binding(&f.engine, &repeat).unwrap();
    assert_eq!(repeated.updated_at, updated.updated_at);
    assert_eq!(repeated.last_scanned_at, updated.last_scanned_at);
    assert_eq!(repeated.metadata, updated.metadata);
    let manual = set_game_adapter(&f.engine, "g", "minecraft-java").unwrap();
    assert_eq!(manual.updated_at, updated.updated_at);
}

#[test]
fn adapter_binding_revalidates_game_match_and_exact_package() {
    let f = Fixture::new();
    make_minecraft_selectable(&f);
    let preview = || prepare_game_adapter_binding(&f.engine, "g", "minecraft-java").unwrap();
    let reviewed = preview();
    update_game(&f.engine, "g", "Renamed after review", None).unwrap();
    assert!(matches!(
        apply_game_adapter_binding(&f.engine, &reviewed),
        Err(AppError::Conflict(_))
    ));
    let reviewed = preview();
    make_dir(&f.root.join("saves"));
    assert!(matches!(
        apply_game_adapter_binding(&f.engine, &reviewed),
        Err(AppError::Conflict(_))
    ));
    let reviewed = preview();
    f.engine.db.conn().execute_batch(
        "UPDATE plugins SET adapter_json=json_set(adapter_json,'$.uninstallNotes[0]','changed package contents')
         WHERE slug='minecraft-java';",
    ).unwrap();
    assert!(matches!(
        apply_game_adapter_binding(&f.engine, &reviewed),
        Err(AppError::Conflict(_))
    ));
    let reviewed = preview();
    f.engine.db.conn().execute_batch(
        "UPDATE plugins SET version='2.0.0', adapter_json=json_set(adapter_json,'$.version','2.0.0')
         WHERE slug='minecraft-java';",
    ).unwrap();
    assert!(matches!(
        apply_game_adapter_binding(&f.engine, &reviewed),
        Err(AppError::Conflict(_))
    ));
    let reviewed = preview();
    f.engine
        .db
        .conn()
        .execute_batch("UPDATE plugins SET enabled=0 WHERE slug='minecraft-java';")
        .unwrap();
    assert!(matches!(
        apply_game_adapter_binding(&f.engine, &reviewed),
        Err(AppError::Denied(_))
    ));
    assert!(f.game().adapter_slug.is_none());
}

#[test]
fn adapter_binding_rechecks_blockers_without_blocking_its_own_active_turn() {
    let f = Fixture::new();
    make_minecraft_selectable(&f);
    f.engine.db.conn().execute_batch(
        "INSERT INTO conversations(id,game_id,created_at,updated_at) VALUES ('c','g','now','now');
         INSERT INTO agent_turns(id,conversation_id,status,created_at) VALUES ('turn','c','running','now');
         INSERT INTO tool_calls(id,conversation_id,name,status,started_at)
           VALUES ('binding','c','set_game_adapter','running','now');",
    ).unwrap();
    let reviewed = prepare_game_adapter_binding(&f.engine, "g", "minecraft-java").unwrap();
    f.engine
        .db
        .conn()
        .execute_batch(
            "INSERT INTO approvals(id,conversation_id,tool_call_id,kind,title,status,created_at)
         VALUES ('own','c','binding','tool','Bind adapter','approved','now'),
                ('other','c',NULL,'tool','Other approval','pending','now');",
        )
        .unwrap();
    assert!(apply_game_adapter_binding(&f.engine, &reviewed)
        .unwrap_err()
        .to_string()
        .contains("待审批或待执行"));
    f.engine
        .db
        .conn()
        .execute_batch("UPDATE approvals SET status='rejected' WHERE id='other';")
        .unwrap();
    let plan = f
        .engine
        .plan_internal(
            vec![FileOp::Mkdir {
                dst: f.root.join("not-created"),
            }],
            "Unfinished plan".into(),
        )
        .unwrap();
    assert!(apply_game_adapter_binding(&f.engine, &reviewed)
        .unwrap_err()
        .to_string()
        .contains("待审批或待执行"));
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE file_transactions SET status='recovery_failed' WHERE id=?1",
            [&plan.id],
        )
        .unwrap();
    assert!(apply_game_adapter_binding(&f.engine, &reviewed)
        .unwrap_err()
        .to_string()
        .contains("待恢复"));
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE file_transactions SET status='failed' WHERE id=?1",
            [&plan.id],
        )
        .unwrap();
    assert_eq!(
        apply_game_adapter_binding(&f.engine, &reviewed)
            .unwrap()
            .adapter_slug
            .as_deref(),
        Some("minecraft-java")
    );
    assert!(!f.root.join("not-created").exists());
}

#[test]
fn remove_game_adapter_preserves_identity_registered_roots_mods_and_files() {
    let f = Fixture::new();
    make_minecraft_selectable(&f);
    set_game_adapter(&f.engine, "g", "minecraft-java").unwrap();
    let program = executable(&f.root.join("game.exe"));
    update_game(&f.engine, "g", "Renamed", program.to_str()).unwrap();
    let world = make_dir(&f.outside.join("world"));
    game_roots::register_user_root(&f.engine, "g", "world-datapacks", &world).unwrap();
    let mod_file = executable(&world.join("installed.zip"));
    let snapshot_file = executable(&f.engine.paths.backups().join("snapshot.zip"));
    let db = &f.engine.db;
    db.conn()
        .execute_batch(
            "UPDATE games SET steam_app_id='123456',cover_path='cover.png',
                installed_mod_count=1,enabled_mod_count=1,updated_at='before',
                metadata_json=json_set(metadata_json,'$.onlineIdentity',json('{\"query\":\"verified\"}'))
               WHERE id='g';
             INSERT INTO file_transactions(id,kind,status,game_id,plan_json,created_at)
               VALUES ('old','install','committed','g','{}','now');
             INSERT INTO installs(id,game_id,name,status,installed_at,updated_at)
               VALUES ('installed-mod','g','Mod','enabled','now','now');
             INSERT INTO install_transactions VALUES ('installed-mod','old','install','now');
             INSERT INTO snapshots(id,game_id,label,root_path,path,created_at)
               VALUES ('backup','g','Backup','root','path','now');
             INSERT INTO snapshot_transactions VALUES ('backup','old');",
        )
        .unwrap();
    let roots = game_roots::registered_user_roots(db, "g").unwrap();
    assert_eq!(roots.get("world-datapacks"), Some(&world));
    let mut expected = serde_json::to_value(f.game()).unwrap();
    assert!(expected["metadata"]["lastIdentification"].is_object());
    let updated = remove_game_adapter(&f.engine, "g").unwrap();
    assert_ne!(updated.updated_at, "before");
    expected["adapterSlug"] = json!(null);
    expected["adapterVersion"] = json!(null);
    expected["updatedAt"] = json!(updated.updated_at);
    assert_eq!(serde_json::to_value(&updated).unwrap(), expected);
    assert_eq!(serde_json::to_value(f.game()).unwrap(), expected);
    assert_eq!(
        serde_json::to_value(remove_game_adapter(&f.engine, "g").unwrap()).unwrap(),
        expected,
        "unbinding an unbound game must not change it again"
    );
    assert_eq!(game_roots::registered_user_roots(db, "g").unwrap(), roots);
    assert!(f
        .engine
        .paths
        .guard(&world, &f.engine.roots().unwrap())
        .is_ok());
    assert_eq!(resolve_location(&f.engine, "g", "root").unwrap(), f.root);
    assert!(matches!(
        resolve_location(&f.engine, "g", "mods"),
        Err(AppError::Invalid(_))
    ));
    assert_eq!(
        decide_launch(&f.engine.paths, &updated).unwrap(),
        LaunchAction::Program(program)
    );
    for file in [mod_file, snapshot_file] {
        assert_eq!(fs::read(file).unwrap(), b"inert test fixture");
    }
    {
        let conn = db.conn();
        assert!(conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM installs i
                   JOIN install_transactions it ON it.install_id=i.id
                   JOIN snapshot_transactions st ON st.transaction_id=it.transaction_id
                   JOIN snapshots s ON s.id=st.snapshot_id
                   WHERE i.id='installed-mod' AND i.game_id='g' AND i.status='enabled'
                     AND s.id='backup' AND s.game_id='g')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap());
    }
    set_game_adapter(&f.engine, "g", "minecraft-java").unwrap();
    assert_eq!(game_roots::registered_user_roots(db, "g").unwrap(), roots);
    let other_world = make_dir(&f.outside.join("other-world"));
    assert!(matches!(
        game_roots::register_user_root(&f.engine, "g", "world-datapacks", &other_world),
        Err(AppError::Conflict(_))
    ));
}

#[test]
fn remove_game_adapter_reuses_plan_approval_and_recovery_checks() {
    let f = Fixture::new();
    make_minecraft_selectable(&f);
    let before = set_game_adapter(&f.engine, "g", "minecraft-java").unwrap();
    let world = make_dir(&f.outside.join("world"));
    game_roots::register_user_root(&f.engine, "g", "world-datapacks", &world).unwrap();
    let dst = world.join("datapacks");
    let plan = f
        .engine
        .plan_file_ops(vec![FileOp::Mkdir { dst: dst.clone() }], "Test".into())
        .unwrap();
    let db = &f.engine.db;
    db.conn()
        .execute(
            "INSERT INTO approvals(id,kind,title,payload_json,created_at)
               VALUES ('pending','tool','Review',?1,'now')",
            [json!({"id": plan.id}).to_string()],
        )
        .unwrap();
    let assert_conflict = |message: &str| {
        let error = remove_game_adapter(&f.engine, "g").unwrap_err();
        assert!(matches!(error, AppError::Conflict(_)));
        assert!(error.to_string().contains(message), "{error}");
        let current = f.game();
        assert_eq!(current.adapter_slug, before.adapter_slug);
        assert_eq!(current.adapter_version, before.adapter_version);
        assert_eq!(current.updated_at, before.updated_at);
        assert!(!dst.exists());
    };
    assert_conflict("待审批或待执行");
    resolve_game_conflict(
        &f.engine,
        "g",
        "pending",
        SelectAdapter,
        false,
        unexpected_live_approval,
    )
    .unwrap();
    f.engine.approve_plan(&plan.id).unwrap();
    assert_conflict("待审批或待执行");
    resolve_game_conflict(
        &f.engine,
        "g",
        &plan.id,
        SelectAdapter,
        true,
        unexpected_live_approval,
    )
    .unwrap();
    db.conn()
        .execute(
            "INSERT INTO file_transactions(id,kind,status,game_id,plan_json,created_at)
               VALUES ('recovery','files','recovery_failed','another-game','{}','now')",
            [],
        )
        .unwrap();
    assert_conflict("待恢复");
    db.conn()
        .execute(
            "UPDATE file_transactions SET status='failed' WHERE id='recovery'",
            [],
        )
        .unwrap();
    assert!(remove_game_adapter(&f.engine, "g")
        .unwrap()
        .adapter_slug
        .is_none());
    assert!(!dst.exists());
}

#[test]
fn remove_game_adapter_is_atomic_and_does_not_need_package_or_directory() {
    let f = Fixture::new();
    let db = &f.engine.db;
    fs::remove_dir(&f.root).unwrap();
    db.conn()
        .execute_batch(
            "UPDATE games SET adapter_slug='missing',adapter_version='1.0.0' WHERE id='g';
             CREATE TRIGGER fail_unbind AFTER UPDATE OF adapter_slug ON games
               BEGIN SELECT RAISE(ABORT,'test'); END;",
        )
        .unwrap();
    let before = serde_json::to_value(f.game()).unwrap();
    assert!(matches!(
        remove_game_adapter(&f.engine, "g"),
        Err(AppError::Db(_))
    ));
    assert_eq!(serde_json::to_value(f.game()).unwrap(), before);
    db.conn().execute_batch("DROP TRIGGER fail_unbind").unwrap();
    assert!(matches!(
        remove_game_adapter(&f.engine, "missing-id"),
        Err(AppError::NotFound(_))
    ));
    let updated = remove_game_adapter(&f.engine, "g").unwrap();
    assert!(updated.adapter_slug.is_none());
    assert!(updated.adapter_version.is_none());
    assert!(!f.root.exists());
}

#[test]
fn remove_game_adapter_ipc_accepts_only_id_and_returns_game() {
    use tauri::{test, webview::InvokeRequest};

    let dir = tempfile::tempdir().unwrap();
    let state = crate::app::AppServices::initialize(dir.path().join("data")).unwrap();
    let root = make_dir(&dir.path().join("game"));
    insert_game(&state.engine, "g", &root);
    state
        .engine
        .db
        .conn()
        .execute(
            "UPDATE games SET adapter_slug='minecraft-java',adapter_version='1.0.0' WHERE id='g'",
            [],
        )
        .unwrap();
    let app = test::mock_builder()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            crate::commands::remove_game_adapter
        ])
        .build(test::mock_context(test::noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let response = test::get_ipc_response(
        &window,
        InvokeRequest {
            cmd: "remove_game_adapter".into(),
            callback: tauri::ipc::CallbackFn(1),
            error: tauri::ipc::CallbackFn(2),
            url: (if cfg!(windows) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            })
            .parse()
            .unwrap(),
            body: tauri::ipc::InvokeBody::Json(json!({"id":"g"})),
            headers: Default::default(),
            invoke_key: test::INVOKE_KEY.into(),
        },
    )
    .unwrap();
    let updated = response.deserialize::<serde_json::Value>().unwrap();
    assert_eq!(updated["id"], "g");
    assert_eq!(updated["name"], "Original");
    assert_eq!(updated["adapterSlug"], json!(null));
    assert_eq!(updated["adapterVersion"], json!(null));
}

fn make_minecraft_selectable(f: &Fixture) {
    crate::plugins::seed_builtins(&f.engine.db, &f.engine.paths).unwrap();
    let adapter = minecraft_adapter();
    let marker = adapter
        .markers
        .iter()
        .find(|marker| matches!(marker.kind, crate::plugins::MarkerKind::Directory))
        .unwrap();
    make_dir(&f.root.join(&marker.path));
}

fn unbound_mods_directory_preview(f: &Fixture) -> FilePlan {
    f.engine
        .plan_file_ops(
            vec![FileOp::Mkdir {
                dst: f.root.join("~mods"),
            }],
            "PBZ 安装预览".into(),
        )
        .unwrap()
}

#[cfg(windows)]
#[test]
fn historical_plain_plan_destination_still_blocks_canonical_game_removal() {
    let f = Fixture::new();
    let mut plan = unbound_mods_directory_preview(&f);
    let missing = f.root.join("removed-parent").join("new");
    let ordinary = user_path(&missing).to_str().unwrap().replace('\\', "/");
    assert_ne!(ordinary, missing.to_str().unwrap());
    plan.entries[0].operation = FileOp::Mkdir {
        dst: PathBuf::from(&ordinary),
    };
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE file_transactions SET plan_json=?1 WHERE id=?2",
            params![serde_json::to_string(&plan).unwrap(), plan.id],
        )
        .unwrap();
    let conflicts = list_game_conflicts(&f.engine, "g", Remove).unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].id, plan.id);
    assert!(matches!(
        remove_game(&f.engine, "g"),
        Err(AppError::Conflict(_))
    ));
    assert!(!missing.exists());
}

#[cfg(windows)]
#[test]
fn recorded_overlap_checks_components_and_keeps_verbatim_only_roots_distinct() {
    let overlaps = |root, path| overlaps_game(&[Path::new(path)].into(), &[PathBuf::from(root)]);
    for (left, right) in [
        (r"\\?\C:\Games\Arcade", "C:/Games/Arcade/missing"),
        (
            r"\\?\UNC\server\share\Arcade",
            r"\\server\share\Arcade\missing",
        ),
        (r"C:\Games\Arcade", r"\\?\C:\Games\Arcade\literal."),
    ] {
        assert!(overlaps(left, right));
        assert!(overlaps(right, left));
    }
    for (left, right) in [
        (r"\\?\C:\Games\Arcade", "C:/Games/Arcade-old/mods"),
        (r"\\?\C:\Games\Arcade", "C:/Games/arcade/mods"),
        (r"\\?\C:\Games\Arcade", "C:/Games/Arcade/../Elsewhere"),
        (r"\\?\C:\Games\literal.", "C:/Games/literal./mods"),
        (r"\\?\UNC\server\share\Arcade", r"\\server\share2\Arcade"),
        (r"\\.\C:\Games\Arcade", "C:/Games/Arcade"),
    ] {
        assert!(!overlaps(left, right));
        assert!(!overlaps(right, left));
    }
}

#[cfg(unix)]
#[test]
fn user_root_with_non_utf8_name_cannot_authorize_a_different_directory() {
    use std::os::unix::ffi::OsStrExt;
    let f = Fixture::new();
    make_minecraft_selectable(&f);
    set_game_adapter(&f.engine, "g", "minecraft-java").unwrap();
    let root = make_dir(&f.outside.join(std::ffi::OsStr::from_bytes(b"world-\xff")));
    let lossy = make_dir(Path::new(root.to_string_lossy().as_ref()));
    assert!(matches!(
        game_roots::register_user_root(&f.engine, "g", "world-datapacks", &root),
        Err(AppError::Invalid(_))
    ));
    assert!(game_roots::registered_user_roots(&f.engine.db, "g")
        .unwrap()
        .is_empty());
    assert!(!f.engine.roots().unwrap().contains(&lossy));
}

#[test]
fn selecting_package_checks_approval_and_transaction_state_for_mods_preview() {
    let f = Fixture::new();
    make_minecraft_selectable(&f);
    let plan = unbound_mods_directory_preview(&f);
    // A stale, unapproved standalone preview must not block adapter selection.
    let updated = set_game_adapter(&f.engine, "g", "minecraft-java").unwrap();
    assert_eq!(updated.adapter_slug.as_deref(), Some("minecraft-java"));
    assert!(!f.root.join("~mods").exists());
    // A pending approval linked by its direct FilePlan payload still blocks it.
    f.engine
        .db
        .conn()
        .execute(
            "INSERT INTO approvals(id,kind,title,payload_json,created_at)
         VALUES ('approval-pbz','tool','安装 PBZ',?1,'now')",
            [serde_json::to_string(&plan).unwrap()],
        )
        .unwrap();
    assert!(set_game_adapter(&f.engine, "g", "minecraft-java").is_err());

    f.engine
        .db
        .conn()
        .execute(
            "UPDATE approvals SET status='rejected' WHERE id='approval-pbz'",
            [],
        )
        .unwrap();
    let rebound = set_game_adapter(&f.engine, "g", "minecraft-java").unwrap();
    assert_eq!(rebound.adapter_slug.as_deref(), Some("minecraft-java"));
    let status: String = f
        .engine
        .db
        .conn()
        .query_row(
            "SELECT status FROM file_transactions WHERE id=?1",
            [&plan.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "planned");
    assert!(!f.root.join("~mods").exists());
    assert!(remove_game(&f.engine, "g")
        .unwrap_err()
        .to_string()
        .contains("待审批或待执行"));
    assert_eq!(f.game().adapter_slug.as_deref(), Some("minecraft-java"));
    f.engine.approve_plan(&plan.id).unwrap();
    assert!(set_game_adapter(&f.engine, "g", "minecraft-java").is_err());

    f.engine
        .db
        .conn()
        .execute(
            "UPDATE file_transactions SET status='recovery_failed' WHERE id=?1",
            [&plan.id],
        )
        .unwrap();
    assert!(set_game_adapter(&f.engine, "g", "minecraft-java").is_err());

    f.engine
        .db
        .conn()
        .execute(
            "UPDATE file_transactions SET status='planned',game_id='g' WHERE id=?1",
            [&plan.id],
        )
        .unwrap();
    assert!(set_game_adapter(&f.engine, "g", "minecraft-java").is_err());
}

#[test]
fn conflicts_list_plan_identity_details_and_registered_root_overlap() {
    let f = Fixture::new();
    let other = make_dir(&f.outside.join("other"));
    insert_game(&f.engine, "other", &other);
    let user_root = make_dir(&f.outside.join("user-root"));
    f.engine
        .db
        .conn()
        .execute(
            "INSERT INTO game_user_roots VALUES ('g','external-mods',?1,'now')",
            [user_root.to_str().unwrap()],
        )
        .unwrap();
    let src = user_root.join("source.jar");
    let dst = other.join("copy.jar");
    fs::write(&src, "not applied").unwrap();
    let overlapping = f
        .engine
        .plan_internal(
            vec![FileOp::Copy {
                src: src.clone(),
                dst: dst.clone(),
            }],
            "Copy review".into(),
        )
        .unwrap();
    let explicit = f
        .engine
        .plan_file_ops(
            vec![FileOp::Mkdir {
                dst: other.join("explicit"),
            }],
            "Explicit game review".into(),
        )
        .unwrap();
    let unrelated = f
        .engine
        .plan_internal(
            vec![FileOp::Mkdir {
                dst: other.join("unrelated"),
            }],
            "Other game review".into(),
        )
        .unwrap();
    let standalone = unbound_mods_directory_preview(&f);
    {
        let conn = f.engine.db.conn();
        conn.execute(
            "UPDATE file_transactions SET game_id='g' WHERE id=?1",
            [&explicit.id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO approvals(id,kind,title,detail,payload_json,created_at)
             VALUES ('review','tool','Review explicit plan','Review detail',?1,'now')",
            [json!({"plan": explicit}).to_string()],
        )
        .unwrap();
    }
    let conflicts = list_game_conflicts(&f.engine, "g", SelectAdapter).unwrap();
    assert_eq!(conflicts.len(), 3);
    let by_id = |id: &str| conflicts.iter().find(|c| c.id == id);
    let copy = by_id(&overlapping.id).unwrap();
    assert_eq!(copy.kind, ConflictKind::FilePlan);
    assert_eq!(copy.status, "planned");
    assert_eq!(copy.summary, "Copy review");
    assert_eq!(copy.detail.as_deref(), Some("计划操作：复制 (copy) ×1"));
    assert!(copy.paths.contains(&paths::display_path(&src)));
    assert!(copy.paths.contains(&paths::display_path(&dst)));
    assert!(copy.can_resolve);
    assert!(by_id(&explicit.id).is_some());
    assert!(by_id(&unrelated.id).is_none());
    assert!(by_id(&standalone.id).is_none());
    let approval = by_id("review").unwrap();
    assert_eq!(
        serde_json::to_value(approval).unwrap(),
        json!({
            "id": "review", "kind": "approval", "status": "pending",
            "title": "Review explicit plan", "summary": "Explicit game review",
            "paths": [user_path(&other.join("explicit"))], "detail": "Review detail；计划操作：新建目录 (mkdir) ×1", "planId": explicit.id,
            "canResolve": true, "resolutionHint": "解除会拒绝审批，不会执行计划或回滚文件。"
        })
    );
    assert!(!dst.exists());
}

fn unexpected_live_approval(_: &str, _: &str) -> AppResult<()> {
    panic!("must not reach the Agent approval waiter")
}

#[test]
fn conflict_change_accepts_snake_case_and_defaults_to_adapter_selection() {
    assert_eq!(GameChange::default(), SelectAdapter);
    assert_eq!(
        serde_json::from_value::<GameChange>(json!("select_adapter")).unwrap(),
        SelectAdapter
    );
    assert_eq!(
        serde_json::from_value::<GameChange>(json!("remove")).unwrap(),
        Remove
    );
    assert!(serde_json::from_value::<GameChange>(json!("selectAdapter")).is_err());
}

#[test]
fn removing_game_can_discard_standalone_mkdir_without_touching_files() {
    let f = Fixture::new();
    make_minecraft_selectable(&f);
    insert_game(&f.engine, "other", &f.outside);
    fs::write(f.root.join("existing.txt"), b"existing game file").unwrap();
    let before = transaction::fingerprint(&f.root).unwrap();
    let target = f.root.join("~mods");
    let plan = unbound_mods_directory_preview(&f);

    assert!(remove_game(&f.engine, "g")
        .unwrap_err()
        .to_string()
        .contains("待审批或待执行"));
    assert!(list_game_conflicts(&f.engine, "g", SelectAdapter)
        .unwrap()
        .is_empty());
    assert!(list_game_conflicts(&f.engine, "other", Remove)
        .unwrap()
        .is_empty());
    for (game_id, change) in [("g", SelectAdapter), ("other", Remove)] {
        assert!(matches!(
            resolve_game_conflict(
                &f.engine,
                game_id,
                &plan.id,
                change,
                false,
                unexpected_live_approval
            ),
            Err(AppError::NotFound(_))
        ));
    }
    assert_eq!(
        set_game_adapter(&f.engine, "g", "minecraft-java")
            .unwrap()
            .adapter_slug
            .as_deref(),
        Some("minecraft-java")
    );

    let conflicts = list_game_conflicts(&f.engine, "g", Remove).unwrap();
    assert_eq!(conflicts.len(), 1);
    let conflict = &conflicts[0];
    assert_eq!(conflict.id, plan.id);
    assert_eq!(conflict.kind, ConflictKind::FilePlan);
    assert_eq!(conflict.status, "planned");
    assert_eq!(conflict.summary, "PBZ 安装预览");
    assert_eq!(
        conflict.detail.as_deref(),
        Some("计划操作：新建目录 (mkdir) ×1")
    );
    assert_eq!(conflict.paths, [paths::display_path(&target)]);
    assert!(conflict.can_resolve);
    assert!(!target.exists());

    resolve_game_conflict(
        &f.engine,
        "g",
        &plan.id,
        Remove,
        false,
        unexpected_live_approval,
    )
    .unwrap();
    assert!(list_game_conflicts(&f.engine, "g", Remove)
        .unwrap()
        .is_empty());
    assert_eq!(transaction::fingerprint(&f.root).unwrap(), before);
    remove_game(&f.engine, "g").unwrap();
    assert!(matches!(
        games::get_game(&f.engine.db, "g"),
        Err(AppError::NotFound(_))
    ));
    assert!(games::get_game(&f.engine.db, "other").is_ok());
    assert_eq!(transaction::fingerprint(&f.root).unwrap(), before);
    assert!(!target.exists());
}

#[test]
fn resolving_unbound_planned_conflict_allows_adapter_selection_without_applying() {
    let f = Fixture::new();
    make_minecraft_selectable(&f);
    let dst = f.root.join("plugin-setup");
    let plan = f
        .engine
        .plan_internal(
            vec![FileOp::Mkdir { dst: dst.clone() }],
            "Orphan preview".into(),
        )
        .unwrap();
    assert!(set_game_adapter(&f.engine, "g", "minecraft-java")
        .unwrap_err()
        .to_string()
        .contains("待审批或待执行"));
    resolve_game_conflict(
        &f.engine,
        "g",
        &plan.id,
        SelectAdapter,
        false,
        unexpected_live_approval,
    )
    .unwrap();
    assert!(list_game_conflicts(&f.engine, "g", SelectAdapter)
        .unwrap()
        .is_empty());
    assert_eq!(
        set_game_adapter(&f.engine, "g", "minecraft-java")
            .unwrap()
            .adapter_slug
            .as_deref(),
        Some("minecraft-java")
    );
    let row: (String, String, String, Option<String>) = f
        .engine
        .db
        .conn()
        .query_row(
            "SELECT status, journal_json, error, finished_at FROM file_transactions WHERE id=?1",
            [&plan.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(row.0, "failed");
    assert_eq!(row.1, "[]");
    assert!(row.2.contains("用户解除"));
    assert!(row.3.is_some());
    assert_eq!(f.engine.get_plan(&plan.id).unwrap().summary, plan.summary);
    assert!(f.engine.approve_plan(&plan.id).is_err());
    assert!(!dst.exists());
}

#[test]
fn rejecting_pending_rollback_approval_preserves_files_and_history() {
    let f = Fixture::new();
    make_minecraft_selectable(&f);
    let dst = f.root.join("committed");
    let plan = f
        .engine
        .plan_file_ops(
            vec![FileOp::Mkdir { dst: dst.clone() }],
            "Committed plan".into(),
        )
        .unwrap();
    f.engine.approve_plan(&plan.id).unwrap();
    f.engine.apply_file_ops(&plan.id).unwrap();
    let payload = json!({"originalPlan": plan}).to_string();
    f.engine.db.conn().execute(
        "INSERT INTO approvals(id,kind,title,payload_json,created_at) VALUES ('rollback','tool','Rollback',?1,'now')",
        [&payload],
    ).unwrap();
    assert!(set_game_adapter(&f.engine, "g", "minecraft-java").is_err());
    resolve_game_conflict(
        &f.engine,
        "g",
        "rollback",
        SelectAdapter,
        false,
        unexpected_live_approval,
    )
    .unwrap();
    let approval: (String, String, Option<String>) = f
        .engine
        .db
        .conn()
        .query_row(
            "SELECT status, payload_json, resolved_at FROM approvals WHERE id='rollback'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(approval.0, "rejected");
    assert_eq!(approval.1, payload);
    assert!(approval.2.is_some());
    let status: String = f
        .engine
        .db
        .conn()
        .query_row(
            "SELECT status FROM file_transactions WHERE id=?1",
            [&plan.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "committed");
    assert!(list_game_conflicts(&f.engine, "g", SelectAdapter)
        .unwrap()
        .is_empty());
    set_game_adapter(&f.engine, "g", "minecraft-java").unwrap();
    assert!(dst.is_dir());
}

#[test]
fn approved_plan_cannot_be_resolved_without_confirmation_or_by_another_game() {
    let f = Fixture::new();
    insert_game(&f.engine, "other", &f.outside);
    let plan = unbound_mods_directory_preview(&f);
    f.engine.approve_plan(&plan.id).unwrap();
    for change in [SelectAdapter, Remove] {
        let conflict = list_game_conflicts(&f.engine, "g", change)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(conflict.status, "approved");
        assert!(conflict.can_resolve);
        assert!(conflict.resolution_hint.unwrap().contains("明确确认"));
        assert!(matches!(
            resolve_game_conflict(
                &f.engine,
                "other",
                &plan.id,
                change,
                true,
                unexpected_live_approval
            ),
            Err(AppError::NotFound(_))
        ));
        let denied = resolve_game_conflict(
            &f.engine,
            "g",
            &plan.id,
            change,
            false,
            unexpected_live_approval,
        )
        .unwrap_err();
        assert!(denied.to_string().contains("已批准"));
        assert_eq!(
            list_game_conflicts(&f.engine, "g", change).unwrap()[0].status,
            "approved"
        );
    }
    resolve_game_conflict(
        &f.engine,
        "g",
        &plan.id,
        Remove,
        true,
        unexpected_live_approval,
    )
    .unwrap();
    assert!(list_game_conflicts(&f.engine, "g", SelectAdapter)
        .unwrap()
        .is_empty());
    assert!(f.engine.apply_file_ops(&plan.id).is_err());
    assert!(!f.root.join("~mods").exists());
}

#[test]
fn recovery_conflicts_are_global_and_cannot_be_resolved() {
    let f = Fixture::new();
    insert_game(&f.engine, "other", &f.outside);
    let plan = unbound_mods_directory_preview(&f);
    for status in ["applying", "rolling_back", "recovery_failed"] {
        f.engine
            .db
            .conn()
            .execute(
                "UPDATE file_transactions SET status=?1 WHERE id=?2",
                params![status, plan.id],
            )
            .unwrap();
        for change in [SelectAdapter, Remove] {
            let conflict = list_game_conflicts(&f.engine, "other", change)
                .unwrap()
                .pop()
                .unwrap();
            assert_eq!(conflict.kind, ConflictKind::Recovery);
            assert_eq!(conflict.id, plan.id);
            assert_eq!(conflict.status, status);
            assert!(!conflict.can_resolve);
            assert!(conflict.paths.is_empty());
            assert!(conflict.detail.is_none());
            let denied = resolve_game_conflict(
                &f.engine,
                "other",
                &plan.id,
                change,
                true,
                unexpected_live_approval,
            )
            .unwrap_err();
            assert!(denied.to_string().contains("待恢复"));
            assert_eq!(
                list_game_conflicts(&f.engine, "g", change).unwrap()[0].status,
                status
            );
        }
    }
}

#[tokio::test]
async fn resolving_live_approval_rejects_the_waiter_without_overwriting_an_in_flight_answer() {
    use crate::agent::{
        approval::{ApprovalResponse, Approvals},
        events::Events,
    };
    use std::sync::Arc;

    let f = Fixture::new();
    let plan = unbound_mods_directory_preview(&f);
    f.engine.db.conn().execute_batch(
        "INSERT INTO conversations(id,game_id,created_at,updated_at) VALUES ('c','g','now','now');
         INSERT INTO tool_calls (id,conversation_id,name,status,approval_status,started_at)
           VALUES ('call','c','apply_file_ops','running','pending','now');",
    ).unwrap();
    let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
    let approvals = Approvals::new(Events {
        db: f.engine.db.clone(),
        sink: Arc::new(move |event| {
            events_tx.send(event.clone()).unwrap();
        }),
    });
    let waiter = tokio::spawn({
        let approvals = approvals.clone();
        let payload = json!(plan);
        async move {
            approvals
                .request(
                    "turn",
                    "c",
                    "msg",
                    "call",
                    "apply_file_ops",
                    payload,
                    &tokio_util::sync::CancellationToken::new(),
                )
                .await
        }
    });
    let request = tokio::time::timeout(std::time::Duration::from_secs(5), events_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let id = request.payload["approvalId"].as_str().unwrap();
    let conflict = list_game_conflicts(&f.engine, "g", SelectAdapter)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(conflict.id, id);
    assert_eq!(conflict.plan_id.as_deref(), Some(plan.id.as_str()));

    // A response may already have consumed the runtime entry while its durable
    // status is still pending. In that window the resolver cannot claim success.
    let error = resolve_game_conflict(&f.engine, "g", id, SelectAdapter, false, |_, _| {
        Err(AppError::NotFound("waiter taken".into()))
    })
    .unwrap_err();
    assert!(matches!(error, AppError::Conflict(_)));
    assert_eq!(
        list_game_conflicts(&f.engine, "g", SelectAdapter).unwrap()[0].status,
        "pending"
    );
    resolve_game_conflict(&f.engine, "g", id, SelectAdapter, false, |conv, id| {
        approvals.respond(
            conv,
            id,
            ApprovalResponse {
                approved: false,
                answer: serde_json::Value::Null,
            },
        )
    })
    .unwrap();
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), waiter)
            .await
            .unwrap()
            .unwrap(),
        Err(AppError::Denied(_))
    ));
    let resolved = events_rx.recv().await.unwrap();
    assert_eq!(resolved.kind, "approval-resolved");
    assert_eq!(resolved.payload["status"], "rejected");
    assert!(list_game_conflicts(&f.engine, "g", SelectAdapter)
        .unwrap()
        .is_empty());
    assert!(!f.root.join("~mods").exists());
}
