use super::*;
use crate::{
    agent::{
        approval::ApprovalResponse,
        events::AgentEvent,
        tools::{AgentTool, PreparedTool, ToolDefinition, ToolRegistry},
    },
    db::Db,
    mcp::{McpServers, ServerConfig},
    mcp_connections::LOCAL_SERVER_ID,
    settings::{SecretStore, Settings},
    sources::{
        resolutions::ResolutionScope, FileHashes, ResolveResult, ResolvedMod, SourceState,
        SourceStatus,
    },
    storage::AppPaths,
    transaction::FileEngine,
};
use rmcp::{
    model::{ClientRequest, Request},
    service::{PeerRequestOptions, RunningService},
    RoleClient, ServiceExt,
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Condvar,
    },
};
use tokio::sync::{mpsc, Notify};

#[derive(Default)]
struct MockSecrets(StdMutex<HashMap<String, String>>);
impl SecretStore for MockSecrets {
    fn read(&self, key: &str) -> AppResult<Option<String>> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn write(&self, key: &str, value: &str) -> AppResult<()> {
        self.0.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    runtime: AgentRuntime,
    events: mpsc::UnboundedReceiver<AgentEvent>,
}

fn fixture() -> Fixture {
    fixture_with_native_tools(&["read_file"])
}

fn fixture_with_native_tools(names: &[&str]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(dir.path().join("data")).unwrap();
    let db = Db::open(&paths.db_file()).unwrap();
    let settings = Settings::with_secret_store(db.clone(), Arc::new(MockSecrets::default()));
    let (tx, events) = mpsc::unbounded_channel();
    let mut runtime = AgentRuntime::new(
        FileEngine::new(db, paths),
        settings,
        Arc::new(move |event| {
            let _ = tx.send(event.clone());
        }),
    )
    .unwrap();
    let registry = ToolRegistry::default();
    for name in names {
        registry
            .register(runtime.registry.get(name).unwrap())
            .unwrap();
    }
    runtime.registry = registry;
    Fixture {
        _dir: dir,
        runtime,
        events,
    }
}

fn set_mode(runtime: &AgentRuntime, mode: &str) {
    runtime
        .settings
        .set("permission_mode", json!(mode))
        .unwrap();
}

async fn next_event(rx: &mut mpsc::UnboundedReceiver<AgentEvent>, kind: &str) -> AgentEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = rx.recv().await.unwrap();
            if event.kind == kind {
                return event;
            }
        }
    })
    .await
    .unwrap()
}

async fn eventually(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn connect(
    server: &LocalMcpServer,
    runtime: &AgentRuntime,
) -> RunningService<RoleClient, ()> {
    let endpoint = server.status().await.endpoint.unwrap();
    let token = runtime.settings.read_secret(TOKEN_REF).unwrap().unwrap();
    let http = reqwest_mcp::Client::builder()
        .redirect(reqwest_mcp::redirect::Policy::none())
        .build()
        .unwrap();
    let config = StreamableHttpClientTransportConfig::with_uri(endpoint)
        .auth_header(token)
        .reinit_on_expired_session(false)
        .control_request_timeout(Duration::from_secs(1));
    tokio::time::timeout(
        Duration::from_secs(5),
        ().serve(StreamableHttpClientTransport::with_client(http, config)),
    )
    .await
    .unwrap()
    .unwrap()
}

async fn call(client: &RunningService<RoleClient, ()>, name: &str, args: Value) -> CallToolResult {
    tokio::time::timeout(
        Duration::from_secs(5),
        client.peer().call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(args.as_object().unwrap().clone()),
        ),
    )
    .await
    .unwrap()
    .unwrap()
}

struct MockWrite {
    name: &'static str,
    effect: Effect,
    executions: Arc<AtomicUsize>,
    started: Arc<Notify>,
    barrier: Option<Arc<(StdMutex<bool>, Condvar)>>,
    echo: Option<String>,
}

#[async_trait::async_trait]
impl AgentTool for MockWrite {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.into(),
            description: "Local test fixture".into(),
            parameters: json!({"type":"object"}),
            effect: Effect::ManagerWrite,
        }
    }
    async fn prepare(&self, _: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        Ok(PreparedTool {
            preview: json!({"mockPlan":args}),
            effect: self.effect,
            plan: None,
        })
    }
    async fn execute(&self, context: &ToolContext, _: Value, _: PreparedTool) -> AppResult<Value> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        let engine = context.engine.clone();
        let started = self.started.clone();
        let barrier = self.barrier.clone();
        let echo = self.echo.clone();
        tokio::task::spawn_blocking(move || {
            started.notify_one();
            if let Some(barrier) = barrier {
                let (lock, condvar) = &*barrier;
                let released = lock.lock().unwrap();
                let _ = condvar
                    .wait_timeout_while(released, Duration::from_secs(4), |released| !*released)
                    .unwrap();
            }
            std::fs::write(engine.paths.root.join("mock-committed"), "completed")?;
            Ok(json!({"committed":true,"echo":echo}))
        })
        .await
        .map_err(|_| AppError::Other("mock join failed".into()))?
    }
}

fn register_write(
    fixture: &Fixture,
    effect: Effect,
    barrier: Option<Arc<(StdMutex<bool>, Condvar)>>,
    echo: Option<String>,
) -> (Arc<AtomicUsize>, Arc<Notify>) {
    let count = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(Notify::new());
    fixture
        .runtime
        .registry
        .register(Arc::new(MockWrite {
            name: "apply_file_ops",
            effect,
            executions: count.clone(),
            started: started.clone(),
            barrier,
            echo,
        }))
        .unwrap();
    (count, started)
}

