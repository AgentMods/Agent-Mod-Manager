//! Offline desktop IPC regression using the stock GameBanana configuration.
use super::{browse_invoke, browse_ipc_window};
use crate::agent::runtime::AgentRuntime;
use crate::app::AppServices;
use crate::mcp::{tests::MockSecrets, McpRuntime, McpServers};
use crate::settings::{AppSettings, Settings, MODEL_API_KEY};
use crate::storage::AppPaths;
use axum::{
    body::to_bytes,
    extract::{Request, State},
    http::header,
    routing::post,
    Router,
};
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::{ErrorKind, Write},
    net::SocketAddr,
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::async_runtime::{block_on, spawn};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;
use zip::{write::SimpleFileOptions, ZipWriter};

const SOURCE: &str = "gamebanana";
const LIBRARY_ID: &str = "agent-match-fixture";
const LIBRARY_NAME: &str = "星环遗迹：决定版";
const ALIAS: &str = "Signal Ruins Definitive Edition";
const DECOY: &str = "Signal Ruins Online";
const TARGET: &str = "Signal Ruins (Definitive Edition)";
const TARGET_ID: u64 = 89;
const PROJECT: &str = "Signal Ruins Load Order Studio";
const MODEL: &str = "browse-fixture-model";
const MODEL_KEY: &str = "browse-fixture-key";
type Calls = Arc<Mutex<Vec<Value>>>;

// The stock command and arguments stay in place. Build a small portable stdio
// relay for the private runtime archive so the real launcher and connection run.
fn bundled_fixture(root: &Path, address: SocketAddr) -> McpRuntime {
    let source = root.join("relay.rs");
    fs::write(
        &source,
        r#"
use std::{io, net::{Shutdown, TcpStream}, thread};

fn main() -> io::Result<()> {
    let server = TcpStream::connect(env!("AMM_TEST_MCP_ADDRESS"))?;
    let mut writer = server.try_clone()?;
    thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut writer);
        let _ = writer.shutdown(Shutdown::Write);
    });
    io::copy(&mut &server, &mut io::stdout().lock())?;
    Ok(())
}
"#,
    )
    .unwrap();
    let suffix = std::env::consts::EXE_SUFFIX;
    let executable = root.join(format!("uvx{suffix}"));
    let compilation = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .env("AMM_TEST_MCP_ADDRESS", address.to_string())
        .output()
        .unwrap();
    assert!(
        compilation.status.success(),
        "failed to build GameBanana test relay: {}",
        String::from_utf8_lossy(&compilation.stderr),
    );
    let archive = root.join("runtime.zip");
    let mut zip = ZipWriter::new(File::create(&archive).unwrap());
    for path in [
        format!("bin/node{suffix}"),
        format!("bin/uv{suffix}"),
        "npm/bin/npx-cli.js".into(),
        "npm/bin/npm-cli.js".into(),
    ] {
        zip.start_file(path, SimpleFileOptions::default()).unwrap();
    }
    zip.start_file(format!("bin/uvx{suffix}"), SimpleFileOptions::default())
        .unwrap();
    zip.write_all(&fs::read(executable).unwrap()).unwrap();
    zip.finish().unwrap();
    McpRuntime::bundled(archive, root.join("runtime-cache"))
}

async fn serve_catalog(listener: TcpListener, calls: Calls) -> std::io::Result<()> {
    let (socket, _) = listener.accept().await?;
    let (read, mut write) = socket.into_split();
    let mut requests = BufReader::new(read).lines();
    let schemas: Value =
        serde_json::from_str(include_str!("../../mcp/browse/gamebanana.json")).unwrap();
    let tools = schemas
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, schema)| json!({"name": name, "inputSchema": schema}))
        .collect::<Vec<_>>();
    while let Some(line) = requests.next_line().await? {
        let request: Value = serde_json::from_str(&line).unwrap();
        let result = match request["method"].as_str().unwrap() {
            "initialize" => json!({
                "protocolVersion": request["params"]["protocolVersion"],
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "GameBanana Fixture", "version": "1"},
            }),
            "notifications/initialized" => continue,
            "tools/list" => json!({"tools": tools}),
            "tools/call" => catalog_result(&request["params"], &calls),
            other => panic!("unexpected GameBanana MCP method {other}"),
        };
        let response = json!({"jsonrpc": "2.0", "id": request["id"], "result": result});
        write.write_all(format!("{response}\n").as_bytes()).await?;
    }
    Ok(())
}

