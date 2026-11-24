//! Offline first publication and update: model reply → reviewed release copies →
//! public catalog → byte-exact install in a separate manager.
use crate::{
    db::Db,
    error::{AppError, AppResult},
    manager_catalog::CatalogClient,
    manager_publication::{self, PublicationState, PublicationTarget, PublishedPlugin, Remote},
    mcp::tests::MockSecrets,
    plugin_workflow, plugins,
    publication_copy::{listing_from_reply, PublicationListing},
    settings::Settings,
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
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;

const SLUG: &str = "minecraft-java";
const GAME_NAME: &str = "Minecraft Java Edition";
const COVER_URL: &str = "https://www.glossmod.com/manager-plugins/covers/minecraft-java.png";
const ADAPTER_BYTES: &[u8] =
    include_bytes!("../tests/fixtures/game-adapters/minecraft-java/adapter.json");
const SKILL_BYTES: &[u8] =
    include_bytes!("../tests/fixtures/game-adapters/minecraft-java/SKILL.md");
const PLUGIN_ID: i64 = 41001;
const GAMES_PATH: &str = "/api/v1/manager-plugins/games";
const OFFICIAL_ADAPTER_SHA256: &str =
    "77981d4d1b0b1261174297cbe1d7eaf49b7e16ba5c94d12a87416d6eecc51afd";
const OFFICIAL_SKILL_SHA256: &str =
    "c54e1d07b4e9c7e60bae60d541e10e27d94aa8f60322fdd13ba25b3a1a5263d7";

// These fixture blobs are byte-for-byte the official manager-plugin testdata
// at glossmod.com commit 67c02c7, under
// server/router/managerplugins/testdata/v1/minecraft-java/.
// Their official Git blob IDs are 377b49596359b8aed2763f404bcdda330be761e4
// and 9cce994774660c5f1ff425b0f7f477668f200611, respectively.

// Both the creator list and the public catalog reflect what the mock actually accepted.
#[derive(Clone)]
struct AcceptedRelease {
    plugin: PublishedPlugin,
    adapter: String,
    skill: String,
}

#[derive(Default)]
struct MockPublisher {
    owner_reads: AtomicUsize,
    releases: Mutex<Vec<AcceptedRelease>>,
}

#[async_trait::async_trait]
impl Remote for MockPublisher {
    async fn call(&self, name: &str, args: Value, cancel: &CancellationToken) -> AppResult<Value> {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let mut releases = self.releases.lock().unwrap();
        if name == "list_my_manager_plugins" {
            self.owner_reads.fetch_add(1, Ordering::Relaxed);
            assert_eq!(args["offset"], 0);
            // A similar plugin's title and slug prefix must never select it for update.
            let mut items = vec![json!({
                "id": 41002, "releaseId": 53002, "slug": "minecraft-java-server",
                "name": "Minecraft Java Edition", "version": "8.0.0", "tags": [], "hidden": false
            })];
            if let Some(release) = releases.last() {
                items.push(json!(release.plugin));
            }
            return Ok(json!({"items": items}));
        }
        match (name, releases.last()) {
            ("publish_manager_plugin", None) => assert!(args.get("plugin_id").is_none()),
            ("publish_manager_plugin_release", Some(previous)) => {
                assert_eq!(args["plugin_id"], previous.plugin.id);
            }
            _ => panic!(
                "unexpected publication tool {name} for {} accepted releases",
                releases.len()
            ),
        }
        let adapter = args["adapter"].as_str().unwrap();
        let skill = args["skill"].as_str().unwrap();
        let package = plugins::parse_package(SLUG, adapter, skill)?;
        let plugin = PublishedPlugin {
            id: PLUGIN_ID,
            release_id: 51001 + releases.len() as i64,
            slug: package.adapter.slug,
            name: package.adapter.name,
            version: package.adapter.version,
            tags: serde_json::from_value(args["tags"].clone())?,
            hidden: false,
            reason: String::new(),
        };
        let receipt = json!({"plugin": plugin});
        releases.push(AcceptedRelease {
            plugin,
            adapter: adapter.into(),
            skill: skill.into(),
        });
        Ok(receipt)
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
    async fn start(release: &AcceptedRelease) -> Self {
        let published = &release.plugin;
        let package = plugins::parse_package(SLUG, &release.adapter, &release.skill).unwrap();
        let release_id = published.release_id.to_string();
        let steam_app_id = package.adapter.steam_app_id.clone().unwrap_or_default();
        let games = json!({
            "data": [{
                "id": 31001,
                "name": package.adapter.display_game_name(),
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
            adapter: release.adapter.as_bytes().to_vec(),
            skill: release.skill.as_bytes().to_vec(),
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
    // Hash the Git blob's LF form regardless of the checkout's line endings.
    for (fixture, expected) in [
        (ADAPTER_BYTES, OFFICIAL_ADAPTER_SHA256),
        (SKILL_BYTES, OFFICIAL_SKILL_SHA256),
    ] {
        let text = std::str::from_utf8(fixture).unwrap().replace("\r\n", "\n");
        assert_eq!(hex::encode(Sha256::digest(text)), expected);
    }
    let mut manifest: Value = serde_json::from_slice(ADAPTER_BYTES).unwrap();
    manifest["gameName"] = json!(GAME_NAME);
    manifest["name"] = json!("Local instance manager");
    assert_publication_roundtrip(manifest).await;
}

#[tokio::test]
async fn legacy_source_game_name_survives_first_release_update_and_reimport() {
    let manifest: Value = serde_json::from_slice(ADAPTER_BYTES).unwrap();
    assert!(manifest.get("gameName").is_none());
    assert_publication_roundtrip(manifest).await;
}

async fn assert_publication_roundtrip(mut authoring_adapter: Value) {
    let publisher_temp = tempfile::tempdir().unwrap();
    let importer_temp = tempfile::tempdir().unwrap();
    let publisher = engine(publisher_temp.path(), "publisher");
    let importer = engine(importer_temp.path(), "importer");
    authoring_adapter["coverUrl"] = json!(COVER_URL);
    let authoring_adapter = serde_json::to_vec_pretty(&authoring_adapter).unwrap();
    let source_draft = plugin_workflow::plan_write_draft(
        &publisher,
        SLUG,
        std::str::from_utf8(&authoring_adapter).unwrap(),
        std::str::from_utf8(SKILL_BYTES).unwrap(),
    )
    .unwrap();
    apply(&publisher, &source_draft.plan);
    let source = &source_draft.draft;
    let package = plugins::load_package(&source.path).unwrap();
    let settings =
        Settings::with_secret_store(publisher.db.clone(), Arc::new(MockSecrets::default()));
    settings
        .write_secret("mcp.glossmod_http.api_key", "offline-roundtrip-fixture")
        .unwrap();
    let remote = MockPublisher::default();
    let cancel = CancellationToken::new();
    // Publish the same committed 1.0.0 source twice. Only the first receipt can
    // identify the existing remote plugin when preparing the second release.
    let long_description = format!(
        "## 数据包\n\n{}\n```text\nDatapack layout\n```\n\n[参考](https://example.org/guide)\n<em>原样保留</em>",
        "- 查看 `datapacks` 与世界存档。\n".repeat(256)
    );
    assert!(long_description.chars().count() > 4096);
    let cases = [
        (
            PublicationTarget::First,
            None,
            "1.0.0",
            json!({
                "title": "Minecraft Java 模组管理", "summary": "按实例管理模组和资源包。",
                "description": "识别游戏实例目录。\n\n查看加载器模组和资源包的安装布局。",
                "tags": ["模组", "资源包"]
            }),
        ),
        (
            PublicationTarget::Release {
                plugin_id: PLUGIN_ID,
            },
            Some("1.0.0"),
            "1.0.1",
            json!({
                "title": "Minecraft Java 世界与模组", "summary": "按世界区分数据包。",
                "description": long_description,
                "tags": ["数据包", "世界存档"]
            }),
        ),
    ];
    for (index, (target, previous_version, version, reply)) in cases.into_iter().enumerate() {
        let listing = listing_from_reply(&package, &reply.to_string()).unwrap();
        assert_eq!(listing.title, reply["title"]);
        assert_eq!(listing.summary, reply["summary"]);
        assert_eq!(listing.description, reply["description"]);
        assert_eq!(listing.tags[0], GAME_NAME);
        assert_eq!(json!(listing.tags[1..].to_vec()), reply["tags"]);
        assert_eq!(listing.cover_url.as_deref(), Some(COVER_URL));
        assert!(
            listing.cover_png.is_empty(),
            "new listings do not generate a PNG cover"
        );

        let reads_before = remote.owner_reads.load(Ordering::Relaxed);
        let intended = manager_publication::prepare_automatic_with(
            &publisher,
            &settings,
            &remote,
            Some(source.id.clone()),
            None,
            listing.clone(),
            &cancel,
        )
        .await
        .unwrap();
        assert!(
            remote.owner_reads.load(Ordering::Relaxed) > reads_before,
            "preparation must consult the current owner list"
        );
        assert_eq!(intended.preview.target, target);
        assert_eq!(intended.preview.original_version.as_deref(), Some("1.0.0"));
        assert_eq!(intended.preview.remote_version.as_deref(), previous_version);
        assert_eq!(intended.preview.version, version);
        assert_eq!(intended.preview.listing.as_ref(), Some(&listing));
        assert_eq!(intended.preview.tags, listing.tags);
        let approved = manager_publication::approve_publication(
            &publisher,
            &intended.id,
            &intended.preview.digest,
        )
        .unwrap();
        assert_eq!(remote.releases.lock().unwrap().len(), index);
        let published = manager_publication::publish_automatic_with(
            &publisher,
            &settings,
            &remote,
            &approved.id,
            &approved.preview.digest,
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(published.state, PublicationState::Completed);
        assert_eq!(
            published
                .preview
                .listing
                .as_ref()
                .and_then(|listing| listing.cover_url.as_deref()),
            Some(COVER_URL)
        );
        let release = {
            let releases = remote.releases.lock().unwrap();
            assert_eq!(releases.len(), index + 1);
            releases.last().unwrap().clone()
        };
        assert_eq!(published.remote.as_ref(), Some(&release.plugin));
        assert_eq!(release.plugin.version, version);
        assert_eq!(release.plugin.tags, approved.preview.tags);
        assert_eq!(published.preview.tags, release.plugin.tags);
        assert_eq!(release.adapter, approved.preview.files[0].text);
        assert_eq!(release.skill, approved.preview.files[1].text);
        let published_adapter: Value = serde_json::from_str(&release.adapter).unwrap();
        assert_eq!(published_adapter["gameName"], GAME_NAME);
        assert_eq!(published_adapter["name"], listing.title);
        assert_eq!(published_adapter["coverUrl"], COVER_URL);
        assert_eq!(
            crate::publication_copy::read_display(&release.skill).as_deref(),
            Some(listing.description.as_str())
        );
        assert_eq!(
            std::fs::read(source.path.join("adapter.json")).unwrap(),
            authoring_adapter
        );
        assert_eq!(
            std::fs::read(source.path.join("SKILL.md")).unwrap(),
            SKILL_BYTES
        );
        assert_catalog_install(&importer, &release, &listing, previous_version).await;
    }
}

async fn assert_catalog_install(
    importer: &FileEngine,
    release: &AcceptedRelease,
    listing: &PublicationListing,
    previous_version: Option<&str>,
) {
    let catalog = LoopbackCatalog::start(release).await;
    let client = CatalogClient::for_fixture(&catalog.origin).unwrap();
    let search = client.search(&listing.title).await.unwrap();
    assert_eq!(search.total_games, 1);
    let [game] = search.games.as_slice() else {
        panic!("expected one catalog game")
    };
    let [plugin] = game.plugins.as_slice() else {
        panic!("expected one public plugin")
    };
    assert_eq!(game.name, GAME_NAME);
    assert_eq!(plugin.id, release.plugin.id.to_string());
    assert_eq!(plugin.release_id, release.plugin.release_id.to_string());
    assert_eq!(plugin.name, listing.title);
    assert_eq!(plugin.description, listing.summary);
    assert_eq!(plugin.tags, listing.tags);
    assert_eq!(plugin.version, release.plugin.version);

    let imported = client
        .prepare_import(importer, &plugin.id, &plugin.release_id)
        .await
        .unwrap();
    let display = imported
        .preview
        .display
        .as_ref()
        .expect("published release display");
    assert_eq!(display.description, listing.description);
    assert_eq!(display.cover_url.as_deref(), Some(COVER_URL));
    assert_eq!(
        imported.preview.installed_version.as_deref(),
        previous_version
    );
    assert_eq!(imported.preview.game_name, GAME_NAME);
    assert_eq!(imported.preview.adapter_json, release.adapter);
    let imported_adapter: Value = serde_json::from_str(&imported.preview.adapter_json).unwrap();
    assert_eq!(imported_adapter["gameName"], GAME_NAME);
    assert_eq!(imported_adapter["coverUrl"], COVER_URL);
    assert_eq!(imported.preview.skill_markdown, release.skill);
    assert_eq!(
        imported.preview.hashes,
        plugin_workflow::PackageHashes::of(&release.adapter, &release.skill)
    );

    apply(importer, &imported.plan);
    let validation = plugin_workflow::validate_draft(importer, &imported.draft_id).unwrap();
    assert!(validation.valid, "{:?}", validation.issues);
    let install = plugin_workflow::plan_install_plugin(importer, &imported.draft_id).unwrap();
    apply(importer, &install.plan);
    let reconcile = plugin_workflow::reconcile_deployments(importer).unwrap();
    assert_eq!(reconcile.registered, vec![SLUG.to_owned()]);
    let installed_adapter = std::fs::read(install.install_path.join("adapter.json")).unwrap();
    assert_eq!(installed_adapter, release.adapter.as_bytes());
    let installed_adapter: Value = serde_json::from_slice(&installed_adapter).unwrap();
    assert_eq!(installed_adapter["coverUrl"], COVER_URL);
    assert_eq!(
        std::fs::read(install.install_path.join("SKILL.md")).unwrap(),
        release.skill.as_bytes()
    );
    let installed = plugins::list_plugins(&importer.db).unwrap();
    let plugin_info = installed.iter().find(|p| p.slug == SLUG).unwrap();
    assert_eq!(plugin_info.game_name, GAME_NAME);
    assert_eq!(plugin_info.name, listing.title);
    assert_eq!(plugin_info.version, plugin.version);

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
