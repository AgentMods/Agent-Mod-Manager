use super::*;
use crate::{db::Db, plugins, storage::AppPaths};
use rusqlite::params;
use serde_json::json;
use std::io::Write;

struct Fixture {
    _temp: tempfile::TempDir,
    base: PathBuf,
    engine: FileEngine,
    game: Game,
    root: PathBuf,
}
impl Fixture {
    fn new(slug: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let paths = AppPaths::new(base.join("manager")).unwrap();
        let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
        plugins::seed_builtins(&engine.db, &engine.paths).unwrap();
        let root = base.join("game");
        fs::create_dir(&root).unwrap();
        for marker in plugins::adapter_for(&engine.db, slug).unwrap().markers {
            match marker.kind {
                MarkerKind::File => write(&root.join(marker.path), "fixture"),
                MarkerKind::Directory => fs::create_dir_all(root.join(marker.path)).unwrap(),
            }
        }
        let game = games::register_game(&engine.db, &root, Some(slug)).unwrap();
        Self {
            _temp: temp,
            base,
            engine,
            game,
            root,
        }
    }
    fn user(&self, id: &str) -> PathBuf {
        let path = self.base.join(format!("user-{id}"));
        fs::create_dir_all(&path).unwrap();
        game_roots::register_user_root(&self.engine, &self.game.id, id, &path).unwrap()
    }
    fn read(&self, options: &ReadOptions) -> LogReport {
        read_logs(&self.engine, &self.game.id, options).unwrap()
    }
    fn diagnose(&self, options: &ReadOptions) -> Diagnosis {
        diagnose_game(&self.engine, &self.game.id, options).unwrap()
    }
    fn read_error(&self) -> AppError {
        read_logs(&self.engine, &self.game.id, &ReadOptions::default()).unwrap_err()
    }
    fn install(&self, id: &str, status: &str, files: &str, dependencies: &str) {
        self.install_for(&self.game.id, id, status, files, dependencies);
    }
    fn install_for(&self, game_id: &str, id: &str, status: &str, files: &str, dependencies: &str) {
        self.engine.db.conn().execute(
            "INSERT INTO installs(id,game_id,name,status,files_json,dependencies_json,installed_at,updated_at) VALUES (?1,?2,?1,?3,?4,?5,'now','now')",
            params![id,game_id,status,files,dependencies],
        ).unwrap();
    }
    fn transaction(&self, id: &str, status: &str, game: &str) {
        self.engine.db.conn().execute(
            "INSERT INTO file_transactions(id,kind,status,game_id,plan_json,created_at) VALUES (?1,'copy',?2,?3,'{}','now')",
            params![id,status,game],
        ).unwrap();
    }
}
fn write(path: &Path, bytes: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}
fn latest(report: &LogReport) -> &DiagnosticFile {
    report
        .files
        .iter()
        .find(|f| Path::new(&f.file).ends_with("logs/latest.log"))
        .unwrap()
}

fn fabric_startup_json(diagnosis: &Diagnosis) -> Value {
    serde_json::to_value(diagnosis)
        .unwrap()
        .get("fabricStartup")
        .cloned()
        .expect("built-in latest.log declaration should produce fabricStartup")
}

fn fabric_disk_json(diagnosis: &Diagnosis) -> Value {
    serde_json::to_value(diagnosis)
        .unwrap()
        .get("fabricDisk")
        .cloned()
        .expect("Minecraft Java diagnosis should include fabricDisk")
}

