use super::*;
use crate::{
    agent::{
        policy::Effect,
        tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition, ToolRegistry},
    },
    db::Db,
    error::{AppError, AppResult},
    settings::{SecretStore, Settings},
    storage::AppPaths,
    transaction::FileEngine,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct MockSecrets(Mutex<HashMap<String, String>>);
impl SecretStore for MockSecrets {
    fn read(&self, key: &str) -> AppResult<Option<String>> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn write(&self, key: &str, value: &str) -> AppResult<()> {
        self.0.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
}

fn fixture() -> (tempfile::TempDir, McpServers, Settings, ToolContext) {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(dir.path().join("data")).unwrap();
    let db = Db::open(&paths.db_file()).unwrap();
    let servers = McpServers::new(db.clone());
    let settings = Settings::with_secret_store(db.clone(), Arc::new(MockSecrets::default()));
    let context = ToolContext {
        engine: FileEngine::new(db, paths),
        cancel: CancellationToken::new(),
        progress: None,
        resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
    };
    (dir, servers, settings, context)
}

#[test]
fn sqlite_crud_presets_and_secret_references() {
    let (_dir, servers, settings, _) = fixture();
    servers.seed_presets().unwrap();
    servers.seed_presets().unwrap();
    let presets = servers.list().unwrap();
    assert_eq!(presets.len(), 4);
    assert!(presets
        .iter()
        .all(|server| !server.enabled && server.builtin));
    let http = servers.get("glossmod_http").unwrap();
    assert_eq!(
        http.url.as_deref(),
        Some("https://www.glossmod.com/api/v1/mcp")
    );
    let stdio = servers.get("glossmod_3dm").unwrap();
    assert_eq!(stdio.command.as_deref(), Some("uvx"));
    assert_eq!(stdio.args, vec!["glossmod-mcp"]);
    let key = &stdio.env_secret_refs["GLOSSMOD_API_KEY"];
    servers
        .write_secret(&settings, key, "mock-secret-value")
        .unwrap();
    assert_eq!(
        settings.read_secret(key).unwrap().as_deref(),
        Some("mock-secret-value")
    );
    let encoded: String = servers
        .db
        .conn()
        .query_row(
            "SELECT env_json FROM mcp_servers WHERE id='glossmod_3dm'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!encoded.contains("mock-secret-value"));
    assert!(encoded.contains(key));
    let mut local = ServerConfig::http("local", "Local", "http://127.0.0.1:12345/mcp");
    servers.create(&local).unwrap();
    assert!(servers.create(&local).is_err());
    local.name = "Edited".into();
    servers.update(&local).unwrap();
    servers.set_enabled("local", true).unwrap();
    assert!(servers.get("local").unwrap().enabled);
    assert_eq!(servers.get("local").unwrap().name, "Edited");
    servers.remove("local").unwrap();
    assert!(servers.get("local").is_err());
    assert!(servers
        .write_secret(&settings, "model.api_key", "wrong scope")
        .is_err());
}

#[test]
fn microsoft_mcp_presets_are_credential_free_and_preserve_existing_config() {
    let (_dir, servers, _, _) = fixture();
    // Simulate an existing database that already has a custom server with this ID.
    let existing = ServerConfig::http(
        "markitdown",
        "Local converter",
        "http://127.0.0.1:12345/mcp",
    );
    servers.create(&existing).unwrap();
    servers.seed_presets().unwrap();
    assert_eq!(servers.get("markitdown").unwrap(), existing);

    let mut playwright = servers.get("playwright").unwrap();
    assert_eq!(playwright.transport, McpTransport::Stdio);
    assert_eq!(
        playwright.command.as_deref(),
        Some(if cfg!(windows) { "npx.cmd" } else { "npx" })
    );
    assert_eq!(
        playwright.args,
        [
            "--yes",
            "@playwright/mcp@0.0.85",
            "--headless",
            "--isolated",
            "--no-webmcp"
        ]
    );
    assert!(playwright.env_secret_refs.is_empty() && playwright.bearer_secret_ref.is_none());
    assert!(!playwright.enabled && playwright.builtin);
    assert!(ConnectionConsent::after_user_confirmation(&playwright, false).is_err());

    playwright.name = "My browser".into();
    servers.update(&playwright).unwrap();
    servers.remove("markitdown").unwrap();
    servers.seed_presets().unwrap();
    assert_eq!(servers.get("playwright").unwrap(), playwright);
    let markitdown = servers.get("markitdown").unwrap();
    assert_eq!(markitdown.transport, McpTransport::Stdio);
    assert_eq!(markitdown.command.as_deref(), Some("uvx"));
    assert_eq!(
        markitdown.args,
        [
            "--with",
            "azure-ai-contentunderstanding==1.2.0b3",
            "markitdown-mcp==0.0.1a8"
        ]
    );
    assert!(markitdown.env_secret_refs.is_empty() && markitdown.bearer_secret_ref.is_none());
    assert!(!markitdown.enabled && markitdown.builtin);
    assert!(ConnectionConsent::after_user_confirmation(&markitdown, false).is_err());
}

#[test]
fn unsafe_plaintext_configuration_and_namespace_collisions_are_rejected() {
    for url in [
        "http://example.com/mcp",
        "https://user:secret@example.com/mcp",
        "https://example.com/mcp?token=secret",
        "file:///tmp/mcp",
    ] {
        assert!(ServerConfig::http("remote", "Remote", url)
            .validate()
            .is_err());
    }
    let mut stdio = ServerConfig::stdio("local", "Local", "uvx", vec!["glossmod-mcp".into()]);
    stdio
        .env_secret_refs
        .insert("GLOSSMOD_API_KEY".into(), "plaintext-secret".into());
    assert!(stdio.validate().is_err());
    stdio.env_secret_refs.clear();
    stdio
        .env_secret_refs
        .insert("LD_PRELOAD".into(), "mcp.inject".into());
    assert!(stdio.validate().is_err());
    assert!(serde_json::from_value::<ServerConfig>(json!({"id":"x","name":"x","transport":"stdio","command":"x","args":[],"url":null,"bearer_secret_ref":null,"env_json":{"API_KEY":"plaintext"}})).is_err());
    use super::client::namespaced_tool_name as name;
    let a = name("a-b", "read.file");
    assert_ne!(a, name("a_b", "read.file"));
    assert_ne!(a, name("a-b", "read_file"));
    assert!(a.starts_with("mcp__") && a.len() <= 64);
    assert!(a.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
}

async fn mock_http(mode: &'static str) -> (String, CancellationToken, Arc<AtomicBool>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let stop = CancellationToken::new();
    let stop_task = stop.clone();
    let auth = Arc::new(AtomicBool::new(false));
    let seen_auth = auth.clone();
    tokio::spawn(async move {
        loop {
            let accepted =
                tokio::select! { _=stop_task.cancelled()=>break, value=listener.accept()=>value };
            let Ok((mut socket, _)) = accepted else { break };
            let auth = seen_auth.clone();
            let cancel = stop_task.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let header_end = loop {
                    let mut buf = [0; 4096];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 || request.len() > 512 * 1024 {
                        return;
                    }
                    request.extend_from_slice(&buf[..n]);
                    if let Some(pos) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        break pos + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
                if headers.contains("authorization: bearer mock-secret-value") {
                    auth.store(true, Ordering::SeqCst);
                }
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")
                            .and_then(|s| s.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while request.len() < header_end + length {
                    let mut buf = [0; 4096];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                let mut status = "200 OK";
                let body = if headers.starts_with("get ") {
                    status = "405 Method Not Allowed";
                    String::new()
                } else {
                    let Ok(req) = serde_json::from_slice::<Value>(&request[header_end..]) else {
                        return;
                    };
                    let method = req["method"].as_str().unwrap_or("");
                    if req.get("id").is_none() {
                        status = "202 Accepted";
                        String::new()
                    } else {
                        let result = match method {
                            "initialize" => {
                                json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"mock","version":"1"}})
                            }
                            "tools/list" => {
                                let mut tool = json!({"name":"echo","description":"Mock tool","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true,"destructiveHint":false}});
                                if matches!(mode, "credential_echo" | "tool_error") {
                                    tool["description"] =
                                        json!("Mock tool Bearer mock-secret-value");
                                } else if mode == "credential_name" {
                                    tool["name"] = json!("mock-secret-value");
                                } else if mode == "credential_schema" {
                                    tool["inputSchema"]["description"] = json!("mock-secret-value");
                                }
                                if mode == "duplicate" {
                                    json!({"tools":[tool.clone(),tool]})
                                } else if mode == "two" {
                                    let mut other = tool.clone();
                                    other["name"] = json!("other");
                                    json!({"tools":[tool,other]})
                                } else if mode == "cursor" {
                                    json!({"tools":[],"nextCursor":"same"})
                                } else if mode == "count" {
                                    let tools: Vec<Value> = (0..=super::client::MAX_TOOLS)
                                        .map(|i| {
                                            let mut tool = tool.clone();
                                            tool["name"] = json!(format!("tool_{i}"));
                                            tool
                                        })
                                        .collect();
                                    json!({"tools":tools})
                                } else if mode == "schema" {
                                    let mut tool = tool;
                                    tool["inputSchema"]["description"] =
                                        json!("x".repeat(super::client::MAX_SCHEMA_BYTES + 1));
                                    json!({"tools":[tool]})
                                } else {
                                    json!({"tools":[tool]})
                                }
                            }
                            "tools/call" => {
                                if mode == "slow" {
                                    tokio::select! {_=cancel.cancelled()=>return,_=tokio::time::sleep(Duration::from_secs(60))=>{}}
                                }
                                if mode == "rpc_error" {
                                    let body = json!({
                                        "jsonrpc":"2.0", "id":req["id"],
                                        "error":{"code":-32603,"message":"Bearer mock-secret-value","data":{"credential":"mock-secret-value"}}
                                    }).to_string();
                                    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                                    let _ = socket.write_all(response.as_bytes()).await;
                                    return;
                                }
                                let text = if mode == "large" {
                                    "x".repeat(super::client::MAX_RESULT_BYTES + 1)
                                } else if mode == "expansion" {
                                    "x".repeat(super::client::MAX_RESULT_BYTES / 2)
                                } else if matches!(mode, "credential_echo" | "tool_error") {
                                    "Bearer mock-secret-value; ordinary output".into()
                                } else {
                                    req["params"]["arguments"].to_string()
                                };
                                if matches!(mode, "credential_echo" | "tool_error") {
                                    json!({
                                        "content":[{"type":"text","text":text}],
                                        "structuredContent":{"mock-secret-value":[{"credential":"mock-secret-value"}],"ordinary":42},
                                        "_meta":{"credential":"mock-secret-value"},
                                        "isError":mode == "tool_error"
                                    })
                                } else {
                                    json!({"content":[{"type":"text","text":text}],"isError":false})
                                }
                            }
                            _ => json!({}),
                        };
                        json!({"jsonrpc":"2.0","id":req["id"],"result":result}).to_string()
                    }
                };
                let response=format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    (url, stop, auth)
}

async fn connect_http(servers: &McpServers, settings: &Settings, url: &str, id: &str) -> McpClient {
    let mut config = ServerConfig::http(id, "Mock", url);
    config.enabled = true;
    config.bearer_secret_ref = Some("mcp.mock.key".into());
    servers
        .write_secret(settings, "mcp.mock.key", "mock-secret-value")
        .unwrap();
    servers.create(&config).unwrap();
    let consent = ConnectionConsent::after_user_confirmation(&config, false).unwrap();
    McpClient::connect(
        servers.clone(),
        settings,
        id,
        consent,
        &CancellationToken::new(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn http_mock_auth_tools_bridge_and_revocation() {
    let (_dir, servers, settings, context) = fixture();
    let (url, stop, auth) = mock_http("echo").await;
    let client = connect_http(&servers, &settings, &url, "mock_http").await;
    let registry = ToolRegistry::default();
    let names = client
        .register_tools(&registry, &context.cancel)
        .await
        .unwrap();
    assert_eq!(names.len(), 1);
    assert!(auth.load(Ordering::SeqCst));
    let tool = registry.get(&names[0]).unwrap();
    assert_eq!(tool.definition().effect, Effect::ExternalWrite); // ignores readOnlyHint
    let args = json!({"message":"hello"});
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert_eq!(prepared.effect, Effect::ExternalWrite);
    assert_eq!(prepared.preview["trust"], "untrusted_external");
    let result = tool
        .execute(&context, args.clone(), prepared)
        .await
        .unwrap();
    assert_eq!(result["content"][0]["text"], args.to_string());
    assert!(client
        .register_tools(&registry, &context.cancel)
        .await
        .is_err());
    let bypass = PreparedTool {
        effect: Effect::Read,
        preview: json!({}),
        plan: None,
    };
    assert!(tool.execute(&context, args.clone(), bypass).await.is_err());
    servers.set_enabled("mock_http", false).unwrap();
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert!(tool.execute(&context, args, prepared).await.is_err());
    stop.cancel();
}

#[tokio::test]
async fn credential_echo_is_redacted_before_agent_results_and_history_persistence() {
    use crate::history::{FunctionCall, History, ToolCall, ToolCallUpdate};

    let (_dir, servers, settings, context) = fixture();
    let history = History::new(servers.db.clone());
    let conversation = history.create_conversation("MCP redaction", None).unwrap();
    for mode in ["credential_echo", "tool_error"] {
        let (url, stop, _) = mock_http(mode).await;
        let client = connect_http(&servers, &settings, &url, mode).await;
        let registry = ToolRegistry::default();
        let names = client
            .register_tools(&registry, &context.cancel)
            .await
            .unwrap();
        let tool = registry.get(&names[0]).unwrap();
        assert!(!tool.definition().description.contains("mock-secret-value"));
        assert!(tool.definition().description.contains("[REDACTED]"));
        // Rotation in Settings must not forget the credential this active
        // connection actually transmitted during its confirmed handshake.
        settings
            .write_secret("mcp.mock.key", "rotated-mock-value")
            .unwrap();
        let args = json!({"ordinary":"preserved"});
        let prepared = tool.prepare(&context, &args).await.unwrap();
        let result = tool.execute(&context, args, prepared).await.unwrap();
        let encoded = serde_json::to_string(&result).unwrap();
        assert!(!encoded.contains("mock-secret-value"));
        assert!(!format!("{result:?}").contains("mock-secret-value"));
        assert_eq!(
            result["content"][0]["text"],
            "Bearer [REDACTED]; ordinary output"
        );
        assert_eq!(
            result["structuredContent"]["[REDACTED]"][0]["credential"],
            "[REDACTED]"
        );
        assert_eq!(result["structuredContent"]["ordinary"], 42);
        assert_eq!(result["_meta"]["credential"], "[REDACTED]");
        assert_eq!(result["isError"], mode == "tool_error");

        let call = ToolCall {
            id: format!("call_{mode}"),
            kind: "function".into(),
            function: FunctionCall {
                name: names[0].clone(),
                arguments: "{}".into(),
            },
        };
        let assistant = history
            .append_message(&conversation.id, "assistant", "", Some(json!([call])), None)
            .unwrap();
        history
            .set_tool_call(
                &conversation.id,
                Some(&assistant.id),
                &call,
                ToolCallUpdate {
                    status: "completed".into(),
                    approval_status: "approved".into(),
                    result: Some(result),
                    error: None,
                },
            )
            .unwrap();
        history
            .append_message(&conversation.id, "tool", &encoded, None, Some(&call.id))
            .unwrap();
        let persisted = history.get_conversation(&conversation.id).unwrap();
        assert!(!serde_json::to_string(&persisted)
            .unwrap()
            .contains("mock-secret-value"));
        let persisted: String = servers
            .db
            .conn()
            .query_row(
                "SELECT result_json FROM tool_calls WHERE id=?1",
                [&call.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!persisted.contains("mock-secret-value"));
        assert!(persisted.contains("[REDACTED]"));
        client.disconnect_and_wait().await.unwrap();
        stop.cancel();
    }
}

#[tokio::test]
async fn remote_error_details_and_credential_bearing_contracts_are_withheld() {
    let (_dir, servers, settings, context) = fixture();
    for mode in ["rpc_error", "credential_name", "credential_schema"] {
        let (url, stop, _) = mock_http(mode).await;
        let client = connect_http(&servers, &settings, &url, mode).await;
        let error = if mode == "rpc_error" {
            client
                .call_tool("echo", json!({}), &context.cancel)
                .await
                .unwrap_err()
        } else {
            client.list_tools(&context.cancel).await.unwrap_err()
        };
        assert!(!error.to_string().contains("mock-secret-value"));
        assert!(!format!("{error:?}").contains("mock-secret-value"));
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains("mock-secret-value"));
        assert!(error.to_string().contains("remote details suppressed"));
        client.disconnect_and_wait().await.unwrap();
        stop.cancel();
    }
}

#[tokio::test]
async fn redaction_expansion_cannot_bypass_result_size_limit() {
    let (_dir, servers, settings, context) = fixture();
    let (url, stop, _) = mock_http("expansion").await;
    let mut config = ServerConfig::http("expansion", "Mock", &url);
    config.enabled = true;
    config.bearer_secret_ref = Some("mcp.expansion.key".into());
    servers
        .write_secret(&settings, "mcp.expansion.key", "x")
        .unwrap();
    servers.create(&config).unwrap();
    let consent = ConnectionConsent::after_user_confirmation(&config, false).unwrap();
    let client = McpClient::connect(servers, &settings, &config.id, consent, &context.cancel)
        .await
        .unwrap();
    let error = client
        .call_tool("echo", json!({}), &context.cancel)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("exceeds limits"));
    client.disconnect_and_wait().await.unwrap();
    stop.cancel();
}

#[tokio::test]
async fn credential_backend_errors_do_not_expose_provider_details() {
    struct FailingSecrets;
    impl SecretStore for FailingSecrets {
        fn read(&self, _: &str) -> AppResult<Option<String>> {
            Err(AppError::Other("backend included mock-secret-value".into()))
        }
        fn write(&self, _: &str, _: &str) -> AppResult<()> {
            Ok(())
        }
    }
    let (_dir, servers, _, context) = fixture();
    let settings = Settings::with_secret_store(servers.db.clone(), Arc::new(FailingSecrets));
    settings
        .write_secret("mcp.failed.key", "mock-secret-value")
        .unwrap();
    let mut config = ServerConfig::http("failed_keychain", "Mock", "http://127.0.0.1:9/mcp");
    config.enabled = true;
    config.bearer_secret_ref = Some("mcp.failed.key".into());
    servers.create(&config).unwrap();
    let consent = ConnectionConsent::after_user_confirmation(&config, false).unwrap();
    let error =
        match McpClient::connect(servers, &settings, &config.id, consent, &context.cancel).await {
            Err(error) => error,
            Ok(_) => panic!("failed credential provider must prevent connection"),
        };
    assert!(!error.to_string().contains("mock-secret-value"));
    assert!(!format!("{error:?}").contains("mock-secret-value"));
    assert!(!serde_json::to_string(&error)
        .unwrap()
        .contains("mock-secret-value"));
}

#[tokio::test]
async fn bounds_duplicate_pagination_cancellation_and_large_results() {
    let (_dir, servers, settings, context) = fixture();
    for mode in ["duplicate", "cursor", "count", "schema", "large", "slow"] {
        let (url, stop, _) = mock_http(mode).await;
        let client = connect_http(&servers, &settings, &url, mode).await;
        if matches!(mode, "duplicate" | "cursor" | "count" | "schema") {
            assert!(client.list_tools(&context.cancel).await.is_err());
        } else if mode == "large" {
            assert!(client
                .call_tool("echo", json!({}), &context.cancel)
                .await
                .is_err());
        } else {
            let cancel = CancellationToken::new();
            let trigger = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                trigger.cancel();
            });
            assert!(matches!(
                client.call_tool("echo", json!({}), &cancel).await,
                Err(AppError::Cancelled)
            ));
        }
        stop.cancel();
    }
}

#[tokio::test]
async fn disabled_and_stale_confirmations_cannot_connect() {
    let (_dir, servers, settings, context) = fixture();
    let mut config = ServerConfig::stdio("blocked", "Blocked", "does-not-exist", vec![]);
    servers.create(&config).unwrap();
    assert!(ConnectionConsent::after_user_confirmation(&config, false).is_err());
    let consent = ConnectionConsent::after_user_confirmation(&config, true).unwrap();
    assert!(McpClient::connect(
        servers.clone(),
        &settings,
        &config.id,
        consent,
        &context.cancel
    )
    .await
    .is_err());
    let consent = ConnectionConsent::after_user_confirmation(&config, true).unwrap();
    config.enabled = true;
    servers.update(&config).unwrap();
    assert!(matches!(
        McpClient::connect(servers, &settings, &config.id, consent, &context.cancel).await,
        Err(AppError::Conflict(_))
    ));
}

#[tokio::test]
async fn batch_registration_capacity_failure_leaves_no_mcp_namespace() {
    struct Existing(String);
    #[async_trait::async_trait]
    impl AgentTool for Existing {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.0.clone(),
                description: String::new(),
                parameters: json!({"type":"object"}),
                effect: Effect::Read,
            }
        }
        async fn execute(&self, _: &ToolContext, _: Value, _: PreparedTool) -> AppResult<Value> {
            Ok(Value::Null)
        }
    }
    let (_dir, servers, settings, context) = fixture();
    let (url, stop, _) = mock_http("two").await;
    let client = connect_http(&servers, &settings, &url, "atomic").await;
    let registry = ToolRegistry::default();
    for i in 0..255 {
        registry
            .register(Arc::new(Existing(format!("existing_{i}"))))
            .unwrap();
    }
    assert!(client
        .register_tools(&registry, &context.cancel)
        .await
        .is_err());
    assert_eq!(registry.definitions().len(), 255);
    for remote in ["echo", "other"] {
        assert!(registry
            .get(&super::client::namespaced_tool_name("atomic", remote))
            .is_err());
    }
    client.disconnect_and_wait().await.unwrap();
    stop.cancel();
}

#[cfg(unix)]
#[tokio::test]
async fn confirmed_stdio_mock_receives_keychain_secret_without_ambient_credentials() {
    let (_dir, servers, settings, context) = fixture();
    let python = if std::path::Path::new("/usr/bin/python3").exists() {
        "/usr/bin/python3"
    } else {
        "python3"
    };
    let script = std::path::Path::new(file!())
        .parent()
        .unwrap()
        .join("mock_stdio.py");
    let script = if script.is_absolute() {
        script
    } else {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(script)
    };
    let mut config = ServerConfig::stdio(
        "stdio_mock",
        "Mock",
        python,
        vec![script.to_string_lossy().into_owned()],
    );
    config.enabled = true;
    config
        .env_secret_refs
        .insert("GLOSSMOD_API_KEY".into(), "mcp.stdio.key".into());
    config
        .env_secret_refs
        .insert("SECOND_API_KEY".into(), "mcp.stdio.second_key".into());
    servers
        .write_secret(&settings, "mcp.stdio.key", "mock-secret-value")
        .unwrap();
    servers
        .write_secret(&settings, "mcp.stdio.second_key", "mock-\"quoted\\env")
        .unwrap();
    servers.create(&config).unwrap();
    let consent = ConnectionConsent::after_user_confirmation(&config, true).unwrap();
    let client = McpClient::connect(
        servers.clone(),
        &settings,
        &config.id,
        consent,
        &context.cancel,
    )
    .await
    .unwrap();
    assert_eq!(client.list_tools(&context.cancel).await.unwrap().len(), 1);
    let result = client
        .call_tool("echo", json!({"test":true}), &context.cancel)
        .await
        .unwrap();
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["secretPresent"], true);
    assert_eq!(text["ambientSecretPresent"], false);
    assert_eq!(text["echoedCredential"], "[REDACTED]");
    assert_eq!(text["secondCredential"], "[REDACTED]");
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains("mock-secret-value"));
    let pid = text["pid"].as_u64().unwrap();
    assert!(std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .unwrap()
        .success());
    servers.remove(&config.id).unwrap();
    assert!(client.list_tools(&context.cancel).await.is_err());
    client.disconnect_and_wait().await.unwrap();
    assert!(!std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success());
}
