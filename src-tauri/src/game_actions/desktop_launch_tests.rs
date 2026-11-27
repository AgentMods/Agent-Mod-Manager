use super::*;
use crate::{
    game_actions::{self, tests::Fixture},
    paths,
};
use serde_json::json;
use std::fs;

fn file(root: &Path, components: &[&str]) -> PathBuf {
    let selected = components
        .iter()
        .fold(root.to_path_buf(), |p, name| p.join(name));
    fs::create_dir_all(selected.parent().unwrap()).unwrap();
    game_actions::tests::executable(&selected)
}

fn options(f: &Fixture) -> Value {
    json!(list_game_launch_options(&f.engine, "g").unwrap())
}

fn decision(f: &Fixture, method: Option<&str>) -> AppResult<DesktopAction> {
    let mut resolved = None;
    launch_with(&f.engine, "g", method, |action| {
        resolved = Some(action);
        Ok(())
    })?;
    Ok(resolved.expect("launch must dispatch exactly once"))
}

fn steam(f: &Fixture, id: &str) {
    f.engine
        .db
        .conn()
        .execute("UPDATE games SET steam_app_id=?1 WHERE id='g'", [id])
        .unwrap();
}

#[test]
fn explicit_launch_dispatches_only_the_selected_method_and_revalidates() {
    let f = Fixture::new();
    steam(&f, "123");
    let direct = file(&f.root, &["base.exe"]);
    let custom = file(&f.root, &["mods and tools", "packs", "deep", "mod.EXE"]);
    game_actions::update_game(&f.engine, "g", "Original", direct.to_str()).unwrap();
    let args = vec![
        " leading and trailing ".into(),
        "".into(),
        "literal & | ; > % ' \"".into(),
    ];
    let saved = set_game_mod_launcher(
        &f.engine,
        "g",
        json!({"name":"  Load mods  ", "path":paths::display_path(&custom), "args":args}),
    )
    .unwrap();
    assert_eq!(saved.metadata["keep"], json!({"value": 1}));
    assert_eq!(saved.metadata["launchPath"], json!(direct));
    assert_eq!(
        saved.metadata["modLauncher"],
        json!({"name":"Load mods", "path":custom, "args":args})
    );
    assert_eq!(
        json!(&saved)["metadata"]["modLauncher"]["path"],
        paths::display_path(&custom)
    );
    assert_eq!(
        options(&f),
        json!([
            {"id":"steam","label":null}, {"id":"direct","label":null}, {"id":"custom","label":"Load mods"}
        ])
    );
    for method in [Some("direct"), None] {
        assert_eq!(
            decision(&f, method).unwrap(),
            DesktopAction::Standard(LaunchAction::Program(direct.clone()))
        );
    }
    assert_eq!(
        decision(&f, Some("steam")).unwrap(),
        DesktopAction::Standard(LaunchAction::Steam("steam://rungameid/123".into()))
    );
    let action = decision(&f, Some("custom")).unwrap();
    assert_eq!(
        action,
        DesktopAction::Mod {
            program: custom.clone(),
            args: args.clone()
        }
    );
    let DesktopAction::Mod { program, args } = action else {
        unreachable!()
    };
    let command = game_actions::program_command(&program, &args).unwrap();
    assert_eq!(command.get_program(), custom.as_os_str());
    assert_eq!(command.get_current_dir(), custom.parent());
    assert!(command.get_args().eq(args.iter().map(std::ffi::OsStr::new)));

    fs::remove_file(&direct).unwrap();
    fs::remove_file(&custom).unwrap();
    let never_dispatch = |_| panic!("must not dispatch a different method");
    for method in [
        None,
        Some("direct"),
        Some("custom"),
        Some("smapi"),
        Some("mod-engine-2"),
        Some("other"),
        Some(""),
    ] {
        assert!(
            launch_with(&f.engine, "g", method, never_dispatch).is_err(),
            "{method:?}"
        );
    }
    assert_eq!(options(&f), json!([{"id":"steam","label":null}]));
    assert!(decision(&f, Some("steam")).is_ok());
    steam(&f, "bad/steam-id");
    assert_eq!(options(&f), json!([]));
    assert!(launch_with(&f.engine, "g", Some("steam"), never_dispatch).is_err());
    assert!(launch_with(&f.engine, "missing", Some("steam"), never_dispatch).is_err());
}

