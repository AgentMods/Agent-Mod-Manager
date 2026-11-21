use super::{provider, Step};
use crate::{
    artifacts,
    download::DownloadArtifact,
    error::AppError,
    games::{self, Game},
    install::InstalledFile,
    install_dependencies::{self, GraphReviewRequest},
    plugins,
    sources::{ResolvedMod, Sources},
    transaction::FileEngine,
};
use serde_json::{json, Value};
use sha2::Digest;
use std::{path::Path, path::PathBuf, sync::Arc};

const ROOT_PROJECT: &str = "RootProj";
const ROOT_VERSION: &str = "Root0001";
const GAME_VERSION: &str = "1.21.1";
const LOADER: &str = "fabric";

fn fixture() -> (tempfile::TempDir, FileEngine, Game) {
    let (temp, engine, root) = crate::transaction::tests::setup();
    plugins::test_fixtures::install(&engine.db, &engine.paths).unwrap();
    std::fs::write(root.join("options.txt"), "offline fixture").unwrap();
    let game = games::register_game(&engine.db, &root, Some("minecraft-java")).unwrap();
    assert_eq!(game.adapter_slug.as_deref(), Some("minecraft-java"));
    (temp, engine, game)
}

fn register_game(engine: &FileEngine, root: &Path) -> Game {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("options.txt"), "offline fixture").unwrap();
    games::register_game(&engine.db, root, Some("minecraft-java")).unwrap()
}

fn record_root_artifact(engine: &FileEngine) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let filename = "root.jar";
    let path = engine.paths.downloads().join(format!("{id}-{filename}"));
    let bytes = b"inert offline catalog fixture";
    std::fs::write(&path, bytes).unwrap();
    let artifact = DownloadArtifact {
        id: id.clone(),
        path,
        filename: filename.into(),
        size: bytes.len() as u64,
        sha256: crate::util::sha256_file(
            &engine.paths.downloads().join(format!("{id}-{filename}")),
        )
        .unwrap(),
        sha512: String::new(),
        integrity_verified: false,
        requires_review: true,
        auto_install_allowed: false,
        warnings: Vec::new(),
    };
    let resolved = ResolvedMod {
        source_id: "modrinth".into(),
        project_id: ROOT_PROJECT.into(),
        version_id: ROOT_VERSION.into(),
        name: "Root mod".into(),
        version: "1.0.0".into(),
        download_url: "https://cdn.modrinth.com/data/RootProj/root.jar".into(),
        filename: filename.into(),
        size: Some(bytes.len() as u64),
        hashes: Default::default(),
        homepage_url: None,
        warnings: Vec::new(),
    };
    artifacts::record(engine, &artifact, &resolved).unwrap();
    id
}

fn journal_install(
    engine: &FileEngine,
    install_id: &str,
    game_id: &str,
    project_id: &str,
    version_id: &str,
) {
    journal_install_with_files(
        engine,
        install_id,
        game_id,
        project_id,
        version_id,
        None,
        &[],
    );
}

fn journal_install_with_files(
    engine: &FileEngine,
    install_id: &str,
    game_id: &str,
    project_id: &str,
    version_id: &str,
    install_root: Option<&Path>,
    files: &[InstalledFile],
) {
    let transaction_id = format!("journal-{install_id}");
    let metadata = json!({
        "downloadOrigin": {
            "artifactId": uuid::Uuid::new_v4().to_string(),
            "sourceId": "modrinth",
            "projectId": project_id,
            "versionId": version_id,
        }
    });
    let mut conn = engine.db.conn();
    let tx = conn.transaction().unwrap();
    tx.execute(
        "INSERT INTO file_transactions (id,kind,status,plan_json,created_at)
         VALUES (?1,'install','committed','{}','fixture')",
        [&transaction_id],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO installs (id,game_id,name,version,source,status,install_root,files_json,metadata_json,installed_at,updated_at)
         VALUES (?1,?2,?1,'1.0.0','modrinth','enabled',?3,?4,?5,'fixture','fixture')",
        rusqlite::params![
            install_id,
            game_id,
            install_root.map(|path| path.to_string_lossy().into_owned()),
            serde_json::to_string(files).unwrap(),
            metadata.to_string()
        ],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO install_transactions (install_id,transaction_id,action,created_at)
         VALUES (?1,?2,'install','fixture')",
        rusqlite::params![install_id, transaction_id],
    )
    .unwrap();
    tx.commit().unwrap();
}

fn sources(steps: Vec<Step>) -> (Sources, super::MockApi) {
    let (provider, mock) = provider("modrinth", steps);
    (
        Sources {
            providers: vec![Arc::new(provider)],
        },
        mock,
    )
}