fn write_jar(path: &Path, metadata: Option<&[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut archive = zip::ZipWriter::new(fs::File::create(path).unwrap());
    if let Some(metadata) = metadata {
        archive
            .start_file("fabric.mod.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(metadata).unwrap();
    }
    archive.finish().unwrap();
}

fn write_fabric_jar(path: &Path, id: &str, version: &str) {
    let metadata = serde_json::to_vec(&json!({
        "schemaVersion": 1,
        "id": id,
        "version": version
    }))
    .unwrap();
    write_jar(path, Some(&metadata));
}

fn write_fabric_selection(root: &Path, mods: &[(&str, &str)]) {
    let mut lines = vec![
        "[12:34:56] [main/INFO]: Loading Minecraft 1.21.1 with Fabric Loader 0.16.14".to_string(),
        format!(
            "[12:34:57] [main/INFO]: Loading {} mod{}:",
            mods.len(),
            if mods.len() == 1 { "" } else { "s" }
        ),
    ];
    lines.extend(
        mods.iter()
            .map(|(id, version)| format!("\t- {id} {version}")),
    );
    write(&root.join("logs/latest.log"), lines.join("\n"));
}

fn write_bethesda_plugin(path: &Path, masters: &[&str]) {
    let mut data = Vec::new();
    let mut subrecord = |code: &[u8; 4], payload: &[u8]| {
        data.extend_from_slice(code);
        data.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        data.extend_from_slice(payload);
    };
    subrecord(b"HEDR", &[0; 12]);
    for master in masters {
        let mut payload = master.as_bytes().to_vec();
        payload.push(0);
        subrecord(b"MAST", &payload);
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"TES4");
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&data);
    write(path, bytes);
}

fn installed_files_json(path: &Path, sha256: &str) -> String {
    json!([{
        "path": path.to_string_lossy(),
        "sha256": sha256,
        "size": 1,
        "before": null
    }])
    .to_string()
}

#[test]
fn fabric_startup_json_reports_selection_and_separate_failure_evidence() {
    let f = Fixture::new("minecraft-java");
    let selection = concat!(
        "[12:34:56] [main/INFO]: Loading Minecraft 1.21.1 with Fabric Loader 0.16.14\n",
        "[12:34:56] [main/INFO]: Loading 3 mods:\n",
        "\t- minecraft 1.21.1\n",
        "\t- fabricloader 0.16.14\n",
        "\t- sodium 0.6.13",
    );
    write(&f.root.join("logs/latest.log"), selection);

    let result = f.diagnose(&ReadOptions::default());
    assert_eq!(
        fs::read(f.root.join("logs/latest.log")).unwrap(),
        selection.as_bytes()
    );
    let startup = fabric_startup_json(&result);
    assert_eq!(startup["status"], "selection_reported");
    assert_eq!(startup["file"], "logs/latest.log");
    assert_eq!(
        startup["modifiedAt"],
        latest(&result.logs).modified_at.as_deref().unwrap()
    );
    assert_eq!(startup["gameVersion"], "1.21.1");
    assert_eq!(startup["loaderVersion"], "0.16.14");
    assert_eq!(startup["startupLine"], 1);
    assert_eq!(startup["selectionLine"], 2);
    assert!(startup["failureLine"].is_null());
    assert_eq!(startup["declaredCount"], 3);
    assert_eq!(startup["omittedCount"], 0);
    let mods = startup["mods"].as_array().unwrap();
    assert_eq!(mods.len(), 3);
    for (index, (id, version)) in [
        ("minecraft", "1.21.1"),
        ("fabricloader", "0.16.14"),
        ("sodium", "0.6.13"),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(mods[index]["id"], id);
        assert_eq!(mods[index]["version"], version);
        assert_eq!(mods[index]["line"], index + 3);
        assert!(mods[index]["nested"].is_boolean());
    }

    let failure = concat!(
        "[12:34:56] [main/INFO]: Loading Minecraft 1.21.1 with Fabric Loader 0.16.14\n",
        "[12:34:57] [main/WARN]: Mod resolution failed\n",
    );
    write(&f.root.join("logs/latest.log"), failure);
    let failed = f.diagnose(&ReadOptions::default());
    assert_eq!(
        fs::read(f.root.join("logs/latest.log")).unwrap(),
        failure.as_bytes()
    );
    let startup = fabric_startup_json(&failed);
    assert_eq!(startup["status"], "resolution_failed");
    assert_eq!(startup["file"], "logs/latest.log");
    assert_eq!(startup["startupLine"], 1);
    assert!(startup["selectionLine"].is_null());
    assert_eq!(startup["failureLine"], 2);

    let evidence = failed
        .evidence
        .iter()
        .find(|e| e.source == "fabric_startup")
        .expect("resolution failure should be observed evidence");
    assert_eq!(evidence.line, Some(2));
    assert!(failed.causes.iter().any(|cause| {
        cause.kind == CauseKind::Dependency
            && cause.basis == Basis::Observed
            && cause.evidence_ids.contains(&evidence.id)
    }));
}

const FABRIC_FAILURE_PREFIX: &str = concat!(
    "[12:34:56] [main/INFO]: Loading Minecraft 1.21.1 with Fabric Loader 0.16.14\n",
    "[12:34:56] [main/INFO]: Fabric is preparing JARs on first launch, this may take a few seconds...\n",
    "[12:34:57] [main/WARN]: Mod resolution failed\n",
    "[12:34:57] [main/INFO]: Immediate reason: [HARD_DEP_NO_CANDIDATE amm_smoke_probe 1.0.0 {depends amm_smoke_required @ [>=2.0.0]}, ROOT_FORCELOAD_SINGLE amm_smoke_probe 1.0.0]\n",
    "[12:34:57] [main/INFO]: Reason: [HARD_DEP amm_smoke_probe 1.0.0 {depends amm_smoke_required @ [>=2.0.0]}]\n",
);
const FABRIC_ERROR: &str = concat!(
    "[12:34:57] [main/ERROR]: Incompatible mods found!\n",
    "net.fabricmc.loader.impl.FormattedException: Some of your mods are incompatible with the game or each other!\n",
);
const FABRIC_FRAMES: &str = concat!(
    "\tat net.fabricmc.loader.impl.FormattedException.ofLocalized(FormattedException.java:51) ~[fabric-loader-0.16.14.jar:?]\n",
    "\tat net.fabricmc.loader.impl.FabricLoaderImpl.load(FabricLoaderImpl.java:196) ~[fabric-loader-0.16.14.jar:?]\n",
);
const FABRIC_MISSING: &str = "Mod 'AMM Smoke Probe' (amm_smoke_probe) 1.0.0 requires version 2.0.0 or later of amm_smoke_required, which is missing!";
const FABRIC_MISMATCH: &str = "Mod 'AMM Smoke Probe' (amm_smoke_probe) 1.0.0 requires version 2.0.0 or later of mod 'AMM Smoke Dependency' (amm_smoke_required), but only the wrong version is present: 1.0.0!";
const FABRIC_ZH_MISSING: &str = "模组 'AMM Smoke Probe' (amm_smoke_probe) 1.0.0 需要 amm_smoke_required 的 2.0.0 及以上版本，但没有安装它！";
const FABRIC_ZH_MISMATCH: &str = "模组 'AMM Smoke Probe' (amm_smoke_probe) 1.0.0 需要 模组 'AMM Smoke Dependency' (amm_smoke_required) 的 2.0.0 及以上版本，但已经安装了的版本 1.0.0 不对！";

fn fabric_failure_with_fix(detail: &str) -> String {
    format!(
        "{FABRIC_FAILURE_PREFIX}[12:34:57] [main/INFO]: Fix: add [], remove [], replace []\n\
         {FABRIC_ERROR}A potential solution has been determined, this may resolve your problem:\n\
         \t - Install amm_smoke_required, version 2.0.0 or later.\nMore details:\n\t - {detail}\n{FABRIC_FRAMES}"
    )
}

fn fabric_zh_failure_with_fix(detail: &str) -> String {
    format!(
        "{FABRIC_FAILURE_PREFIX}[12:34:57] [main/INFO]: Fix: add [], remove [], replace []\n\
         {FABRIC_ERROR}确定了一种可能的解决方法，这样做可能会解决你的问题：\n\
         \t - 安装 amm_smoke_required，2.0.0 及以上版本。\n更多信息：\n\t - {detail}\n{FABRIC_FRAMES}"
    )
}

fn assert_fabric_dependency(result: &Diagnosis, log: &str, expected: &str, detail: &str) {
    assert_eq!(fabric_startup_json(result)["status"], "resolution_failed");
    let rows: Vec<_> = result
        .evidence
        .iter()
        .filter(|e| e.source == "fabric_dependency")
        .collect();
    assert_eq!(rows.len(), 1);
    let row = rows[0];
    assert!(Path::new(&row.file).ends_with("logs/latest.log"));
    let index = log.lines().position(|line| line.ends_with(detail)).unwrap();
    assert_eq!(row.line, Some(index + 1));
    assert_eq!(row.text, log.lines().nth(index).unwrap());
    assert!(result.causes.iter().any(|cause| {
        cause.kind == CauseKind::Dependency
            && cause.basis == Basis::Observed
            && cause.summary.contains(expected)
            && cause.evidence_ids.contains(&row.id)
    }));
    assert!(result.repair_suggestions.iter().all(|suggestion| {
        suggestion.requires_confirmation && suggestion.file_ops_input.is_none()
    }));
}

#[test]
fn fabric_resolution_distinguishes_missing_from_mismatch_without_mutating() {
    let f = Fixture::new("minecraft-java");
    let path = f.root.join("logs/latest.log");
    for (detail, expected) in [
        (FABRIC_MISSING, "amm_smoke_required 缺失"),
        (FABRIC_MISMATCH, "amm_smoke_required 版本不匹配"),
    ] {
        let log = fabric_failure_with_fix(detail);
        write(&path, &log);
        let before = f.engine.db.conn().total_changes();
        let result = f.diagnose(&ReadOptions {
            max_evidence: 1,
            ..Default::default()
        });
        assert_fabric_dependency(&result, &log, expected, detail);
        assert_eq!(fs::read_to_string(&path).unwrap(), log);
        assert_eq!(f.engine.db.conn().total_changes(), before);
    }
}

#[test]
fn fabric_resolution_supports_named_log_and_no_fix_plural_mismatch() {
    let f = Fixture::new("minecraft-java");
    let detail = FABRIC_MISMATCH.replace(
        "version is present: 1.0.0",
        "versions are present: 1.0.0/1.5.0",
    );
    let log = format!(
        "{FABRIC_FAILURE_PREFIX}[12:34:57] [main/INFO]: No fix?\n{FABRIC_ERROR} - {detail}\n\
         \t - You need to install version 2.0.0 or later of mod 'AMM Smoke Dependency' (amm_smoke_required).\n{FABRIC_FRAMES}"
    )
    .replace("[12:34:57] [main/WARN]: ", "[12:34:57] [main/WARN] (FabricLoader/Resolution) ")
    .replace("[12:34:57] [main/INFO]: ", "[12:34:57] [main/INFO] (FabricLoader/Resolution) ")
    .replace("[12:34:57] [main/ERROR]: ", "[12:34:57] [main/ERROR] (FabricLoader) ");
    write(&f.root.join("logs/latest.log"), &log);
    assert_fabric_dependency(
        &f.diagnose(&ReadOptions::default()),
        &log,
        "amm_smoke_required 版本不匹配",
        &detail,
    );
}

#[test]
fn fabric_resolution_reads_chinese_required_dependencies_in_utf8_and_explicit_gbk() {
    let f = Fixture::new("minecraft-java");
    for (detail, expected) in [
        (FABRIC_ZH_MISSING, "amm_smoke_required 缺失"),
        (FABRIC_ZH_MISMATCH, "amm_smoke_required 版本不匹配"),
    ] {
        let log = fabric_zh_failure_with_fix(detail);
        let path = f.root.join("logs/latest.log");
        write(&path, &log);
        assert_fabric_dependency(&f.diagnose(&ReadOptions::default()), &log, expected, detail);

        let (encoded, _, corrupt) = encoding_rs::GBK.encode(&log);
        assert!(!corrupt);
        write(&path, &encoded);
        let result = f.diagnose(&ReadOptions {
            legacy_log_encoding: Some(LegacyLogEncoding::Gbk),
            ..Default::default()
        });
        assert_eq!(latest(&result.logs).encoding.as_deref(), Some("gbk"));
        assert_fabric_dependency(&result, &log, expected, detail);
        assert_eq!(fs::read(&path).unwrap(), encoded.as_ref());
    }
}

#[test]
fn fabric_resolution_supports_simple_intervals_and_wildcards_in_both_locales() {
    let f = Fixture::new("minecraft-java");
    let path = f.root.join("logs/latest.log");
    for range in [
        "any version between 2.1.0 (inclusive) and 2.3.1 (exclusive)",
        "any 2.1.x version",
    ] {
        let detail = FABRIC_MISSING.replace("version 2.0.0 or later", range);
        let log = fabric_failure_with_fix(&detail);
        write(&path, &log);
        assert_fabric_dependency(
            &f.diagnose(&ReadOptions::default()),
            &log,
            "amm_smoke_required 缺失",
            &detail,
        );
    }
    for range in [
        "从 2.1.0（含）到 2.3.1（不含）的任意版本",
        "任意 2.1.x 版本",
    ] {
        let detail = FABRIC_ZH_MISMATCH
            .replace("2.0.0 及以上版本", range)
            .replace("AMM Smoke Dependency", "太阳 的 模组")
            .replace("版本 1.0.0 不对", "版本 1.0.0/1.5.0 不对");
        let log = format!(
            "{FABRIC_FAILURE_PREFIX}[12:34:57] [main/INFO]: No fix?\n{FABRIC_ERROR} - {detail}\n\
             \t - 你需要安装 模组 '太阳 的 模组' (amm_smoke_required) 的 {range}。\n{FABRIC_FRAMES}"
        );
        write(&path, &log);
        assert_fabric_dependency(
            &f.diagnose(&ReadOptions::default()),
            &log,
            "amm_smoke_required 版本不匹配",
            &detail,
        );
    }
}

#[test]
fn fabric_resolution_does_not_mix_english_and_chinese_grammar() {
    let f = Fixture::new("minecraft-java");
    let zh = fabric_zh_failure_with_fix(FABRIC_ZH_MISSING);
    for log in [
        zh.replace("更多信息：", "More details:"),
        zh.replace(FABRIC_ZH_MISSING, FABRIC_MISSING),
        zh.replace(" 需要 ", " 建议安装 "),
        zh.replace("，但没有安装它！", "，但由于其他限制条件无法载入！"),
        zh.replace("2.0.0 及以上版本，但", "任意 2.bad.x 版本，但"),
        zh.replace("2.0.0 及以上版本，但", "任意 2.x 版本 或 任意 3.x 版本，但"),
        format!(
            "{FABRIC_FAILURE_PREFIX}[12:34:57] [main/INFO]: No fix?\n{FABRIC_ERROR} - {FABRIC_ZH_MISSING}\n\
             \t - You need to install version 2.0.0 or later of amm_smoke_required.\n{FABRIC_FRAMES}"
        ),
        format!(
            "{FABRIC_FAILURE_PREFIX}[12:34:57] [main/INFO]: No fix?\n{FABRIC_ERROR} - {FABRIC_ZH_MISSING}\n\
             \t - 你需要安装 amm_smoke_required 的 2.0.0 及以上版本。\n \
             - {FABRIC_MISMATCH}\n\t - You need to install version 2.0.0 or later of amm_smoke_required.\n{FABRIC_FRAMES}"
        ),
    ] {
        write(&f.root.join("logs/latest.log"), &log);
        let result = f.diagnose(&ReadOptions::default());
        assert_eq!(fabric_startup_json(&result)["status"], "resolution_failed");
        assert!(result.evidence.iter().any(|row| row.source == "fabric_startup"));
        assert!(result.evidence.iter().all(|row| row.source != "fabric_dependency"), "{log}");
    }
}

#[test]
fn fabric_resolution_falls_back_for_unscoped_partial_or_unknown_details() {
    let f = Fixture::new("minecraft-java");
    let log = fabric_failure_with_fix(FABRIC_MISSING);
    for input in [
        log.replace(FABRIC_FRAMES, ""),
        log.replace("(FormattedException.java:51)", "("),
        log.replace("More details:", "更多详情："),
        log.replace("requires version", "recommends version"),
        log.replace(
            "which is missing!",
            "which can't be loaded due to other constraints!",
        ),
        log.replace(
            " requires version 2.0.0 or later of ",
            " requires any version from https://invalid.test of ",
        ),
        log.replace("[main/ERROR]:", "[Render thread/ERROR]:"),
        log.replace("[main/ERROR]: ", "[main/ERROR] (OtherLoader) "),
        log.replace("[main/INFO]: Reason:", "[main/INFO] (OtherLoader) Reason:"),
        log.replace(
            "More details:\n",
            "More details:\n[12:34:57] [main/INFO]: unrelated message\n",
        ),
        log.replace(
            FABRIC_MISSING,
            &format!("{FABRIC_MISSING}\n\t - {FABRIC_MISSING}"),
        ),
        log.replace(
            FABRIC_MISSING,
            &(0..17)
                .map(|i| FABRIC_MISSING.replace("amm_smoke_required", &format!("dep_{i}")))
                .collect::<Vec<_>>()
                .join("\n\t - "),
        ),
    ] {
        write(&f.root.join("logs/latest.log"), &input);
        let result = f.diagnose(&ReadOptions::default());
        assert_eq!(fabric_startup_json(&result)["status"], "resolution_failed");
        assert!(result
            .evidence
            .iter()
            .any(|row| row.source == "fabric_startup"));
        assert!(
            result
                .evidence
                .iter()
                .all(|row| row.source != "fabric_dependency"),
            "{input}"
        );
    }

    write(&f.root.join("logs/latest.log"), &log);
    for options in [
        ReadOptions {
            max_lines_per_file: 12,
            ..Default::default()
        },
        ReadOptions {
            max_bytes_per_file: log.len() - 3,
            ..Default::default()
        },
        ReadOptions {
            max_line_chars: 120,
            ..Default::default()
        },
    ] {
        let result = f.diagnose(&options);
        assert!(result.logs.truncated);
        assert!(result
            .evidence
            .iter()
            .any(|row| row.source == "fabric_startup"));
        assert!(result
            .evidence
            .iter()
            .all(|row| row.source != "fabric_dependency"));
    }
    let repeated = format!("{log}{FABRIC_FAILURE_PREFIX}");
    write(&f.root.join("logs/latest.log"), repeated);
    let result = f.diagnose(&ReadOptions::default());
    assert_eq!(fabric_startup_json(&result)["status"], "ambiguous");
    assert!(result
        .evidence
        .iter()
        .all(|row| row.source != "fabric_dependency"));
}

#[test]
#[ignore = "requires native logs produced by scripts/run-fabric-resolver-smoke.mjs"]
fn fabric_replays_real_resolver_smoke() {
    let capture = PathBuf::from(std::env::var_os("AMM_FABRIC_SMOKE_DIR").unwrap());
    for (mode, failure) in [
        (
            "missing-dependency",
            Some(("amm_smoke_required 缺失", FABRIC_MISSING)),
        ),
        (
            "wrong-version",
            Some(("amm_smoke_required 版本不匹配", FABRIC_MISMATCH)),
        ),
        (
            "zh-missing-dependency",
            Some(("amm_smoke_required 缺失", FABRIC_ZH_MISSING)),
        ),
        (
            "zh-wrong-version",
            Some(("amm_smoke_required 版本不匹配", FABRIC_ZH_MISMATCH)),
        ),
        ("base", None),
    ] {
        let f = Fixture::new("minecraft-java");
        let log = fs::read_to_string(capture.join(mode).join("logs/latest.log")).unwrap();
        let imported = f.root.join("logs/latest.log");
        write(&imported, &log);
        let before = f.engine.db.conn().total_changes();
        let result = f.diagnose(&ReadOptions::default());
        assert_eq!(fs::read_to_string(imported).unwrap(), log);
        assert_eq!(f.engine.db.conn().total_changes(), before);
        if let Some((expected, detail)) = failure {
            assert_fabric_dependency(&result, &log, expected, detail);
        } else {
            let startup = fabric_startup_json(&result);
            assert_eq!(startup["status"], "selection_reported");
            assert_eq!(startup["declaredCount"], 5);
            assert!(startup["mods"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == "amm_smoke_probe"));
            assert!(result
                .evidence
                .iter()
                .all(|row| row.source != "fabric_dependency"));
        }
    }
}

#[test]
fn fabric_startup_is_unavailable_without_its_declared_file_and_ignores_crash_latest() {
    let f = Fixture::new("minecraft-java");
    write(
        &f.root.join("crash-reports/latest.log"),
        concat!(
            "[12:34:56] [main/INFO]: Loading Minecraft 1.21.1 with Fabric Loader 0.16.14\n",
            "[12:34:56] [main/INFO]: Loading 1 mods:\n",
            "\t- sodium 0.6.13\n",
            "[12:34:57] [main/WARN]: Mod resolution failed\n",
        ),
    );
    let result = f.diagnose(&ReadOptions::default());
    let startup = fabric_startup_json(&result);
    assert_eq!(startup["status"], "unavailable");
    assert_eq!(startup["file"], "logs/latest.log");
    assert!(startup["startupLine"].is_null());
    assert!(!result.evidence.iter().any(|e| e.source == "fabric_startup"));

    let mut adapter = plugins::adapter_for(&f.engine.db, "minecraft-java").unwrap();
    adapter
        .log_paths
        .retain(|path| path.path != "logs/latest.log");
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE plugins SET adapter_json=?1 WHERE slug='minecraft-java'",
            [serde_json::to_string(&adapter).unwrap()],
        )
        .unwrap();
    write(
        &f.root.join("logs/latest.log"),
        "[12:34:57] [main/WARN]: Mod resolution failed\n",
    );
    let undeclared = f.diagnose(&ReadOptions::default());
    assert!(serde_json::to_value(&undeclared)
        .unwrap()
        .get("fabricStartup")
        .is_none());
}

