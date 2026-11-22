use super::*;
use crate::mcp_connections::McpConnections;
use std::collections::BTreeMap;

fn wire_plugin_revocation(
    engine: &FileEngine,
    connections: &McpConnections,
    registry: &ToolRegistry,
    servers: &McpServers,
) {
    let (connections, registry, servers) = (connections.clone(), registry.clone(), servers.clone());
    engine.set_plugin_mcp_change_handler(move |slug| {
        connections.invalidate_plugin(&registry, &servers, slug);
    });
}

#[test]
fn custom_http_authentication_uses_the_endpoint_to_select_the_key_format() {
    let (_dir, servers, settings, _) = fixture();
    let key = "mcp.custom.key";
    servers.write_secret(&settings, key, "fixture-key").unwrap();
    for (endpoint, expected) in [
        ("https://MOD.3dmgame.com:443/mcp/", "fixture-key"),
        ("https://mod.3dmgame.com/mcp", "fixture-key"),
        ("https://mod.3dmgame.com:444/mcp", "Bearer fixture-key"),
        ("https://mod.3dmgame.com/other", "Bearer fixture-key"),
        (
            "https://mod.3dmgame.com.example.org/mcp",
            "Bearer fixture-key",
        ),
    ] {
        let mut config = ServerConfig::http("custom", "custom", endpoint);
        config.bearer_secret_ref = Some(key.into());
        let (headers, _) = client::http_authorization(&settings, &config).unwrap();
        assert_eq!(headers["Authorization"], expected, "{endpoint}");
        assert!(headers["Authorization"].is_sensitive());
    }
}

#[tokio::test]
async fn http_auth_rejection_reports_status_without_disclosing_the_body_or_challenge() {
    use axum::{http::StatusCode, response::IntoResponse, routing::post, Router};

    for status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
        for challenge in [false, true] {
            let app = Router::new().route(
                "/mcp",
                post(move || async move {
                    let mut response = (
                        status,
                        [("content-type", "application/json")],
                        r#"{"message":"private-body"}"#,
                    )
                        .into_response();
                    if challenge {
                        response.headers_mut().insert(
                            "www-authenticate",
                            "Bearer realm=private-challenge".parse().unwrap(),
                        );
                    }
                    response
                }),
            );
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/mcp", listener.local_addr().unwrap());
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let (_dir, servers, settings, _) = fixture();
            let mut config = ServerConfig::http("rejected", "Rejected", &url);
            config.enabled = true;
            servers.create(&config).unwrap();
            let registry = ToolRegistry::default();
            let result = McpConnections::default()
                .connect(settings, &registry, config, false)
                .await;
            server.abort();
            let message = result
                .err()
                .expect("connection should be rejected")
                .to_string();
            assert!(
                message.contains(&format!("HTTP {}", status.as_u16())),
                "{message}"
            );
            assert!(!message.contains("private-"));
            assert!(registry.definitions().is_empty());
        }
    }
}