fn request(
    artifact_id: &str,
    game_id: &str,
    replace_install_id: Option<&str>,
    project_pins: Value,
) -> GraphReviewRequest {
    serde_json::from_value(json!({
        "artifactId": artifact_id,
        "gameId": game_id,
        "replaceInstallId": replace_install_id,
        "gameVersion": GAME_VERSION,
        "loader": LOADER,
        "projectPins": project_pins,
    }))
    .unwrap()
}

fn dependency(project: &str, version: Option<&str>) -> Value {
    json!({
        "dependency_type": "required",
        "project_id": project,
        "version_id": version,
        "file_name": null,
    })
}

fn version(id: &str, project: &str, dependencies: Value) -> String {
    json!({
        "id": id,
        "project_id": project,
        "game_versions": [GAME_VERSION],
        "loaders": [LOADER],
        "dependencies": dependencies,
    })
    .to_string()
}

fn selected_version(id: &str, project: &str) -> Value {
    json!({
        "id": id,
        "project_id": project,
        "game_versions": [GAME_VERSION],
        "loaders": [LOADER],
        "version_type": "release",
    })
}

fn installed_file(path: impl Into<PathBuf>, bytes: &[u8]) -> InstalledFile {
    InstalledFile {
        path: path.into(),
        sha256: hex::encode(sha2::Sha256::digest(bytes)),
        size: bytes.len() as u64,
        before: None,
    }
}

fn exact_child_steps() -> Vec<Step> {
    let child = dependency("ChildPrj", Some("ChildV01"));
    vec![
        Step::ok(
            "/v2/version/Root0001",
            version(ROOT_VERSION, ROOT_PROJECT, json!([child])),
        ),
        Step::ok(
            "/v2/version/ChildV01",
            version("ChildV01", "ChildPrj", json!([])),
        ),
    ]
}

async fn review_with_steps(
    engine: FileEngine,
    artifact_id: &str,
    game_id: &str,
    steps: Vec<Step>,
) -> (Value, super::MockApi) {
    let (sources, mock) = sources(steps);
    let result = install_dependencies::review_graph(
        engine,
        &sources,
        request(artifact_id, game_id, None, json!({})),
    )
    .await
    .unwrap();
    (serde_json::to_value(result).unwrap(), mock)
}

#[derive(Debug, PartialEq, Eq)]
struct ReviewScopeSnapshot {
    installs: Vec<(String, String, String, String, String)>,
    games: Vec<(String, String)>,
    user_roots: Vec<(String, String, String)>,
}

