use super::*;
use crate::{
    agent::{
        catalog_tools,
        policy::Effect,
        tools::{ToolContext, ToolRegistry},
    },
    db::Db,
    storage::AppPaths,
    transaction::FileOp,
};
use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::Response,
    Router,
};
use serde_json::json;
use std::{
    collections::HashMap,
    convert::Infallible,
    fs,
    sync::{Arc, Mutex},
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const PLUGIN_ID: &str = "9007199254740993";
const RELEASE_ID: &str = "31";

fn files() -> (String, String) {
    let (_, adapter, skill) = plugins::test_fixtures::PACKAGES[2];
    let mut adapter: Value = serde_json::from_str(adapter).unwrap();
    adapter["mcpServers"] = json!([{"id":"example", "name":"测试", "transport":"streamable_http", "url":"https://example.org/mcp"}]);
    (
        format!(
            "{}\r\n  \r\n",
            serde_json::to_string_pretty(&adapter)
                .unwrap()
                .replace('\n', "\r\n")
        ),
        format!(
            "{}\r\n\r\n原文\t \r\n",
            skill.replace("\r\n", "\n").replace('\n', "\r\n")
        ),
    )
}
fn catalog() -> Value {
    json!({"data":[{"id":1,"name":"Fallout 4","slug":"fallout4","aliases":["辐射四"],"steamAppId":"377160","plugins":[{
        "id":9007199254740993_i64,"releaseId":31,"slug":"fallout4","name":"Fallout 4","version":"1.0.0","platforms":["windows"],"description":"安装说明\r\n第二行", "authorId":7,"authorName":"作者","steamAppId":"377160","tags":[],"hidden":false,
        "adapterUrl":release_path(RELEASE_ID,"adapter.json"),"skillUrl":release_path(RELEASE_ID,"SKILL.md")
    }]}]})
}
fn candidate(body: &mut Value) -> &mut Value {
    &mut body["data"][0]["plugins"][0]
}

#[derive(Clone)]
struct Reply {
    status: StatusCode,
    bytes: Vec<u8>,
    chunked: bool,
}
impl Reply {
    fn ok(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            status: StatusCode::OK,
            bytes: bytes.into(),
            chunked: false,
        }
    }
}
#[derive(Default)]
struct Traffic {
    replies: HashMap<String, Reply>,
    requests: Vec<String>,
}
struct Server {
    client: CatalogClient,
    traffic: Arc<Mutex<Traffic>>,
    task: JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(State(traffic): State<Arc<Mutex<Traffic>>>, request: Request) -> Response<Body> {
    assert_eq!(request.method(), "GET");
    for header in ["authorization", "cookie", "x-api-key", "mcp-session-id"] {
        assert!(!request.headers().contains_key(header));
    }
    let path = request.uri().to_string();
    let mut traffic = traffic.lock().unwrap();
    traffic.requests.push(path.clone());
    let reply = traffic
        .replies
        .get(&path)
        .cloned()
        .unwrap_or_else(|| Reply {
            status: StatusCode::NOT_FOUND,
            bytes: vec![],
            chunked: false,
        });
    let mut response = Response::builder().status(reply.status);
    if reply.status.is_redirection() {
        response = response.header("location", "/unexpected-redirect");
    }
    let body = if reply.chunked {
        let bytes = Bytes::from(reply.bytes);
        let half = bytes.len() / 2;
        Body::from_stream(futures_util::stream::iter([
            Ok::<_, Infallible>(bytes.slice(..half)),
            Ok(bytes.slice(half..)),
        ]))
    } else {
        Body::from(reply.bytes)
    };
    response.body(body).unwrap()
}
impl Server {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let traffic = Arc::new(Mutex::new(Traffic::default()));
        let router = Router::new().fallback(serve).with_state(traffic.clone());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let server = Self {
            client: CatalogClient::for_fixture(&origin).unwrap(),
            traffic,
            task,
        };
        server.set_catalog(catalog());
        let (a, s) = files();
        server.set_file("adapter.json", Reply::ok(a));
        server.set_file("SKILL.md", Reply::ok(s));
        server
    }
    fn set(&self, path: &str, reply: Reply) {
        self.traffic
            .lock()
            .unwrap()
            .replies
            .insert(path.to_owned(), reply);
    }
    fn set_file(&self, file: &str, reply: Reply) {
        self.set(&release_path(RELEASE_ID, file), reply);
    }
    fn set_catalog(&self, body: Value) {
        self.set(GAMES_PATH, Reply::ok(serde_json::to_vec(&body).unwrap()));
    }
    fn requests(&self) -> Vec<String> {
        self.traffic.lock().unwrap().requests.clone()
    }
}
fn context() -> (tempfile::TempDir, ToolContext) {
    let temp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(temp.path().join("manager")).unwrap();
    let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
    (
        temp,
        ToolContext {
            engine,
            cancel: CancellationToken::new(),
            progress: None,
            resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
        },
    )
}
fn apply(engine: &FileEngine, plan: &FilePlan) {
    engine.approve_plan(&plan.id).unwrap();
    engine.apply_file_ops(&plan.id).unwrap();
}

#[tokio::test]
async fn discovery_is_read_only_and_isolates_bad_rows_without_losing_long_official_metadata() {
    let server = Server::new().await;
    let mut body = catalog();
    let plugin = candidate(&mut body);
    plugin["authorName"] = json!("作".repeat(700));
    plugin["tags"] = json!(["长".repeat(1100)]);
    plugin["description"] = json!("说明\r\n下一行\t内容。".repeat(450));
    let mut other = plugin.clone();
    other["id"] = "23".into();
    other["releaseId"] = "52".into();
    other["authorName"] = "另一作者".into();
    other["adapterUrl"] = json!(release_path("52", "adapter.json"));
    other["skillUrl"] = json!(release_path("52", "SKILL.md"));
    let list = body["data"][0]["plugins"].as_array_mut().unwrap();
    list.extend([other, json!({"hidden": true}), json!({"slug":"broken"})]);
    body["data"]
        .as_array_mut()
        .unwrap()
        .push(json!({"plugins":false}));
    server.set_catalog(body);
    for query in ["377160", "辐射四", "Fallout4", "FaLLoUt 4", ""] {
        let r = server.client.search(query).await.unwrap();
        assert_eq!((r.total_games, r.skipped_entries), (1, 2));
        assert_eq!(r.games[0].plugins.len(), 2);
        assert_eq!(
            serde_json::to_value(&r.games[0].plugins[0]).unwrap()["id"],
            PLUGIN_ID
        );
    }
    let r = server.client.search("missing").await.unwrap();
    assert!(r.games.is_empty());
    assert_eq!(r.total_games, 1);
    server.set_catalog(json!({"data":[]}));
    let empty = server.client.search("").await.unwrap();
    assert_eq!((empty.total_games, empty.skipped_entries), (0, 0));
    server.set_catalog(json!({"data":[{}]}));
    assert!(matches!(
        server.client.search("").await,
        Err(AppError::Invalid(_))
    ));
    assert!(server.requests().iter().all(|path| path == GAMES_PATH));
}