#[test]
fn fabric_startup_respects_byte_and_physical_line_limits() {
    let f = Fixture::new("minecraft-java");
    let startup_line =
        "[12:34:56] [main/INFO]: Loading Minecraft 1.21.1 with Fabric Loader 0.16.14";
    let contents =
        format!("{startup_line}\n[12:34:56] [main/INFO]: Loading 1 mods:\n\t- sodium 0.6.13\n");
    write(&f.root.join("logs/latest.log"), &contents);

    let byte_limited = f.diagnose(&ReadOptions {
        max_bytes_per_file: startup_line.len() + 1,
        ..Default::default()
    });
    let startup = fabric_startup_json(&byte_limited);
    assert_eq!(startup["status"], "startup_observed");
    assert_eq!(startup["startupLine"], 1);
    assert!(startup["selectionLine"].is_null());
    assert!(startup["declaredCount"].is_null());
    assert!(startup["mods"].as_array().unwrap().is_empty());
    assert!(latest(&byte_limited.logs).truncated);
    assert!(!byte_limited
        .evidence
        .iter()
        .any(|e| e.source == "fabric_startup"));

    let line_limited = f.diagnose(&ReadOptions {
        max_lines_per_file: 1,
        ..Default::default()
    });
    let startup = fabric_startup_json(&line_limited);
    assert_eq!(startup["status"], "startup_observed");
    assert_eq!(startup["startupLine"], 1);
    assert!(startup["selectionLine"].is_null());
    assert!(startup["declaredCount"].is_null());
    assert!(startup["mods"].as_array().unwrap().is_empty());
    assert!(latest(&line_limited.logs).truncated);
}