fn review_scope_snapshot(engine: &FileEngine) -> ReviewScopeSnapshot {
    let conn = engine.db.conn();
    let installs = conn
        .prepare(
            "SELECT id,status,COALESCE(install_root,''),files_json,metadata_json
             FROM installs ORDER BY id",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let games = conn
        .prepare("SELECT id,root_path FROM games ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let user_roots = conn
        .prepare("SELECT game_id,root_id,path FROM game_user_roots ORDER BY game_id,root_id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    ReviewScopeSnapshot {
        installs,
        games,
        user_roots,
    }
}

fn assert_evidence_counts(
    evidence: &Value,
    status: &str,
    matched: u64,
    modified: u64,
    missing: u64,
    unverified: u64,
) {
    assert_eq!(evidence["status"], status);
    assert_eq!(evidence["matched"], matched);
    assert_eq!(evidence["modified"], modified);
    assert_eq!(evidence["missing"], missing);
    assert_eq!(evidence["unverified"], unverified);
    assert_eq!(evidence["limitReached"], false);
}

#[tokio::test]
async fn desktop_graph_review_pins_project_only_edge_and_matches_in_edge_order() {
    let (_temp, engine, game) = fixture();
    let artifact_id = record_root_artifact(&engine);
    journal_install(&engine, "local-pinned", &game.id, "PinnedPrj", "PinnedV1");

    let pinned = dependency("PinnedPrj", None);
    let choices = dependency("ChoicePrj", None);
    let mut wrong_game = selected_version("WrongGame", "ChoicePrj");
    wrong_game["game_versions"] = json!(["1.20.1"]);
    let mut wrong_loader = selected_version("WrongLoad", "ChoicePrj");
    wrong_loader["loaders"] = json!(["forge"]);
    let (sources, mock) = sources(vec![
        Step::ok(
            "/v2/version/Root0001",
            version(ROOT_VERSION, ROOT_PROJECT, json!([pinned, choices])),
        ),
        Step::ok(
            "/v2/version/PinnedV1",
            version("PinnedV1", "PinnedPrj", json!([])),
        ),
        Step::ok(
            "/v2/project/ChoicePrj/version",
            json!([
                wrong_game,
                selected_version("ChoiceV1", "ChoicePrj"),
                wrong_loader,
            ])
            .to_string(),
        ),
        Step::ok(
            "/v2/version/ChoiceV1",
            version("ChoiceV1", "ChoicePrj", json!([])),
        ),
    ]);

    let result = install_dependencies::review_graph(
        engine,
        &sources,
        request(
            &artifact_id,
            &game.id,
            None,
            json!({"PinnedPrj": "PinnedV1"}),
        ),
    )
    .await
    .unwrap();
    let result = serde_json::to_value(result).unwrap();

    assert_eq!(result["graph"]["complete"], true);
    assert_eq!(result["graph"]["edges"].as_array().unwrap().len(), 2);
    assert_eq!(result["checks"].as_array().unwrap().len(), 2);
    assert_eq!(
        result["graph"]["edges"][0]["declaration"]["project_id"],
        "PinnedPrj"
    );
    assert_eq!(
        result["graph"]["edges"][1]["declaration"]["project_id"],
        "ChoicePrj"
    );
    assert_eq!(result["checks"][0]["dependency"]["project_id"], "PinnedPrj");
    assert_eq!(result["checks"][0]["dependency"]["version_id"], "PinnedV1");
    assert_eq!(result["checks"][0]["status"], "recorded_exact");
    assert_eq!(
        result["checks"][0]["matches"][0]["installId"],
        "local-pinned"
    );
    assert_eq!(result["checks"][1]["dependency"]["project_id"], "ChoicePrj");
    assert_eq!(result["checks"][1]["dependency"]["version_id"], "ChoiceV1");
    assert_eq!(result["checks"][1]["status"], "not_recorded");

    assert_eq!(mock.params(0).len(), 0);
    assert_eq!(mock.params(1).len(), 0);
    let filters = mock.params(2);
    assert_eq!(
        serde_json::from_str::<Value>(&filters["game_versions"]).unwrap(),
        json!([GAME_VERSION])
    );
    assert_eq!(
        serde_json::from_str::<Value>(&filters["loaders"]).unwrap(),
        json!([LOADER])
    );
    assert_eq!(mock.params(3).len(), 0);
    for index in 0..4 {
        let params = mock.params(index);
        assert!(params.values().all(|value| {
            value != &game.id && value != &artifact_id && value != "local-pinned"
        }));
    }
    mock.assert_finished(4);
}

#[tokio::test]
async fn desktop_graph_review_ignores_matching_install_in_another_game() {
    let (temp, engine, game) = fixture();
    let other_game = register_game(&engine, &temp.path().join("other-game"));
    let artifact_id = record_root_artifact(&engine);
    journal_install(
        &engine,
        "foreign-match",
        &other_game.id,
        "OtherPrj",
        "OtherV01",
    );
    let dep = dependency("OtherPrj", Some("OtherV01"));
    let (sources, mock) = sources(vec![
        Step::ok(
            "/v2/version/Root0001",
            version(ROOT_VERSION, ROOT_PROJECT, json!([dep])),
        ),
        Step::ok(
            "/v2/version/OtherV01",
            version("OtherV01", "OtherPrj", json!([])),
        ),
    ]);

    let result = install_dependencies::review_graph(
        engine,
        &sources,
        request(&artifact_id, &game.id, None, json!({})),
    )
    .await
    .unwrap();
    let result = serde_json::to_value(result).unwrap();

    assert_eq!(result["checks"][0]["status"], "not_recorded");
    assert_eq!(result["checks"][0]["matchCount"], 0);
    assert!(result["checks"][0]["matches"]
        .as_array()
        .unwrap()
        .is_empty());
    mock.assert_finished(2);
}

#[tokio::test]
async fn desktop_graph_review_replacement_keeps_other_project_matches() {
    let (_temp, engine, game) = fixture();
    let artifact_id = record_root_artifact(&engine);
    journal_install(&engine, "replace-root", &game.id, ROOT_PROJECT, "RootOld1");
    journal_install(&engine, "installed-child", &game.id, "ChildPrj", "ChildV01");
    let dep = dependency("ChildPrj", Some("ChildV01"));
    let (sources, mock) = sources(vec![
        Step::ok(
            "/v2/version/Root0001",
            version(ROOT_VERSION, ROOT_PROJECT, json!([dep])),
        ),
        Step::ok(
            "/v2/version/ChildV01",
            version("ChildV01", "ChildPrj", json!([])),
        ),
    ]);

    let result = install_dependencies::review_graph(
        engine,
        &sources,
        request(&artifact_id, &game.id, Some("replace-root"), json!({})),
    )
    .await
    .unwrap();
    let result = serde_json::to_value(result).unwrap();

    assert_eq!(result["excludedInstallId"], "replace-root");
    assert_eq!(result["checks"][0]["status"], "recorded_exact");
    assert_eq!(result["checks"][0]["matches"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["checks"][0]["matches"][0]["installId"],
        "installed-child"
    );
    mock.assert_finished(2);
}

#[tokio::test]
async fn desktop_graph_review_rejects_replacement_from_another_project_before_network() {
    let (_temp, engine, game) = fixture();
    let artifact_id = record_root_artifact(&engine);
    journal_install(
        &engine,
        "not-root-project",
        &game.id,
        "OtherPrj",
        "OtherV01",
    );
    let (sources, mock) = sources(Vec::new());

    let error = install_dependencies::review_graph(
        engine,
        &sources,
        request(&artifact_id, &game.id, Some("not-root-project"), json!({})),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, AppError::Conflict(_)));
    mock.assert_finished(0);
}

#[tokio::test]
async fn desktop_graph_review_rejects_pending_recovery_before_network() {
    let (_temp, engine, game) = fixture();
    let artifact_id = record_root_artifact(&engine);
    engine
        .db
        .conn()
        .execute(
            "INSERT INTO file_transactions (id,kind,status,plan_json,created_at)
             VALUES ('pending-recovery','install','recovery_failed','{}','fixture')",
            [],
        )
        .unwrap();
    let (sources, mock) = sources(Vec::new());

    let error = install_dependencies::review_graph(
        engine,
        &sources,
        request(&artifact_id, &game.id, None, json!({})),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, AppError::Conflict(_)));
    mock.assert_finished(0);
}

#[tokio::test]
async fn desktop_graph_review_rechecks_install_file_evidence_on_each_review() {
    let (_temp, engine, game) = fixture();
    let artifact_id = record_root_artifact(&engine);
    let game_root = Path::new(&game.root_path);
    let install_root = game_root.join("mods");
    std::fs::create_dir_all(&install_root).unwrap();
    let file_path = install_root.join("child.jar");
    let original = b"fixture bytes";
    std::fs::write(&file_path, original).unwrap();
    journal_install_with_files(
        &engine,
        "child-install",
        &game.id,
        "ChildPrj",
        "ChildV01",
        Some(&install_root),
        &[installed_file(&file_path, original)],
    );

    let db_before = review_scope_snapshot(&engine);
    let roots_before = engine.roots().unwrap();
    let (matched, mock) =
        review_with_steps(engine.clone(), &artifact_id, &game.id, exact_child_steps()).await;
    mock.assert_finished(2);
    assert_eq!(matched["checks"][0]["status"], "recorded_exact");
    let evidence = &matched["fileEvidence"]["child-install"];
    assert_evidence_counts(evidence, "matched", 1, 0, 0, 0);
    assert_eq!(evidence["issues"], json!([]));

    // Same-size edits must be caught by the recorded SHA-256, not size alone.
    let changed = b"fixture byte!";
    assert_eq!(changed.len(), original.len());
    std::fs::write(&file_path, changed).unwrap();
    let (modified, mock) =
        review_with_steps(engine.clone(), &artifact_id, &game.id, exact_child_steps()).await;
    mock.assert_finished(2);
    let evidence = &modified["fileEvidence"]["child-install"];
    assert_evidence_counts(evidence, "mismatch", 0, 1, 0, 0);
    assert_eq!(
        evidence["issues"],
        json!([{"path": "mods/child.jar", "state": "modified"}])
    );

    std::fs::remove_file(&file_path).unwrap();
    let (missing, mock) =
        review_with_steps(engine.clone(), &artifact_id, &game.id, exact_child_steps()).await;
    mock.assert_finished(2);
    let evidence = &missing["fileEvidence"]["child-install"];
    assert_evidence_counts(evidence, "mismatch", 0, 0, 1, 0);
    assert_eq!(
        evidence["issues"],
        json!([{"path": "mods/child.jar", "state": "missing"}])
    );

    assert_eq!(review_scope_snapshot(&engine), db_before);
    assert_eq!(engine.roots().unwrap(), roots_before);
}

#[tokio::test]
async fn desktop_graph_review_omits_file_evidence_for_ambiguous_exact_matches() {
    let (_temp, engine, game) = fixture();
    let artifact_id = record_root_artifact(&engine);
    let install_root = Path::new(&game.root_path).join("mods");
    std::fs::create_dir_all(&install_root).unwrap();
    for (install_id, filename) in [
        ("child-install-a", "child-a.jar"),
        ("child-install-b", "child-b.jar"),
    ] {
        let path = install_root.join(filename);
        let bytes = format!("{filename} bytes");
        std::fs::write(&path, &bytes).unwrap();
        journal_install_with_files(
            &engine,
            install_id,
            &game.id,
            "ChildPrj",
            "ChildV01",
            Some(&install_root),
            &[installed_file(path, bytes.as_bytes())],
        );
    }

    let db_before = review_scope_snapshot(&engine);
    let (result, mock) =
        review_with_steps(engine.clone(), &artifact_id, &game.id, exact_child_steps()).await;
    mock.assert_finished(2);
    assert_eq!(result["checks"][0]["status"], "unconfirmed");
    assert_eq!(result["checks"][0]["reason"], "ambiguous_records");
    assert_eq!(result["checks"][0]["matchCount"], 2);
    assert_eq!(result["fileEvidence"], json!({}));
    assert_eq!(review_scope_snapshot(&engine), db_before);
}

#[tokio::test]
async fn desktop_graph_review_omits_file_evidence_for_unresolved_project_only_edge() {
    let (_temp, engine, game) = fixture();
    let artifact_id = record_root_artifact(&engine);
    let install_root = Path::new(&game.root_path).join("mods");
    std::fs::create_dir_all(&install_root).unwrap();
    let path = install_root.join("choice.jar");
    let bytes = b"project-only fixture";
    std::fs::write(&path, bytes).unwrap();
    journal_install_with_files(
        &engine,
        "choice-install",
        &game.id,
        "ChoicePrj",
        "ChoiceV1",
        Some(&install_root),
        &[installed_file(path, bytes)],
    );

    let declaration = dependency("ChoicePrj", None);
    let steps = vec![
        Step::ok(
            "/v2/version/Root0001",
            version(ROOT_VERSION, ROOT_PROJECT, json!([declaration])),
        ),
        Step::ok(
            "/v2/project/ChoicePrj/version",
            json!([
                selected_version("ChoiceV1", "ChoicePrj"),
                selected_version("ChoiceV2", "ChoicePrj"),
            ])
            .to_string(),
        ),
    ];
    let db_before = review_scope_snapshot(&engine);
    let (result, mock) = review_with_steps(engine.clone(), &artifact_id, &game.id, steps).await;
    mock.assert_finished(2);
    assert_eq!(result["graph"]["edges"][0]["state"], "choice_required");
    assert_eq!(
        result["graph"]["edges"][0]["selected_version_id"],
        Value::Null
    );
    assert_eq!(result["checks"][0]["status"], "recorded_project");
    assert_eq!(result["fileEvidence"], json!({}));
    assert_eq!(review_scope_snapshot(&engine), db_before);
}

#[cfg(unix)]
#[tokio::test]
async fn desktop_graph_review_marks_out_of_scope_and_symlink_paths_unverified() {
    use std::os::unix::fs::symlink;

    let (temp, engine, game) = fixture();
    let artifact_id = record_root_artifact(&engine);
    let game_root = Path::new(&game.root_path);
    let install_root = game_root.join("mods");
    std::fs::create_dir_all(&install_root).unwrap();
    let outside_path = temp.path().join("outside.jar");
    let bytes = b"outside fixture bytes";
    std::fs::write(&outside_path, bytes).unwrap();
    let symlink_path = install_root.join("linked.jar");
    symlink(&outside_path, &symlink_path).unwrap();
    journal_install_with_files(
        &engine,
        "unsafe-path-install",
        &game.id,
        "ChildPrj",
        "ChildV01",
        Some(&install_root),
        &[
            installed_file(&outside_path, bytes),
            installed_file(&symlink_path, bytes),
        ],
    );

    let db_before = review_scope_snapshot(&engine);
    let roots_before = engine.roots().unwrap();
    let (result, mock) =
        review_with_steps(engine.clone(), &artifact_id, &game.id, exact_child_steps()).await;
    mock.assert_finished(2);
    let evidence = &result["fileEvidence"]["unsafe-path-install"];
    assert_evidence_counts(evidence, "unverified", 0, 0, 0, 2);
    let issues = evidence["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 1);
    assert!(issues.iter().all(|issue| issue["state"] == "unverified"));
    assert!(issues
        .iter()
        .any(|issue| issue["path"] == "mods/linked.jar"));
    assert_eq!(review_scope_snapshot(&engine), db_before);
    assert_eq!(engine.roots().unwrap(), roots_before);
}
