//! Offline end-to-end fixtures for the Agent's local mod-library workflow.
//!
//! These tests use only loopback model/MCP endpoints and inert local archives.
//! They never access a real mod site or launch an external browser.

use crate::{
    agent::{
        approval::ApprovalResponse,
        events::AgentEvent,
        policy::PermissionMode,
        runtime::{AgentRuntime, TurnRequest},
    },
    artifacts,
    download::DownloadArtifact,
    error::AppResult,
    history::History,
    mcp::{ConnectionConsent, McpClient, McpRuntime, McpServers, ServerConfig},
    mod_library::{self, LibraryEntry},
    settings::{SecretStore, Settings},
    sources::ResolvedMod,
    transaction::{self, FileEngine},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256, Sha512};
use std::{
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

pub(super) struct CatalogArtifactFixture {
    pub artifact_id: String,
    pub filename: String,
    pub path: PathBuf,
    pub sha256: String,
}

/// Write and register an inert archive exactly as a completed local download
/// would be recorded. The source URL is reserved under `.invalid` and is never
/// requested by these tests.
pub(super) fn catalog_archive(
    engine: &FileEngine,
    filename: &str,
    files: &[(&str, &[u8])],
) -> CatalogArtifactFixture {
    std::fs::create_dir_all(engine.paths.downloads()).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let path = engine.paths.downloads().join(format!("{id}-{filename}"));
    let mut archive = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    for (name, bytes) in files {
        archive
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(bytes).unwrap();
    }
    archive.finish().unwrap();

    let bytes = std::fs::read(&path).unwrap();
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let artifact = DownloadArtifact {
        id: id.clone(),
        path: path.clone(),
        filename: filename.into(),
        size: bytes.len() as u64,
        sha256: sha256.clone(),
        sha512: hex::encode(Sha512::digest(&bytes)),
        integrity_verified: false,
        requires_review: true,
        auto_install_allowed: false,
        warnings: vec![],
    };
    let source = ResolvedMod {
        source_id: "modrinth".into(),
        project_id: "offline-library-fixture".into(),
        version_id: "offline-library-fixture-v1".into(),
        name: "Offline library fixture".into(),
        version: "1.0.0-fixture".into(),
        download_url: "https://fixture.invalid/archive.zip".into(),
        filename: filename.into(),
        size: Some(artifact.size),
        hashes: Default::default(),
        homepage_url: Some("https://mods.example.test/mods/offline-library-fixture".into()),
        warnings: vec![],
    };
    artifacts::record(engine, &artifact, &source).unwrap();

    CatalogArtifactFixture {
        artifact_id: id,
        filename: filename.into(),
        path,
        sha256,
    }
}

pub(super) fn ingest_library_fixture(
    engine: &FileEngine,
    game_id: &str,
    game_name: &str,
    filename: &str,
    files: &[(&str, &[u8])],
) -> (CatalogArtifactFixture, LibraryEntry) {
    let archive = catalog_archive(engine, filename, files);
    let record = artifacts::for_path(&engine.db, &archive.path)
        .unwrap()
        .expect("catalog fixture should be recorded");
    let entry = mod_library::ingest_download(engine, &record, game_name, Some(game_id)).unwrap();
    (archive, entry)
}

/// Run the real AgentRuntime against a loopback scripted model endpoint.
pub(super) async fn scripted_agent(
    engine: FileEngine,
    conversation_game_id: Option<&str>,
    request_count: usize,
    respond: impl FnMut(usize, &Value) -> String + Send + 'static,
) -> (
    AgentRuntime,
    String,
    tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    JoinHandle<Vec<Value>>,
) {
    assert!(request_count > 0, "the model script must expect a reply");
    let (url, model_task) = model_server(request_count, respond).await;
    let settings = Settings::with_secret_store(engine.db.clone(), Arc::new(NoSecrets));
    let mut config = settings.load().unwrap();
    config.base_url = url;
    config.model = "mod-library-mock-model".into();
    settings.save(&config).unwrap();
    let conversation = History::new(engine.db.clone())
        .create_conversation("Offline Mod Library", conversation_game_id)
        .unwrap()
        .id;
    let (tx, events) = tokio::sync::mpsc::unbounded_channel();
    let agent = AgentRuntime::new(
        engine,
        settings,
        Arc::new(move |event| {
            let _ = tx.send(event.clone());
        }),
    )
    .unwrap();
    agent.set_test_context_window("mod-library-mock-model", None);
    (agent, conversation, events, model_task)
}

struct NoSecrets;

impl SecretStore for NoSecrets {
    fn read(&self, _: &str) -> AppResult<Option<String>> {
        Ok(None)
    }

    fn write(&self, _: &str, _: &str) -> AppResult<()> {
        Ok(())
    }
}

async fn model_server(
    request_count: usize,
    mut respond: impl FnMut(usize, &Value) -> String + Send + 'static,
) -> (String, JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut requests = Vec::with_capacity(request_count);
        for index in 0..request_count {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_json_http_request(&mut socket).await.unwrap();
            let response = respond(index, &request);
            requests.push(request);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.len()
            );
            socket.write_all(header.as_bytes()).await.unwrap();
            for chunk in response.as_bytes().chunks(7) {
                socket.write_all(chunk).await.unwrap();
            }
        }
        requests
    });
    (format!("http://{address}/v1"), task)
}

pub(super) fn turn_request(
    conversation_id: &str,
    content: &str,
    permission_mode: PermissionMode,
) -> TurnRequest {
    TurnRequest {
        conversation_id: conversation_id.into(),
        content: content.into(),
        attachments: None,
        model: None,
        permission_mode: Some(permission_mode),
        reasoning_effort: None,
    }
}