#[cfg(unix)]
#[test]
fn fabric_startup_does_not_follow_symlink_or_mutate_files_or_database() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new("minecraft-java");
    let external = f.base.join("external-latest.log");
    let content = concat!(
        "[12:34:56] [main/INFO]: Loading Minecraft 1.21.1 with Fabric Loader 0.16.14\n",
        "[12:34:56] [main/INFO]: Loading 1 mods:\n",
        "\t- sodium 0.6.13\n",
        "[12:34:57] [main/WARN]: Mod resolution failed\n",
    );
    write(&external, content);
    fs::create_dir_all(f.root.join("logs")).unwrap();
    symlink(&external, f.root.join("logs/latest.log")).unwrap();
    f.install("sentinel", "enabled", "[]", "[]");
    f.transaction("txn-sentinel", "committed", &f.game.id);

    let install_before: (String, String, String, String) = f
        .engine
        .db
        .conn()
        .query_row(
            "SELECT status,files_json,dependencies_json,updated_at FROM installs WHERE id='sentinel'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let transaction_before: (String, String, String) = f
        .engine
        .db
        .conn()
        .query_row(
            "SELECT status,plan_json,created_at FROM file_transactions WHERE id='txn-sentinel'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();

    let result = f.diagnose(&ReadOptions::default());
    assert!(serde_json::to_value(&result)
        .unwrap()
        .get("fabricStartup")
        .is_none());
    assert!(result
        .logs
        .issues
        .iter()
        .any(|issue| issue.path == "logs/latest.log"));
    assert_eq!(result.logs.bytes_read, 0);
    assert!(!result.evidence.iter().any(|e| e.source == "fabric_startup"));

    let install_after: (String, String, String, String) = f
        .engine
        .db
        .conn()
        .query_row(
            "SELECT status,files_json,dependencies_json,updated_at FROM installs WHERE id='sentinel'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let transaction_after: (String, String, String) = f
        .engine
        .db
        .conn()
        .query_row(
            "SELECT status,plan_json,created_at FROM file_transactions WHERE id='txn-sentinel'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(fs::read(&external).unwrap(), content.as_bytes());
    assert_eq!(install_after, install_before);
    assert_eq!(transaction_after, transaction_before);
}

#[test]
fn all_six_adapters_read_only_their_registered_declarations() {
    for slug in [
        "minecraft-java",
        "skyrim-se",
        "fallout4",
        "stardew-valley",
        "cyberpunk-2077",
        "baldurs-gate-3",
    ] {
        let f = Fixture::new(slug);
        let adapter = plugins::adapter_for(&f.engine.db, slug).unwrap();
        let ids: BTreeSet<_> = adapter
            .log_paths
            .iter()
            .chain(&adapter.load_order.paths)
            .map(|p| p.root.as_str())
            .collect();
        for root in &adapter.install_roots {
            if root.kind == RootKind::RegisteredUser && ids.contains(root.id.as_str()) {
                f.user(&root.id);
            }
        }
        let ctx = context(&f.engine, &f.game.id).unwrap();
        for declaration in &adapter.log_paths {
            let path = ctx.resolve(declaration).unwrap();
            if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("txt" | "log")
            ) {
                write(&path, "failed to load fixture\n");
            } else {
                write(
                    &path.join("AlteredFiles-fixture.log"),
                    "failed to load fixture\n",
                );
            }
        }
        for declaration in &adapter.load_order.paths {
            write(&ctx.resolve(declaration).unwrap(), "\n");
        }
        let report = f.read(&ReadOptions::default());
        assert!(report.issues.is_empty(), "{slug}: {:?}", report.issues);
        assert!(
            report
                .files
                .iter()
                .any(|p| p.kind == FileKind::Log && p.status == ReadStatus::Read),
            "{slug}"
        );
        assert!(
            f.diagnose(&ReadOptions::default())
                .causes
                .iter()
                .any(|c| c.kind == CauseKind::Loader && c.basis == Basis::Heuristic),
            "{slug}"
        );
    }
}

#[test]
fn registered_game_enabled_adapter_and_confirmed_platform_are_required() {
    let f = Fixture::new("skyrim-se");
    assert!(matches!(
        read_logs(&f.engine, "not-registered", &ReadOptions::default()),
        Err(AppError::NotFound(_))
    ));
    for platform in ["unknown", "linux"] {
        f.engine
            .db
            .conn()
            .execute(
                "UPDATE games SET platform=?1 WHERE id=?2",
                params![platform, f.game.id],
            )
            .unwrap();
        assert!(read_logs(&f.engine, &f.game.id, &ReadOptions::default()).is_err());
    }
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE games SET platform='windows' WHERE id=?1",
            [&f.game.id],
        )
        .unwrap();
    f.engine
        .db
        .conn()
        .execute("UPDATE plugins SET enabled=0 WHERE slug='skyrim-se'", [])
        .unwrap();
    assert!(matches!(f.read_error(), AppError::Denied(_)));
}

#[test]
fn strict_limits_and_options_cannot_supply_arbitrary_paths() {
    let f = Fixture::new("minecraft-java");
    assert!(serde_json::from_value::<ReadOptions>(json!({"path":"/etc/passwd"})).is_err());
    let invalid = [
        ReadOptions {
            max_files: 0,
            ..Default::default()
        },
        ReadOptions {
            max_files: 33,
            ..Default::default()
        },
        ReadOptions {
            max_bytes_per_file: 1024 * 1024 + 1,
            ..Default::default()
        },
        ReadOptions {
            max_total_bytes: 4 * 1024 * 1024 + 1,
            ..Default::default()
        },
        ReadOptions {
            max_lines_per_file: 0,
            ..Default::default()
        },
        ReadOptions {
            max_line_chars: 8193,
            ..Default::default()
        },
        ReadOptions {
            max_directory_entries: 0,
            ..Default::default()
        },
        ReadOptions {
            max_evidence: 129,
            ..Default::default()
        },
    ];
    for options in invalid {
        assert!(matches!(
            read_logs(&f.engine, &f.game.id, &options),
            Err(AppError::Invalid(_))
        ));
    }
}

#[test]
fn bom_crlf_blank_lines_and_exact_evidence_numbers_are_preserved() {
    let f = Fixture::new("minecraft-java");
    write(&f.root.join("logs/latest.log"), "\u{feff}started\r\n\r\nmissing dependency core\r\nUnsupportedClassVersionError\r\nfailed to load Example\r\nFileNotFoundException: ignored/path");
    let result = f.diagnose(&ReadOptions::default());
    assert_eq!(latest(&result.logs).lines[0].text, "started");
    assert_eq!(latest(&result.logs).lines[1].text, "");
    assert_eq!(
        result.evidence.iter().map(|e| e.line).collect::<Vec<_>>(),
        vec![Some(3), Some(4), Some(5), Some(6)]
    );
    for kind in [
        CauseKind::Dependency,
        CauseKind::Version,
        CauseKind::Loader,
        CauseKind::MissingFile,
    ] {
        assert!(result
            .causes
            .iter()
            .any(|c| c.kind == kind && c.basis == Basis::Heuristic));
    }
    assert!(result
        .causes
        .iter()
        .flat_map(|c| &c.evidence_ids)
        .all(|id| result.evidence.iter().any(|e| e.id == *id)));
    assert!(result
        .repair_suggestions
        .iter()
        .all(|s| s.requires_confirmation && s.file_ops_input.is_none()));
}

#[test]
fn clipped_long_line_does_not_hide_complete_eof_line_or_become_evidence() {
    let f = Fixture::new("minecraft-java");
    write(
        &f.root.join("logs/latest.log"),
        format!("failed to load {}\nfailed to load", "x".repeat(100)),
    );
    let result = f.diagnose(&ReadOptions {
        max_line_chars: 16,
        ..Default::default()
    });
    let file = latest(&result.logs);
    assert_eq!(file.lines.len(), 2);
    assert!(file.lines[0].truncated);
    assert!(!file.lines[1].truncated);
    assert_eq!(result.evidence.len(), 1);
    assert_eq!(result.evidence[0].line, Some(2));
    assert!(file.truncated);
}

#[test]
fn byte_clipped_physical_line_is_never_promoted_to_evidence() {
    let f = Fixture::new("minecraft-java");
    write(
        &f.root.join("logs/latest.log"),
        "ok\nmissing dependency core\n",
    );
    let result = f.diagnose(&ReadOptions {
        max_bytes_per_file: 22,
        ..Default::default()
    });
    assert_eq!(latest(&result.logs).bytes_read, 22);
    assert!(latest(&result.logs).truncated);
    assert_eq!(latest(&result.logs).lines.len(), 1);
    assert!(result.evidence.is_empty());
    assert_eq!(result.status, "unknown");
}

#[test]
fn incomplete_utf8_at_prefix_boundary_is_not_a_corrupt_encoding() {
    let f = Fixture::new("minecraft-java");
    write(&f.root.join("logs/latest.log"), "ok\n中文日志\n");
    let report = f.read(&ReadOptions {
        max_bytes_per_file: 4,
        ..Default::default()
    });
    let file = latest(&report);
    assert_eq!(file.status, ReadStatus::Read);
    assert_eq!(file.lines.len(), 1);
    assert_eq!(file.lines[0].number, 1);
    assert_eq!(file.lines[0].text, "ok");
    assert!(file.truncated);
}

#[test]
fn line_limit_and_display_control_expansion_are_bounded() {
    let f = Fixture::new("minecraft-java");
    write(
        &f.root.join("logs/latest.log"),
        "\u{202e}hello\nok\nmissing dependency core\n",
    );
    let result = f.diagnose(&ReadOptions {
        max_lines_per_file: 2,
        max_line_chars: 8,
        ..Default::default()
    });
    let file = latest(&result.logs);
    assert_eq!(file.lines.len(), 2);
    assert!(file.lines[0].truncated);
    assert_eq!(file.lines[0].text, "\\u{202e}");
    assert!(file.lines.iter().all(|l| l.text.chars().count() <= 8));
    assert!(result.evidence.is_empty());
    assert!(result.logs.truncated);
}

#[test]
fn binary_and_unknown_encodings_do_not_become_empty_successful_text() {
    let f = Fixture::new("minecraft-java");
    let cases: &[(&[u8], ReadStatus)] = &[
        (b"missing dependency\0core", ReadStatus::Binary),
        (b"a\x01b", ReadStatus::Binary),
        (b"\xff\xfea\0b", ReadStatus::UnknownEncoding),
        (b"\xfe\xff\xd8\0", ReadStatus::UnknownEncoding),
        (b"\xff\xfe\0\0a\0\0\0", ReadStatus::UnknownEncoding),
        (b"\0\0\xfe\xff\0\0\0a", ReadStatus::UnknownEncoding),
        (b"\xff\xfea\0\0\0", ReadStatus::Binary),
        (b"error \xff corrupt", ReadStatus::UnknownEncoding),
        (b"\xed\xa0\x80", ReadStatus::UnknownEncoding),
    ];
    for (bytes, expected) in cases {
        write(&f.root.join("logs/latest.log"), bytes);
        let result = f.diagnose(&ReadOptions::default());
        assert_eq!(latest(&result.logs).status, *expected);
        assert_eq!(latest(&result.logs).bytes_read, bytes.len());
        assert!(latest(&result.logs).lines.is_empty());
        assert!(result.evidence.is_empty());
        assert_eq!(result.status, "unknown");
    }
}

fn utf16(contents: &str, little: bool) -> Vec<u8> {
    let mut bytes = if little {
        vec![0xff, 0xfe]
    } else {
        vec![0xfe, 0xff]
    };
    for unit in contents.encode_utf16() {
        bytes.extend(if little {
            unit.to_le_bytes()
        } else {
            unit.to_be_bytes()
        });
    }
    bytes
}

#[test]
fn utf16_boms_decode_both_endiannesses_and_keep_physical_lines() {
    let f = Fixture::new("minecraft-java");
    for (little, expected) in [(true, "utf-16le"), (false, "utf-16be")] {
        let contents = "started\r\n\rMissing dependency \u{6a21}\u{7ec4}\r\nfailed to load example";
        let bytes = utf16(contents, little);
        write(&f.root.join("logs/latest.log"), &bytes);
        let result = f.diagnose(&ReadOptions {
            legacy_log_encoding: Some(LegacyLogEncoding::Windows1252),
            ..Default::default()
        });
        let log = latest(&result.logs);
        assert_eq!(log.encoding.as_deref(), Some(expected));
        assert_eq!(log.bytes_read, bytes.len());
        assert_eq!(log.lines[1].text, "");
        assert_eq!(log.lines[2].text, "Missing dependency \u{6a21}\u{7ec4}");
        assert_eq!(
            result.evidence.iter().map(|e| e.line).collect::<Vec<_>>(),
            [Some(3), Some(4)]
        );
    }
}

#[test]
fn legacy_log_encodings_are_explicit_strict_fallbacks() {
    let f = Fixture::new("minecraft-java");
    for (legacy, name, suffix) in [
        (LegacyLogEncoding::Windows1252, "windows-1252", "\u{e9}"),
        (LegacyLogEncoding::Windows1251, "windows-1251", "\u{414}"),
        (LegacyLogEncoding::Gbk, "gbk", "\u{6a21}\u{7ec4}"),
        (LegacyLogEncoding::Big5, "big5", "\u{4f86}\u{6e90}"),
        (
            LegacyLogEncoding::ShiftJis,
            "shift_jis",
            "\u{30c6}\u{30b9}\u{30c8}",
        ),
    ] {
        let raw = format!("ok\nmissing dependency {suffix}\n");
        let (encoded, _, had_errors) = legacy.encoding().encode(&raw);
        assert!(!had_errors);
        write(&f.root.join("logs/latest.log"), encoded.as_ref());
        assert_eq!(
            latest(&f.read(&ReadOptions::default())).status,
            ReadStatus::UnknownEncoding
        );
        let options = ReadOptions {
            legacy_log_encoding: Some(legacy),
            ..Default::default()
        };
        let result = f.diagnose(&options);
        let file = latest(&result.logs);
        assert_eq!(file.encoding.as_deref(), Some(name));
        assert_eq!(result.evidence[0].line, Some(2));
        assert_eq!(
            result.evidence[0].text,
            format!("missing dependency {suffix}")
        );
        write(&f.root.join("logs/latest.log"), &raw);
        let result = f.read(&options);
        assert_eq!(latest(&result).encoding.as_deref(), Some("utf-8"));
        assert_eq!(
            latest(&result).lines[1].text,
            format!("missing dependency {suffix}")
        );
    }
}

#[test]
fn malformed_bom_and_legacy_files_are_not_retried_with_lossy_decoders() {
    let f = Fixture::new("minecraft-java");
    for (bytes, encoding) in [
        (
            &b"\xef\xbb\xbfmissing dependency \xff"[..],
            LegacyLogEncoding::Windows1252,
        ),
        (&b"\xff\xfe\x00\xd8"[..], LegacyLogEncoding::Windows1252),
        (&b"missing dependency \x81"[..], LegacyLogEncoding::Gbk),
    ] {
        write(&f.root.join("logs/latest.log"), bytes);
        let result = f.diagnose(&ReadOptions {
            legacy_log_encoding: Some(encoding),
            ..Default::default()
        });
        assert_eq!(latest(&result.logs).status, ReadStatus::UnknownEncoding);
        assert!(result.evidence.is_empty());
    }
    assert!(serde_json::from_value::<ReadOptions>(json!({"legacyLogEncoding":"utf-7"})).is_err());
}

#[test]
fn byte_budget_can_cut_utf16_and_legacy_characters_without_partial_evidence() {
    let f = Fixture::new("minecraft-java");
    let first = "failed to load core \u{6a21}\u{7ec4}\r\n";
    let next = "missing dependency ";
    let raw = format!("{first}{next}\u{6a21}\u{7ec4}\n");
    let gbk = encoding_rs::GBK.encode(&raw).0;
    for (bytes, max, legacy, label) in [
        (
            utf16(&raw, true),
            2 + (first.encode_utf16().count() + next.len()) * 2 + 1,
            None,
            "utf-16le",
        ),
        (
            gbk.into_owned(),
            encoding_rs::GBK.encode(first).0.len() + next.len() + 1,
            Some(LegacyLogEncoding::Gbk),
            "gbk",
        ),
    ] {
        write(&f.root.join("logs/latest.log"), bytes);
        let result = f.diagnose(&ReadOptions {
            max_bytes_per_file: max,
            legacy_log_encoding: legacy,
            ..Default::default()
        });
        let file = latest(&result.logs);
        assert_eq!(file.encoding.as_deref(), Some(label));
        assert_eq!(file.bytes_read, max);
        assert!(file.truncated);
        assert_eq!(result.evidence.len(), 1);
        assert_eq!(result.evidence[0].line, Some(1));
        assert_eq!(result.evidence[0].text, first.trim_end());
    }
}

#[test]
fn huge_logs_file_count_and_aggregate_byte_budget_are_bounded() {
    let f = Fixture::new("minecraft-java");
    write(&f.root.join("logs/latest.log"), "one\n".repeat(300_000));
    write(&f.root.join("crash-reports/a.txt"), "two\n".repeat(30));
    write(&f.root.join("crash-reports/b.log"), "three\n");
    let report = f.read(&ReadOptions {
        max_files: 3,
        max_bytes_per_file: 5,
        max_total_bytes: 7,
        ..Default::default()
    });
    assert_eq!(report.files.len(), 3);
    assert_eq!(report.bytes_read, 7);
    assert_eq!(report.files.iter().map(|p| p.bytes_read).sum::<usize>(), 7);
    assert!(report.files.iter().all(|p| p.bytes_read <= 5));
    assert_eq!(report.files[2].status, ReadStatus::LimitReached);
    assert!(report.truncated);
    assert_eq!(
        f.read(&ReadOptions {
            max_files: 1,
            ..Default::default()
        })
        .files
        .len(),
        1
    );
}

#[test]
fn log_directory_scan_is_shallow_with_an_entry_budget() {
    let f = Fixture::new("minecraft-java");
    for i in 0..10 {
        write(&f.root.join(format!("crash-reports/{i}.txt")), "hello\n");
    }
    write(
        &f.root.join("crash-reports/nested/deep.txt"),
        "missing dependency hidden\n",
    );
    write(&f.root.join("crash-reports/not-a-log.json"), "private data");
    let report = f.read(&ReadOptions {
        max_directory_entries: 2,
        ..Default::default()
    });
    assert!(report.truncated);
    assert!(report.files.len() <= 3);
    for report in [report, f.read(&ReadOptions::default())] {
        assert!(!report
            .files
            .iter()
            .any(|p| p.file.ends_with("deep.txt") || p.file.ends_with(".json")));
    }
}

#[cfg(unix)]
#[test]
fn symlink_files_and_directories_are_denied_even_for_in_root_aliases() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new("minecraft-java");
    let outside = f.base.join("outside");
    write(&outside.join("secret.log"), "missing dependency secret\n");
    write(&f.root.join("internal.log"), "missing dependency alias\n");
    fs::create_dir(f.root.join("logs")).unwrap();
    for target in [outside.join("secret.log"), f.root.join("internal.log")] {
        symlink(target, f.root.join("logs/latest.log")).unwrap();
        let result = f.diagnose(&ReadOptions::default());
        assert!(result
            .logs
            .issues
            .iter()
            .any(|i| i.path == "logs/latest.log"));
        assert!(result.evidence.is_empty());
        assert_eq!(result.logs.bytes_read, 0);
        fs::remove_file(f.root.join("logs/latest.log")).unwrap();
    }
    symlink(outside, f.root.join("crash-reports")).unwrap();
    let report = f.read(&ReadOptions::default());
    assert!(report.issues.iter().any(|i| i.path == "crash-reports"));
    assert_eq!(report.bytes_read, 0);
}