#[tokio::test]
async fn loopback_random_port_auth_origin_host_token_storage_and_rotation() {
    let fixture = fixture();
    let server = LocalMcpServer::new(fixture.runtime.clone());
    assert!(!server.status().await.running);
    assert!(fixture
        .runtime
        .settings
        .read_secret(TOKEN_REF)
        .unwrap()
        .is_none());
    let endpoint = server.start().await.unwrap();
    assert!(server.status().await.running);
    assert!(server.start().await.is_err());
    let url = url::Url::parse(&endpoint).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert!(url.port().unwrap() > 0);
    let token = fixture
        .runtime
        .settings
        .read_secret(TOKEN_REF)
        .unwrap()
        .unwrap();
    assert_eq!(token.len(), 64);
    assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(!serde_json::to_string(&server.status().await)
        .unwrap()
        .contains(&token));
    assert!(!format!("{:?}", server.status().await).contains(&token));
    let rows: Vec<String> = fixture
        .runtime
        .engine
        .db
        .conn()
        .prepare("SELECT value FROM settings")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(rows.iter().all(|value| !value.contains(&token)));
    let http = reqwest_mcp::Client::new();
    for method in [
        reqwest_mcp::Method::GET,
        reqwest_mcp::Method::POST,
        reqwest_mcp::Method::DELETE,
    ] {
        assert_eq!(
            http.request(method, &endpoint)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    assert_eq!(
        http.get(&endpoint)
            .bearer_auth("incorrect")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let request = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}});
    let result = http
        .post(&endpoint)
        .bearer_auth(&token)
        .header("origin", "https://untrusted.example")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(request.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(result.status(), 403);
    let result = http
        .post(&endpoint)
        .bearer_auth(&token)
        .header("host", "rebound.example")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(request.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(result.status(), 403);
    let result = http
        .post(&endpoint)
        .bearer_auth(&token)
        .header("content-type", "application/json")
        .body(
            json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"echo":token}})
                .to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(result.status(), 400);
    server.stop().await.unwrap();
    assert!(!server.status().await.running);
    server.start().await.unwrap();
    let rotated = fixture
        .runtime
        .settings
        .read_secret(TOKEN_REF)
        .unwrap()
        .unwrap();
    assert!(token != rotated);
    server.stop().await.unwrap();
}

#[tokio::test]
async fn tools_are_allowlisted_and_real_reads_reuse_desktop_file_guards() {
    let fixture = fixture_with_native_tools(&["read_file", "plan_file_ops", "download_web_mod"]);
    let count = Arc::new(AtomicUsize::new(0));
    for name in [
        "mcp__remote__write",
        "purge_trash",
        "configure_settings",
        "start_process",
    ] {
        fixture
            .runtime
            .registry
            .register(Arc::new(MockWrite {
                name,
                effect: Effect::ManagerWrite,
                executions: count.clone(),
                started: Arc::new(Notify::new()),
                barrier: None,
                echo: None,
            }))
            .unwrap();
    }
    let server = LocalMcpServer::new(fixture.runtime.clone());
    server.start().await.unwrap();
    let client = connect(&server, &fixture.runtime).await;
    let listed = client.peer().list_tools(None).await.unwrap();
    assert_eq!(listed.tools.len(), 4);
    assert!(listed.tools.iter().any(|tool| tool.name == "read_file"));
    let planner = listed
        .tools
        .iter()
        .find(|t| t.name == "plan_file_ops")
        .unwrap();
    assert_eq!(
        planner.input_schema["properties"]["operations"]["items"]["properties"]["op"]["enum"],
        json!(["copy", "move", "mkdir"])
    );
    for name in [
        "mcp__remote__write",
        "purge_trash",
        "configure_settings",
        "start_process",
        "amm_approve",
        "download_web_mod",
    ] {
        assert_eq!(
            call(&client, name, json!({"approved":true})).await.is_error,
            Some(true)
        );
    }
    assert_eq!(count.load(Ordering::SeqCst), 0);
    let file = fixture.runtime.engine.paths.root.join("managed.txt");
    std::fs::write(&file, "shared engine").unwrap();
    let result = call(&client, "read_file", json!({"path":file})).await;
    assert_eq!(result.is_error, Some(false));
    assert!(result.content[0]
        .as_text()
        .unwrap()
        .text
        .contains("shared engine"));
    let outside = fixture._dir.path().join("outside.txt");
    std::fs::write(&outside, "not allowed").unwrap();
    assert_eq!(
        call(&client, "read_file", json!({"path":outside}))
            .await
            .is_error,
        Some(true)
    );
    client.cancel().await.unwrap();
    server.stop().await.unwrap();
}

