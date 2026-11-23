use super::*;
use crate::{
    artifacts,
    mcp::glossmod_download::{GlossModDownload, TOOL},
    sources::resolutions::ResolutionScope,
};
use axum::{
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use std::sync::atomic::AtomicUsize;

#[test]
fn only_the_official_mod_inn_endpoint_accepts_guest_access_for_a_configured_key() {
    let (_dir, servers, settings, _) = fixture();
    servers.seed_presets().unwrap();
    let mut config = servers.get("glossmod_http").unwrap();
    assert!(config.is_glossmod_endpoint());
    assert!(client::http_authorization(&settings, &config)
        .unwrap()
        .0
        .is_empty());
    for url in [
        "https://www.glossmod.com.example.test/api/v1/mcp",
        "https://www.glossmod.com/api/other",
        "http://www.glossmod.com/api/v1/mcp",
    ] {
        config.url = Some(url.into());
        assert!(!config.is_glossmod_endpoint());
        assert!(client::http_authorization(&settings, &config).is_err());
    }
}

#[tokio::test]
async fn approved_mod_inn_file_is_signed_once_and_saved_without_exposing_the_url() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let signed = format!("{origin}/asset?signature=private-fixture-ticket");
    let calls = Arc::new(AtomicUsize::new(0));
    let signing_calls = calls.clone();
    let zip: &[u8] = b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";
    let router = Router::new().route("/mcp", post(move |headers:HeaderMap,body:String|{
        let signed = signed.clone(); let calls = signing_calls.clone();
        async move {
            assert_eq!(headers[header::AUTHORIZATION], "Bearer test-service-key");
            let req: Value = serde_json::from_str(&body).unwrap();
            if req.get("id").is_none() { return StatusCode::ACCEPTED.into_response(); }
            let result = match req["method"].as_str().unwrap() {
                "initialize" => json!({"protocolVersion":"2025-03-26","serverInfo":{"name":"fixture","version":"1"},"capabilities":{"tools":{}}}),
                "tools/call" => {
                    assert_eq!(req["params"]["name"], TOOL);
                    assert_eq!(req["params"]["arguments"], json!({"work_id":42,"file_id":103}));
                    calls.fetch_add(1,Ordering::SeqCst);
                    let file=json!({"url":signed,"name":"selected.zip","download_count":1,"file_download_count":1});
                    json!({"structuredContent":file,"content":[{"type":"text","text":file.to_string()}]})
                },
                _ => panic!("unexpected method"),
            };
            ([(header::CONTENT_TYPE,"application/json")],json!({"jsonrpc":"2.0","id":req["id"],"result":result}).to_string()).into_response()
        }
    })).route("/asset",get(move |headers: HeaderMap, uri: axum::http::Uri|async move {
        assert_eq!(uri.query(), Some("signature=private-fixture-ticket"));
        assert!(!headers.contains_key(header::AUTHORIZATION));
        zip
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let (_dir, servers, settings, mut ctx) = fixture();
    let mut config = ServerConfig::http("fixture", "Fixture", &format!("{origin}/mcp"));
    config.enabled = true;
    config.bearer_secret_ref = Some("mcp.fixture.key".into());
    servers
        .write_secret(&settings, "mcp.fixture.key", "test-service-key")
        .unwrap();
    servers.create(&config).unwrap();
    let consent = ConnectionConsent::after_user_confirmation(&config, false).unwrap();
    let client = McpClient::connect(
        servers,
        &settings,
        "fixture",
        consent,
        &McpRuntime::default(),
        &ctx.cancel,
    )
    .await
    .unwrap();
    // Inject the fixture client directly; production dispatch requires the official HTTPS endpoint.
    let tool = GlossModDownload::new(client.clone(), "test_tool".into());
    let args = json!({"work_id":42,"file_id":103,"gameName":"Example Game","author":"Public Author","projectPage":"https://example.com/project"});
    let mut bad_metadata = args.clone();
    bad_metadata["projectPage"] = json!("https://example.com/project?signature=private-input");
    assert_eq!(
        tool.history_arguments(&bad_metadata.to_string()).as_deref(),
        Some("{}")
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &tool.history_arguments(&args.to_string()).unwrap()
        )
        .unwrap(),
        args,
    );
    assert!(tool.prepare(&ctx, &args).await.is_err());
    let chat = crate::history::History::new(ctx.engine.db.clone())
        .create_conversation("download", None)
        .unwrap();
    ctx.resolution_scope = ResolutionScope::Agent(chat.id);
    let first = tool.prepare(&ctx, &args).await.unwrap();
    assert_eq!(first.effect, Effect::UncatalogedDownload);
    let mut changed = args.clone();
    changed["file_id"] = json!(104);
    assert!(tool.execute(&ctx, changed, first).await.is_err());
    assert!(tool
        .prepare(&ctx, &json!({"work_id":42,"file_id":103,"url":"forged"}))
        .await
        .is_err());
    let cancelled = tool.prepare(&ctx, &args).await.unwrap();
    let mut cancelled_ctx = ctx.clone();
    cancelled_ctx.cancel = CancellationToken::new();
    cancelled_ctx.cancel.cancel();
    assert!(matches!(
        tool.execute(&cancelled_ctx, args.clone(), cancelled).await,
        Err(AppError::Cancelled)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(tool
        .prepare(&ctx, &json!({"work_id":42,"file_id":103}))
        .await
        .is_err());
    let approved = tool.prepare(&ctx, &args).await.unwrap();
    assert_eq!(
        approved.preview["classification"]["gameName"],
        "Example Game"
    );
    let receipt = tool.execute(&ctx, args, approved).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(receipt["cataloged"], true);
    assert_eq!(
        receipt["artifact"]["sha256"],
        hex::encode(Sha256::digest(zip))
    );
    assert_eq!(
        std::fs::read(receipt["artifact"]["path"].as_str().unwrap()).unwrap(),
        zip
    );
    let root = ctx.engine.paths.mod_library().canonicalize().unwrap();
    let library_path = std::path::Path::new(receipt["artifact"]["path"].as_str().unwrap());
    assert_eq!(
        receipt["artifact"]["projectPage"],
        "https://example.com/project"
    );
    assert_eq!(receipt["artifact"]["authorHint"], "Public Author");
    assert!(library_path.canonicalize().unwrap().starts_with(root));
    let records = artifacts::list(&ctx.engine.db).unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(receipt["artifactId"], record.artifact.id);
    assert_eq!(record.artifact.path, library_path.canonicalize().unwrap());
    let persisted = record.native_library_intent().unwrap();
    assert_eq!(
        persisted.project_page.as_deref(),
        Some("https://example.com/project")
    );
    assert_eq!(persisted.author_hint.as_deref(), Some("Public Author"));
    let (page, author): (String, String) = ctx.engine.db.conn().query_row(
        "SELECT project_page,author_hint FROM mod_library_entries WHERE id=?1 AND state='ready'",
        [&record.artifact.id], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(page, "https://example.com/project");
    assert_eq!(author, "Public Author");
    assert_eq!(record.source_id, "glossmod_http");
    assert_eq!(record.project_id, "42");
    assert_eq!(record.version_id, "103");
    for value in [receipt.to_string(), serde_json::to_string(record).unwrap()] {
        assert!(!value.contains("private-fixture-ticket") && !value.contains("test-service-key"));
    }
    client.disconnect_and_wait().await.unwrap();
    server.abort();
}

/// Read-only discovery: a successful get_work_file_download counts as a download,
/// so the live probe deliberately stops before invoking that tool.
#[tokio::test]
#[ignore = "requires network access to the public Mod Inn MCP endpoint"]
async fn live_mod_inn_guest_registers_attachment_transfer_for_approval() {
    let (_dir, servers, settings, context) = fixture();
    servers.seed_presets().unwrap();
    servers.set_enabled("glossmod_http", true).unwrap();
    let config = servers.get("glossmod_http").unwrap();
    let consent = ConnectionConsent::after_user_confirmation(&config, false).unwrap();
    let client = McpClient::connect(
        servers,
        &settings,
        "glossmod_http",
        consent,
        &McpRuntime::default(),
        &context.cancel,
    )
    .await
    .unwrap();
    let registry = ToolRegistry::default();
    client
        .register_tools(&registry, &context.cancel)
        .await
        .unwrap();
    for name in ["list_works", "list_work_files"] {
        assert_eq!(
            registry
                .get(&client::namespaced_tool_name("glossmod_http", name))
                .unwrap()
                .definition()
                .effect,
            Effect::ExternalWrite
        );
    }
    let signing = registry
        .get(&client::namespaced_tool_name("glossmod_http", TOOL))
        .unwrap()
        .definition();
    assert_eq!(signing.effect, Effect::UncatalogedDownload);
    assert!(signing.description.contains("artifact.path"));
    client.disconnect_and_wait().await.unwrap();
}