#[cfg(unix)]
#[test]
fn special_files_and_replaced_registered_roots_are_not_opened() {
    use std::os::unix::{fs::symlink, net::UnixListener};
    let f = Fixture::new("minecraft-java");
    fs::create_dir(f.root.join("logs")).unwrap();
    let _socket = UnixListener::bind(f.root.join("logs/latest.log")).unwrap();
    let report = f.read(&ReadOptions::default());
    assert_eq!(latest(&report).status, ReadStatus::Denied);
    assert_eq!(report.bytes_read, 0);
    let moved = f.base.join("moved-game");
    fs::rename(&f.root, &moved).unwrap();
    symlink(moved, &f.root).unwrap();
    assert!(matches!(f.read_error(), AppError::Denied(_)));
}

#[test]
fn noncanonical_database_roots_are_rejected() {
    let f = Fixture::new("minecraft-java");
    fs::create_dir(f.root.join("child")).unwrap();
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE games SET root_path=?1 WHERE id=?2",
            params![f.root.join("child/..").to_str().unwrap(), f.game.id],
        )
        .unwrap();
    assert!(matches!(f.read_error(), AppError::Denied(_)));
}

#[test]
fn metadata_never_authorizes_user_root_reads_and_missing_logs_are_unknown() {
    let f = Fixture::new("skyrim-se");
    let fake = f.base.join("fake-profile");
    write(&fake.join("SKSE/skse64.log"), "failed to load secret\n");
    f.engine.db.conn().execute("UPDATE games SET metadata_json=?1 WHERE id=?2", params![json!({"userRoots":{"documents-profile":fake},"registeredUserRoots":{"local-profile":fake}}).to_string(),f.game.id]).unwrap();
    let result = f.diagnose(&ReadOptions::default());
    assert_eq!(result.logs.issues.len(), 2);
    assert!(result.logs.files.is_empty());
    assert_eq!(result.status, "unknown");
    assert!(result.causes.iter().all(|c| c.kind == CauseKind::Unknown));
    let real = f.user("documents-profile");
    write(&real.join("SKSE/skse64.log"), "failed to load real\n");
    let report = f.read(&ReadOptions::default());
    assert!(report
        .files
        .iter()
        .any(|p| p.file.starts_with(real.to_str().unwrap()) && p.status == ReadStatus::Read));
    assert!(!report
        .files
        .iter()
        .any(|p| p.file.starts_with(fake.to_str().unwrap())));
}

#[test]
fn bethesda_order_reports_lines_duplicates_missing_files_and_unsafe_names() {
    let f = Fixture::new("skyrim-se");
    let profile = f.user("local-profile");
    fs::create_dir(f.root.join("Data/folder.esp")).unwrap();
    write(&profile.join("Plugins.txt"), "\u{feff}# header\r\n*Skyrim.esm\r\n*Missing.esp\r\nDisabled.esp\r\n*Missing.esp\r\n*../outside.esp\r\n*C:\\outside.esm\r\n*wrong.txt\r\n*folder.esp\n");
    let result = f.diagnose(&ReadOptions::default());
    assert!(result
        .causes
        .iter()
        .any(|c| c.kind == CauseKind::LoadOrder && c.basis == Basis::Observed));
    assert!(result
        .evidence
        .iter()
        .any(|e| e.line == Some(3) && e.source == "load_order"));
    assert!(!result.evidence.iter().any(|e| e.line == Some(4)));
    for line in [5, 6, 7, 8, 9] {
        assert!(result.evidence.iter().any(|e| e.line == Some(line)));
    }
    let duplicate = result
        .causes
        .iter()
        .find(|c| c.summary.contains("重复"))
        .unwrap();
    assert!(duplicate.evidence_ids.iter().all(|id| result
        .evidence
        .iter()
        .any(|e| e.id == *id && e.line == Some(5))));
    assert!(result.unknowns.iter().any(|u| u.contains("VFS")));
}

#[test]
fn bethesda_plugin_headers_report_only_bounded_missing_masters() {
    let f = Fixture::new("skyrim-se");
    let profile = f.user("local-profile");
    write_bethesda_plugin(&f.root.join("Data/Skyrim.esm"), &[]);
    write_bethesda_plugin(
        &f.root.join("Data/NeedsMaster.esp"),
        &["Skyrim.esm", "MissingMaster.esm"],
    );
    write_bethesda_plugin(&f.root.join("Data/UnlistedMaster.esp"), &["Provided.esm"]);
    write_bethesda_plugin(&f.root.join("Data/Provided.esm"), &[]);
    write(
        &profile.join("Plugins.txt"),
        "*Skyrim.esm\n*NeedsMaster.esp\n*UnlistedMaster.esp\n",
    );

    let result = f.diagnose(&ReadOptions::default());
    let json = serde_json::to_value(&result).unwrap();
    let report = &json["bethesdaPlugins"];
    assert_eq!(report["status"], "partial");
    assert_eq!(report["plugins"].as_array().unwrap().len(), 3);
    let needs = report["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "NeedsMaster.esp")
        .unwrap();
    assert_eq!(needs["headerStatus"], "parsed");
    assert_eq!(needs["missingMasters"], json!(["MissingMaster.esm"]));
    assert_eq!(needs["masters"], json!(["Skyrim.esm", "MissingMaster.esm"]));
    assert!(result
        .causes
        .iter()
        .any(|cause| { cause.kind == CauseKind::Dependency && cause.basis == Basis::Observed }));
    let finding = result
        .evidence
        .iter()
        .find(|e| e.source == "bethesda_plugin_header")
        .unwrap();
    assert_eq!(finding.line, Some(2));
    assert!(finding.text.contains("MissingMaster.esm"));
    assert!(result.unknowns.iter().any(|note| note.contains("MO2/VFS")));
    assert_eq!(fs::read(f.root.join("Data/NeedsMaster.esp")).unwrap(), {
        let mut data = Vec::new();
        let mut subrecord = |code: &[u8; 4], payload: &[u8]| {
            data.extend_from_slice(code);
            data.extend_from_slice(&(payload.len() as u16).to_le_bytes());
            data.extend_from_slice(payload);
        };
        subrecord(b"HEDR", &[0; 12]);
        for master in ["Skyrim.esm", "MissingMaster.esm"] {
            let mut payload = master.as_bytes().to_vec();
            payload.push(0);
            subrecord(b"MAST", &payload);
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"TES4");
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&[0; 16]);
        bytes.extend_from_slice(&data);
        bytes
    });
}