pub(super) fn final_reply() -> String {
    format!(
        "{}{}data: [DONE]\n\n",
        event(json!({"choices":[{"index":0,"delta":{"content":"完成"},"finish_reason":"stop"}]})),
        event(json!({"choices":[],"usage":{"total_tokens":12}}))
    )
}

pub(super) async fn next_event(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<AgentEvent>,
    expected: &str,
) -> AgentEvent {
    let mut observed = Vec::new();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let event = events.recv().await.expect("Agent event stream closed");
            if event.kind == expected {
                return event;
            }
            observed.push((event.kind.clone(), event.payload.clone()));
            assert_ne!(event.kind, "error", "Agent failed: {}", event.payload);
            assert_ne!(
                event.kind, "done",
                "turn ended before {expected}; observed events: {observed:#?}"
            );
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {expected}; observed events: {observed:#?}"))
}

/// One tool call in the model's SSE response. Keep the mock protocol identical
/// to the regular Agent integration-test harness.
pub(super) fn model_tool_call(name: &str, id: &str, arguments: Value) -> String {
    format!(
        "{}data: [DONE]\n\n",
        event(json!({"choices":[{"index":0,"delta":{"tool_calls":[{
            "index":0,"id":id,"type":"function",
            "function":{"name":name,"arguments":arguments.to_string()}
        }]},"finish_reason":"tool_calls"}]}))
    )
}

fn find_string(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Object(object) => object
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| object.values().find_map(|child| find_string(child, key))),
        Value::Array(values) => values.iter().find_map(|child| find_string(child, key)),
        _ => None,
    }
}

fn install_id_from_model_body(body: &Value) -> String {
    body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|message| message["content"].as_str())
        .filter_map(|content| serde_json::from_str::<Value>(content).ok())
        .find_map(|content| find_string(&content, "installId"))
        .expect("prior install tool result should include installId")
}

fn tool_message_values(body: &Value) -> Vec<Value> {
    body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|message| message["role"] == "tool")
        .filter_map(|message| message["content"].as_str())
        .filter_map(|content| serde_json::from_str(content).ok())
        .collect()
}

fn approved_archive_request(game_id: &str, artifact_id: &str) -> Value {
    json!({"request":{
        "gameId":game_id,
        "artifactId":artifact_id,
        "mode":"approvedMappings",
        "sourceKind":"archive",
        "mappings":[
            {"source":"OfflineMod/mod.dat","destination":"mods/OfflineMod/mod.dat"}
        ]
    }})
}

fn mcp_tool_definition(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
) -> Value {
    json!({"name":name,"description":description,"inputSchema":{
        "type":"object","properties":properties,"required":required,"additionalProperties":false
    }})
}

fn mock_mcp_tool_name(server: &str, tool: &str) -> String {
    crate::mcp::client::namespaced_tool_name(server, tool)
}

fn event(content: Value) -> String {
    format!("data: {content}\n\n")
}