#[test]
fn rejects_ambiguous_ids_and_unsafe_release_paths_without_hiding_unrelated_candidates() {
    for (field, value) in [
        ("adapterUrl",json!("https://elsewhere.example/api/v1/manager-plugins/releases/31/adapter.json")),
        ("adapterUrl",json!("https://www.glossmod.com@evil.test/api/v1/manager-plugins/releases/31/adapter.json")),
        ("adapterUrl",json!("//www.glossmod.com/api/v1/manager-plugins/releases/31/adapter.json")),
        ("skillUrl",json!("/api/v1/manager-plugins/releases/30/SKILL.md")),
        ("skillUrl",json!("/api/v1/manager-plugins/releases/31/adapter.json")),
        ("skillUrl",json!("/api/v1/manager-plugins/releases/31/%53KILL.md")),
        ("skillUrl",json!("/api/v1/manager-plugins/releases/30/../31/SKILL.md")),
        ("skillUrl",json!("/api/v1/manager-plugins/releases/31/SKILL.md?x")),
        ("authorId",json!(0)), ("releaseId",json!("031")), ("id",json!(2.5)), ("platforms",json!(["unsupported"])),
        ("gameName",json!("label\u{202e}hidden")), ("version",json!("invalid")), ("name",json!("label\u{202e}hidden")), ("slug",json!("../escape"))
    ] {
        let mut body = catalog();
        candidate(&mut body)[field] = value;
        assert!(parse_catalog(&body.to_string()).is_err(), "{field}");
    }
    for key in ["id", "releaseId"] {
        let mut body = catalog();
        let mut b = candidate(&mut body).clone();
        if key == "id" {
            b["releaseId"] = 42.into();
            b["adapterUrl"] = json!(release_path("42", "adapter.json"));
            b["skillUrl"] = json!(release_path("42", "SKILL.md"));
        } else {
            b["id"] = 43.into();
        }
        body["data"][0]["plugins"].as_array_mut().unwrap().push(b);
        assert!(parse_catalog(&body.to_string()).is_err(), "{key}");
        let mut distinct = candidate(&mut body).clone();
        distinct["id"] = 61.into();
        distinct["releaseId"] = 62.into();
        distinct["adapterUrl"] = json!(release_path("62", "adapter.json"));
        distinct["skillUrl"] = json!(release_path("62", "SKILL.md"));
        body["data"][0]["plugins"]
            .as_array_mut()
            .unwrap()
            .push(distinct);
        let result = parse_catalog(&body.to_string()).unwrap().search;
        assert_eq!(result.skipped_entries, 2);
        assert_eq!(result.games[0].plugins[0].id, "61");
    }
}

#[tokio::test]
async fn unavailable_or_mixed_release_ids_and_redirects_cannot_stage_a_draft() {
    let (_temp, ctx) = context();
    let server = Server::new().await;
    for (id, release) in [("31", PLUGIN_ID), (PLUGIN_ID, "32"), ("999", RELEASE_ID)] {
        assert!(matches!(
            server.client.prepare_import(&ctx.engine, id, release).await,
            Err(AppError::NotFound(_))
        ));
    }
    for value in ["0", "+31", "31/..", "00031", "9223372036854775808"] {
        assert!(matches!(
            server.client.preview(&ctx.engine, PLUGIN_ID, value).await,
            Err(AppError::Invalid(_))
        ));
    }
    let mut body = catalog();
    candidate(&mut body)["hidden"] = true.into();
    server.set_catalog(body);
    assert!(matches!(
        server
            .client
            .preview(&ctx.engine, PLUGIN_ID, RELEASE_ID)
            .await,
        Err(AppError::NotFound(_))
    ));
    assert!(server.requests().iter().all(|p| p == GAMES_PATH));
    server.set_catalog(catalog());
    for status in [
        StatusCode::FOUND,
        StatusCode::PARTIAL_CONTENT,
        StatusCode::NOT_FOUND,
        StatusCode::GONE,
        StatusCode::BAD_GATEWAY,
    ] {
        server.set_file(
            "SKILL.md",
            Reply {
                status,
                bytes: b"private server message".to_vec(),
                chunked: false,
            },
        );
        let err = server
            .client
            .prepare_import(&ctx.engine, PLUGIN_ID, RELEASE_ID)
            .await
            .unwrap_err();
        assert!(!err.to_string().contains("private server message"));
    }
    assert!(!server
        .requests()
        .iter()
        .any(|p| p == "/unexpected-redirect"));
    assert!(!ctx.engine.paths.staging().join("plugin-scratch").exists());
}