#[test]
fn bethesda_plugin_headers_report_disabled_and_late_masters() {
    let f = Fixture::new("skyrim-se");
    let profile = f.user("local-profile");
    for name in [
        "Skyrim.esm",
        "Late.esm",
        "Disabled.esm",
        "NeedsLate.esp",
        "NeedsDisabled.esp",
    ] {
        write_bethesda_plugin(&f.root.join(format!("Data/{name}")), &[]);
    }
    write_bethesda_plugin(&f.root.join("Data/NeedsLate.esp"), &["Late.esm"]);
    write_bethesda_plugin(&f.root.join("Data/NeedsDisabled.esp"), &["Disabled.esm"]);
    write(
        &profile.join("Plugins.txt"),
        "*NeedsLate.esp\nDisabled.esm\n*NeedsDisabled.esp\n*Skyrim.esm\n*Late.esm\n",
    );

    let result = f.diagnose(&ReadOptions::default());
    let report = serde_json::to_value(&result).unwrap();
    let plugins = report["bethesdaPlugins"]["plugins"].as_array().unwrap();
    let late = plugins
        .iter()
        .find(|row| row["name"] == "NeedsLate.esp")
        .unwrap();
    assert_eq!(late["loadOrder"], 1);
    assert_eq!(late["mastersAfterPlugin"], json!(["Late.esm"]));
    let disabled = plugins
        .iter()
        .find(|row| row["name"] == "NeedsDisabled.esp")
        .unwrap();
    assert_eq!(disabled["loadOrder"], 2);
    assert_eq!(disabled["disabledMasters"], json!(["Disabled.esm"]));
    assert!(result
        .causes
        .iter()
        .any(|cause| cause.kind == CauseKind::LoadOrder && cause.basis == Basis::Observed));
    assert!(result
        .causes
        .iter()
        .any(|cause| cause.kind == CauseKind::Dependency && cause.basis == Basis::Observed));
    assert!(result
        .evidence
        .iter()
        .any(|e| e.source == "bethesda_plugin_order" && e.line == Some(1)));
    assert!(result
        .evidence
        .iter()
        .any(|e| e.source == "bethesda_plugin_order" && e.line == Some(3)));
}

#[test]
fn bethesda_plugin_header_failures_are_unknown_not_dependency_claims() {
    let f = Fixture::new("fallout4");
    let profile = f.user("local-profile");
    write(&f.root.join("Data/Fallout4.esm"), b"not-a-tes4-plugin");
    write(&f.root.join("Data/Truncated.esp"), b"TES4\x01\x00");
    write(
        &profile.join("Plugins.txt"),
        "*Fallout4.esm\n*Truncated.esp\n",
    );
    let result = f.diagnose(&ReadOptions::default());
    let report = serde_json::to_value(&result).unwrap();
    let plugins = report["bethesdaPlugins"]["plugins"].as_array().unwrap();
    assert!(plugins.iter().all(|row| row["headerStatus"] == "invalid"));
    assert!(!result
        .evidence
        .iter()
        .any(|e| e.source == "bethesda_plugin_header"));
    assert!(result.unknowns.iter().any(|note| note.contains("TES4")));
    assert_eq!(
        fs::read(f.root.join("Data/Truncated.esp")).unwrap(),
        b"TES4\x01\x00"
    );
}

#[test]
fn load_order_has_priority_over_logs_for_the_total_read_budget() {
    let f = Fixture::new("fallout4");
    let profile = f.base.join("z-order");
    let documents = f.base.join("a-documents");
    write(&profile.join("Plugins.txt"), "*Fallout4.esm\n");
    write(&documents.join("F4SE/f4se.log"), "failed to load noise\n");
    game_roots::register_user_root(&f.engine, &f.game.id, "local-profile", &profile).unwrap();
    game_roots::register_user_root(&f.engine, &f.game.id, "documents-profile", &documents).unwrap();
    let budget = b"*Fallout4.esm\n".len();
    let report = f.read(&ReadOptions {
        max_total_bytes: budget,
        ..Default::default()
    });
    assert_eq!(report.files[0].kind, FileKind::LoadOrder);
    assert_eq!(report.files[0].status, ReadStatus::Read);
    assert_eq!(report.files[1].status, ReadStatus::LimitReached);
    assert_eq!(report.bytes_read, budget);
}

#[test]
fn bethesda_legacy_plugin_order_uses_format_codepage_not_log_override() {
    let f = Fixture::new("skyrim-se");
    let profile = f.user("local-profile");
    let path = profile.join("Plugins.txt");
    write(&f.root.join("Data/Caf\u{e9}.esp"), "present");
    let bytes = b"# profile\r\n*Caf\xe9.esp\r\n*\xc9tranger.esl\r\n";
    write(&path, bytes);
    let result = f.diagnose(&ReadOptions {
        legacy_log_encoding: Some(LegacyLogEncoding::ShiftJis),
        ..Default::default()
    });
    let order = result
        .logs
        .files
        .iter()
        .find(|f| f.kind == FileKind::LoadOrder)
        .unwrap();
    assert_eq!(order.encoding.as_deref(), Some("windows-1252"));
    assert_eq!(order.lines[1].text, "*Caf\u{e9}.esp");
    let evidence: Vec<_> = result
        .evidence
        .iter()
        .filter(|e| e.source == "load_order")
        .collect();
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].line, Some(3));
    assert_eq!(evidence[0].text, "*\u{c9}tranger.esl");
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn bg3_uses_explicit_user_profile_and_restricts_launcher_cache_log_names() {
    let f = Fixture::new("baldurs-gate-3");
    write(
        &f.root.join("PlayerProfiles/Public/modsettings.lsx"),
        "game-root decoy\n",
    );
    assert!(f.read(&ReadOptions::default()).files.is_empty());
    let data = f.user("profile");
    write(
        &data.join("PlayerProfiles/Public/modsettings.lsx"),
        "<save>\n  <region id=\"ModuleSettings\"/>\n</save>\n",
    );
    let cache = f.user("launcher-cache");
    write(&cache.join("AlteredFiles.txt"), "missing file fixture\n");
    write(
        &cache.join("private-network.txt"),
        "private network payload",
    );
    let result = f.diagnose(&ReadOptions::default());
    assert!(result
        .logs
        .files
        .iter()
        .any(|p| p.file.starts_with(data.to_str().unwrap()) && p.kind == FileKind::LoadOrder));
    assert!(!result
        .logs
        .files
        .iter()
        .any(|p| p.file.starts_with(f.root.to_str().unwrap())
            || p.file.ends_with("private-network.txt")));
    assert!(result.unknowns.iter().any(|u| u.contains("UUID/LSX")));
    assert_eq!(
        result
            .logs
            .files
            .iter()
            .find(|p| p.kind == FileKind::LoadOrder)
            .unwrap()
            .lines[1]
            .number,
        2
    );
}

#[test]
fn bg3_utf16_order_inconsistency_reports_the_physical_line_but_never_edits_the_profile() {
    const XML: &str = r#"<?xml version="1.0" encoding="UTF-16"?>
<save>
  <region id="ModuleSettings">
    <node id="root"><children>
      <node id="Mods"><children>
        <node id="ModuleShortDesc">
          <attribute id="UUID" type="FixedString" value="11111111-aaaa-bbbb-cccc-111111111111"/>
        </node>
      </children></node>
      <node id="ModOrder"><children>
        <node id="Module">
          <attribute id="UUID" type="FixedString" value="22222222-aaaa-bbbb-cccc-222222222222"/>
        </node>
      </children></node>
    </children></node>
  </region>
</save>"#;
    let f = Fixture::new("baldurs-gate-3");
    let profile = f.user("profile");
    let path = profile.join("PlayerProfiles/Public/modsettings.lsx");
    let bytes = utf16(&XML.replace('\n', "\r"), false);
    write(&path, &bytes);
    let result = f.diagnose(&ReadOptions::default());
    assert!(result
        .causes
        .iter()
        .any(|c| c.kind == CauseKind::LoadOrder && c.basis == Basis::Observed));
    let finding = result
        .evidence
        .iter()
        .find(|e| e.source == "load_order")
        .unwrap();
    assert_eq!(finding.file, path.display().to_string());
    assert_eq!(finding.line, Some(12));
    assert!(finding
        .text
        .contains("22222222-aaaa-bbbb-cccc-222222222222"));
    assert!(result
        .repair_suggestions
        .iter()
        .all(|s| s.file_ops_input.is_none()));
    assert_eq!(fs::read(&path).unwrap(), bytes);

    let clipped = f.diagnose(&ReadOptions {
        max_lines_per_file: 14,
        ..Default::default()
    });
    assert!(clipped.logs.truncated);
    assert!(clipped
        .unknowns
        .iter()
        .any(|note| note.contains("不完整 XML")));
    assert!(clipped.evidence.iter().all(|e| e.source != "load_order"));
}

#[test]
fn common_patterns_and_success_negations_are_only_heuristic() {
    assert_eq!(
        pattern(
            "requires mods which aren't installed (SpaceCore)",
            "stardew-valley"
        )
        .unwrap()
        .0,
        CauseKind::Dependency
    );
    assert_eq!(
        pattern("unsupported runtime version", "cyberpunk-2077")
            .unwrap()
            .0,
        CauseKind::Version
    );
    assert_eq!(
        pattern("could not load: FileNotFoundException", "skyrim-se")
            .unwrap()
            .0,
        CauseKind::MissingFile
    );
    for line in [
        "No missing dependencies",
        "0 errors; all dependencies satisfied",
        "no version mismatch",
        "Example depends on Core",
        "loaded Example successfully",
    ] {
        assert!(pattern(line, "minecraft-java").is_none(), "{line}");
    }
}

#[test]
fn benign_log_returns_unknown_not_a_health_or_repair_success_claim() {
    let f = Fixture::new("minecraft-java");
    write(
        &f.root.join("logs/latest.log"),
        "loaded successfully; no errors\n",
    );
    let result = f.diagnose(&ReadOptions::default());
    assert_eq!(result.status, "unknown");
    assert_eq!(result.causes.len(), 1);
    assert_eq!(result.causes[0].basis, Basis::Unknown);
    assert!(result.evidence.is_empty());
    assert!(result.repair_suggestions.is_empty());
}

#[test]
fn missing_required_marker_is_observed_not_a_verified_root_cause() {
    let f = Fixture::new("stardew-valley");
    fs::remove_file(f.root.join("Stardew Valley.dll")).unwrap();
    let result = f.diagnose(&ReadOptions::default());
    assert!(result
        .causes
        .iter()
        .any(|c| c.kind == CauseKind::MissingFile && c.basis == Basis::Observed));
    assert!(result
        .evidence
        .iter()
        .any(|e| e.file.ends_with("Stardew Valley.dll")
            && e.line.is_none()
            && e.source == "filesystem"));
    assert_eq!(result.status, "findings");
}

#[test]
fn installed_file_objects_and_legacy_paths_are_checked_without_dependency_claims() {
    let f = Fixture::new("minecraft-java");
    write(
        &f.root.join("mods/present.jar"),
        "not executed or opened as archive",
    );
    let files = json!([
        {"path":f.root.join("mods/missing.jar"),"sha256":"not-verified","size":10,"before":null},
        f.root.join("mods/legacy.jar"), {"path":f.root.join("mods/present.jar"),"sha256":"also-not-verified","size":0}
    ]);
    f.install(
        "active",
        "enabled",
        &files.to_string(),
        "[{\"id\":\"Core\",\"version\":\">=2\"}]",
    );
    f.install(
        "disabled",
        "disabled",
        &json!([f.root.join("mods/disabled.jar")]).to_string(),
        "[]",
    );
    let result = f.diagnose(&ReadOptions::default());
    let evidence: Vec<_> = result
        .evidence
        .iter()
        .filter(|e| e.source == "install_record")
        .collect();
    assert_eq!(evidence.len(), 2);
    assert!(evidence
        .iter()
        .all(|e| e.line.is_none() && !e.file.ends_with("disabled.jar")));
    assert!(result.unknowns.iter().any(|u| u.contains("未解算")));
    assert!(!result
        .causes
        .iter()
        .any(|c| c.kind == CauseKind::Dependency));
}

