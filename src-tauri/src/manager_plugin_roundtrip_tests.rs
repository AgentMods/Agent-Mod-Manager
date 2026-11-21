//! Offline contract joining publication, the public manager-plugin catalog,
//! catalog import, and the existing reviewed install workflow.
use crate::{
    db::Db,
    error::{AppError, AppResult},
    manager_catalog::CatalogClient,
    manager_publication::{self, PublicationState, PublicationTarget, PublishedPlugin, Remote},
    plugin_workflow,
    plugins::{self, GamePackage},
    storage::AppPaths,
    transaction::{FileEngine, FilePlan},
};
use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{header, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;

const SLUG: &str = "minecraft-java";
const ADAPTER_BYTES: &[u8] =
    include_bytes!("../tests/fixtures/game-adapters/minecraft-java/adapter.json");
const SKILL_BYTES: &[u8] =
    include_bytes!("../tests/fixtures/game-adapters/minecraft-java/SKILL.md");
const GAMES_PATH: &str = "/api/v1/manager-plugins/games";
const OFFICIAL_ADAPTER_SHA256: &str =
    "5f44312d0d8f78dd946c793ff684efe023f71e926764f403d8e8c3e1206aa41c";
const OFFICIAL_SKILL_SHA256: &str =
    "114b6ba609a87ce358a3f024639a3def2c5b43d079e4cbcee4a0839dc22b0af4";

// These fixture files are byte-for-byte the official manager-plugin testdata
// at glossmod.com commit 67c02c7, under
// server/router/managerplugins/testdata/v1/minecraft-java/.
// Their official Git blob IDs are 377b49596359b8aed2763f404bcdda330be761e4
// and 9cce994774660c5f1ff425b0f7f477668f200611, respectively.

#[derive(Default)]
struct MockPublisher {
    calls: Mutex<Vec<(String, Value)>>,
}

#[async_trait::async_trait]
impl Remote for MockPublisher {
    async fn call(&self, name: &str, args: Value, cancel: &CancellationToken) -> AppResult<Value> {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        self.calls
            .lock()
            .unwrap()
            .push((name.to_owned(), args.clone()));

        if name != "publish_manager_plugin" {
            return Err(AppError::Invalid(format!(
                "unexpected publication tool: {name}"
            )));
        }
        let adapter = args["adapter"]
            .as_str()
            .ok_or_else(|| AppError::Invalid("missing adapter fixture".into()))?;
        let skill = args["skill"]
            .as_str()
            .ok_or_else(|| AppError::Invalid("missing Skill fixture".into()))?;
        let package = plugins::parse_package(SLUG, adapter, skill)?;
        Ok(json!({
            "plugin": {
                "id": 41001,
                "releaseId": 51001,
                "slug": package.adapter.slug,
                "version": package.adapter.version,
                "tags": args["tags"],
                "hidden": false,
                "name": package.adapter.name,
                "reason": ""
            }
        }))
    }
}

#[derive(Clone)]
struct CatalogState {
    release_id: String,
    catalog_json: String,
    adapter: Vec<u8>,
    skill: Vec<u8>,
    requests: Arc<Mutex<Vec<String>>>,
}

struct LoopbackCatalog {
    origin: String,
    requests: Arc<Mutex<Vec<String>>>,
    server: JoinHandle<()>,
}

impl Drop for LoopbackCatalog {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl LoopbackCatalog {
    async fn start(
        published: &PublishedPlugin,
        package: &GamePackage,
        adapter: Vec<u8>,
        skill: Vec<u8>,
    ) -> Self {
        let release_id = published.release_id.to_string();
        let steam_app_id = package.adapter.steam_app_id.clone().unwrap_or_default();
        let games = json!({
            "data": [{
                "id": 31001,
                "name": package.adapter.name,
                "slug": package.adapter.slug,
                "aliases": [],
                "steamAppId": steam_app_id,
                "plugins": [{
                    "id": published.id,
                    "releaseId": published.release_id,
                    "slug": published.slug,
                    "name": package.adapter.name,
                    "version": published.version,
                    "platforms": package.adapter.platforms,
                    "description": package.skill.description,
                    "authorId": 61001,
                    "authorName": "Offline contract fixture",
                    "steamAppId": steam_app_id,
                    "tags": published.tags,
                    "hidden": published.hidden,
                    "adapterUrl": format!("/api/v1/manager-plugins/releases/{release_id}/adapter.json"),
                    "skillUrl": format!("/api/v1/manager-plugins/releases/{release_id}/SKILL.md")
                }]
            }]
        })
        .to_string();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(CatalogState {
            release_id,
            catalog_json: games,
            adapter,
            skill,
            requests: requests.clone(),
        });
        let app = Router::new()
            .route(GAMES_PATH, get(get_games))
            .route(
                "/api/v1/manager-plugins/releases/{release_id}/{filename}",
                get(get_release_file),
            )
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            origin,
            requests,
            server,
        }
    }
}

fn response(status: StatusCode, content_type: &'static str, body: impl Into<Body>) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(body.into())
        .unwrap()
}

async fn get_games(State(state): State<Arc<CatalogState>>) -> Response {
    state.requests.lock().unwrap().push(GAMES_PATH.into());
    response(
        StatusCode::OK,
        "application/json",
        state.catalog_json.clone(),
    )
}

async fn get_release_file(
    State(state): State<Arc<CatalogState>>,
    AxumPath((release_id, filename)): AxumPath<(String, String)>,
) -> Response {
    let path = format!("/api/v1/manager-plugins/releases/{release_id}/{filename}");
    state.requests.lock().unwrap().push(path);
    if release_id != state.release_id {
        return response(StatusCode::NOT_FOUND, "text/plain", "release not found");
    }
    match filename.as_str() {
        "adapter.json" => response(StatusCode::OK, "application/json", state.adapter.clone()),
        "SKILL.md" => response(
            StatusCode::OK,
            "text/markdown; charset=utf-8",
            state.skill.clone(),
        ),
        _ => response(StatusCode::NOT_FOUND, "text/plain", "file not found"),
    }
}

fn engine(temp_root: &Path, name: &str) -> FileEngine {
    let paths = AppPaths::new(temp_root.join(name)).unwrap();
    let db = Db::open(&paths.db_file()).unwrap();
    FileEngine::new(db, paths)
}

fn apply(engine: &FileEngine, plan: &FilePlan) {
    engine.approve_plan(&plan.id).unwrap();
    engine.apply_file_ops(&plan.id).unwrap();
}

#[tokio::test]
async fn published_manager_plugin_is_listed_imported_installed_byte_exact() {
    assert_eq!(
        hex::encode(Sha256::digest(ADAPTER_BYTES)),
        OFFICIAL_ADAPTER_SHA256
    );
    assert_eq!(
        hex::encode(Sha256::digest(SKILL_BYTES)),
        OFFICIAL_SKILL_SHA256
    );
    let adapter_text = std::str::from_utf8(ADAPTER_BYTES).unwrap();
    let skill_text = std::str::from_utf8(SKILL_BYTES).unwrap();
    let package = plugins::parse_package(SLUG, adapter_text, skill_text).unwrap();
    let publisher_temp = tempfile::tempdir().unwrap();
    let importer_temp = tempfile::tempdir().unwrap();
    let publisher = engine(publisher_temp.path(), "publisher");
    let importer = engine(importer_temp.path(), "importer");

    let source_draft =
        plugin_workflow::plan_write_draft(&publisher, SLUG, adapter_text, skill_text).unwrap();
    apply(&publisher, &source_draft.plan);
    let tags = vec![
        "official-fixture".to_owned(),
        "roundtrip-contract".to_owned(),
    ];
    let intended = manager_publication::prepare_draft(
        &publisher,
        &source_draft.draft.id,
        PublicationTarget::First,
        tags,
    )
    .unwrap();
    let approved = manager_publication::approve_publication(
        &publisher,
        &intended.id,
        &intended.preview.digest,
    )
    .unwrap();
    let mock_publisher = MockPublisher::default();
    let published_record = manager_publication::publish_with(
        &publisher,
        &mock_publisher,
        &approved.id,
        &approved.preview.digest,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(published_record.state, PublicationState::Completed);
    let published = published_record.remote.clone().unwrap();

    let calls = mock_publisher.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "publish_manager_plugin");
    assert_eq!(
        calls[0].1["adapter"].as_str().unwrap().as_bytes(),
        ADAPTER_BYTES
    );
    assert_eq!(
        calls[0].1["skill"].as_str().unwrap().as_bytes(),
        SKILL_BYTES
    );
    assert!(calls.iter().all(|(name, _)| name != "publish_work"));

    // The loopback public directory is populated from the accepted publication
    // payload and its verified plugin/release receipt, as a separate user sees it.
    let posted_adapter = calls[0].1["adapter"].as_str().unwrap().as_bytes().to_vec();
    let posted_skill = calls[0].1["skill"].as_str().unwrap().as_bytes().to_vec();
    let catalog = LoopbackCatalog::start(&published, &package, posted_adapter, posted_skill).await;
    let catalog_client = CatalogClient::for_fixture(&catalog.origin).unwrap();
    let search = catalog_client.search(SLUG).await.unwrap();
    assert_eq!(search.total_games, 1);
    assert_eq!(search.games.len(), 1);
    assert_eq!(search.games[0].plugins.len(), 1);
    assert_eq!(search.games[0].plugins[0].id, published.id.to_string());
    assert_eq!(
        search.games[0].plugins[0].release_id,
        published.release_id.to_string()
    );

    let imported = catalog_client
        .prepare_import(
            &importer,
            &published.id.to_string(),
            &published.release_id.to_string(),
        )
        .await
        .unwrap();
    assert_eq!(imported.preview.adapter_json.as_bytes(), ADAPTER_BYTES);
    assert_eq!(imported.preview.skill_markdown.as_bytes(), SKILL_BYTES);
    assert_eq!(
        imported.preview.hashes,
        plugin_workflow::PackageHashes::of(adapter_text, skill_text)
    );

    apply(&importer, &imported.plan);
    let validation = plugin_workflow::validate_draft(&importer, &imported.draft_id).unwrap();
    assert!(validation.valid, "{:?}", validation.issues);
    let install = plugin_workflow::plan_install_plugin(&importer, &imported.draft_id).unwrap();
    apply(&importer, &install.plan);
    let reconcile = plugin_workflow::reconcile_deployments(&importer).unwrap();
    assert_eq!(reconcile.registered, vec![SLUG.to_owned()]);
    assert_eq!(
        std::fs::read(install.install_path.join("adapter.json")).unwrap(),
        ADAPTER_BYTES
    );
    assert_eq!(
        std::fs::read(install.install_path.join("SKILL.md")).unwrap(),
        SKILL_BYTES
    );
    let installed_version: String = importer
        .db
        .conn()
        .query_row("SELECT version FROM plugins WHERE slug=?1", [SLUG], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(installed_version, package.adapter.version);

    let requests = catalog.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|path| path.as_str() == GAMES_PATH)
            .count(),
        2
    );
    assert_eq!(
        requests
            .iter()
            .filter(|path| path.ends_with("/adapter.json"))
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|path| path.ends_with("/SKILL.md"))
            .count(),
        1
    );
}