#[tokio::test]
async fn mod_version_list_exports_nullable_filters_and_preserves_source_status_and_errors() {
    let name = "list_mod_versions";
    let fixture = fixture_with_native_tools(&[name]);
    set_mode(&fixture.runtime, "request_approval");
    let server = LocalMcpServer::new(fixture.runtime.clone());
    server.start().await.unwrap();
    let client = connect(&server, &fixture.runtime).await;

    let listed = client.peer().list_tools(None).await.unwrap();
    let tool = listed.tools.iter().find(|tool| tool.name == name).unwrap();
    let definition = fixture.runtime.registry.get(name).unwrap().definition();
    assert_eq!(
        tool.input_schema.as_ref(),
        definition.parameters.as_object().unwrap()
    );

    let response = call(
        &client,
        name,
        json!({
            "source_id":"3dm","project_id":"123","game_id":null,
            "game_version":null,"loader":null,"offset":10000,"limit":50
        }),
    )
    .await;
    assert_eq!(response.is_error, Some(false));
    let payload = response.structured_content.unwrap();
    assert_eq!(payload["result"]["status"]["source_id"], "3dm");
    assert_eq!(payload["result"]["status"]["state"], "unsupported");
    assert_eq!(payload["result"]["versions"], json!([]));

    let unknown = call(
        &client,
        name,
        json!({"source_id":"unknown","project_id":"123"}),
    )
    .await;
    assert_eq!(unknown.is_error, Some(true));
    assert!(unknown.structured_content.unwrap()["result"]["error"].is_string());

    let conv = payload["conversationId"].as_str().unwrap();
    let approvals: u64 = fixture
        .runtime
        .engine
        .db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM approvals WHERE conversation_id=?1",
            [conv],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(approvals, 0);

    client.cancel().await.unwrap();
    server.stop().await.unwrap();
}