#[test]
fn saving_replacing_and_clearing_mod_launcher_keeps_ordinary_approval_unchanged() {
    let f = Fixture::new();
    steam(&f, "123");
    let old_approval = game_actions::prepare_launch(&f.engine, "g").unwrap();
    let first = file(&f.root, &["launcher.exe"]);
    let second = file(&f.root, &["other.exe"]);
    let outside = file(&f.outside, &["outside.exe"]);
    let protected = file(&f.root, &[".env.exe"]);
    let directory = f.root.join("directory.exe");
    fs::create_dir(&directory).unwrap();
    for value in [
        json!({"name":"One","path":first,"args":["", "--option"]}),
        json!({"name":"Two","path":second,"args":[]}),
    ] {
        let saved = set_game_mod_launcher(&f.engine, "g", value.clone()).unwrap();
        assert_eq!(saved.metadata["modLauncher"], value);
    }
    let saved = f.game();
    for invalid in [
        json!({"name":"ok","path":outside,"args":[]}),
        json!({"name":"ok","path":protected,"args":[]}),
        json!({"name":"ok","path":directory,"args":[]}),
        json!({"name":"ok","path":"other.exe","args":[]}),
        json!({"name":"\t\n","path":second,"args":[]}),
        json!({"name":"ok\rsite","path":second,"args":[]}),
        json!({"name":"ok","path":second,"args":["bad\0argument"]}),
        json!({"name":"ok","path":second,"args":"--not-argv"}),
        json!({"name":"ok","path":second}),
        json!({"name":"ok","path":second,"args":[],"cwd":f.outside}),
    ] {
        assert!(set_game_mod_launcher(&f.engine, "g", invalid).is_err());
        assert_eq!(f.game().metadata, saved.metadata);
        assert_eq!(f.game().updated_at, saved.updated_at);
    }
    assert_eq!(
        game_actions::prepare_launch(&f.engine, "g").unwrap(),
        old_approval
    );
    review::apply_launch_with(&f.engine, &old_approval, |action| {
        assert_eq!(action, LaunchAction::Steam("steam://rungameid/123".into()));
        Ok(())
    })
    .unwrap();

    // A removed root is not valid for explicit launch, including Steam, but
    // clearing the preference is a DB-only operation and still works.
    fs::rename(&f.root, f.outside.join("old game")).unwrap();
    assert!(list_game_launch_options(&f.engine, "g").is_err());
    assert!(decision(&f, Some("steam")).is_err());
    assert!(decision(&f, Some("custom")).is_err());
    assert!(decision(&f, None).is_ok());
    assert_eq!(
        set_game_mod_launcher(&f.engine, "g", Value::Null)
            .unwrap()
            .metadata,
        json!({"keep":{"value":1}})
    );
}