#[tokio::test]
async fn checks_streamed_limits_utf8_and_package_identity_before_planning() {
    let (_temp, ctx) = context();
    let server = Server::new().await;
    let (adapter, skill) = files();
    for (name, limit) in [
        ("adapter.json", plugin_workflow::ADAPTER_LIMIT),
        ("SKILL.md", plugin_workflow::SKILL_LIMIT),
    ] {
        for chunked in [false, true] {
            server.set_file(
                name,
                Reply {
                    status: StatusCode::OK,
                    bytes: vec![b'a'; limit + 1],
                    chunked,
                },
            );
            assert!(
                matches!(server.client.prepare_import(&ctx.engine,PLUGIN_ID,RELEASE_ID).await,Err(AppError::Invalid(s)) if s.contains("大小"))
            );
        }
        server.set_file(name, Reply::ok([0xff]));
        assert!(
            matches!(server.client.prepare_import(&ctx.engine,PLUGIN_ID,RELEASE_ID).await,Err(AppError::Invalid(s)) if s.contains("UTF-8"))
        );
        server.set_file(
            name,
            Reply::ok(if name == "SKILL.md" {
                skill.clone()
            } else {
                adapter.clone()
            }),
        );
    }
    for (key, value) in [
        ("version", json!("2.0.0")),
        ("slug", json!("fallout3")),
        ("name", json!("Other")),
        ("platforms", json!(["macos"])),
    ] {
        let mut body = catalog();
        candidate(&mut body)[key] = value;
        server.set_catalog(body);
        assert!(
            server
                .client
                .prepare_import(&ctx.engine, PLUGIN_ID, RELEASE_ID)
                .await
                .is_err(),
            "{key}"
        );
    }
    server.set_catalog(catalog());
    server.set_file("SKILL.md", Reply::ok(skill.replace("fallout4", "fallout3")));
    assert!(server
        .client
        .prepare_import(&ctx.engine, PLUGIN_ID, RELEASE_ID)
        .await
        .is_err());
    server.set(GAMES_PATH, Reply::ok([0xff]));
    assert!(
        matches!(server.client.search("").await, Err(AppError::Invalid(s)) if s.contains("UTF-8"))
    );
    server.set(GAMES_PATH, Reply::ok(br#"{"data":null}"#.to_vec()));
    assert!(server.client.search("").await.is_err());
    assert!(!ctx.engine.paths.staging().join("plugin-scratch").exists());
}

#[tokio::test]
async fn reviewed_agent_import_preserves_exact_bytes_after_server_disappears_then_installs_and_rolls_back(
) {
    let (_temp, ctx) = context();
    let server = Server::new().await;
    let (slug, old_adapter, old_skill) = plugins::test_fixtures::PACKAGES[2];
    let old_adapter = old_adapter.replace("1.0.0", "0.9.0");
    let draft =
        plugin_workflow::plan_write_draft(&ctx.engine, slug, &old_adapter, old_skill).unwrap();
    apply(&ctx.engine, &draft.plan);
    let old_install = plugin_workflow::plan_install_plugin(&ctx.engine, &draft.draft.id).unwrap();
    apply(&ctx.engine, &old_install.plan);
    plugin_workflow::reconcile_deployments(&ctx.engine).unwrap();
    let registry = ToolRegistry::default();
    catalog_tools::register_with(&registry, server.client.clone()).unwrap();
    let selection = json!({"plugin_id":PLUGIN_ID,"release_id":RELEASE_ID});
    for (tool_name, args) in [
        ("search_cloud_plugins", json!({"query":"辐射四"})),
        ("preview_cloud_plugin", selection.clone()),
    ] {
        let tool = registry.get(tool_name).unwrap();
        let prepared = tool.prepare(&ctx, &args).await.unwrap();
        assert_eq!(prepared.effect, Effect::Read);
        assert!(prepared.plan.is_none());
        let output = tool.execute(&ctx, args, prepared).await.unwrap();
        assert_eq!(output["trust"], "untrusted_instructions");
        if tool_name == "search_cloud_plugins" {
            assert_eq!(output["candidates"][0]["pluginId"], PLUGIN_ID);
        } else {
            assert_eq!(output["preview"]["installedVersion"], "0.9.0");
            assert_eq!(output["preview"]["mcpServers"][0]["name"], "测试");
        }
    }
    let tool = registry.get("import_cloud_plugin").unwrap();
    let prepared = tool.prepare(&ctx, &selection).await.unwrap();
    let (adapter, skill) = files();
    assert_eq!(prepared.preview["preview"]["adapterJson"], adapter);
    assert_eq!(prepared.preview["preview"]["skillMarkdown"], skill);
    assert_eq!(prepared.effect, Effect::ManagerWrite);
    assert!(prepared.plan.as_ref().unwrap().requires_review);
    assert_eq!(
        prepared.preview["preview"]["hashes"],
        json!(PackageHashes::of(&adapter, &skill))
    );
    assert_eq!(plugin_workflow::list_drafts(&ctx.engine).unwrap().len(), 1);
    server.task.abort();
    // Even changed execute-time arguments cannot substitute a release after preparation.
    let result = tool
        .execute(&ctx, json!({"plugin_id":"999","release_id":"99"}), prepared)
        .await
        .unwrap();
    let id = result["draftId"].as_str().unwrap();
    let validation = plugin_workflow::validate_draft(&ctx.engine, id).unwrap();
    assert!(validation.valid);
    assert_eq!(
        fs::read(validation.draft.path.join("adapter.json")).unwrap(),
        adapter.as_bytes()
    );
    assert_eq!(
        fs::read(validation.draft.path.join("SKILL.md")).unwrap(),
        skill.as_bytes()
    );
    let install = plugin_workflow::plan_install_plugin(&ctx.engine, id).unwrap();
    assert!(install.plan.entries.iter().all(|e| e.before.is_some()));
    apply(&ctx.engine, &install.plan);
    plugin_workflow::reconcile_after_apply(&ctx.engine, &install.plan).unwrap();
    assert!(!validation.draft.path.exists());
    assert_eq!(plugin_workflow::list_drafts(&ctx.engine).unwrap().len(), 1);
    assert_eq!(
        fs::read(install.install_path.join("SKILL.md")).unwrap(),
        skill.as_bytes()
    );
    let cloud = plugin_workflow::catalog_update_candidates(&ctx.engine).unwrap();
    assert_eq!(cloud.len(), 1);
    assert_eq!(cloud[0].slug, slug);
    assert_eq!(cloud[0].plugin_id.as_deref(), Some(PLUGIN_ID));
    assert_eq!(cloud[0].deployment_id, install.plan.id);
    ctx.engine.rollback_transaction(&install.plan.id).unwrap();
    plugin_workflow::reconcile_deployments(&ctx.engine).unwrap();
    assert!(plugin_workflow::catalog_update_candidates(&ctx.engine)
        .unwrap()
        .is_empty());
    assert_eq!(
        fs::read(install.install_path.join("adapter.json")).unwrap(),
        old_adapter.as_bytes()
    );
    assert_eq!(
        fs::read(install.install_path.join("SKILL.md")).unwrap(),
        old_skill.as_bytes()
    );
}

#[tokio::test]
async fn scratch_tampering_cannot_commit_and_cancelled_agent_import_does_not_apply() {
    let (_temp, ctx) = context();
    let server = Server::new().await;
    let prepared = server
        .client
        .prepare_import(&ctx.engine, PLUGIN_ID, RELEASE_ID)
        .await
        .unwrap();
    let FileOp::Copy { src, .. } = &prepared.plan.entries[0].operation else {
        panic!("expected staged copy");
    };
    fs::write(src, "changed").unwrap();
    ctx.engine.approve_plan(&prepared.plan.id).unwrap();
    assert!(ctx.engine.apply_file_ops(&prepared.plan.id).is_err());
    assert!(plugin_workflow::list_drafts(&ctx.engine)
        .unwrap()
        .is_empty());
    let registry = ToolRegistry::default();
    catalog_tools::register_with(&registry, server.client.clone()).unwrap();
    let tool = registry.get("import_cloud_plugin").unwrap();
    let args = json!({"plugin_id":PLUGIN_ID,"release_id":RELEASE_ID});
    let prepared = tool.prepare(&ctx, &args).await.unwrap();
    ctx.cancel.cancel();
    assert!(matches!(
        tool.execute(&ctx, args.clone(), prepared).await,
        Err(AppError::Cancelled)
    ));
    assert!(matches!(
        tool.prepare(&ctx, &args).await,
        Err(AppError::Cancelled)
    ));
    assert!(plugin_workflow::list_drafts(&ctx.engine)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn accepts_both_files_at_their_exact_limits_and_bounds_a_chunked_catalog() {
    let (_temp, ctx) = context();
    let server = Server::new().await;
    let (mut adapter, mut skill) = files();
    adapter.push_str(&" ".repeat(plugin_workflow::ADAPTER_LIMIT - adapter.len()));
    skill.push_str(&" ".repeat(plugin_workflow::SKILL_LIMIT - skill.len()));
    server.set_file("adapter.json", Reply::ok(adapter.clone()));
    server.set_file("SKILL.md", Reply::ok(skill.clone()));
    let result = server
        .client
        .prepare_import(&ctx.engine, PLUGIN_ID, RELEASE_ID)
        .await
        .unwrap();
    assert_eq!(
        result.plan.total_bytes,
        (adapter.len() + skill.len()) as u64
    );
    assert_eq!(result.preview.adapter_json, adapter);
    assert_eq!(result.preview.skill_markdown, skill);
    server.set(
        GAMES_PATH,
        Reply {
            status: StatusCode::OK,
            bytes: vec![b' '; CATALOG_LIMIT + 1],
            chunked: true,
        },
    );
    assert!(
        matches!(server.client.search("").await, Err(AppError::Invalid(s)) if s.contains("大小"))
    );
}

#[tokio::test]
async fn catalog_artwork_is_the_validated_manifest_game_cover_even_without_a_skill_footer() {
    let server = Server::new().await;
    let (original, _) = files();
    let mut manifest: Value = serde_json::from_str(&original).unwrap();
    let cover = "https://assets.example.org/game/cover.jpg?edition=original";
    manifest["coverUrl"] = json!(cover);
    server.set_file("adapter.json", Reply::ok(manifest.to_string()));
    let display = server.client.display(RELEASE_ID).await.unwrap().unwrap();
    assert_eq!(display.cover_url.as_deref(), Some(cover));
    assert!(display.description.is_empty());
    assert!(!server
        .requests()
        .iter()
        .any(|request| request.contains(cover)));
    manifest["coverUrl"] = Value::Null;
    server.set_file("adapter.json", Reply::ok(manifest.to_string()));
    assert!(server.client.display(RELEASE_ID).await.unwrap().is_none());
    manifest["coverUrl"] = json!("file:///private/cover.png");
    server.set_file("adapter.json", Reply::ok(manifest.to_string()));
    assert!(server.client.display(RELEASE_ID).await.is_err());
}

fn lifecycle_transaction(engine: &FileEngine, kind: &str) -> String {
    engine.db.conn().query_row(
        "SELECT id FROM file_transactions WHERE kind=?1 AND status='committed' ORDER BY rowid DESC LIMIT 1",
        [kind], |row| row.get(0),
    ).unwrap()
}

fn assert_no_catalog_drafts(engine: &FileEngine) {
    let count: usize = engine
        .db
        .conn()
        .query_row("SELECT count(*) FROM plugin_drafts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    let scratch = engine.paths.staging().join("plugin-catalog");
    assert!(!scratch.exists() || fs::read_dir(scratch).unwrap().next().is_none());
}

async fn add_with_catalog(
    server: &Server,
    engine: &FileEngine,
    root: &std::path::Path,
) -> AppResult<crate::games::Game> {
    let request =
        crate::games::CatalogGameRegistration::new(&engine.paths, root, Some("扫描到的游戏"))?;
    let preview = server.client.preview(engine, PLUGIN_ID, RELEASE_ID).await?;
    plugin_workflow::register_catalog_game(engine, preview, request)
}

fn fallout_root(root: &std::path::Path) {
    fs::create_dir_all(root.join("Data")).unwrap();
    fs::write(root.join("Fallout4.exe"), "fixture").unwrap();
    fs::write(root.join("Data/Fallout4.esm"), "fixture").unwrap();
}

#[tokio::test]
async fn release_game_name_reaches_the_library_on_catalog_install_and_reuse() {
    let (temp, ctx) = context();
    let server = Server::new().await;
    let (source, _) = files();
    let mut manifest: Value = serde_json::from_str(&source).unwrap();
    manifest["name"] = json!("辐射 4 Windows 适配包");
    manifest["gameName"] = json!("辐射 4");
    let mut directory = catalog();
    candidate(&mut directory)["name"] = manifest["name"].clone();
    candidate(&mut directory)["gameName"] = manifest["gameName"].clone();
    server.set_catalog(directory);
    server.set_file("adapter.json", Reply::ok(manifest.to_string()));

    let preview = server
        .client
        .preview(&ctx.engine, PLUGIN_ID, RELEASE_ID)
        .await
        .unwrap();
    let directory = server.client.games().await.unwrap();
    let row = serde_json::to_value(&directory.games[0].plugins[0]).unwrap();
    assert_eq!(row["gameName"], "辐射 4");
    assert_eq!(row["name"], "辐射 4 Windows 适配包");
    assert_eq!(preview.game_name, "辐射 4");
    assert_eq!(preview.plugin.name, "辐射 4 Windows 适配包");
    for folder in ["first", "second"] {
        let path = temp.path().join(folder);
        fallout_root(&path);
        let game = add_with_catalog(&server, &ctx.engine, &path).await.unwrap();
        assert_eq!(game.name, "辐射 4");
    }
    let library = crate::games::list_games(&ctx.engine.db).unwrap();
    assert_eq!(library.len(), 2);
    assert!(library.iter().all(|game| game.name == "辐射 4"));
}

#[tokio::test]
async fn discovered_cloud_release_registers_and_binds_once_and_reuses_exact_installed_package() {
    let (temp, ctx) = context();
    let engine = &ctx.engine;
    let server = Server::new().await;
    let path = temp.path().join("first");
    fallout_root(&path);
    let first = add_with_catalog(&server, engine, &path).await.unwrap();
    assert_eq!(first.name, "扫描到的游戏");
    assert_eq!(first.adapter_slug.as_deref(), Some("fallout4"));
    assert_eq!(first.adapter_version.as_deref(), Some("1.0.0"));
    assert_eq!(first.steam_app_id.as_deref(), Some("377160"));
    assert_eq!(first.platform, "windows");
    assert_eq!(
        add_with_catalog(&server, engine, &path).await.unwrap().id,
        first.id
    );
    let other = temp.path().join("second");
    fallout_root(&other);
    assert_ne!(
        add_with_catalog(&server, engine, &other).await.unwrap().id,
        first.id
    );
    let counts: (i64, i64, i64) = engine.db.conn().query_row(
        "SELECT (SELECT count(*) FROM plugin_deployments), (SELECT count(*) FROM games), (SELECT count(*) FROM mcp_servers WHERE plugin_slug='fallout4' AND enabled=0)",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).unwrap();
    assert_eq!(counts, (1, 2, 1));
    assert_no_catalog_drafts(engine);
    // Neither an edited release with the same version nor a disabled local
    // package is authorization to overwrite/re-enable that package.
    let third = temp.path().join("third");
    fallout_root(&third);
    let (_, skill) = files();
    server.set_file("SKILL.md", Reply::ok(format!("{skill}\nchanged")));
    assert!(matches!(
        add_with_catalog(&server, engine, &third).await,
        Err(AppError::Conflict(_))
    ));
    server.set_file("SKILL.md", Reply::ok(skill));
    plugin_workflow::set_enabled(engine, "fallout4", false).unwrap();
    assert!(matches!(
        add_with_catalog(&server, engine, &third).await,
        Err(AppError::Conflict(_))
    ));
    assert_eq!(crate::games::list_games(&engine.db).unwrap().len(), 2);
    assert!(!plugins::list_plugins(&engine.db).unwrap()[0].enabled);
}

#[tokio::test]
async fn discovered_cloud_release_rolls_back_on_missing_markers_or_registration_failure() {
    let (temp, ctx) = context();
    let engine = &ctx.engine;
    let server = Server::new().await;
    let root = temp.path().join("Fallout4");
    fs::create_dir(&root).unwrap();
    assert!(matches!(
        add_with_catalog(&server, engine, &root).await,
        Err(AppError::Conflict(_))
    ));
    fallout_root(&root);
    engine.db.conn().execute_batch("CREATE TRIGGER reject_game BEFORE INSERT ON games BEGIN SELECT RAISE(ABORT, 'test'); END;").unwrap();
    assert!(add_with_catalog(&server, engine, &root).await.is_err());
    assert!(crate::games::list_games(&engine.db).unwrap().is_empty());
    assert!(plugins::list_plugins(&engine.db).unwrap().is_empty());
    let target = engine.paths.plugins().join("games/fallout4");
    assert!(!target.join("adapter.json").exists() && !target.join("SKILL.md").exists());
    assert_no_catalog_drafts(engine);
    engine
        .db
        .conn()
        .execute_batch("DROP TRIGGER reject_game")
        .unwrap();
    add_with_catalog(&server, engine, &root).await.unwrap();
}

#[tokio::test]
async fn direct_install_commits_one_pinned_release_without_drafts_even_for_concurrent_clicks() {
    let (_temp, ctx) = context();
    let engine = &ctx.engine;
    let server = Server::new().await;
    assert!(matches!(
        server.client.install(engine, PLUGIN_ID, "32").await,
        Err(AppError::NotFound(_))
    ));
    let (a, b) = tokio::join!(
        server.client.install(engine, PLUGIN_ID, RELEASE_ID),
        server.client.install(engine, PLUGIN_ID, RELEASE_ID)
    );
    let err = match (a, b) {
        (Ok(()), Err(error)) | (Err(error), Ok(())) => error,
        result => panic!("expected exactly one install: {result:?}"),
    };
    assert!(
        matches!(err, AppError::Conflict(message) if message.contains("1.0.0") && message.contains("先移除"))
    );
    assert_no_catalog_drafts(engine);
    let target = engine.paths.plugins().join("games/fallout4");
    let (adapter, skill) = files();
    assert_eq!(
        fs::read(target.join("adapter.json")).unwrap(),
        adapter.as_bytes()
    );
    assert_eq!(fs::read(target.join("SKILL.md")).unwrap(), skill.as_bytes());
    let reopened = FileEngine::new(
        Db::open(&engine.paths.db_file()).unwrap(),
        (*engine.paths).clone(),
    );
    assert_eq!(
        plugin_workflow::reconcile_deployments(&reopened)
            .unwrap()
            .registered,
        ["fallout4"]
    );
    assert_eq!(
        plugins::list_plugins(&reopened.db).unwrap()[0].version,
        "1.0.0"
    );
    let id = lifecycle_transaction(engine, "catalog_plugin_install");
    assert!(matches!(
        engine.rollback_transaction(&id),
        Err(AppError::Denied(_))
    ));
    engine.rollback_desktop_transaction(&id).unwrap();
    assert!(plugins::list_plugins(&engine.db).unwrap().is_empty());
}

#[tokio::test]
async fn direct_install_rolls_back_copy_failure_and_registry_failure_without_a_draft() {
    let (_temp, ctx) = context();
    let engine = &ctx.engine;
    let server = Server::new().await;
    engine.inject_failure_before(1);
    assert!(server
        .client
        .install(engine, PLUGIN_ID, RELEASE_ID)
        .await
        .is_err());
    engine.inject_failure_before(usize::MAX);
    // Fail after both package files have been copied, while the terminal
    // SQLite commit is registering the Skill next to its Plugin and MCP.
    engine
        .db
        .conn()
        .execute_batch(
            "CREATE TRIGGER reject_plugin_skill BEFORE INSERT ON skills
        WHEN NEW.name='fallout4' BEGIN SELECT RAISE(ABORT, 'injected registration failure'); END;",
        )
        .unwrap();
    assert!(server
        .client
        .install(engine, PLUGIN_ID, RELEASE_ID)
        .await
        .is_err());
    let target = engine.paths.plugins().join("games/fallout4");
    assert!(!target.join("adapter.json").exists());
    assert!(!target.join("SKILL.md").exists());
    assert_no_catalog_drafts(engine);
    assert!(plugins::list_plugins(&engine.db).unwrap().is_empty());
    let unfinished: usize = engine
        .db
        .conn()
        .query_row(
            "SELECT count(*) FROM file_transactions WHERE status!='rolled_back'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unfinished, 0);
    engine
        .db
        .conn()
        .execute_batch("DROP TRIGGER reject_plugin_skill")
        .unwrap();
    server
        .client
        .install(engine, PLUGIN_ID, RELEASE_ID)
        .await
        .unwrap();
}

#[tokio::test]
async fn removal_retires_old_deployments_preserves_games_and_mods_and_allows_reinstall() {
    let (temp, ctx) = context();
    let engine = &ctx.engine;
    let server = Server::new().await;
    let (adapter, skill) = files();
    let draft = plugin_workflow::plan_write_draft(engine, "fallout4", &adapter, &skill).unwrap();
    apply(engine, &draft.plan);
    // This reviewed manual deployment expects an empty destination.
    let stale = plugin_workflow::plan_install_plugin(engine, &draft.draft.id).unwrap();
    server
        .client
        .install(engine, PLUGIN_ID, RELEASE_ID)
        .await
        .unwrap();
    let installed = lifecycle_transaction(engine, "catalog_plugin_install");
    let dormant_mcp: bool = engine.db.conn().query_row("SELECT enabled=0 AND startup_digest IS NULL FROM mcp_servers WHERE plugin_slug='fallout4'", [], |r| r.get(0)).unwrap();
    assert!(dormant_mcp);
    let game = temp.path().join("game");
    fs::create_dir(&game).unwrap();
    let game = game.canonicalize().unwrap();
    fs::write(game.join("existing-mod.bin"), "keep").unwrap();
    engine.db.conn().execute("INSERT INTO games(id,name,slug,root_path,adapter_slug,adapter_version,created_at,updated_at)
        VALUES ('keep-game','Existing Game','fallout4',?1,'fallout4','1.0.0','old','old')", [game.to_string_lossy()]).unwrap();
    engine
        .db
        .conn()
        .execute(
            "INSERT INTO mods(id,source_id,source_mod_id,game_id,name,created_at)
        VALUES ('keep-mod','fixture','mod','keep-game','Existing Mod','old')",
            [],
        )
        .unwrap();
    engine
        .db
        .conn()
        .execute(
            "INSERT INTO installs(id,game_id,mod_id,name,status,installed_at,updated_at)
        VALUES ('keep-install','keep-game','keep-mod','Existing Mod','enabled','old','old')",
            [],
        )
        .unwrap();
    assert!(
        matches!(plugin_workflow::remove_installed_plugin(engine, "fallout4"),
        Err(AppError::Conflict(message)) if message.contains("Existing Game") && message.contains("游戏库 → 对应游戏更多操作 → 选择适配包 → 移除适配包"))
    );
    crate::game_actions::remove_game_adapter(engine, "keep-game").unwrap();

    let target = engine.paths.plugins().join("games/fallout4");
    let receipt = plugin_workflow::remove_installed_plugin(engine, "fallout4").unwrap();
    let removed = lifecycle_transaction(engine, "plugin_remove");
    assert_eq!(
        serde_json::to_value(receipt).unwrap(),
        json!({"transactionId": removed})
    );
    assert!(!target.exists());
    let FileOp::Move { dst, .. } = &engine.get_plan(&removed).unwrap().entries[0].operation else {
        panic!("removal must park the whole package");
    };
    assert_eq!(
        fs::read(dst.join("adapter.json")).unwrap(),
        adapter.as_bytes()
    );
    assert!(matches!(
        engine.rollback_transaction(&removed),
        Err(AppError::Denied(_))
    ));
    assert!(matches!(
        engine.rollback_desktop_transaction(&installed),
        Err(AppError::Conflict(_))
    ));
    engine.approve_plan(&stale.plan.id).unwrap();
    assert!(
        matches!(engine.apply_file_ops(&stale.plan.id), Err(AppError::Conflict(message)) if message.contains("属于已移除"))
    );
    // Even writing back identical bytes out of band must not make an older
    // committed deployment eligible for restart reconciliation.
    fs::create_dir(&target).unwrap();
    fs::write(target.join("adapter.json"), adapter).unwrap();
    fs::write(target.join("SKILL.md"), skill).unwrap();
    let relocated_fixture = target.with_file_name("test-out-of-band");
    let reopened = FileEngine::new(
        Db::open(&engine.paths.db_file()).unwrap(),
        (*engine.paths).clone(),
    );
    assert!(reopened.recover_interrupted().unwrap().is_empty());
    assert!(plugin_workflow::reconcile_deployments(&reopened)
        .unwrap()
        .registered
        .is_empty());
    assert!(plugins::list_plugins(&reopened.db).unwrap().is_empty());
    let counts: (usize, usize, usize, usize) = reopened
        .db
        .conn()
        .query_row(
            "SELECT (SELECT count(*) FROM skills WHERE name='fallout4'),
                (SELECT count(*) FROM mcp_servers WHERE plugin_slug='fallout4'),
                (SELECT count(*) FROM games WHERE id='keep-game' AND adapter_slug IS NULL),
                (SELECT count(*) FROM mods WHERE id='keep-mod' AND game_id='keep-game')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(counts, (0, 0, 1, 1));
    let install: String = reopened
        .db
        .conn()
        .query_row(
            "SELECT status FROM installs WHERE id='keep-install'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(install, "enabled");
    assert_eq!(fs::read(game.join("existing-mod.bin")).unwrap(), b"keep");
    fs::rename(target, relocated_fixture).unwrap();
    server
        .client
        .install(&reopened, PLUGIN_ID, RELEASE_ID)
        .await
        .unwrap();
    assert_eq!(
        plugins::list_plugins(&reopened.db).unwrap()[0].version,
        "1.0.0"
    );
    assert!(matches!(
        reopened.rollback_desktop_transaction(&removed),
        Err(AppError::Conflict(_))
    ));
}

#[tokio::test]
async fn direct_install_refuses_a_different_version_but_keeps_manual_overwrite_workflow() {
    let (_temp, ctx) = context();
    let engine = &ctx.engine;
    let server = Server::new().await;
    let (current, skill) = files();
    let older = current.replace("1.0.0", "0.9.0");
    assert_ne!(older, current);
    let older = plugin_workflow::plan_write_draft(engine, "fallout4", &older, &skill).unwrap();
    apply(engine, &older.plan);
    let deployed = plugin_workflow::plan_install_plugin(engine, &older.draft.id).unwrap();
    apply(engine, &deployed.plan);
    plugin_workflow::reconcile_deployments(engine).unwrap();
    assert_eq!(
        plugins::list_plugins(&engine.db).unwrap()[0].version,
        "0.9.0"
    );
    server
        .client
        .update_installed_at_startup(engine, |_| panic!("manual draft is not a cloud install"))
        .await
        .unwrap();
    assert!(server.requests().is_empty());
    assert!(
        matches!(server.client.install(engine, PLUGIN_ID, RELEASE_ID).await,
        Err(AppError::Conflict(message)) if message.contains("0.9.0") && message.contains("先移除"))
    );
    assert_eq!(
        plugins::list_plugins(&engine.db).unwrap()[0].version,
        "0.9.0"
    );
    let current = plugin_workflow::plan_write_draft(engine, "fallout4", &current, &skill).unwrap();
    apply(engine, &current.plan);
    let overwrite = plugin_workflow::plan_install_plugin(engine, &current.draft.id).unwrap();
    assert!(
        overwrite.plan.requires_review && overwrite.plan.entries.iter().all(|e| e.before.is_some())
    );
    apply(engine, &overwrite.plan);
    plugin_workflow::reconcile_deployments(engine).unwrap();
    assert_eq!(
        plugins::list_plugins(&engine.db).unwrap()[0].version,
        "1.0.0"
    );
}

fn startup_release(slug: &str, plugin_id: &str, release_id: &str, version: &str) -> Value {
    let mut body = catalog();
    let row = candidate(&mut body);
    row["id"] = json!(plugin_id);
    row["releaseId"] = json!(release_id);
    row["slug"] = json!(slug);
    row["version"] = json!(version);
    row["adapterUrl"] = json!(release_path(release_id, "adapter.json"));
    row["skillUrl"] = json!(release_path(release_id, "SKILL.md"));
    row.clone()
}

fn serve_startup_releases(server: &Server, rows: Vec<Value>) {
    let mut body = catalog();
    body["data"][0]["plugins"] = json!(rows);
    server.set_catalog(body);
}

fn serve_startup_release_files(
    server: &Server,
    row: &Value,
    manifest_version: &str,
) -> (String, String) {
    let (adapter, skill) = files();
    let slug = row["slug"].as_str().unwrap();
    let release_id = row["releaseId"].as_str().unwrap();
    let mut adapter: Value = serde_json::from_str(&adapter).unwrap();
    adapter["slug"] = json!(slug);
    adapter["version"] = json!(manifest_version);
    adapter.as_object_mut().unwrap().remove("mcpServers");
    let adapter = serde_json::to_string_pretty(&adapter).unwrap();
    let skill = skill.replace("name: fallout4", &format!("name: {slug}"));
    server.set(
        &release_path(release_id, "adapter.json"),
        Reply::ok(adapter.clone()),
    );
    server.set(
        &release_path(release_id, "SKILL.md"),
        Reply::ok(skill.clone()),
    );
    (adapter, skill)
}

#[tokio::test]
async fn startup_with_no_installs_skips_network_and_checks_again_after_an_outage() {
    let (_temp, ctx) = context();
    let engine = &ctx.engine;
    let server = Server::new().await;
    server
        .client
        .update_installed_at_startup(engine, |_| panic!("unexpected update"))
        .await
        .unwrap();
    assert!(server.requests().is_empty());
    server
        .client
        .install(engine, PLUGIN_ID, RELEASE_ID)
        .await
        .unwrap();
    server.set(
        GAMES_PATH,
        Reply {
            status: StatusCode::SERVICE_UNAVAILABLE,
            bytes: vec![],
            chunked: false,
        },
    );
    assert!(server
        .client
        .update_installed_at_startup(engine, |_| panic!("unexpected update"))
        .await
        .is_err());
    assert_eq!(
        plugins::list_plugins(&engine.db).unwrap()[0].version,
        "1.0.0"
    );

    let next = startup_release("fallout4", PLUGIN_ID, "32", "1.1.0");
    serve_startup_release_files(&server, &next, "1.1.0");
    serve_startup_releases(&server, vec![next]);
    server
        .client
        .update_installed_at_startup(engine, |slug| assert_eq!(slug, "fallout4"))
        .await
        .unwrap();
    assert_eq!(
        plugins::list_plugins(&engine.db).unwrap()[0].version,
        "1.1.0"
    );
}

#[tokio::test]
async fn startup_requires_both_pinned_identity_fields_and_strictly_higher_precedence() {
    let (_temp, ctx) = context();
    let engine = &ctx.engine;
    let server = Server::new().await;
    server
        .client
        .install(engine, PLUGIN_ID, RELEASE_ID)
        .await
        .unwrap();
    let initial_requests = server.requests().len();
    let changes = [
        ("fallout4", "23", "2.0.0"),
        ("different-slug", PLUGIN_ID, "2.0.0"),
        ("fallout4", PLUGIN_ID, "1.0.0+rebuild"),
        ("fallout4", PLUGIN_ID, "1.0.0-rc.10"),
    ];
    for (slug, id, version) in changes {
        serve_startup_releases(&server, vec![startup_release(slug, id, "32", version)]);
        server
            .client
            .update_installed_at_startup(engine, |_| panic!("unexpected update"))
            .await
            .unwrap();
    }
    assert_eq!(&server.requests()[initial_requests..], &[GAMES_PATH; 4]);
    assert_eq!(
        plugins::list_plugins(&engine.db).unwrap()[0].version,
        "1.0.0"
    );
}

#[tokio::test]
async fn startup_checks_the_catalog_once_and_isolates_a_bad_release_from_another_update() {
    let (_temp, ctx) = context();
    let engine = &ctx.engine;
    let server = Server::new().await;
    let second_id = "23";
    let second_release = startup_release("second-fallout", second_id, "33", "1.0.0-rc.9");
    serve_startup_release_files(&server, &second_release, "1.0.0-rc.9");
    let first_release = candidate(&mut catalog()).clone();
    serve_startup_releases(&server, vec![first_release, second_release]);
    server
        .client
        .install(engine, PLUGIN_ID, RELEASE_ID)
        .await
        .unwrap();
    server
        .client
        .install(engine, second_id, "33")
        .await
        .unwrap();

    // Give the first candidate a version-mismatched manifest, then verify
    // that the remaining candidate can still commit and notify the caller.
    let installed = plugin_workflow::catalog_update_candidates(engine).unwrap();
    assert_eq!(installed.len(), 2);
    let bad = &installed[0];
    let good = &installed[1];
    let bad_id = bad.plugin_id.as_deref().unwrap();
    let good_id = good.plugin_id.as_deref().unwrap();
    let higher = |slug: &str| {
        if slug == "fallout4" {
            "1.0.1"
        } else {
            "1.0.0-rc.10"
        }
    };
    let bad_row = startup_release(&bad.slug, bad_id, "41", higher(&bad.slug));
    serve_startup_release_files(&server, &bad_row, &bad.version);
    let good_row = startup_release(&good.slug, good_id, "42", higher(&good.slug));
    let (adapter, skill) = serve_startup_release_files(&server, &good_row, higher(&good.slug));
    let different_author = startup_release(&good.slug, "53", "43", "99.0.0");
    let unrelated = startup_release("not-installed", "54", "44", "99.0.0");
    // Even an otherwise valid author/release with the same slug cannot replace
    // the recorded ID, and unrelated catalog entries never trigger downloads.
    serve_startup_releases(
        &server,
        vec![different_author, unrelated, bad_row, good_row],
    );
    let initial_requests = server.requests().len();
    let notified = Mutex::new(Vec::new());
    server
        .client
        .update_installed_at_startup(engine, |slug| {
            notified.lock().unwrap().push(slug.to_owned());
        })
        .await
        .unwrap();
    assert_eq!(
        notified.into_inner().unwrap(),
        std::slice::from_ref(&good.slug)
    );
    let requests = server.requests();
    let requests = &requests[initial_requests..];
    assert_eq!(requests[0], GAMES_PATH);
    let mut files = requests[1..].to_vec();
    files.sort();
    assert_eq!(
        files,
        [
            release_path("41", "SKILL.md"),
            release_path("41", "adapter.json"),
            release_path("42", "SKILL.md"),
            release_path("42", "adapter.json"),
        ]
    );
    let installed = plugins::list_plugins(&engine.db).unwrap();
    assert_eq!(
        installed
            .iter()
            .find(|p| p.slug == bad.slug)
            .unwrap()
            .version,
        bad.version
    );
    assert_eq!(
        installed
            .iter()
            .find(|p| p.slug == good.slug)
            .unwrap()
            .version,
        higher(&good.slug)
    );
    let target = engine.paths.plugins().join("games").join(&good.slug);
    assert_eq!(
        fs::read(target.join("adapter.json")).unwrap(),
        adapter.as_bytes()
    );
    assert_eq!(fs::read(target.join("SKILL.md")).unwrap(), skill.as_bytes());
    assert_no_catalog_drafts(engine);
}

#[tokio::test]
async fn startup_requires_a_unique_catalog_slug_for_legacy_install_without_an_id() {
    let (_temp, ctx) = context();
    let engine = &ctx.engine;
    let server = Server::new().await;
    server
        .client
        .install(engine, PLUGIN_ID, RELEASE_ID)
        .await
        .unwrap();
    let deployment = lifecycle_transaction(engine, "catalog_plugin_install");
    assert_eq!(
        engine
            .db
            .conn()
            .execute(
                "DELETE FROM catalog_plugin_origins WHERE transaction_id=?1",
                [&deployment],
            )
            .unwrap(),
        1
    );
    let next = startup_release("fallout4", "23", "32", "2.0.0");
    let older = startup_release("fallout4", "24", "33", "0.9.0");
    let mut hidden = older.clone();
    hidden["hidden"] = json!(true);
    let mut invalid = older.clone();
    invalid["version"] = json!("invalid");
    let initial_requests = server.requests().len();
    for competing in [older, hidden, invalid] {
        serve_startup_releases(&server, vec![next.clone(), competing]);
        server
            .client
            .update_installed_at_startup(engine, |_| panic!("ambiguous legacy slug"))
            .await
            .unwrap();
    }
    assert_eq!(&server.requests()[initial_requests..], &[GAMES_PATH; 3]);
    assert_eq!(
        plugins::list_plugins(&engine.db).unwrap()[0].version,
        "1.0.0"
    );

    serve_startup_release_files(&server, &next, "2.0.0");
    serve_startup_releases(&server, vec![next]);
    let notified = Mutex::new(Vec::new());
    server
        .client
        .update_installed_at_startup(engine, |slug| {
            notified.lock().unwrap().push(slug.to_owned());
        })
        .await
        .unwrap();
    assert_eq!(notified.into_inner().unwrap(), ["fallout4"]);
    assert_eq!(
        plugins::list_plugins(&engine.db).unwrap()[0].version,
        "2.0.0"
    );
    let cloud = plugin_workflow::catalog_update_candidates(engine).unwrap();
    assert_eq!(cloud[0].plugin_id.as_deref(), Some("23"));
}