#[tokio::test]
async fn modrinth_graph_is_exported_read_only_and_rejects_local_paths() {
    let name = "resolve_modrinth_dependency_graph";
    let fixture = fixture_with_native_tools(&[name]);
    set_mode(&fixture.runtime, "request_approval");
    let server = LocalMcpServer::new(fixture.runtime.clone());
    server.start().await.unwrap();
    let client = connect(&server, &fixture.runtime).await;

    let listed = client.peer().list_tools(None).await.unwrap();
    let tool = listed.tools.iter().find(|tool| tool.name == name).unwrap();
    let definition = fixture.runtime.registry.get(name).unwrap().definition();
    assert_eq!(
        tool.input_schema.as_ref(),
        definition.parameters.as_object().unwrap()
    );
    let response = call(
        &client,
        name,
        json!({
            "version_id":"demo1234", "game_version":"1.21.1", "loader":"fabric",
            "gameDirectory":"/private/path-not-for-provider"
        }),
    )
    .await;
    assert_eq!(response.is_error, Some(true));
    assert_eq!(
        fixture
            .runtime
            .engine
            .db
            .conn()
            .query_row("SELECT count(*) FROM approvals", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        fixture
            .runtime
            .engine
            .db
            .conn()
            .query_row("SELECT count(*) FROM file_transactions", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        0
    );

    client.cancel().await.unwrap();
    server.stop().await.unwrap();
}

#[tokio::test]
async fn download_mod_rejects_forged_unknown_and_desktop_resolutions_before_approval() {
    let fixture = fixture_with_native_tools(&["download_mod"]);
    set_mode(&fixture.runtime, "request_approval");
    let desktop_resolution = fixture
        .runtime
        .resolutions
        .publish(
            ResolutionScope::Desktop,
            ResolveResult {
                status: SourceStatus {
                    source_id: "modrinth".into(),
                    state: SourceState::Available,
                    message: String::new(),
                },
                download: Some(ResolvedMod {
                    source_id: "modrinth".into(),
                    project_id: "testproject".into(),
                    version_id: "testversion".into(),
                    name: "Fixture mod".into(),
                    version: "1.0.0".into(),
                    download_url: "https://cdn.modrinth.com/data/test/versions/test/file.jar"
                        .into(),
                    filename: "fixture.jar".into(),
                    size: Some(1),
                    hashes: FileHashes::default(),
                    homepage_url: None,
                    warnings: Vec::new(),
                }),
                homepage_url: None,
                dependencies: None,
            },
        )
        .unwrap();
    let desktop_id = desktop_resolution
        .download
        .unwrap()
        .resolution_id
        .to_string();

    let server = LocalMcpServer::new(fixture.runtime.clone());
    server.start().await.unwrap();
    let client = connect(&server, &fixture.runtime).await;
    let listed = client.peer().list_tools(None).await.unwrap();
    let tool = listed
        .tools
        .iter()
        .find(|tool| tool.name == "download_mod")
        .unwrap();
    let definition = fixture
        .runtime
        .registry
        .get("download_mod")
        .unwrap()
        .definition();
    assert_eq!(
        tool.input_schema.as_ref(),
        definition.parameters.as_object().unwrap()
    );
    assert_eq!(
        definition.parameters["properties"],
        json!({"resolution_id":{"type":"string","format":"uuid"}})
    );
    assert_eq!(definition.parameters["required"], json!(["resolution_id"]));
    assert_eq!(definition.parameters["additionalProperties"], false);
    for args in [
        json!({"resolved":{
            "source_id":"modrinth","project_id":"testproject","version_id":"testversion",
            "name":"Fixture mod","version":"1.0.0",
            "download_url":"https://cdn.modrinth.com/data/test/versions/test/file.jar",
            "filename":"fixture.jar","size":1,"hashes":{},"homepage_url":null,"warnings":[]
        }}),
        json!({"resolution_id":"00000000-0000-4000-8000-000000000000"}),
        json!({"resolution_id":desktop_id}),
    ] {
        assert_eq!(
            call(&client, "download_mod", args).await.is_error,
            Some(true)
        );
    }
    let approvals: u64 = fixture
        .runtime
        .engine
        .db
        .conn()
        .query_row("SELECT count(*) FROM approvals", [], |row| {
            row.get::<_, u64>(0)
        })
        .unwrap();
    assert_eq!(
        approvals, 0,
        "invalid, unknown, and cross-scope references must fail before requesting approval"
    );
    assert!(crate::artifacts::list(&fixture.runtime.engine.db)
        .unwrap()
        .is_empty());

    client.cancel().await.unwrap();
    server.stop().await.unwrap();
}

#[tokio::test]
async fn modrinth_dependency_check_is_listed_and_runs_without_approval() {
    let fixture = fixture();
    crate::agent::install_tools::register(&fixture.runtime.registry).unwrap();
    set_mode(&fixture.runtime, "request_approval");
    let engine = &fixture.runtime.engine;
    let game_root = fixture._dir.path().join("game");
    std::fs::create_dir(&game_root).unwrap();
    let game = crate::games::register_game(&engine.db, &game_root, None).unwrap();
    let server = LocalMcpServer::new(fixture.runtime.clone());
    server.start().await.unwrap();
    let client = connect(&server, &fixture.runtime).await;

    let name = "check_modrinth_dependencies";
    let listed = client.peer().list_tools(None).await.unwrap();
    let tool = listed.tools.iter().find(|tool| tool.name == name).unwrap();
    let definition = fixture.runtime.registry.get(name).unwrap().definition();
    assert_eq!(
        tool.input_schema.as_ref(),
        definition.parameters.as_object().unwrap()
    );
    let schema = &definition.parameters["properties"]["request"];
    assert_eq!(schema["required"], json!(["gameId", "dependencies"]));
    let dependencies = &schema["properties"]["dependencies"];
    assert_eq!(dependencies["type"], json!(["array", "null"]));
    assert_eq!(dependencies["maxItems"], 128);

    let request = json!({"gameId": game.id, "dependencies": null});
    let expected = crate::install_dependencies::check(
        engine,
        &serde_json::from_value(request.clone()).unwrap(),
    )
    .unwrap();
    let response = call(&client, name, json!({"request": request})).await;
    assert_eq!(response.is_error, Some(false));
    assert_eq!(
        response.structured_content.unwrap()["result"],
        serde_json::to_value(expected).unwrap()
    );
    client.cancel().await.unwrap();
    server.stop().await.unwrap();
}

#[tokio::test]
async fn diagnose_game_exports_fabric_failure_evidence_without_approval() {
    use std::io::Write;

    let fixture = fixture();
    crate::agent::diagnostic_tools::register(&fixture.runtime.registry).unwrap();
    set_mode(&fixture.runtime, "request_approval");
    let engine = &fixture.runtime.engine;
    crate::plugins::seed_builtins(&engine.db, &engine.paths).unwrap();
    let root = fixture._dir.path().join("fabric-instance");
    std::fs::create_dir_all(root.join("logs")).unwrap();
    std::fs::create_dir_all(root.join("mods")).unwrap();
    let path = root.join("mods/fixture.jar");
    let mut jar = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    jar.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    jar.write_all(br#"{"schemaVersion":1,"id":"fixture","version":"1.2.3"}"#)
        .unwrap();
    jar.finish().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(root.join("options.txt"), "fixture").unwrap();
    let log = b"[12:34:56] [main/INFO]: Loading Minecraft 1.21.1 with Fabric Loader 0.16.14\n[12:34:57] [main/WARN]: Mod resolution failed\n";
    std::fs::write(root.join("logs/latest.log"), log).unwrap();
    let game = crate::games::register_game(&engine.db, &root, Some("minecraft-java")).unwrap();
    let server = LocalMcpServer::new(fixture.runtime.clone());
    server.start().await.unwrap();
    let client = connect(&server, &fixture.runtime).await;
    let response = call(&client, "diagnose_game", json!({"game_id": game.id})).await;
    assert_eq!(response.is_error, Some(false));
    let data = response.structured_content.unwrap();
    assert_eq!(
        data["result"]["fabricStartup"]["status"],
        "resolution_failed"
    );
    assert_eq!(data["result"]["evidence"][0]["line"], 2);
    assert_eq!(data["result"]["fabricDisk"]["jars"][0]["id"], "fixture");
    assert_eq!(
        data["result"]["fabricDisk"]["jars"][0]["logRelation"]["status"],
        "unverified"
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(std::fs::read(root.join("logs/latest.log")).unwrap(), log);
    client.cancel().await.unwrap();
    server.stop().await.unwrap();
}

#[tokio::test]
async fn verify_install_files_is_listed_and_returns_evidence_without_approval() {
    let fixture = fixture();
    crate::agent::install_tools::register(&fixture.runtime.registry).unwrap();
    set_mode(&fixture.runtime, "request_approval");
    let engine = &fixture.runtime.engine;
    let game_root = fixture._dir.path().join("game");
    std::fs::create_dir(&game_root).unwrap();
    let game = crate::games::register_game(&engine.db, &game_root, None).unwrap();
    {
        let conn = engine.db.conn();
        conn.execute(
            "INSERT INTO installs(id,game_id,name,install_root,installed_at,updated_at)
            VALUES ('i_fixture',?1,'Fixture',?2,'now','now')",
            rusqlite::params![game.id, game.root_path],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO file_transactions(id,kind,status,plan_json,created_at)
            VALUES ('txn_fixture','install','committed','{}','now');
            INSERT INTO install_transactions(install_id,transaction_id,action,created_at)
            VALUES ('i_fixture','txn_fixture','install','now');",
        )
        .unwrap();
    }
    let server = LocalMcpServer::new(fixture.runtime.clone());
    server.start().await.unwrap();
    let client = connect(&server, &fixture.runtime).await;
    let name = "verify_install_files";
    let listed = client.peer().list_tools(None).await.unwrap();
    assert!(listed.tools.iter().any(|tool| tool.name == name));
    let result = call(
        &client,
        name,
        json!({"game_id":game.id,"install_id":"i_fixture"}),
    )
    .await;
    assert_eq!(result.is_error, Some(false));
    let data = result.structured_content.unwrap();
    assert_eq!(data["result"]["installId"], "i_fixture");
    assert_eq!(data["result"]["evidence"]["status"], "unverified");
    client.cancel().await.unwrap();
    server.stop().await.unwrap();
}

#[tokio::test]
async fn inspect_install_is_listed_and_returns_read_only_review_without_approval() {
    let fixture = fixture();
    crate::agent::install_tools::register(&fixture.runtime.registry).unwrap();
    set_mode(&fixture.runtime, "request_approval");
    let engine = &fixture.runtime.engine;
    let game_root = fixture._dir.path().join("game");
    std::fs::create_dir(&game_root).unwrap();
    let game_root = std::fs::canonicalize(game_root).unwrap();
    let game = crate::games::register_game(&engine.db, &game_root, None).unwrap();
    let file = game_root.join("fixture.jar");
    std::fs::write(&file, b"owned").unwrap();
    let manifest = json!([{
        "path": file,
        "sha256": crate::util::sha256_file(&file).unwrap(),
        "size": 5,
        "before": null
    }]);
    engine
        .db
        .conn()
        .execute(
            "INSERT INTO installs(id,game_id,name,status,install_root,files_json,metadata_json,installed_at,updated_at)
             VALUES ('i_inspect',?1,'Fixture','enabled',?2,?3,'{}','now','now')",
            rusqlite::params![game.id, game.root_path, manifest.to_string()],
        )
        .unwrap();

    let server = LocalMcpServer::new(fixture.runtime.clone());
    server.start().await.unwrap();
    let client = connect(&server, &fixture.runtime).await;
    let name = "inspect_install";
    let listed = client.peer().list_tools(None).await.unwrap();
    let tool = listed.tools.iter().find(|tool| tool.name == name).unwrap();
    let definition = fixture.runtime.registry.get(name).unwrap().definition();
    assert_eq!(
        tool.input_schema.as_ref(),
        definition.parameters.as_object().unwrap()
    );
    assert_eq!(definition.effect, Effect::Read);

    let result = call(
        &client,
        name,
        json!({"game_id":game.id,"install_id":"i_inspect"}),
    )
    .await;
    assert_eq!(result.is_error, Some(false));
    let data = result.structured_content.unwrap();
    assert_eq!(data["result"]["installId"], "i_inspect");
    assert_eq!(data["result"]["gameId"], game.id);
    assert_eq!(data["result"]["status"], "enabled");
    assert_eq!(data["result"]["canUninstall"], true);
    assert_eq!(
        data["result"]["reconciliation"]["recommendedAction"],
        "safe_uninstall"
    );
    assert!(data["result"]["reconciliation"]["rollbackTransactionId"].is_null());
    assert_eq!(data["result"]["files"][0]["state"], "owned");
    assert_eq!(data["result"]["files"][0]["replaced"], false);
    assert_eq!(std::fs::read(&file).unwrap(), b"owned");

    let approvals: u64 = engine
        .db
        .conn()
        .query_row("SELECT COUNT(*) FROM approvals", [], |row| row.get(0))
        .unwrap();
    assert_eq!(approvals, 0);
    let status: String = engine
        .db
        .conn()
        .query_row(
            "SELECT status FROM installs WHERE id='i_inspect'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "enabled");

    client.cancel().await.unwrap();
    server.stop().await.unwrap();
}

#[tokio::test]
async fn direct_tools_wait_for_shared_desktop_approval_and_persist_same_events() {
    let mut fixture = fixture();
    let (count, _) = register_write(&fixture, Effect::ManagerWrite, None, None);
    let server = LocalMcpServer::new(fixture.runtime.clone());
    server.start().await.unwrap();
    let client = connect(&server, &fixture.runtime).await;
    let session = call(&client, "amm_session", json!({"title":"External test"}))
        .await
        .structured_content
        .unwrap();
    let conv = session["conversationId"].as_str().unwrap();
    History::new(fixture.runtime.engine.db.clone())
        .set_permission_mode(conv, Some("full_trust"))
        .unwrap();
    let peer = client.peer().clone();
    let job = tokio::spawn(async move {
        peer.call_tool(
            CallToolRequestParams::new("apply_file_ops").with_arguments(
                json!({"permission_mode":"full_trust","approved":true})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap()
    });
    let approval = next_event(&mut fixture.events, "approval-request").await;
    assert_eq!(approval.conversation_id, conv);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert!(!job.is_finished());
    assert_eq!(
        call(
            &client,
            "amm_approve",
            json!({"approved":true,"approvalId":approval.payload["approvalId"]})
        )
        .await
        .is_error,
        Some(true)
    );
    let events = call(&client, "amm_events", json!({"after":0,"limit":100}))
        .await
        .structured_content
        .unwrap();
    assert!(events["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["kind"] == "approval-request"));
    fixture
        .runtime
        .respond_approval(
            conv,
            approval.payload["approvalId"].as_str().unwrap(),
            ApprovalResponse {
                approved: true,
                answer: Value::Null,
            },
        )
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), job)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let messages = History::new(fixture.runtime.engine.db.clone())
        .get_conversation(conv)
        .unwrap()
        .messages;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "assistant");
    assert_eq!(messages[1].role, "tool");
    assert!(messages[1].content.contains("committed"));
    let result_event = next_event(&mut fixture.events, "tool-result").await;
    assert_eq!(result_event.conversation_id, conv);
    assert_eq!(result_event.turn_id, approval.turn_id);
    client.cancel().await.unwrap();
    server.stop().await.unwrap();
}

#[tokio::test]
async fn global_auto_and_full_trust_match_policy_but_forbidden_never_executes() {
    for (mode, effect, expected) in [
        ("auto_approve", Effect::GameWrite, 1),
        ("full_trust", Effect::ManagerWrite, 1),
        ("full_trust", Effect::Forbidden, 0),
    ] {
        let fixture = fixture();
        set_mode(&fixture.runtime, mode);
        let (count, _) = register_write(&fixture, effect, None, None);
        let server = LocalMcpServer::new(fixture.runtime.clone());
        server.start().await.unwrap();
        let client = connect(&server, &fixture.runtime).await;
        let result = call(&client, "apply_file_ops", json!({})).await;
        assert_eq!(count.load(Ordering::SeqCst), expected);
        assert_eq!(result.is_error, Some(expected == 0));
        client.cancel().await.unwrap();
        server.stop().await.unwrap();
    }
}

#[tokio::test]
async fn cancellation_cleans_shared_pending_approval_without_executing() {
    let mut fixture = fixture();
    let (count, _) = register_write(&fixture, Effect::ManagerWrite, None, None);
    let server = LocalMcpServer::new(fixture.runtime.clone());
    server.start().await.unwrap();
    let client = connect(&server, &fixture.runtime).await;
    let request = client
        .peer()
        .send_cancellable_request(
            ClientRequest::CallToolRequest(Request::new(CallToolRequestParams::new(
                "apply_file_ops",
            ))),
            PeerRequestOptions::no_options(),
        )
        .await
        .unwrap();
    let event = next_event(&mut fixture.events, "approval-request").await;
    request
        .cancel(Some("test cancellation".into()))
        .await
        .unwrap();
    let resolved = next_event(&mut fixture.events, "approval-resolved").await;
    assert_eq!(resolved.payload["status"], "cancelled");
    next_event(&mut fixture.events, "done").await;
    assert_eq!(count.load(Ordering::SeqCst), 0);
    let status: String = fixture
        .runtime
        .engine
        .db
        .conn()
        .query_row(
            "SELECT status FROM tool_calls WHERE id=?1",
            [event.payload["toolCallId"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "cancelled");
    client.cancel().await.unwrap();
    server.stop().await.unwrap();
}

#[tokio::test]
async fn cancelled_stop_command_still_drains_and_persists_inflight_blocking_commit() {
    let fixture = fixture();
    set_mode(&fixture.runtime, "full_trust");
    let barrier = Arc::new((StdMutex::new(false), Condvar::new()));
    let (_, started) = register_write(&fixture, Effect::ManagerWrite, Some(barrier.clone()), None);
    let server = LocalMcpServer::new(fixture.runtime.clone());
    let connections = McpConnections::default();
    server.start_desktop(&connections).await.unwrap();
    let client = connect(&server, &fixture.runtime).await;
    let request = client
        .peer()
        .send_cancellable_request(
            ClientRequest::CallToolRequest(Request::new(CallToolRequestParams::new(
                "apply_file_ops",
            ))),
            PeerRequestOptions::no_options(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    request.cancel(None).await.unwrap();
    let owner = server.clone();
    let stop_connections = connections.clone();
    let stop = tokio::spawn(async move { owner.stop_desktop(&stop_connections).await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!stop.is_finished());
    let status = server.status().await;
    assert!(status.stopping);
    assert!(!status.startup_requested);
    assert_eq!(status.active_calls, 1);
    stop.abort();
    let _ = stop.await;
    *barrier.0.lock().unwrap() = true;
    barrier.1.notify_all();
    eventually(|| {
        fixture
            .runtime
            .engine
            .paths
            .root
            .join("mock-committed")
            .exists()
    })
    .await;
    tokio::time::timeout(Duration::from_secs(8), async {
        while server.status().await.stopping {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(connections.list().await.is_empty());
    let status: String = fixture
        .runtime
        .engine
        .db
        .conn()
        .query_row(
            "SELECT status FROM tool_calls ORDER BY rowid DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "done");
    let result: String = fixture
        .runtime
        .engine
        .db
        .conn()
        .query_row(
            "SELECT content FROM messages WHERE role='tool' ORDER BY rowid DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(result.contains("committed"));
    let _ = client.cancel().await;
    server.stop().await.unwrap();
}

#[tokio::test]
async fn independent_sessions_and_auth_token_echo_never_enter_history_or_events() {
    let fixture = fixture();
    set_mode(&fixture.runtime, "full_trust");
    let server = LocalMcpServer::new(fixture.runtime.clone());
    server.start().await.unwrap();
    let token = fixture
        .runtime
        .settings
        .read_secret(TOKEN_REF)
        .unwrap()
        .unwrap();
    register_write(&fixture, Effect::ManagerWrite, None, Some(token.clone()));
    let first = connect(&server, &fixture.runtime).await;
    let second = connect(&server, &fixture.runtime).await;
    let one = call(&first, "amm_session", json!({}))
        .await
        .structured_content
        .unwrap();
    let two = call(&second, "amm_session", json!({}))
        .await
        .structured_content
        .unwrap();
    assert_ne!(one["conversationId"], two["conversationId"]);
    let result = call(&first, "apply_file_ops", json!({})).await;
    assert!(!serde_json::to_string(&result).unwrap().contains(&token));
    assert!(serde_json::to_string(&result)
        .unwrap()
        .contains("[REDACTED]"));
    {
        let conn = fixture.runtime.engine.db.conn();
        for query in [
            "SELECT content FROM messages",
            "SELECT payload_json FROM agent_events",
            "SELECT result_json FROM tool_calls",
        ] {
            let rows: Vec<String> = conn
                .prepare(query)
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert!(rows.iter().all(|row| !row.contains(&token)));
        }
    }
    first.cancel().await.unwrap();
    second.cancel().await.unwrap();
    server.stop().await.unwrap();
}

#[tokio::test]
async fn stdio_proxy_forwards_real_protocol_to_existing_authenticated_http_server() {
    let fixture = fixture();
    let server = LocalMcpServer::new(fixture.runtime.clone());
    let endpoint = server.start().await.unwrap();
    let token = fixture
        .runtime
        .settings
        .read_secret(TOKEN_REF)
        .unwrap()
        .unwrap();
    let (external, proxy) = tokio::io::duplex(128 * 1024);
    let (input, output) = tokio::io::split(proxy);
    let job = tokio::spawn(async move { run_proxy_io(&endpoint, &token, input, output).await });
    let (input, output) = tokio::io::split(external);
    let client = tokio::time::timeout(
        Duration::from_secs(5),
        ().serve(AsyncRwTransport::<RoleClient, _, _>::new(input, output)),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(client
        .peer()
        .list_tools(None)
        .await
        .unwrap()
        .tools
        .iter()
        .any(|tool| tool.name == "read_file"));
    let result = call(&client, "amm_session", json!({"title":"Proxied"})).await;
    let id = result.structured_content.unwrap()["conversationId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        History::new(fixture.runtime.engine.db.clone())
            .get_conversation(&id)
            .unwrap()
            .conversation
            .title,
        "Proxied"
    );
    client.cancel().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), job)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_ok());
    server.stop().await.unwrap();
}

#[tokio::test]
async fn proxy_rejects_remote_credential_urls_and_invalid_tokens_without_connecting() {
    for (endpoint, token) in [
        ("https://example.com/mcp", "bad"),
        ("http://localhost:1234/mcp", "bad"),
        ("http://127.0.0.1:1234/mcp?token=secret", "bad"),
        ("http://127.0.0.1:1234/mcp", "bad"),
    ] {
        let result = run_proxy_io(endpoint, token, tokio::io::empty(), tokio::io::sink()).await;
        assert!(matches!(result, Err(AppError::Invalid(_))));
    }
}

#[tokio::test]
async fn started_local_server_is_registered_for_the_agent_and_revoked_on_stop() {
    let fixture = fixture();
    let runtime = &fixture.runtime;
    let server = LocalMcpServer::new(runtime.clone());
    let connections = McpConnections::default();
    let status = server.start_desktop(&connections).await.unwrap();
    assert!(status.running && status.startup_requested);
    let endpoint = status.endpoint.unwrap();
    let entries = connections.list().await;
    assert_eq!(entries.len(), 1);
    let info = &entries[0];
    assert_eq!(info.server_id, LOCAL_SERVER_ID);
    assert!(info
        .tools
        .iter()
        .any(|name| name.starts_with("mcp__amm_local__")));
    for name in &info.tools {
        assert!(runtime.registry.get(name).is_ok());
    }
    let servers = McpServers::new(runtime.engine.db.clone());
    let stored = servers.get(LOCAL_SERVER_ID).unwrap();
    assert_eq!(stored.url.as_deref(), Some(endpoint.as_str()));
    assert_eq!(stored.bearer_secret_ref.as_deref(), Some(TOKEN_REF));
    // The private preference is a boolean only, and public settings saves leave it alone.
    runtime
        .settings
        .save(&runtime.settings.load().unwrap())
        .unwrap();
    let preference: String = runtime
        .settings
        .db
        .conn()
        .query_row(
            "SELECT value FROM settings WHERE key=?1",
            [STARTUP_REQUEST_KEY],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(preference, "true");
    assert!(runtime.settings.get(STARTUP_REQUEST_KEY).is_err());

    // A late startup hook must not detach an explicit desktop start that already won.
    server.restore_if_requested(&connections).await.unwrap();
    assert_eq!(
        server.status().await.endpoint.as_deref(),
        Some(endpoint.as_str())
    );
    server.stop_desktop(&connections).await.unwrap();
    for name in &info.tools {
        assert!(runtime.registry.get(name).is_err());
    }
    assert!(matches!(
        servers.get(LOCAL_SERVER_ID),
        Err(AppError::NotFound(_))
    ));
    assert!(connections.list().await.is_empty());
    assert!(!server.startup_requested().unwrap());
    assert!(!server.status().await.running);
    server.stop_desktop(&connections).await.unwrap();
    LocalMcpServer::new(runtime.clone())
        .restore_if_requested(&connections)
        .await
        .unwrap();
    assert!(connections.list().await.is_empty());
}

#[tokio::test]
async fn restored_local_server_replaces_stale_entry_and_rotates_token() {
    let fixture = fixture();
    let runtime = &fixture.runtime;
    let servers = McpServers::new(runtime.engine.db.clone());
    let connections = McpConnections::default();
    let first = LocalMcpServer::new(runtime.clone());
    first.start_desktop(&connections).await.unwrap();
    let old_token = runtime.settings.read_secret(TOKEN_REF).unwrap().unwrap();
    // Shut down the listener without the desktop opt-out, then leave an expired
    // config on disk as if the previous process had exited without cleaning up.
    first.stop_with_agent(&connections).await.unwrap();
    drop(first);
    let stale = ServerConfig::http(
        LOCAL_SERVER_ID,
        "previous session",
        "http://127.0.0.1:1/mcp",
    );
    servers.create(&stale).unwrap();

    let mut next_runtime = runtime.clone();
    next_runtime.settings = Settings::with_secret_store(
        Db::open(&runtime.engine.paths.db_file()).unwrap(),
        Arc::new(MockSecrets::default()),
    );
    let next = LocalMcpServer::new(next_runtime.clone());
    assert!(next.startup_requested().unwrap());
    assert!(!next.status().await.running);
    let connections = McpConnections::default();
    next.restore_if_requested(&connections).await.unwrap();
    let status = next.status().await;
    assert!(status.running && status.startup_requested);
    assert_eq!(servers.get(LOCAL_SERVER_ID).unwrap().url, status.endpoint);
    assert_ne!(status.endpoint, stale.url);
    let token = next_runtime
        .settings
        .read_secret(TOKEN_REF)
        .unwrap()
        .unwrap();
    assert_ne!(token, old_token);
    assert_eq!(connections.list().await[0].server_id, LOCAL_SERVER_ID);
    next.stop_desktop(&connections).await.unwrap();
    assert!(!next.startup_requested().unwrap());
}

#[tokio::test]
async fn fresh_local_server_cleans_stale_entry_without_starting() {
    let fixture = fixture();
    let runtime = &fixture.runtime;
    let servers = McpServers::new(runtime.engine.db.clone());
    let stale = ServerConfig::http(
        LOCAL_SERVER_ID,
        "previous session",
        "http://127.0.0.1:1/mcp",
    );
    servers.create(&stale).unwrap();
    let server = LocalMcpServer::new(runtime.clone());
    let connections = McpConnections::default();
    assert!(!server.startup_requested().unwrap());
    server.restore_if_requested(&connections).await.unwrap();
    let status = server.status().await;
    assert!(!status.running && !status.startup_requested && status.endpoint.is_none());
    assert!(connections.list().await.is_empty());
    assert!(matches!(
        servers.get(LOCAL_SERVER_ID),
        Err(AppError::NotFound(_))
    ));
    assert!(runtime.settings.read_secret(TOKEN_REF).unwrap().is_none());
}

#[tokio::test]
async fn local_connection_failure_rolls_back_and_saved_intent_can_be_cancelled() {
    let fixture = fixture();
    let runtime = &fixture.runtime;
    let connections = McpConnections::default();
    let server = LocalMcpServer::new(runtime.clone());
    runtime
        .engine
        .db
        .conn()
        .execute_batch(
            "CREATE TRIGGER reject_local_connection BEFORE INSERT ON mcp_servers
         WHEN NEW.id='amm_local' BEGIN SELECT RAISE(ABORT, 'test connection failure'); END;",
        )
        .unwrap();
    let error = server.start_desktop(&connections).await.unwrap_err();
    assert!(error
        .to_string()
        .contains("无法将本地 MCP 服务连接到 Agent"));
    assert!(!server.startup_requested().unwrap());
    assert!(server.status().await.endpoint.is_none());
    assert!(connections.list().await.is_empty());

    // Simulate startup with an existing opt-in while the same failure persists.
    server.remember_startup_request().unwrap();
    let error = server.restore_if_requested(&connections).await.unwrap_err();
    assert!(error.to_string().contains("test connection failure"));
    let status = server.status().await;
    assert!(status.startup_requested && !status.running && status.endpoint.is_none());
    assert_eq!(
        serde_json::to_value(status).unwrap()["startupRequested"],
        json!(true)
    );
    assert!(connections.list().await.is_empty());
    server.stop_desktop(&connections).await.unwrap();
    assert!(!server.startup_requested().unwrap());
    server.restore_if_requested(&connections).await.unwrap();
    assert!(!server.status().await.running);
}

#[tokio::test]
async fn failure_to_save_local_intent_rolls_back_the_agent_and_listener() {
    let fixture = fixture();
    let runtime = &fixture.runtime;
    let servers = McpServers::new(runtime.engine.db.clone());
    let connections = McpConnections::default();
    let server = LocalMcpServer::new(runtime.clone());
    runtime
        .settings
        .db
        .conn()
        .execute_batch(&format!(
            "CREATE TRIGGER reject_local_startup_request BEFORE INSERT ON settings
         WHEN NEW.key='{STARTUP_REQUEST_KEY}' BEGIN SELECT RAISE(ABORT, 'test save failure'); END;"
        ))
        .unwrap();
    let error = server.start_desktop(&connections).await.unwrap_err();
    assert!(error.to_string().contains("无法保存本地 MCP 启动偏好"));
    assert!(server.status().await.endpoint.is_none());
    assert!(!server.startup_requested().unwrap());
    assert!(connections.list().await.is_empty());
    assert!(!runtime.registry.definitions().iter().any(|definition| {
        definition["function"]["name"]
            .as_str()
            .is_some_and(|name| name.starts_with("mcp__amm_local__"))
    }));
    assert!(matches!(
        servers.get(LOCAL_SERVER_ID),
        Err(AppError::NotFound(_))
    ));
}