#[test]
fn diagnosis_reads_current_install_journal_without_updating_cached_status() {
    let f = Fixture::new("minecraft-java");
    for (id, state, transaction_state) in [
        ("applied", "planned", "committed"),
        ("undone", "enabled", "rolled_back"),
    ] {
        let files = json!([f.root.join(format!("mods/{id}.jar"))]);
        f.install(id, state, &files.to_string(), "[]");
        let txn = format!("txn-{id}");
        f.transaction(&txn, transaction_state, &f.game.id);
        f.engine.db.conn().execute(
            "INSERT INTO install_transactions(install_id,transaction_id,action,created_at) VALUES(?1,?2,'install','now')",
            params![id,txn],
        ).unwrap();
    }
    let result = f.diagnose(&ReadOptions::default());
    let observations: Vec<_> = result
        .evidence
        .iter()
        .filter(|e| e.source == "install_record")
        .collect();
    assert_eq!(observations.len(), 1);
    assert!(observations[0].file.ends_with("applied.jar"));
    let stored: String = f
        .engine
        .db
        .conn()
        .query_row(
            "SELECT status FROM installs WHERE id='applied'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, "planned");
}

#[test]
fn records_cannot_authorize_relative_outside_manager_or_other_game_paths() {
    let f = Fixture::new("minecraft-java");
    let other = f.base.join("other-game");
    write(&other.join("options.txt"), "fixture");
    games::register_game(&f.engine.db, &other, Some("minecraft-java")).unwrap();
    let files = json!([
        "../../secret.txt",
        f.base.join("outside-missing.jar"),
        f.engine.paths.root.join("manager-missing.jar"),
        other.join("mods/other-missing.jar"),
        f.root.join("mods/../missing.jar")
    ]);
    f.install("malicious", "enabled", &files.to_string(), "[]");
    let result = f.diagnose(&ReadOptions::default());
    assert!(!result.evidence.iter().any(|e| e.source == "install_record"));
    assert!(
        result
            .unknowns
            .iter()
            .filter(|u| u.contains("越权/未知路径"))
            .count()
            >= 5
    );
}

#[test]
fn malformed_multibyte_oversized_records_and_unknown_output_are_bounded() {
    let f = Fixture::new("minecraft-java");
    f.install("a-malformed", "enabled", "{}", "[]");
    f.install(
        "b-oversized",
        "enabled",
        &json!(["中".repeat(100_000)]).to_string(),
        "[]",
    );
    let files: Vec<_> = (0..600)
        .map(|i| format!("../../not-authorized-{i}.jar"))
        .collect();
    f.install(
        "c-many-files",
        "enabled",
        &serde_json::to_string(&files).unwrap(),
        "[]",
    );
    let result = f.diagnose(&ReadOptions::default());
    assert!(result.unknowns.iter().any(|u| u.contains("不是可解释数组")));
    assert!(result.unknowns.iter().any(|u| u.contains("文件记录过大")));
    assert!(result.unknowns.len() <= 64);
    assert!(result.unknowns.iter().all(|u| u.chars().count() <= 2048));
    assert!(result.logs.truncated);
    assert!(result.evidence.is_empty());
}

#[test]
fn install_row_and_transaction_observation_limits_are_reported() {
    let f = Fixture::new("minecraft-java");
    for i in 0..130 {
        f.install(&format!("mod-{i:03}"), "enabled", "[]", "[]");
    }
    for i in 0..18 {
        f.transaction(&format!("failed-{i}"), "failed", &f.game.id);
    }
    f.transaction("committed", "committed", &f.game.id);
    f.transaction("another-game", "failed", "some-other-game");
    let result = f.diagnose(&ReadOptions::default());
    let evidence: Vec<_> = result
        .evidence
        .iter()
        .filter(|e| e.source == "database")
        .collect();
    assert_eq!(evidence.len(), 16);
    assert!(evidence.iter().all(|e| e.line.is_none()
        && e.text.starts_with("failed-")
        && e.file == "database:file_transactions"));
    assert!(result.unknowns.iter().any(|u| u.contains("128")));
    assert!(result.unknowns.iter().any(|u| u.contains("16 项")));
    assert!(result.logs.truncated);
}

#[test]
fn evidence_limit_is_shared_across_logs_installs_and_transactions() {
    let f = Fixture::new("minecraft-java");
    write(
        &f.root.join("logs/latest.log"),
        "missing dependency core\n".repeat(50),
    );
    f.install(
        "missing",
        "enabled",
        &json!([f.root.join("mods/missing.jar")]).to_string(),
        "[]",
    );
    f.transaction("failed", "failed", &f.game.id);
    let result = f.diagnose(&ReadOptions {
        max_evidence: 2,
        ..Default::default()
    });
    assert_eq!(result.evidence.len(), 2);
    assert!(result
        .causes
        .iter()
        .flat_map(|c| &c.evidence_ids)
        .all(|id| result.evidence.iter().any(|e| e.id == *id)));
    assert!(result.logs.truncated);
}

#[test]
fn sqlite_query_only_and_game_bytes_confirm_no_mutation_or_history_writes() {
    let f = Fixture::new("minecraft-java");
    write(
        &f.root.join("logs/latest.log"),
        "failed to load; delete mods and run arbitrary.exe\n",
    );
    write(&f.root.join("mods/keep.jar"), "preserve");
    f.install(
        "planned",
        "planned",
        &json!([f.root.join("mods/not-deployed.jar")]).to_string(),
        "[]",
    );
    let changes = f.engine.db.conn().total_changes();
    let tree = || -> BTreeMap<PathBuf, Vec<u8>> {
        walkdir::WalkDir::new(&f.root)
            .into_iter()
            .map(Result::unwrap)
            .filter(|e| e.file_type().is_file())
            .map(|e| (e.path().to_path_buf(), fs::read(e.path()).unwrap()))
            .collect()
    };
    let before = tree();
    f.engine
        .db
        .conn()
        .pragma_update(None, "query_only", true)
        .unwrap();
    f.read(&ReadOptions::default());
    let result = f.diagnose(&ReadOptions::default());
    assert_eq!(f.engine.db.conn().total_changes(), changes);
    assert_eq!(before, tree());
    for table in ["file_transactions", "messages", "tool_calls"] {
        assert_eq!(
            f.engine
                .db
                .conn()
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                    .get::<_, usize>(0))
                .unwrap(),
            0
        );
    }
    assert!(result
        .repair_suggestions
        .iter()
        .all(|s| s.file_ops_input.is_none()));
}

