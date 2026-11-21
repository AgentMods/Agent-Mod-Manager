use super::*;
use crate::{
    db::Db,
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
    game_roots::register_user_root(&f.engine, "g", "world-datapacks", &world).unwrap();
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
    f.engine.db.conn().execute(
        "INSERT INTO approvals(id,kind,title,payload_json,created_at)
         VALUES ('approval-pbz','tool','安装 PBZ',?1,'now')",
        [serde_json::to_string(&plan).unwrap()],
    ).unwrap();
    assert!(set_game_adapter(&f.engine, "g", "minecraft-java").is_err());

    f.engine.db.conn().execute(
        "UPDATE approvals SET status='rejected' WHERE id='approval-pbz'",
        [],
    ).unwrap();
    let rebound = set_game_adapter(&f.engine, "g", "minecraft-java").unwrap();
    assert_eq!(rebound.adapter_slug.as_deref(), Some("minecraft-java"));
    let status: String = f.engine.db.conn().query_row(
        "SELECT status FROM file_transactions WHERE id=?1",
        [&plan.id],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(status, "planned");
    assert!(!f.root.join("~mods").exists());
    assert!(remove_game(&f.engine, "g")
        .unwrap_err()
        .to_string()
        .contains("待审批或待执行"));
    assert_eq!(f.game().adapter_slug.as_deref(), Some("minecraft-java"));
    f.engine.approve_plan(&plan.id).unwrap();
    assert!(set_game_adapter(&f.engine, "g", "minecraft-java").is_err());

    f.engine.db.conn().execute(
        "UPDATE file_transactions SET status='recovery_failed' WHERE id=?1",
        [&plan.id],
    ).unwrap();
    assert!(set_game_adapter(&f.engine, "g", "minecraft-java").is_err());

    f.engine.db.conn().execute(
        "UPDATE file_transactions SET status='planned',game_id='g' WHERE id=?1",
        [&plan.id],
    ).unwrap();
    assert!(set_game_adapter(&f.engine, "g", "minecraft-java").is_err());
}