#[tokio::test]
async fn manual_description_edits_preserve_pending_and_live_connections() {
    let (_dir, servers, settings, context) = fixture();
    let (url, stop, _) = mock_http("echo").await;
    let _stop = stop.drop_guard();
    let mut config = ServerConfig::http("manual", "Manual", &url);
    config.enabled = true;
    config.description = Some("自定义服务".into());
    servers.create(&config).unwrap();
    assert_eq!(servers.list().unwrap(), [config.clone()]);
    let original = config.clone();
    config.description = Some("连接前修改的说明".into());
    assert!(!servers.update(&config).unwrap());

    let connections = McpConnections::default();
    let registry = ToolRegistry::default();
    let connected = connections
        .connect(settings, &registry, original, false)
        .await
        .unwrap();
    let existing = registry.get(&connected.tools[0]).unwrap();
    let args = json!({});
    let prepared = existing.prepare(&context, &args).await.unwrap();
    assert!(prepared.preview["server"].get("description").is_none());
    let digest = servers.startup_digest(&config.id).unwrap().unwrap();

    config.description = None;
    connections
        .revoke_if_needed(&registry, &config.id, || servers.update(&config))
        .await
        .unwrap();
    assert_eq!(servers.list().unwrap(), [config.clone()]);
    let description: Option<String> = servers
        .db
        .conn()
        .query_row(
            "SELECT description FROM mcp_servers WHERE id=?1",
            [&config.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(description.is_none());
    assert_eq!(servers.startup_digest(&config.id).unwrap(), Some(digest));
    assert_eq!(connections.list().await.len(), 1);
    assert!(registry.get(&connected.tools[0]).is_ok());
    existing.execute(&context, args, prepared).await.unwrap();

    config.url = Some(format!("{url}/changed"));
    config.description = Some("随新连接配置保存".into());
    connections
        .revoke_if_needed(&registry, &config.id, || servers.update(&config))
        .await
        .unwrap();
    assert_eq!(servers.get(&config.id).unwrap(), config);
    assert!(servers.startup_digest(&config.id).unwrap().is_none());
    assert!(connections.list().await.is_empty());
    assert!(registry.get(&connected.tools[0]).is_err());
}

#[tokio::test]
async fn plugin_runtime_revokes_only_stale_connections() {
    let (_dir, servers, settings, context) = fixture();
    let (url, stop, _) = mock_http("echo").await;
    let _stop = stop.drop_guard();
    register_test_plugin(&servers, "first", true, true);
    register_test_plugin(&servers, "second", true, true);
    let owned = |slug, suffix| {
        let mut config = plugin_server(slug, suffix);
        config.url = Some(url.clone());
        config
    };
    let changed = owned("first", "changed");
    let stable = owned("first", "stable");
    let other = owned("second", "other");
    sync_test_plugin(&servers, "first", &[changed.clone(), stable.clone()]);
    sync_test_plugin(&servers, "second", std::slice::from_ref(&other));
    let mut manual = ServerConfig::http("manual", "Manual", &url);
    manual.enabled = true;
    servers.create(&manual).unwrap();

    let connections = McpConnections::default();
    let registry = ToolRegistry::default();
    wire_plugin_revocation(&context.engine, &connections, &registry, &servers);

    let mut tampered = changed.clone();
    tampered.enabled = true;
    tampered.credential_url = Some("https://example.com/credentials".into());
    assert!(matches!(
        connections
            .connect(settings.clone(), &registry, tampered, false)
            .await,
        Err(AppError::Conflict(_))
    ));
    assert!(!servers.get(&changed.id).unwrap().enabled);
    assert!(servers.startup_requests().unwrap().is_empty());

    let mut tools = BTreeMap::new();
    for mut config in [
        changed.clone(),
        stable.clone(),
        other.clone(),
        manual.clone(),
    ] {
        config.enabled = true;
        let connection = connections
            .connect(settings.clone(), &registry, config, false)
            .await
            .unwrap();
        tools.insert(connection.server_id, connection.tools[0].clone());
    }
    // Failed manual CRUD cannot silently disconnect a plugin-owned service.
    assert!(matches!(
        connections
            .revoke(&registry, &stable.id, || servers.remove(&stable.id))
            .await,
        Err(AppError::Denied(_))
    ));
    assert!(registry.get(&tools[&stable.id]).is_ok());

    // Keep an Agent tool reference from before the upgrade, like an outstanding approval.
    let old_tool = registry.get(&tools[&changed.id]).unwrap();
    let args = json!({});
    let approved = old_tool.prepare(&context, &args).await.unwrap();
    let mut stable_edited = stable.clone();
    stable_edited.description = Some("适配包目录说明".into());
    let stable_digest = servers.startup_digest(&stable.id).unwrap().unwrap();
    {
        let _files = crate::transaction::write_lock();
        let mut upgrade = changed.clone();
        upgrade.name = "New name".into();
        assert!(sync_test_plugin(
            &servers,
            "first",
            &[upgrade, stable_edited.clone()]
        ));
        context.engine.clone().notify_plugin_mcp_changed("first");
    }
    let connected: Vec<_> = connections
        .list()
        .await
        .into_iter()
        .map(|info| info.server_id)
        .collect();
    assert_eq!(
        connected,
        [manual.id.clone(), stable.id.clone(), other.id.clone()]
    );
    assert_eq!(servers.startup_requests().unwrap(), connected);
    assert!(registry.get(&tools[&changed.id]).is_err());
    stable_edited.enabled = true;
    assert_eq!(servers.get(&stable.id).unwrap(), stable_edited);
    assert_eq!(
        servers.startup_digest(&stable.id).unwrap(),
        Some(stable_digest)
    );
    let stable_tool = registry.get(&tools[&stable.id]).unwrap();
    let stable_approval = stable_tool.prepare(&context, &args).await.unwrap();
    stable_tool
        .execute(&context, args.clone(), stable_approval)
        .await
        .unwrap();
    for config in [&stable, &other, &manual] {
        assert!(registry.get(&tools[&config.id]).is_ok());
    }

    // Roll back and explicitly reconnect the exact old config. Even exact equality
    // now cannot make the captured, previously approved tool reference callable again.
    {
        let _files = crate::transaction::write_lock();
        sync_test_plugin(&servers, "first", &[changed.clone(), stable]);
        context.engine.notify_plugin_mcp_changed("first");
    }
    let mut expected = changed;
    expected.enabled = true;
    connections
        .connect(settings, &registry, expected, false)
        .await
        .unwrap();
    assert!(matches!(
        old_tool.execute(&context, args, approved).await,
        Err(AppError::Conflict(_)) | Err(AppError::Cancelled)
    ));
}

#[tokio::test]
async fn plugin_runtime_cancels_handshake_without_registering_or_remembering() {
    let (_dir, servers, settings, context) = fixture();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    register_test_plugin(&servers, "starting", true, true);
    let mut config = plugin_server("starting", "slow");
    config.url = Some(format!("http://{}/mcp", listener.local_addr().unwrap()));
    sync_test_plugin(&servers, "starting", std::slice::from_ref(&config));
    config.enabled = true;
    let connections = McpConnections::default();
    let registry = ToolRegistry::default();
    wire_plugin_revocation(&context.engine, &connections, &registry, &servers);

    let mut start =
        Box::pin(connections.connect(settings.clone(), &registry, config.clone(), false));
    // Leave the old future suspended at initialize; do not let it clean up yet.
    let (_socket, _) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::select! {
            result = listener.accept() => result.unwrap(),
            _ = &mut start => panic!("initialize finished without a response"),
        }
    })
    .await
    .unwrap();

    let (url, stop, _) = mock_http("echo").await;
    let _stop = stop.drop_guard();
    config.url = Some(url);
    {
        let _files = crate::transaction::write_lock();
        sync_test_plugin(&servers, "starting", std::slice::from_ref(&config));
        context.engine.notify_plugin_mcp_changed("starting");
    }
    assert!(connections.list().await.is_empty());
    assert!(registry.definitions().is_empty());
    assert!(servers.startup_requests().unwrap().is_empty());

    let replacement = connections
        .connect(settings, &registry, config, false)
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), start)
        .await
        .unwrap();
    assert!(matches!(result, Err(AppError::Cancelled)));
    assert_eq!(connections.list().await.len(), 1);
    assert!(registry.get(&replacement.tools[0]).is_ok());
    assert_eq!(servers.startup_requests().unwrap(), [replacement.server_id]);
}