/// A loopback Streamable HTTP MCP server. Its tool replies are supplied by the
/// test; it never reaches a real author service or launches a browser.
pub(super) struct LoopbackMcpFixture {
    pub url: String,
    tools: Vec<Value>,
    requests: Arc<Mutex<Vec<Value>>>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl LoopbackMcpFixture {
    pub async fn start(
        tools: Vec<Value>,
        mut tool_reply: impl FnMut(&str, &Value) -> Value + Send + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let tool_list = tools.clone();
        let (shutdown, mut shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => accepted,
                };
                let Ok((mut socket, _)) = accepted else {
                    break;
                };
                let request = match read_json_http_request(&mut socket).await {
                    Ok(request) => request,
                    Err(_) => break,
                };
                recorded
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(request.clone());
                let response = mcp_response(&request, &tool_list, &mut tool_reply);
                let payload = response.unwrap_or_default();
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                if socket.write_all(header.as_bytes()).await.is_err()
                    || socket.write_all(payload.as_bytes()).await.is_err()
                {
                    break;
                }
            }
        });

        Self {
            url: format!("http://{address}/mcp"),
            tools,
            requests,
            shutdown: Some(shutdown),
            task: Some(task),
        }
    }

    pub fn tool_names(&self) -> Vec<&str> {
        self.tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect()
    }

    pub fn recorded_requests(&self) -> Vec<Value> {
        self.requests
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn recorded_tool_calls(&self) -> Vec<Value> {
        self.recorded_requests()
            .into_iter()
            .filter(|request| request["method"] == "tools/call")
            .collect()
    }

    pub async fn finish(mut self) -> Vec<Value> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(mut task) = self.task.take() {
            if tokio::time::timeout(Duration::from_secs(2), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
        self.recorded_requests()
    }
}

impl Drop for LoopbackMcpFixture {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Connect one of the loopback fixtures and register its namespaced tools in
/// the actual AgentRuntime registry.
pub(super) async fn connect_mock_mcp(
    agent: &AgentRuntime,
    fixture: &LoopbackMcpFixture,
    id: &str,
    name: &str,
) -> (McpClient, Vec<String>) {
    let mut config = ServerConfig::http(id, name, &fixture.url);
    config.enabled = true;
    let servers = McpServers::new(agent.engine.db.clone());
    servers.create(&config).unwrap();
    let consent = ConnectionConsent::after_user_confirmation(&config, false).unwrap();
    let client = McpClient::connect(
        servers,
        &agent.settings,
        id,
        consent,
        &McpRuntime::default(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let names = client
        .register_tools(&agent.registry, &CancellationToken::new())
        .await
        .unwrap();
    (client, names)
}

async fn read_json_http_request(socket: &mut TcpStream) -> std::io::Result<Value> {
    let mut header = Vec::new();
    loop {
        let mut byte = [0_u8; 1];
        socket.read_exact(&mut byte).await?;
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
        if header.len() >= 32 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "oversized mock MCP request headers",
            ));
        }
    }
    let header = String::from_utf8(header)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let body_length = header
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .and_then(|length| length.parse::<usize>().ok())
        })
        .unwrap_or_default();
    let mut body = vec![0; body_length];
    socket.read_exact(&mut body).await?;
    serde_json::from_slice(&body)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn mcp_response(
    request: &Value,
    tools: &[Value],
    tool_reply: &mut impl FnMut(&str, &Value) -> Value,
) -> Option<String> {
    let id = request.get("id")?.clone();
    let result = match request["method"].as_str().unwrap_or_default() {
        "initialize" => json!({
            "protocolVersion":"2025-03-26",
            "capabilities":{"tools":{}},
            "serverInfo":{"name":"offline-agent-fixture","version":"1"}
        }),
        "tools/list" => json!({"tools":tools}),
        "tools/call" => {
            let name = request["params"]["name"].as_str().unwrap_or_default();
            let arguments = &request["params"]["arguments"];
            tool_reply(name, arguments)
        }
        _ => json!({}),
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":result}).to_string())
}

#[tokio::test]
async fn local_library_install_uninstall_and_reinstall_use_one_reviewed_game_path_without_mcp() {
    let (_temp, engine, game_root) = transaction::tests::setup();
    let user_file = game_root.join("user.cfg");
    std::fs::write(&user_file, b"keep user settings").unwrap();
    let game_id = "g".to_owned();
    let (artifact, entry) = ingest_library_fixture(
        &engine,
        &game_id,
        "Game",
        "offline-mod.zip",
        &[
            (
                "OfflineMod/README.md",
                b"Install OfflineMod/mod.dat as mods/OfflineMod/mod.dat.\n",
            ),
            ("OfflineMod/mod.dat", b"inert fixture bytes"),
        ],
    );
    let destination = game_root.join("mods/OfflineMod/mod.dat");
    crate::settings::set_mod_organization_locale_override(&engine.db, Some("zh-CN")).unwrap();
    let tag = crate::mod_organization::save_mod_tag(&engine.db, &game_id, None, "玩法", "#319163")
        .unwrap();
    let mut scripted_request = approved_archive_request(&game_id, &artifact.artifact_id);
    scripted_request["organization"] = json!({"displayName":"离线示例", "tags":["玩法"]});
    let artifact_id = artifact.artifact_id.clone();
    let scripted_game_id = game_id.clone();
    let (agent, conversation, mut events, model_task) =
        scripted_agent(engine, Some(&game_id), 8, move |round, body| match round {
            0 => model_tool_call(
                "read_skill",
                "read-install-skill",
                json!({"name":"system-mod-install"}),
            ),
            1 => model_tool_call(
                "list_mod_library",
                "list-local-library",
                json!({"game_id":scripted_game_id}),
            ),
            2 => model_tool_call(
                "match_mod_install",
                "match-local-entry",
                json!({"game_id":scripted_game_id,"artifact_id":artifact_id}),
            ),
            3 => model_tool_call(
                "install_library_mod",
                "install-local-entry",
                scripted_request.clone(),
            ),
            4 => model_tool_call(
                "inspect_install",
                "inspect-local-entry",
                json!({"game_id":scripted_game_id,"install_id":install_id_from_model_body(body)}),
            ),
            5 => model_tool_call(
                "uninstall_mod",
                "uninstall-local-entry",
                json!({"game_id":scripted_game_id,"install_id":install_id_from_model_body(body)}),
            ),
            6 => model_tool_call(
                "install_library_mod",
                "reinstall-local-entry",
                scripted_request.clone(),
            ),
            7 => final_reply(),
            _ => panic!("unexpected model request {round}"),
        })
        .await;

    let turn = agent
        .start_turn(turn_request(
            &conversation,
            "安装本地库中的 Offline Fixture，核对并可安全卸载后重新安装。",
            PermissionMode::RequestApproval,
        ))
        .unwrap();

    let skill = next_event(&mut events, "tool-result").await;
    assert_eq!(skill.payload["name"], "read_skill");
    assert_eq!(skill.payload["status"], "done");
    let listed = next_event(&mut events, "tool-result").await;
    assert_eq!(listed.payload["name"], "list_mod_library");
    assert_eq!(listed.payload["result"][0]["artifactId"], entry.id);
    let matched = next_event(&mut events, "tool-result").await;
    assert_eq!(matched.payload["name"], "match_mod_install");
    assert_eq!(
        matched.payload["result"]["readme"]["text"],
        "Install OfflineMod/mod.dat as mods/OfflineMod/mod.dat.\n"
    );
    assert!(matched.payload["result"]["candidates"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!destination.exists());

    assert_eq!(matched.payload["result"]["organizationLocale"], "zh-CN");
    assert_eq!(matched.payload["result"]["existingTags"], json!(["玩法"]));
    let install_approval = next_event(&mut events, "approval-request").await;
    assert_eq!(install_approval.turn_id, turn);
    assert_eq!(install_approval.payload["title"], "install_library_mod");
    let planned_entries = install_approval.payload["preview"]["plan"]["entries"]
        .as_array()
        .unwrap();
    assert_eq!(planned_entries.len(), 1);
    assert_eq!(
        planned_entries[0]["operation"]["dst"],
        json!(crate::paths::user_path(&destination).as_ref())
    );
    assert!(
        !destination.exists(),
        "approval must precede the game-file write"
    );
    assert_eq!(std::fs::read(&user_file).unwrap(), b"keep user settings");
    agent
        .respond_approval(
            &conversation,
            install_approval.payload["approvalId"].as_str().unwrap(),
            ApprovalResponse {
                approved: true,
                answer: Value::Null,
            },
        )
        .unwrap();
    let installed = next_event(&mut events, "tool-result").await;
    assert_eq!(installed.payload["name"], "install_library_mod");
    assert_eq!(installed.payload["status"], "done");
    let first_install_id = installed.payload["result"]["installId"]
        .as_str()
        .expect("install receipt includes its managed install ID")
        .to_owned();
    assert_eq!(std::fs::read(&destination).unwrap(), b"inert fixture bytes");

    let inspection = next_event(&mut events, "tool-result").await;
    assert_eq!(inspection.payload["name"], "inspect_install");
    assert_eq!(inspection.payload["result"]["canUninstall"], true);
    assert_eq!(inspection.payload["result"]["files"][0]["state"], "owned");

    let uninstall_approval = next_event(&mut events, "approval-request").await;
    assert_eq!(uninstall_approval.payload["title"], "uninstall_mod");
    assert!(
        destination.exists(),
        "uninstall must also wait for approval"
    );
    agent
        .respond_approval(
            &conversation,
            uninstall_approval.payload["approvalId"].as_str().unwrap(),
            ApprovalResponse {
                approved: true,
                answer: Value::Null,
            },
        )
        .unwrap();
    let uninstalled = next_event(&mut events, "tool-result").await;
    assert_eq!(uninstalled.payload["name"], "uninstall_mod");
    assert_eq!(uninstalled.payload["status"], "done");
    assert!(!destination.exists());

    let reinstall_approval = next_event(&mut events, "approval-request").await;
    assert_eq!(reinstall_approval.payload["title"], "install_library_mod");
    assert!(!destination.exists());
    agent
        .respond_approval(
            &conversation,
            reinstall_approval.payload["approvalId"].as_str().unwrap(),
            ApprovalResponse {
                approved: true,
                answer: Value::Null,
            },
        )
        .unwrap();
    let reinstalled = next_event(&mut events, "tool-result").await;
    assert_eq!(reinstalled.payload["name"], "install_library_mod");
    assert_eq!(reinstalled.payload["status"], "done");
    assert_ne!(reinstalled.payload["result"]["installId"], first_install_id);
    assert_eq!(std::fs::read(&destination).unwrap(), b"inert fixture bytes");
    assert_eq!(std::fs::read(&user_file).unwrap(), b"keep user settings");
    assert_eq!(
        next_event(&mut events, "done").await.payload["status"],
        "done"
    );

    let recorded = model_task.await.unwrap();
    assert_eq!(recorded.len(), 8);
    let initial_tools = recorded[0]["tools"].as_array().unwrap();
    for name in [
        "list_mod_library",
        "match_mod_install",
        "install_library_mod",
    ] {
        assert!(initial_tools
            .iter()
            .any(|tool| tool["function"]["name"] == name));
    }
    let called_names: Vec<_> = agent
        .events
        .replay(&conversation, 0, 200)
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "tool-start")
        .filter_map(|event| event.payload["name"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        called_names,
        [
            "read_skill",
            "list_mod_library",
            "match_mod_install",
            "install_library_mod",
            "inspect_install",
            "uninstall_mod",
            "install_library_mod",
        ]
    );
    assert!(crate::install::list_installs(&agent.engine, &game_id)
        .unwrap()
        .iter()
        .any(|install| install.id == first_install_id
            && install.name == "离线示例"
            && install.tag_ids == [tag.id.clone()]));
    assert_eq!(entry.filename, artifact.filename);
    assert_eq!(
        crate::util::sha256_file(&artifact.path).unwrap(),
        artifact.sha256
    );
}

#[tokio::test]
async fn full_trust_game_detail_removal_uninstalls_and_forgets_without_approval() {
    use crate::install;
    let (_temp, engine, game_root) = transaction::tests::setup();
    let (first_artifact, _) = ingest_library_fixture(
        &engine,
        "g",
        "Game",
        "offline-first.zip",
        &[("OfflineMod/mod.dat", b"fixture")],
    );
    let (second_artifact, _) = ingest_library_fixture(
        &engine,
        "g",
        "Game",
        "offline-second.zip",
        &[("AnotherMod/plugin.dat", b"second fixture")],
    );
    let first_file = game_root.join("mods/OfflineMod/mod.dat");
    let second_file = game_root.join("mods/AnotherMod/plugin.dat");

    let first_request = approved_archive_request("g", &first_artifact.artifact_id);
    let first_preview = install::plan_library_install(
        &engine,
        &serde_json::from_value(first_request["request"].clone()).unwrap(),
    )
    .unwrap();
    engine.approve_plan(&first_preview.plan.id).unwrap();
    engine.apply_file_ops(&first_preview.plan.id).unwrap();
    install::reconcile(&engine.db).unwrap();
    let mut second_request = approved_archive_request("g", &second_artifact.artifact_id);
    second_request["request"]["mappings"][0]["source"] = json!("AnotherMod/plugin.dat");
    second_request["request"]["mappings"][0]["destination"] = json!("mods/AnotherMod/plugin.dat");
    let second_preview = install::plan_library_install(
        &engine,
        &serde_json::from_value(second_request["request"].clone()).unwrap(),
    )
    .unwrap();
    engine.approve_plan(&second_preview.plan.id).unwrap();
    engine.apply_file_ops(&second_preview.plan.id).unwrap();
    install::reconcile(&engine.db).unwrap();

    assert_ne!(first_preview.install_id, second_preview.install_id);
    assert_ne!(first_file, second_file);
    assert_eq!(std::fs::read(&first_file).unwrap(), b"fixture");
    assert_eq!(std::fs::read(&second_file).unwrap(), b"second fixture");

    let targets = [
        (first_preview.install_id.clone(), first_file),
        (second_preview.install_id.clone(), second_file),
    ];
    let steps: Vec<_> = targets
        .iter()
        .flat_map(|(install_id, _)| {
            [
                (
                    "inspect_install",
                    json!({"game_id":"g","install_id":install_id}),
                ),
                (
                    "uninstall_mod",
                    json!({"game_id":"g","install_id":install_id}),
                ),
                (
                    "forget_uninstalled_mod",
                    json!({"game_id":"g","install_id":install_id}),
                ),
            ]
        })
        .collect();
    let (agent, conv, mut events, model) =
        scripted_agent(engine, Some("g"), steps.len() + 1, move |step, _| {
            let Some((name, args)) = steps.get(step) else {
                return final_reply();
            };
            model_tool_call(name, &format!("removal-{step}"), args.clone())
        })
        .await;
    let turn = agent
        .start_turn(turn_request(
            &conv,
            "逐个检查、卸载并从管理列表删除这两个安装",
            PermissionMode::FullTrust,
        ))
        .unwrap();

    for (install_id, file) in &targets {
        let inspected = next_event(&mut events, "tool-result").await;
        assert_eq!(inspected.turn_id, turn);
        assert_eq!(inspected.payload["name"], "inspect_install");
        assert_eq!(inspected.payload["status"], "done");
        assert_eq!(
            inspected.payload["result"]["installId"].as_str(),
            Some(install_id.as_str())
        );
        assert_eq!(inspected.payload["result"]["canUninstall"], true);
        assert_eq!(
            inspected.payload["result"]["reconciliation"]["recommendedAction"],
            "safe_uninstall"
        );
        assert_eq!(
            inspected.payload["result"]["files"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(inspected.payload["result"]["files"][0]["state"], "owned");
        assert!(file.is_file());

        let uninstalled = next_event(&mut events, "tool-result").await;
        assert_eq!(uninstalled.turn_id, turn);
        assert_eq!(uninstalled.payload["name"], "uninstall_mod");
        assert_eq!(uninstalled.payload["status"], "done");
        assert_eq!(uninstalled.payload["result"]["status"], "committed");
        assert!(!file.exists());
        assert_eq!(
            install::get_install(&agent.engine, install_id)
                .unwrap()
                .status,
            "uninstalled"
        );

        let forgotten = next_event(&mut events, "tool-result").await;
        assert_eq!(forgotten.turn_id, turn);
        assert_eq!(forgotten.payload["name"], "forget_uninstalled_mod");
        assert_eq!(forgotten.payload["status"], "done");
        assert_eq!(forgotten.payload["result"]["status"], "removed");
        assert_eq!(
            forgotten.payload["result"]["installId"].as_str(),
            Some(install_id.as_str())
        );
        assert!(!crate::mod_management::visible_installs(&agent.engine, "g")
            .unwrap()
            .iter()
            .any(|install| install.id == *install_id));
        assert_eq!(
            install::get_install(&agent.engine, install_id)
                .unwrap()
                .status,
            "uninstalled"
        );
        assert!(!file.exists());
    }

    let done = next_event(&mut events, "done").await;
    assert_eq!(done.turn_id, turn);
    assert_eq!(done.payload["status"], "done");
    let recorded_events = agent.events.replay(&conv, 0, 200).unwrap();
    assert!(recorded_events.iter().all(|event| event.turn_id == turn));
    assert_eq!(
        recorded_events
            .iter()
            .filter(|event| event.kind == "done")
            .count(),
        1
    );
    assert!(!recorded_events
        .iter()
        .any(|event| event.kind == "approval-request"));
    assert_eq!(
        crate::mod_management::visible_installs(&agent.engine, "g")
            .unwrap()
            .iter()
            .filter(|install| targets.iter().any(|(id, _)| install.id == *id))
            .count(),
        0
    );
    assert_eq!(model.await.unwrap().len(), 7);
}

#[tokio::test]
async fn cancelling_library_install_approval_never_writes_the_game_file() {
    let (_temp, engine, game_root) = transaction::tests::setup();
    let game_id = "g".to_owned();
    let (artifact, _) = ingest_library_fixture(
        &engine,
        &game_id,
        "Game",
        "cancelled-mod.zip",
        &[("OfflineMod/mod.dat", b"must remain inert")],
    );
    let destination = game_root.join("mods/OfflineMod/mod.dat");
    let scripted_request = approved_archive_request(&game_id, &artifact.artifact_id);
    let artifact_id = artifact.artifact_id.clone();
    let scripted_game_id = game_id.clone();
    let (agent, conversation, mut events, model_task) =
        scripted_agent(engine, Some(&game_id), 4, move |round, _| match round {
            0 => model_tool_call(
                "read_skill",
                "cancel-read-install-skill",
                json!({"name":"system-mod-install"}),
            ),
            1 => model_tool_call(
                "list_mod_library",
                "cancel-list-library",
                json!({"game_id":scripted_game_id}),
            ),
            2 => model_tool_call(
                "match_mod_install",
                "cancel-match-entry",
                json!({"game_id":scripted_game_id,"artifact_id":artifact_id}),
            ),
            3 => model_tool_call(
                "install_library_mod",
                "cancel-install-entry",
                scripted_request.clone(),
            ),
            _ => panic!("unexpected model request {round}"),
        })
        .await;
    let turn = agent
        .start_turn(turn_request(
            &conversation,
            "安装本地库中的 Mod。",
            PermissionMode::RequestApproval,
        ))
        .unwrap();
    let approval = next_event(&mut events, "approval-request").await;
    assert_eq!(approval.payload["title"], "install_library_mod");
    let plan_id = approval.payload["preview"]["plan"]["id"]
        .as_str()
        .expect("approval preview includes its exact plan")
        .to_owned();
    assert!(!destination.exists());
    agent.cancel_turn(&conversation, &turn).unwrap();
    let done = next_event(&mut events, "done").await;
    assert_eq!(done.payload["status"], "cancelled");
    assert!(!destination.exists());
    let (approval_status, plan_status, journal): (String, String, String) = {
        let conn = agent.engine.db.conn();
        let approval_status = conn
            .query_row(
                "SELECT status FROM approvals WHERE id=?1",
                [approval.payload["approvalId"].as_str().unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        let (plan_status, journal) = conn
            .query_row(
                "SELECT status,journal_json FROM file_transactions WHERE id=?1",
                [&plan_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        (approval_status, plan_status, journal)
    };
    assert_eq!(approval_status, "cancelled");
    assert_eq!(plan_status, "planned");
    assert_eq!(journal, "[]");
    assert_eq!(model_task.await.unwrap().len(), 4);
}

#[tokio::test]
async fn unavailable_author_mcp_falls_back_to_mock_playwright_before_install() {
    let (_temp, engine, game_root) = transaction::tests::setup();
    let game_id = "g".to_owned();
    let (_archive, entry) = ingest_library_fixture(
        &engine,
        &game_id,
        "Game",
        "author-notes-needed.zip",
        &[("OfflineMod/unknown.bin", b"inert mapped bytes")],
    );
    let destination = game_root.join("mods/OfflineMod/unknown.bin");
    let project_page = entry.project_page.clone().unwrap();
    let author_tool_name = mock_mcp_tool_name("fixture-author", "read_author_install_notes");
    let navigate_tool_name = mock_mcp_tool_name("fixture-playwright", "browser_navigate");
    let snapshot_tool_name = mock_mcp_tool_name("fixture-playwright", "browser_snapshot");

    let authored_page = project_page.clone();
    let author_fixture = LoopbackMcpFixture::start(
        vec![mcp_tool_definition(
            "read_author_install_notes",
            "Read official author-provided install notes for a listed project page.",
            json!({"project_page":{"type":"string"}}),
            &["project_page"],
        )],
        move |name, arguments| {
            assert_eq!(name, "read_author_install_notes");
            assert_eq!(arguments["project_page"], authored_page);
            json!({
                "content":[{"type":"text","text":"Author notes are temporarily unavailable."}],
                "structuredContent":{"status":"unavailable"},
                "isError":true
            })
        },
    )
    .await;

    let browser_page = project_page.clone();
    let browser_fixture = LoopbackMcpFixture::start(
        vec![
            mcp_tool_definition(
                "browser_navigate",
                "Open the already listed release page in the isolated fixture browser.",
                json!({"url":{"type":"string"}}),
                &["url"],
            ),
            mcp_tool_definition(
                "browser_snapshot",
                "Read visible page text from the fixture browser.",
                json!({}),
                &[],
            ),
        ],
        move |name, arguments| match name {
            "browser_navigate" => {
                assert_eq!(arguments["url"], browser_page);
                json!({"content":[{"type":"text","text":"Release page opened in local fixture."}],"isError":false})
            }
            "browser_snapshot" => json!({
                "content":[{"type":"text","text":"Install OfflineMod/unknown.bin at mods/OfflineMod/unknown.bin. Preserve the rest of the game folder."}],
                "isError":false
            }),
            other => panic!("unexpected mock browser tool: {other}"),
        },
    )
    .await;

    let game_for_model = game_id.clone();
    let artifact_for_model = entry.id.clone();
    let page_for_model = project_page.clone();
    let request_for_model = json!({"request":{
        "gameId":game_id,
        "artifactId":entry.id,
        "mode":"approvedMappings",
        "sourceKind":"archive",
        "mappings":[{"source":"OfflineMod/unknown.bin","destination":"mods/OfflineMod/unknown.bin"}]
    }});
    let scripted_author_name = author_tool_name.clone();
    let scripted_navigate_name = navigate_tool_name.clone();
    let scripted_snapshot_name = snapshot_tool_name.clone();
    let (agent, conversation, mut events, model_task) =
        scripted_agent(engine, Some(&game_id), 8, move |round, body| match round {
            0 => model_tool_call(
                "read_skill",
                "fallback-read-install-skill",
                json!({"name":"system-mod-install"}),
            ),
            1 => model_tool_call(
                "list_mod_library",
                "fallback-list-library",
                json!({"game_id":game_for_model}),
            ),
            2 => model_tool_call(
                "match_mod_install",
                "fallback-match-entry",
                json!({"game_id":game_for_model,"artifact_id":artifact_for_model}),
            ),
            3 => model_tool_call(
                &scripted_author_name,
                "fallback-author-notes",
                json!({"project_page":page_for_model}),
            ),
            4 => {
                assert!(tool_message_values(body)
                    .iter()
                    .any(|content| content["isError"] == true));
                model_tool_call(
                    &scripted_navigate_name,
                    "fallback-browser-navigate",
                    json!({"url":page_for_model}),
                )
            }
            5 => model_tool_call(
                &scripted_snapshot_name,
                "fallback-browser-snapshot",
                json!({}),
            ),
            6 => model_tool_call(
                "install_library_mod",
                "fallback-install-entry",
                request_for_model.clone(),
            ),
            7 => final_reply(),
            _ => panic!("unexpected model request {round}"),
        })
        .await;

    let (author_client, author_names) =
        connect_mock_mcp(&agent, &author_fixture, "fixture-author", "Author fixture").await;
    assert_eq!(
        author_names.as_slice(),
        std::slice::from_ref(&author_tool_name)
    );
    let (browser_client, browser_names) = connect_mock_mcp(
        &agent,
        &browser_fixture,
        "fixture-playwright",
        "Playwright fixture",
    )
    .await;
    assert_eq!(
        browser_names,
        [navigate_tool_name.clone(), snapshot_tool_name.clone()]
    );

    let turn = agent
        .start_turn(turn_request(
            &conversation,
            "查本地库并按作者说明安装；作者 MCP 不可用时从已列出的发布页读取说明。",
            PermissionMode::RequestApproval,
        ))
        .unwrap();
    assert_eq!(
        next_event(&mut events, "tool-result").await.payload["name"],
        "read_skill"
    );
    assert_eq!(
        next_event(&mut events, "tool-result").await.payload["name"],
        "list_mod_library"
    );
    let matched = next_event(&mut events, "tool-result").await;
    assert_eq!(matched.payload["name"], "match_mod_install");
    assert_eq!(matched.payload["result"]["readme"], Value::Null);
    assert!(matched.payload["result"]["candidates"]
        .as_array()
        .unwrap()
        .is_empty());

    let author_approval = next_event(&mut events, "approval-request").await;
    assert_eq!(author_approval.turn_id, turn);
    assert_eq!(author_approval.payload["title"], author_tool_name);
    assert!(author_fixture.recorded_tool_calls().is_empty());
    agent
        .respond_approval(
            &conversation,
            author_approval.payload["approvalId"].as_str().unwrap(),
            ApprovalResponse {
                approved: true,
                answer: Value::Null,
            },
        )
        .unwrap();
    let author_result = next_event(&mut events, "tool-result").await;
    assert_eq!(author_result.payload["name"], author_tool_name);
    assert_eq!(author_result.payload["result"]["isError"], true);
    assert_eq!(
        author_result.payload["result"]["structuredContent"]["status"],
        "unavailable"
    );

    let navigate_approval = next_event(&mut events, "approval-request").await;
    assert_eq!(navigate_approval.payload["title"], navigate_tool_name);
    assert_eq!(author_fixture.recorded_tool_calls().len(), 1);
    assert!(browser_fixture.recorded_tool_calls().is_empty());
    agent
        .respond_approval(
            &conversation,
            navigate_approval.payload["approvalId"].as_str().unwrap(),
            ApprovalResponse {
                approved: true,
                answer: Value::Null,
            },
        )
        .unwrap();
    let navigate_result = next_event(&mut events, "tool-result").await;
    assert_eq!(navigate_result.payload["status"], "done");

    let snapshot_approval = next_event(&mut events, "approval-request").await;
    assert_eq!(snapshot_approval.payload["title"], snapshot_tool_name);
    assert_eq!(browser_fixture.recorded_tool_calls().len(), 1);
    agent
        .respond_approval(
            &conversation,
            snapshot_approval.payload["approvalId"].as_str().unwrap(),
            ApprovalResponse {
                approved: true,
                answer: Value::Null,
            },
        )
        .unwrap();
    let snapshot_result = next_event(&mut events, "tool-result").await;
    assert!(snapshot_result.payload["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("mods/OfflineMod/unknown.bin"));

    let install_approval = next_event(&mut events, "approval-request").await;
    assert_eq!(install_approval.payload["title"], "install_library_mod");
    assert!(!destination.exists());
    agent
        .respond_approval(
            &conversation,
            install_approval.payload["approvalId"].as_str().unwrap(),
            ApprovalResponse {
                approved: true,
                answer: Value::Null,
            },
        )
        .unwrap();
    assert_eq!(
        next_event(&mut events, "tool-result").await.payload["status"],
        "done"
    );
    assert_eq!(std::fs::read(&destination).unwrap(), b"inert mapped bytes");
    assert_eq!(
        next_event(&mut events, "done").await.payload["status"],
        "done"
    );

    author_client.disconnect_and_wait().await.unwrap();
    browser_client.disconnect_and_wait().await.unwrap();
    agent.registry.unregister(&author_names);
    agent.registry.unregister(&browser_names);
    let author_requests = author_fixture.finish().await;
    let browser_requests = browser_fixture.finish().await;
    let author_calls: Vec<_> = author_requests
        .iter()
        .filter(|request| request["method"] == "tools/call")
        .collect();
    assert_eq!(author_calls.len(), 1);
    assert_eq!(
        author_calls[0]["params"]["name"],
        "read_author_install_notes"
    );
    let browser_calls: Vec<_> = browser_requests
        .iter()
        .filter(|request| request["method"] == "tools/call")
        .collect();
    assert_eq!(browser_calls.len(), 2);
    assert_eq!(browser_calls[0]["params"]["name"], "browser_navigate");
    assert_eq!(browser_calls[0]["params"]["arguments"]["url"], project_page);
    assert_eq!(browser_calls[1]["params"]["name"], "browser_snapshot");
    assert_eq!(model_task.await.unwrap().len(), 8);
}

#[tokio::test]
async fn available_author_mcp_mapping_installs_directly_without_playwright() {
    let (_temp, engine, game_root) = transaction::tests::setup();
    let game_id = "g".to_owned();
    let (artifact, entry) = ingest_library_fixture(
        &engine,
        &game_id,
        "Game",
        "author-mapped-mod.zip",
        &[("ManualMod/plugin.bin", b"author-mapped fixture bytes")],
    );
    let destination = game_root.join("mods/ManualMod/plugin.bin");
    let project_page = entry.project_page.clone().unwrap();
    let author_tool_name =
        mock_mcp_tool_name("fixture-author-success", "read_author_install_notes");
    let browser_tool_name = mock_mcp_tool_name("fixture-playwright-success", "browser_navigate");

    let expected_author_page = project_page.clone();
    let author_fixture = LoopbackMcpFixture::start(
        vec![mcp_tool_definition(
            "read_author_install_notes",
            "Read the author's install mapping from the listed public release page.",
            json!({"project_page":{"type":"string"}}),
            &["project_page"],
        )],
        move |name, arguments| {
            assert_eq!(name, "read_author_install_notes");
            assert_eq!(arguments["project_page"], expected_author_page);
            json!({
                "content":[{"type":"text","text":"Install ManualMod/plugin.bin at mods/ManualMod/plugin.bin."}],
                "structuredContent":{
                    "status":"available",
                    "mapping":{"source":"ManualMod/plugin.bin","destination":"mods/ManualMod/plugin.bin"}
                },
                "isError":false
            })
        },
    )
    .await;
    let browser_fixture = LoopbackMcpFixture::start(
        vec![mcp_tool_definition(
            "browser_navigate",
            "Open an already listed release page in the local Playwright fixture.",
            json!({"url":{"type":"string"}}),
            &["url"],
        )],
        |name, _| panic!("successful author MCP should skip Playwright tool {name}"),
    )
    .await;

    let scripted_game_id = game_id.clone();
    let scripted_artifact_id = artifact.artifact_id.clone();
    let scripted_project_page = project_page.clone();
    let request_for_model = json!({"request":{
        "gameId":game_id,
        "artifactId":artifact.artifact_id,
        "mode":"approvedMappings",
        "sourceKind":"archive",
        "mappings":[{"source":"ManualMod/plugin.bin","destination":"mods/ManualMod/plugin.bin"}]
    }});
    let scripted_author_name = author_tool_name.clone();
    let (agent, conversation, mut events, model_task) =
        scripted_agent(engine, Some(&game_id), 6, move |round, body| match round {
            0 => model_tool_call(
                "read_skill",
                "author-success-read-install-skill",
                json!({"name":"system-mod-install"}),
            ),
            1 => model_tool_call(
                "list_mod_library",
                "author-success-list-library",
                json!({"game_id":scripted_game_id}),
            ),
            2 => model_tool_call(
                "match_mod_install",
                "author-success-match-entry",
                json!({"game_id":scripted_game_id,"artifact_id":scripted_artifact_id}),
            ),
            3 => model_tool_call(
                &scripted_author_name,
                "author-success-read-notes",
                json!({"project_page":scripted_project_page}),
            ),
            4 => {
                assert!(tool_message_values(body).iter().any(|content| {
                    content["structuredContent"]["status"] == "available"
                        && content["structuredContent"]["mapping"]["destination"]
                            == "mods/ManualMod/plugin.bin"
                }));
                model_tool_call(
                    "install_library_mod",
                    "author-success-install-entry",
                    request_for_model.clone(),
                )
            }
            5 => final_reply(),
            _ => panic!("unexpected model request {round}"),
        })
        .await;

    let (author_client, author_names) = connect_mock_mcp(
        &agent,
        &author_fixture,
        "fixture-author-success",
        "Author success fixture",
    )
    .await;
    assert_eq!(
        author_names.as_slice(),
        std::slice::from_ref(&author_tool_name)
    );
    let (browser_client, browser_names) = connect_mock_mcp(
        &agent,
        &browser_fixture,
        "fixture-playwright-success",
        "Playwright success fixture",
    )
    .await;
    assert_eq!(browser_fixture.tool_names(), ["browser_navigate"]);
    assert_eq!(
        browser_names.as_slice(),
        std::slice::from_ref(&browser_tool_name)
    );

    let turn = agent
        .start_turn(turn_request(
            &conversation,
            "本地规则无法匹配时先读取作者 MCP；有作者说明就直接安装，不使用浏览器。",
            PermissionMode::RequestApproval,
        ))
        .unwrap();
    assert_eq!(
        next_event(&mut events, "tool-result").await.payload["name"],
        "read_skill"
    );
    assert_eq!(
        next_event(&mut events, "tool-result").await.payload["name"],
        "list_mod_library"
    );
    let matched = next_event(&mut events, "tool-result").await;
    assert_eq!(matched.payload["name"], "match_mod_install");
    assert_eq!(matched.payload["result"]["readme"], Value::Null);
    assert!(matched.payload["result"]["candidates"]
        .as_array()
        .unwrap()
        .is_empty());

    let author_approval = next_event(&mut events, "approval-request").await;
    assert_eq!(author_approval.turn_id, turn);
    assert_eq!(author_approval.payload["title"], author_tool_name);
    assert!(author_fixture.recorded_tool_calls().is_empty());
    agent
        .respond_approval(
            &conversation,
            author_approval.payload["approvalId"].as_str().unwrap(),
            ApprovalResponse {
                approved: true,
                answer: Value::Null,
            },
        )
        .unwrap();
    let author_result = next_event(&mut events, "tool-result").await;
    assert_eq!(author_result.payload["name"], author_tool_name);
    assert_eq!(author_result.payload["status"], "done");
    assert_eq!(author_result.payload["result"]["isError"], false);
    assert_eq!(
        author_result.payload["result"]["structuredContent"]["mapping"]["destination"],
        "mods/ManualMod/plugin.bin"
    );

    let install_approval = next_event(&mut events, "approval-request").await;
    assert_eq!(install_approval.payload["title"], "install_library_mod");
    assert_eq!(
        install_approval.payload["preview"]["plan"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(!destination.exists());
    agent
        .respond_approval(
            &conversation,
            install_approval.payload["approvalId"].as_str().unwrap(),
            ApprovalResponse {
                approved: true,
                answer: Value::Null,
            },
        )
        .unwrap();
    let installed = next_event(&mut events, "tool-result").await;
    assert_eq!(installed.payload["name"], "install_library_mod");
    assert_eq!(installed.payload["status"], "done");
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"author-mapped fixture bytes"
    );
    assert_eq!(
        next_event(&mut events, "done").await.payload["status"],
        "done"
    );

    let model_requests = model_task.await.unwrap();
    let definitions = model_requests[0]["tools"].as_array().unwrap();
    assert!(definitions
        .iter()
        .any(|tool| tool["function"]["name"] == browser_tool_name));
    author_client.disconnect_and_wait().await.unwrap();
    browser_client.disconnect_and_wait().await.unwrap();
    agent.registry.unregister(&author_names);
    agent.registry.unregister(&browser_names);
    let author_calls = author_fixture.finish().await;
    let browser_calls = browser_fixture.finish().await;
    assert_eq!(
        author_calls
            .iter()
            .filter(|request| request["method"] == "tools/call")
            .count(),
        1
    );
    assert!(
        browser_calls
            .iter()
            .all(|request| request["method"] != "tools/call"),
        "available author instructions must skip Playwright"
    );
}