#[cfg(windows)]
#[test]
fn windows_custom_rejects_batch_and_junction_escapes_after_an_initial_valid_save() {
    let f = Fixture::new();
    let path = file(&f.root, &["mods", "launcher.exe"]);
    let batch = file(&f.root, &["launch.cmd"]);
    file(&f.outside, &["launcher.exe"]);
    assert!(set_game_mod_launcher(
        &f.engine,
        "g",
        json!({"name":"Unsafe", "path":batch, "args":[]})
    )
    .is_err());
    set_game_mod_launcher(
        &f.engine,
        "g",
        json!({"name":"Mods", "path":path, "args":[]}),
    )
    .unwrap();
    fs::remove_file(&path).unwrap();
    fs::remove_dir(f.root.join("mods")).unwrap();
    let shown = paths::user_path(&f.outside);
    let output = std::process::Command::new("cmd.exe")
        .current_dir(&f.root)
        .args(["/D", "/C", "mklink", "/J", "mods", shown.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(matches!(
        decision(&f, Some("custom")),
        Err(AppError::PathEscape(_))
    ));
    assert!(matches!(
        set_game_mod_launcher(
            &f.engine,
            "g",
            json!({"name":"Mods", "path":path, "args":[]})
        ),
        Err(AppError::PathEscape(_))
    ));
    assert_eq!(options(&f), json!([]));
}

#[cfg(windows)]
#[test]
fn stardew_requires_verified_layout_and_smapi_runtime_files_not_the_display_name() {
    let f = Fixture::new();
    assert!(f.root.to_str().unwrap().starts_with(r"\\?\"));
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE games SET name='Stardew Valley',slug='stardew-valley' WHERE id='g'",
            [],
        )
        .unwrap();
    let vanilla = file(&f.root, &["Stardew Valley.exe"]);
    let smapi = file(&f.root, &["StardewModdingAPI.exe"]);
    assert_eq!(options(&f), json!([]));
    file(&f.root, &["Stardew Valley.dll"]);
    assert_eq!(options(&f), json!([{"id":"direct","label":null}]));
    assert_eq!(
        decision(&f, Some("direct")).unwrap(),
        DesktopAction::Standard(LaunchAction::Program(vanilla))
    );
    let runtime_config = file(&f.root, &["StardewModdingAPI.runtimeconfig.json"]);
    file(&f.root, &["StardewModdingAPI.deps.json"]);
    file(&f.root, &["StardewModdingAPI.dll"]);
    // The installer renames its packaged SMAPI.config.json to config.json.
    assert_eq!(options(&f), json!([{"id":"direct","label":null}]));
    file(&f.root, &["smapi-internal", "config.json"]);
    assert_eq!(
        options(&f),
        json!([{"id":"direct","label":null},{"id":"smapi","label":null}])
    );
    assert_eq!(
        decision(&f, Some("smapi")).unwrap(),
        DesktopAction::Mod {
            program: smapi,
            args: vec![]
        }
    );
    assert!(
        decision(&f, None).is_err(),
        "inferred direct/SMAPI are not legacy defaults"
    );
    assert!(
        game_actions::prepare_launch(&f.engine, "g").is_err(),
        "Agent/MCP cannot select implicit presets"
    );
    fs::remove_file(runtime_config).unwrap();
    assert!(decision(&f, Some("smapi")).is_err());
    assert_eq!(options(&f), json!([{"id":"direct","label":null}]));
}

#[cfg(windows)]
#[test]
fn mod_engine_2_pins_the_registered_elden_ring_and_rechecks_its_config() {
    let f = Fixture::new();
    assert!(f.root.to_str().unwrap().starts_with(r"\\?\"));
    let game_dir = f.root.join("Game");
    let vanilla = file(&game_dir, &["eldenring.exe"]);
    let directory = game_dir.join("ModEngine2");
    let program = file(&directory, &["modengine2_launcher.exe"]);
    file(&directory, &["modengine2", "bin", "modengine2.dll"]);
    let config = file(&directory, &["config_eldenring.toml"]);
    assert_eq!(options(&f), json!([]));
    steam(&f, "1245620");
    assert_eq!(
        options(&f),
        json!([
            {"id":"steam","label":null},{"id":"direct","label":null},{"id":"mod-engine-2","label":null}
        ])
    );
    let expected = DesktopAction::Mod {
        program,
        args: vec![
            "-t".into(),
            "er".into(),
            "-c".into(),
            "config_eldenring.toml".into(),
            "--game-path".into(),
            paths::display_path(&vanilla),
        ],
    };
    assert_eq!(decision(&f, Some("mod-engine-2")).unwrap(), expected);

    // When the user registers only Game, no file above that registration is
    // read, and the same explicit upstream target can still be reconstructed.
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE games SET root_path=?1, steam_app_id=NULL WHERE id='g'",
            [game_dir.to_str().unwrap()],
        )
        .unwrap();
    assert_eq!(options(&f), json!([]));
    file(&game_dir, &["Data0.bdt"]);
    assert_eq!(decision(&f, Some("mod-engine-2")).unwrap(), expected);
    fs::remove_file(config).unwrap();
    assert!(decision(&f, Some("mod-engine-2")).is_err());
    assert_eq!(options(&f), json!([{"id":"direct","label":null}]));
}

#[cfg(windows)]
#[test]
fn mod_engine_2_does_not_scan_unknown_folders_or_launch_a_nonstandard_flat_game() {
    let f = Fixture::new();
    steam(&f, "1245620");
    let flat = file(&f.root, &["eldenring.exe"]);
    let nested = f.root.join("deep").join("ModEngine2");
    for directory in [&f.root, &nested] {
        file(directory, &["modengine2_launcher.exe"]);
        file(directory, &["config_eldenring.toml"]);
        file(directory, &["modengine2", "bin", "modengine2.dll"]);
    }
    assert_eq!(
        decision(&f, Some("direct")).unwrap(),
        DesktopAction::Standard(LaunchAction::Program(flat))
    );
    assert!(decision(&f, Some("mod-engine-2")).is_err());
    file(&f.root, &["Game", "eldenring.exe"]);
    fs::remove_file(f.root.join("modengine2_launcher.exe")).unwrap();
    assert!(decision(&f, Some("mod-engine-2")).is_err());
}

#[cfg(unix)]
#[test]
fn unix_has_no_automatic_windows_presets_or_ambiguous_stardew_direct() {
    let f = Fixture::new();
    steam(&f, "413150");
    file(&f.root, &["StardewValley"]);
    file(&f.root, &["StardewValley-original"]);
    file(&f.root, &["StardewModdingAPI.exe"]);
    assert_eq!(options(&f), json!([{"id":"steam","label":null}]));
}

#[cfg(unix)]
#[test]
fn unix_execv_passes_argv_and_cwd_without_a_shell_fallback() {
    use std::time::{Duration, Instant};
    let f = Fixture::new();
    let program = file(&f.root, &["dir with space", "launcher"]);
    fs::write(&program, "printf 'unsafe fallback' > unexpected").unwrap();
    assert!(game_actions::launch_program(&program, &[]).is_err());
    assert!(!program.parent().unwrap().join("unexpected").exists());
    // The explicitly selected script declares its interpreter. No user-supplied
    // argument can change this program or become a second shell command.
    fs::write(
        &program,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > temporary.txt\nmv temporary.txt args.txt\n",
    )
    .unwrap();
    let args = vec!["two words".into(), "".into(), "; touch unexpected".into()];
    game_actions::launch_program(&program, &args).unwrap();
    let output = program.parent().unwrap().join("args.txt");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !output.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        fs::read_to_string(output).unwrap(),
        "two words\n\n; touch unexpected\n"
    );
    assert!(!program.parent().unwrap().join("unexpected").exists());
}