#[tokio::test]
async fn plugin_runtime_kills_starting_and_running_stdio_children() {
    // The Windows/macOS/Linux desktop test environment has Node from the front-end build.
    // Keep the Rust-only test suite usable when it is not installed.
    if std::process::Command::new("node")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let (_dir, servers, settings, context) = fixture();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    register_test_plugin(&servers, "stdio-owner", true, true);
    let connections = McpConnections::default();
    let registry = ToolRegistry::default();
    wire_plugin_revocation(&context.engine, &connections, &registry, &servers);
    // Side-channel stays open even when a RunningService closes its stdio pipes;
    // only the direct child exiting can close it. Neither branch exits voluntarily.
    let script = r#"
const io = require('node:readline').createInterface({ input: process.stdin });
require('node:net').connect(Number(process.argv[1]), '127.0.0.1');
io.on('line', (line) => {
  const msg = JSON.parse(line);
  if (process.argv[2] === 'hang' || msg.id === undefined) return;
  const result = msg.method === 'initialize'
    ? {protocolVersion:msg.params.protocolVersion,capabilities:{tools:{}},serverInfo:{name:'fixture',version:'1'}}
    : {tools:[{name:'probe',inputSchema:{type:'object'}}]};
  console.log(JSON.stringify({jsonrpc:'2.0', id:msg.id, result}));
});
"#;
    let script = script.lines().collect::<Vec<_>>().join(" ");
    for mode in ["hang", "running"] {
        let mut config = ServerConfig::stdio(
            "plugin_stdio",
            "Fixture",
            "node",
            vec![
                "-e".into(),
                script.clone(),
                "--".into(),
                port.clone(),
                mode.into(),
            ],
        );
        config.plugin_slug = Some("stdio-owner".into());
        sync_test_plugin(&servers, "stdio-owner", std::slice::from_ref(&config));
        config.enabled = true;
        assert!(matches!(
            connections
                .connect(settings.clone(), &registry, config.clone(), false)
                .await,
            Err(AppError::Denied(_))
        ));
        assert!(!servers.get(&config.id).unwrap().enabled);
        let start = connections.connect(settings.clone(), &registry, config, true);
        let accept = async { listener.accept().await.unwrap().0 };
        let revoke = || {
            let _files = crate::transaction::write_lock();
            sync_test_plugin(&servers, "stdio-owner", &[]);
            context.engine.notify_plugin_mcp_changed("stdio-owner");
        };
        tokio::time::timeout(Duration::from_secs(8), async {
            let mut child_socket = if mode == "hang" {
                let (result, socket) = tokio::join!(start, async {
                    let socket = accept.await;
                    revoke();
                    socket
                });
                assert!(matches!(result, Err(AppError::Cancelled)));
                socket
            } else {
                let (result, socket) = tokio::join!(start, accept);
                assert_eq!(result.unwrap().tools.len(), 1);
                revoke();
                socket
            };
            match child_socket.read(&mut [0u8; 1]).await {
                Ok(0) => (),
                Err(err) if err.kind() == std::io::ErrorKind::ConnectionReset => (),
                other => panic!("expected the child's socket to close, got {other:?}"),
            }
        })
        .await
        .unwrap();
        assert!(connections.list().await.is_empty());
        assert!(registry.definitions().is_empty());
        assert!(servers.startup_requests().unwrap().is_empty());
    }
}