fn catalog_result(params: &Value, calls: &Calls) -> Value {
    let name = params["name"].as_str().unwrap();
    let args = &params["arguments"];
    calls.lock().unwrap().push(json!([name, args]));
    let records = match name {
        "search_games" => match args["query"].as_str().unwrap() {
            LIBRARY_NAME => json!([]),
            ALIAS => json!([
                {"_idRow": 71, "_sName": DECOY},
                {"_idRow": TARGET_ID, "_sName": TARGET},
            ]),
            other => panic!("unexpected GameBanana game query {other}"),
        },
        "browse_mods" if args["game_id"] == TARGET_ID => json!([
            {"_idRow": 760, "_idGameRow": TARGET_ID, "_sModelName": "Mod", "_sName": PROJECT},
        ]),
        other => panic!("unexpected GameBanana MCP call {other} with arguments {args}"),
    };
    json!({
        "content": [],
        "structuredContent": {"_aRecords": records, "_aMetadata": {"_bIsComplete": true}},
    })
}

async fn model_reply(
    State(calls): State<Calls>,
    request: Request,
) -> ([(header::HeaderName, &'static str); 1], String) {
    assert_eq!(
        request.headers()[header::AUTHORIZATION],
        format!("Bearer {MODEL_KEY}")
    );
    let body = to_bytes(request.into_body(), 64 * 1024).await.unwrap();
    let request: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(request["model"], MODEL);
    let payload: Value =
        serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
    let reply = if payload.get("options").is_some() {
        calls
            .lock()
            .unwrap()
            .push(json!(["model_suggestion", payload]));
        json!({"index": 1})
    } else if payload.get("items").is_some() {
        if payload.get("targetLocale").is_some() {
            calls
                .lock()
                .unwrap()
                .push(json!(["model_translation", payload]));
            json!({"items": []})
        } else {
            calls
                .lock()
                .unwrap()
                .push(json!(["model_classification", payload]));
            json!({"items": [{"index": 0, "typeKey": "tools"}]})
        }
    } else {
        calls
            .lock()
            .unwrap()
            .push(json!(["model_aliases", payload]));
        json!({"officialTitles": [ALIAS], "abbreviations": [], "baseTitle": null})
    };
    let event = json!({
        "choices": [{"index": 0, "delta": {"content": reply.to_string()}, "finish_reason": "stop"}],
    });
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        format!("data: {event}\n\ndata: [DONE]\n\n"),
    )
}

#[test]
fn agent_alias_and_suggestion_select_gamebanana_catalog_through_desktop_ipc() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let calls = Calls::default();
    let stop = CancellationToken::new();
    let _shutdown_on_drop = stop.clone().drop_guard();
    let (catalog, model) = block_on(async {
        (
            TcpListener::bind("127.0.0.1:0").await.unwrap(),
            TcpListener::bind("127.0.0.1:0").await.unwrap(),
        )
    });
    let model_url = format!("http://{}/v1", model.local_addr().unwrap());
    let runtime = bundled_fixture(&root, catalog.local_addr().unwrap());
    let services =
        AppServices::initialize_paths(AppPaths::new(root.join("data")).unwrap(), runtime).unwrap();
    let db = services.engine.db.clone();
    db.conn()
        .execute(
            "INSERT INTO games (id,name,slug,root_path,created_at,updated_at)
             VALUES (?1,?2,'local-signal-fixture',?3,'now','now')",
            [LIBRARY_ID, LIBRARY_NAME, root.to_str().unwrap()],
        )
        .unwrap();
    crate::app_locale::set(&db, Some("en".into())).unwrap();
    let settings = Settings::with_secret_store(db.clone(), Arc::new(MockSecrets::default()));
    settings
        .save(&AppSettings {
            base_url: model_url,
            model: MODEL.into(),
            ..Default::default()
        })
        .unwrap();
    settings.write_secret(MODEL_API_KEY, MODEL_KEY).unwrap();
    let agent =
        AgentRuntime::new(services.engine.clone(), settings.clone(), Arc::new(|_| {})).unwrap();
    let registry = agent.registry.clone();
    let connections = services.mcp_connections.clone();
    let servers = McpServers::new(db);
    servers.set_enabled(SOURCE, true).unwrap();
    let config = servers.get(SOURCE).unwrap();
    assert!(config.is_stock_browse_source());

    let model_app = Router::new()
        .route("/v1/chat/completions", post(model_reply))
        .with_state(calls.clone());
    let model_stop = stop.clone();
    let model_task = spawn(async move {
        axum::serve(model, model_app)
            .with_graceful_shutdown(model_stop.cancelled_owned())
            .await
    });
    let catalog_calls = calls.clone();
    let catalog_stop = stop.clone();
    let disconnecting = CancellationToken::new();
    let catalog_disconnecting = disconnecting.clone();
    let catalog_task = spawn(async move {
        tokio::select! {
            result = serve_catalog(catalog, catalog_calls) => match result {
                // Windows can reset the loopback socket when disconnect kills the relay.
                Err(error) if catalog_disconnecting.is_cancelled()
                    && error.kind() == ErrorKind::ConnectionReset => Ok(()),
                result => result,
            },
            _ = catalog_stop.cancelled() => Ok(()),
        }
    });
    block_on(connections.connect(settings, &registry, config, true)).unwrap();

    let (_app, window) = browse_ipc_window(services, agent);
    let invoke = |command, body| browse_invoke(&window, command, body).unwrap();
    let sources = invoke("browse_list_sources", json!({}));
    assert!(sources
        .as_array()
        .unwrap()
        .iter()
        .any(|source| { source["id"] == SOURCE && source["status"] == "available" }));
    let grant = invoke("browse_authorize", json!({"sourceIds": [SOURCE]}));
    let load_first_page = |grant: &Value| {
        let search = invoke(
            "browse_start_search",
            json!({
                "authorizationId": grant["authorizationId"], "gameIds": [LIBRARY_ID], "query": "",
            }),
        );
        let deadline = Instant::now() + Duration::from_secs(25);
        let page = loop {
            let frame = invoke(
                "browse_get_page",
                json!({"searchId": search["searchId"], "page": 0}),
            );
            if frame["status"] == "ready" && frame["isEnriching"] == false {
                break frame;
            }
            assert!(
                Instant::now() < deadline,
                "Browse page stalled: {frame}; calls: {:?}",
                calls.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        invoke(
            "browse_cancel_search",
            json!({"searchId": search["searchId"]}),
        );
        page
    };
    let catalog_reads = || {
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| matches!(call[0].as_str(), Some("search_games" | "browse_mods")))
            .cloned()
            .collect::<Vec<Value>>()
    };
    let page = load_first_page(&grant);
    let cold_reads = catalog_reads();
    let cached = load_first_page(&grant);
    assert_eq!(
        page["items"][0]["originalTitle"],
        cached["items"][0]["originalTitle"]
    );
    assert_ne!(page["items"][0]["id"], cached["items"][0]["id"]);
    assert_eq!(catalog_reads(), cold_reads);
    let renewed = invoke("browse_authorize", json!({"sourceIds": [SOURCE]}));
    let fresh = load_first_page(&renewed);
    assert_eq!(fresh["items"][0]["originalTitle"], PROJECT);
    assert_eq!(&catalog_reads()[cold_reads.len()..], cold_reads);
    disconnecting.cancel();
    block_on(connections.disconnect(&registry, &servers, SOURCE)).unwrap();
    let denied = browse_invoke(
        &window,
        "browse_start_search",
        json!({
            "authorizationId": renewed["authorizationId"], "gameIds": [LIBRARY_ID], "query": "",
        }),
    )
    .unwrap_err();
    assert_eq!(denied["key"], "browse.authorization_changed");
    window.destroy().unwrap();
    block_on(async {
        tokio::time::timeout(Duration::from_secs(5), catalog_task)
            .await
            .expect("GameBanana test relay did not close after disconnect")
            .unwrap()
            .unwrap();
        stop.cancel();
        model_task.await.unwrap().unwrap();
    });

    let recorded_calls = calls.lock().unwrap().clone();
    let browse_calls = recorded_calls
        .iter()
        .filter(|call| {
            !matches!(
                call[0].as_str(),
                Some("model_classification" | "model_translation")
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let initial_browse_calls = [
        json!(["search_games", {"query": LIBRARY_NAME, "page": 1}]),
        json!(["model_aliases", {"name": LIBRARY_NAME, "steamTitle": null}]),
        json!(["search_games", {"query": ALIAS, "page": 1}]),
        json!(["model_suggestion", {
            "name": LIBRARY_NAME, "steamTitle": null, "source": "GameBanana", "options": [DECOY, TARGET],
        }]),
        json!(["browse_mods", {"game_id": TARGET_ID, "page": 1, "per_page": 20}]),
    ];
    assert_eq!(
        browse_calls.get(..initial_browse_calls.len()),
        Some(initial_browse_calls.as_slice())
    );
    let classification_calls = recorded_calls
        .iter()
        .filter(|call| call[0] == "model_classification")
        .collect::<Vec<_>>();
    assert_eq!(classification_calls.len(), 1);
    for call in classification_calls {
        assert_eq!(
            call[1],
            json!({
                "items": [{
                    "index": 0,
                    "title": PROJECT,
                    "summary": null,
                    "kind": null,
                    "tags": [],
                }],
            })
        );
    }
    let translation_calls = recorded_calls
        .iter()
        .filter(|call| call[0] == "model_translation")
        .collect::<Vec<_>>();
    assert!(!translation_calls.is_empty());
    assert!(translation_calls
        .iter()
        .all(|call| call[1]["targetLocale"] == "en"));
    assert_eq!(page["issues"], json!([]));
    assert_eq!(page["gameChoices"], json!([]));
    assert_eq!(page["hasNext"], false);
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["sourceId"], SOURCE);
    assert_eq!(items[0]["gameIds"], json!([LIBRARY_ID]));
    assert_eq!(items[0]["typeKey"], "tools");
    assert_eq!(items[0]["title"], PROJECT);
    assert_eq!(items[0]["originalTitle"], PROJECT);
    assert_eq!(items[0]["canOpenProject"], true);
}