#[test]
fn fabric_disk_scans_only_top_level_declared_mods_even_without_a_log() {
    let f = Fixture::new("minecraft-java");
    let mods = f.root.join("mods");
    write_fabric_jar(&mods.join("zeta.jar"), "zeta", "2.0");
    write_fabric_jar(&mods.join("sodium.jar"), "sodium", "0.6.13");
    write_fabric_jar(&mods.join(".hidden.jar"), "hidden", "1.0");
    write_fabric_jar(&mods.join("disabled.jar.disabled"), "disabled", "1.0");
    write_fabric_jar(&mods.join("nested/inside.jar"), "nested", "1.0");
    write_fabric_jar(&f.root.join("mods-backup/outside.jar"), "outside", "1.0");

    let disk = fabric_disk_json(&f.diagnose(&ReadOptions::default()));
    assert_eq!(disk["directory"], "mods/");
    assert_eq!(disk["status"], "scanned");
    let jars = disk["jars"].as_array().unwrap();
    assert_eq!(
        jars.iter()
            .map(|jar| jar["file"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["sodium.jar", "zeta.jar"]
    );
    assert_eq!(jars[0]["metadataStatus"], "present");
    assert_eq!(jars[0]["id"], "sodium");
    assert_eq!(jars[0]["version"], "0.6.13");
    assert_eq!(jars[0]["ownership"], "unrecorded");
    assert_eq!(jars[0]["logRelation"]["status"], "unverified");

    fs::remove_dir_all(&mods).unwrap();
    let missing = fabric_disk_json(&f.diagnose(&ReadOptions::default()));
    assert_eq!(missing["directory"], "mods/");
    assert_eq!(missing["status"], "missing");
    assert!(missing["jars"].as_array().unwrap().is_empty());

    let other = Fixture::new("skyrim-se");
    assert!(
        serde_json::to_value(other.diagnose(&ReadOptions::default()))
            .unwrap()
            .get("fabricDisk")
            .is_none()
    );
}

#[test]
fn fabric_disk_metadata_states_are_explicit_and_rows_are_sorted() {
    let f = Fixture::new("minecraft-java");
    let mods = f.root.join("mods");
    write_fabric_jar(&mods.join("a-present.jar"), "alpha", "1.2.3");
    write_jar(&mods.join("b-absent.jar"), None);
    write_jar(&mods.join("c-invalid.jar"), Some(b"{not-json"));
    let oversized = format!(
        r#"{{"schemaVersion":1,"id":"large","version":"1","padding":"{}"}}"#,
        "x".repeat(128 * 1024)
    );
    write_jar(&mods.join("d-limited.jar"), Some(oversized.as_bytes()));
    let disk = fabric_disk_json(&f.diagnose(&ReadOptions::default()));
    assert_eq!(disk["status"], "partial");
    let jars = disk["jars"].as_array().unwrap();
    assert_eq!(jars.len(), 4);
    for (jar, name, status) in [
        (&jars[0], "a-present.jar", "present"),
        (&jars[1], "b-absent.jar", "absent"),
        (&jars[2], "c-invalid.jar", "invalid"),
        (&jars[3], "d-limited.jar", "limited"),
    ] {
        assert_eq!(jar["file"], name);
        assert_eq!(jar["metadataStatus"], status);
    }
    assert_eq!(jars[0]["id"], "alpha");
    assert_eq!(jars[0]["version"], "1.2.3");
    for jar in &jars[1..] {
        assert!(jar["id"].is_null());
        assert!(jar["version"].is_null());
    }
}

#[test]
fn fabric_disk_relates_only_complete_historical_selections_with_exact_versions() {
    let f = Fixture::new("minecraft-java");
    let mods = f.root.join("mods");
    write_fabric_jar(&mods.join("sodium.jar"), "sodium", "0.6.13");
    write_fabric_jar(&mods.join("orphan.jar"), "orphan", "1.0");

    write_fabric_selection(
        &f.root,
        &[
            ("minecraft", "1.21.1"),
            ("fabricloader", "0.16.14"),
            ("sodium", "0.6.13"),
        ],
    );
    let disk = fabric_disk_json(&f.diagnose(&ReadOptions::default()));
    let jars = disk["jars"].as_array().unwrap();
    assert_eq!(jars[0]["file"], "orphan.jar");
    assert_eq!(jars[0]["logRelation"]["status"], "not_listed");
    assert_eq!(jars[1]["file"], "sodium.jar");
    assert_eq!(jars[1]["logRelation"]["status"], "exact");
    assert_eq!(jars[1]["logRelation"]["line"], 5);
    assert_eq!(jars[1]["logRelation"]["version"], "0.6.13");
    assert!(jars
        .iter()
        .all(|jar| { jar["file"] != "minecraft.jar" && jar["file"] != "fabricloader.jar" }));
    assert!(jars.iter().all(|jar| {
        jar.get("active").is_none()
            && jar.get("loaded").is_none()
            && jar.get("activation").is_none()
    }));

    write_fabric_selection(&f.root, &[("sodium", "0.6.13+build.4")]);
    let differs = fabric_disk_json(&f.diagnose(&ReadOptions::default()));
    let sodium = differs["jars"]
        .as_array()
        .unwrap()
        .iter()
        .find(|jar| jar["file"] == "sodium.jar")
        .unwrap();
    assert_eq!(sodium["logRelation"]["status"], "version_differs");
    assert_eq!(sodium["logRelation"]["version"], "0.6.13+build.4");

    write(
        &f.root.join("logs/latest.log"),
        concat!(
            "[12:34:56] [main/INFO]: Loading Minecraft 1.21.1 with Fabric Loader 0.16.14\n",
            "[12:34:57] [main/INFO]: Loading 2 mods:\n",
            "\t- sodium 0.6.13\n",
        ),
    );
    let incomplete = fabric_disk_json(&f.diagnose(&ReadOptions::default()));
    let sodium = incomplete["jars"]
        .as_array()
        .unwrap()
        .iter()
        .find(|jar| jar["file"] == "sodium.jar")
        .unwrap();
    assert_eq!(sodium["logRelation"]["status"], "unverified");

    write(
        &f.root.join("logs/latest.log"),
        "[12:34:56] [main/INFO]: Loading Minecraft 1.21.1 with Fabric Loader 0.16.14\n",
    );
    let no_selection = fabric_disk_json(&f.diagnose(&ReadOptions::default()));
    let sodium = no_selection["jars"]
        .as_array()
        .unwrap()
        .iter()
        .find(|jar| jar["file"] == "sodium.jar")
        .unwrap();
    assert_eq!(sodium["logRelation"]["status"], "unverified");
}

#[test]
fn fabric_disk_duplicate_current_ids_and_omitted_history_stay_ambiguous_or_unverified() {
    let f = Fixture::new("minecraft-java");
    let mods = f.root.join("mods");
    write_fabric_jar(&mods.join("sodium-a.jar"), "sodium", "0.6.13");
    write_fabric_jar(&mods.join("sodium-b.jar"), "sodium", "0.6.14");
    write_fabric_selection(&f.root, &[("sodium", "0.6.13")]);
    let duplicate = fabric_disk_json(&f.diagnose(&ReadOptions::default()));
    for jar in duplicate["jars"].as_array().unwrap() {
        assert_eq!(jar["logRelation"]["status"], "ambiguous");
    }

    let g = Fixture::new("minecraft-java");
    write_fabric_jar(&g.root.join("mods/zzzz.jar"), "zzzz", "1.0");
    let mut rows: Vec<(String, String)> = (0..100)
        .map(|index| (format!("mod{index:03}"), "1.0".into()))
        .collect();
    rows.push(("zzzz".into(), "1.0".into()));
    let refs: Vec<_> = rows
        .iter()
        .map(|(id, version)| (id.as_str(), version.as_str()))
        .collect();
    write_fabric_selection(&g.root, &refs);
    let omitted = fabric_disk_json(&g.diagnose(&ReadOptions::default()));
    assert_eq!(omitted["jars"][0]["logRelation"]["status"], "unverified");
}

#[test]
fn fabric_disk_ownership_uses_only_exact_enabled_same_game_paths() {
    let f = Fixture::new("minecraft-java");
    let jar_path = f.root.join("mods/sodium.jar");
    write_fabric_jar(&jar_path, "sodium", "0.6.13");
    write_fabric_selection(&f.root, &[("sodium", "0.6.13")]);

    let other_root = f.base.join("other-game");
    fs::create_dir(&other_root).unwrap();
    write(&other_root.join("options.txt"), "fixture");
    let other_game =
        games::register_game(&f.engine.db, &other_root, Some("minecraft-java")).unwrap();
    f.install_for(
        &other_game.id,
        "other-game-owner",
        "enabled",
        &installed_files_json(&jar_path, "same-path-other-game"),
        "[]",
    );
    f.install(
        "disabled-owner",
        "disabled",
        &installed_files_json(&jar_path, "disabled"),
        "[]",
    );
    let unrecorded = fabric_disk_json(&f.diagnose(&ReadOptions::default()));
    assert_eq!(unrecorded["jars"][0]["ownership"], "unrecorded");

    f.install(
        "enabled-owner",
        "enabled",
        &installed_files_json(&jar_path, "hash-does-not-match"),
        "[]",
    );
    let recorded = fabric_disk_json(&f.diagnose(&ReadOptions::default()));
    assert_eq!(recorded["jars"][0]["ownership"], "recorded");

    f.install(
        "second-enabled-owner",
        "enabled",
        &installed_files_json(&jar_path, "another-hash"),
        "[]",
    );
    let ambiguous = fabric_disk_json(&f.diagnose(&ReadOptions::default()));
    assert_eq!(ambiguous["jars"][0]["ownership"], "ambiguous");
}

#[test]
fn fabric_disk_invalid_or_oversized_install_manifests_suppress_unrecorded_claims() {
    for files in [
        "{not-json".to_string(),
        format!("[{}]", " ".repeat(128 * 1024)),
    ] {
        let f = Fixture::new("minecraft-java");
        write_fabric_jar(&f.root.join("mods/sodium.jar"), "sodium", "0.6.13");
        write_fabric_selection(&f.root, &[("sodium", "0.6.13")]);
        f.install("uncertain-owner", "enabled", &files, "[]");
        let disk = fabric_disk_json(&f.diagnose(&ReadOptions::default()));
        assert_eq!(disk["jars"][0]["ownership"], "unverified");
    }
}

#[test]
fn fabric_disk_scan_limits_are_reported_as_partial() {
    let f = Fixture::new("minecraft-java");
    let mods = f.root.join("mods");
    for index in 0..65 {
        write(&mods.join(format!("{index:03}.jar")), b"not a zip");
    }
    let disk = fabric_disk_json(&f.diagnose(&ReadOptions::default()));
    assert_eq!(disk["status"], "partial");
    assert_eq!(disk["jars"].as_array().unwrap().len(), 64);
    assert_eq!(disk["jars"][0]["file"], "000.jar");
    assert_eq!(disk["jars"][63]["file"], "063.jar");

    let g = Fixture::new("minecraft-java");
    let other_mods = g.root.join("mods");
    fs::create_dir_all(&other_mods).unwrap();
    for index in 0..513 {
        write(
            &other_mods.join(format!("entry-{index:03}.txt")),
            b"ignored",
        );
    }
    let limited = fabric_disk_json(&g.diagnose(&ReadOptions::default()));
    assert_eq!(limited["status"], "partial");
    assert!(limited["jars"].as_array().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn fabric_disk_never_reads_symlink_or_hardlink_jars_as_present() {
    use std::os::unix::fs::symlink;

    let f = Fixture::new("minecraft-java");
    let mods = f.root.join("mods");
    let external = f.base.join("external.jar");
    write_fabric_jar(&external, "external", "9.9");
    write_fabric_jar(&mods.join("ordinary.jar"), "ordinary", "1.0");
    symlink(&external, mods.join("symlink.jar")).unwrap();
    fs::hard_link(&external, mods.join("hardlink.jar")).unwrap();

    let disk = fabric_disk_json(&f.diagnose(&ReadOptions::default()));
    let jars = disk["jars"].as_array().unwrap();
    for jar in jars
        .iter()
        .filter(|jar| jar["file"] == "hardlink.jar" || jar["file"] == "symlink.jar")
    {
        assert_eq!(jar["metadataStatus"], "unreadable");
        assert!(jar["id"].is_null());
        assert!(jar["version"].is_null());
    }
    assert!(jars.iter().any(|jar| jar["file"] == "hardlink.jar"));
    assert!(jars.iter().any(|jar| jar["file"] == "symlink.jar"));
    assert!(jars
        .iter()
        .any(|jar| { jar["file"] == "ordinary.jar" && jar["metadataStatus"] == "present" }));
    assert_eq!(
        fs::read(&external).unwrap(),
        fs::read(mods.join("hardlink.jar")).unwrap()
    );

    let g = Fixture::new("minecraft-java");
    let outside = g.base.join("outside-mods");
    fs::create_dir_all(&outside).unwrap();
    write_fabric_jar(&outside.join("outside.jar"), "outside", "1.0");
    symlink(&outside, g.root.join("mods")).unwrap();
    let unavailable = fabric_disk_json(&g.diagnose(&ReadOptions::default()));
    assert_eq!(unavailable["status"], "unavailable");
    assert!(unavailable["jars"].as_array().unwrap().is_empty());
}

#[test]
fn fabric_disk_diagnosis_is_read_only() {
    let f = Fixture::new("minecraft-java");
    let jar = f.root.join("mods/sodium.jar");
    write_fabric_jar(&jar, "sodium", "0.6.13");
    let log = f.root.join("logs/latest.log");
    write_fabric_selection(&f.root, &[("sodium", "0.6.13")]);
    f.install(
        "read-only-sentinel",
        "enabled",
        &installed_files_json(&jar, "recorded-hash"),
        "[]",
    );
    let jar_before = fs::read(&jar).unwrap();
    let log_before = fs::read(&log).unwrap();
    let row_before: (String, String, String, String) = f
        .engine
        .db
        .conn()
        .query_row(
            "SELECT status,files_json,dependencies_json,updated_at FROM installs WHERE id='read-only-sentinel'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let changes = f.engine.db.conn().total_changes();
    f.engine
        .db
        .conn()
        .pragma_update(None, "query_only", true)
        .unwrap();

    let disk = fabric_disk_json(&f.diagnose(&ReadOptions::default()));
    assert_eq!(disk["jars"][0]["ownership"], "recorded");
    assert_eq!(fs::read(jar).unwrap(), jar_before);
    assert_eq!(fs::read(log).unwrap(), log_before);
    let row_after: (String, String, String, String) = f
        .engine
        .db
        .conn()
        .query_row(
            "SELECT status,files_json,dependencies_json,updated_at FROM installs WHERE id='read-only-sentinel'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(row_after, row_before);
    assert_eq!(f.engine.db.conn().total_changes(), changes);
}