#[tokio::test]
async fn one_manager_can_connect_more_than_eight_distinct_servers() {
    const SERVER_COUNT: usize = 9;

    let (_dir, servers, settings, _) = fixture();
    let connections = McpConnections::default();
    let registry = ToolRegistry::default();
    let mut _stops = Vec::with_capacity(SERVER_COUNT);

    for index in 0..SERVER_COUNT {
        let id = format!("fixture-{index}");
        let (url, stop, _) = mock_http("echo").await;
        _stops.push(stop.drop_guard());
        let mut config = ServerConfig::http(&id, &id, &url);
        config.enabled = true;
        servers.create(&config).unwrap();

        let connected = connections
            .connect(settings.clone(), &registry, config, false)
            .await
            .unwrap_or_else(|error| panic!("failed to connect {id}: {error}"));
        assert_eq!(connected.server_id, id);
        assert_eq!(connected.tools.len(), 1);
    }

    let mut connected_ids: Vec<_> = connections
        .list()
        .await
        .into_iter()
        .map(|connection| connection.server_id)
        .collect();
    connected_ids.sort();
    let expected_ids: Vec<_> = (0..SERVER_COUNT)
        .map(|index| format!("fixture-{index}"))
        .collect();

    assert_eq!(connected_ids, expected_ids);
    assert_eq!(registry.definitions().len(), SERVER_COUNT);
}

/// Exercise the packaged CLI and the same connection and navigation path as the desktop.
#[tokio::test]
#[ignore = "requires the prepared runtime and a cached Chromium or access to the Playwright CDN"]
async fn bundled_playwright_navigates_with_managed_chromium() {
    use axum::{response::Html, routing::get, Router};
    let (dir, servers, settings, ctx) = fixture();
    let base = dir.path().canonicalize().unwrap();
    let runtime = McpRuntime::bundled(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/mcp-runtime/runtime.zip"),
        base.join("runtime"),
    );
    servers.seed_presets().unwrap();
    let mut config = servers.get("playwright").unwrap();
    let output = config
        .args
        .iter()
        .position(|arg| arg == "--output-dir")
        .unwrap()
        + 1;
    let downloads = dunce::simplified(&base).join("browser output");
    config.args[output] = downloads.display().to_string();
    config.enabled = true;
    servers.update(&config).unwrap();
    let consent = ConnectionConsent::after_user_confirmation(&config, true).unwrap();
    let client = McpClient::connect(
        servers,
        &settings,
        &config.id,
        consent,
        &runtime,
        &ctx.cancel,
    )
    .await
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/", get(|| async { Html(r#"<title>Chromium fixture</title><a id="download" download="fixture.txt" href="/file">Download</a>"#) }))
        .route("/file", get(|| async { "Chromium downloaded this fixture" }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let result = client
        .call_tool("browser_navigate", json!({"url": url}), &ctx.cancel)
        .await
        .unwrap();
    assert!(
        result.to_string().contains("Page Title: Chromium fixture"),
        "{result}"
    );
    let download = client
        .call_tool("browser_click", json!({"target": "#download"}), &ctx.cancel)
        .await
        .unwrap();
    client.disconnect_and_wait().await.unwrap();
    server.abort();
    assert_eq!(
        std::fs::read_to_string(downloads.join("fixture.txt"))
            .unwrap_or_else(|err| panic!("{err}: {download}")),
        "Chromium downloaded this fixture"
    );
}
