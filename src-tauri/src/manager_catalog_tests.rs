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
        ("version",json!("invalid")), ("name",json!("label\u{202e}hidden")), ("slug",json!("../escape"))
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
        let result = parse_catalog(&body.to_string()).unwrap();
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
    ctx.engine.rollback_transaction(&install.plan.id).unwrap();
    plugin_workflow::reconcile_deployments(&ctx.engine).unwrap();
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
