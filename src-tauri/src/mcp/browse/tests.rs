use super::*;
use serde_json::json;

#[test]
fn browse_real_discovery_schemas_for_all_seven_match_only_allowlisted_contracts() {
    let fixtures = [
        (Source::GlossMod, include_str!("glossmod_http.json")),
        (Source::ThreeDm, include_str!("glossmod_3dm.json")),
        (Source::Nexus, include_str!("nexusmods.json")),
        (Source::CurseForge, include_str!("curseforge.json")),
        (Source::GameBanana, include_str!("gamebanana.json")),
        (Source::ModIo, include_str!("modio.json")),
        (Source::Thunderstore, include_str!("thunderstore.json")),
    ];
    for (source, fixture) in fixtures {
        let value: Value = serde_json::from_str(fixture).unwrap();
        for operation in [Operation::Games, Operation::Browse, Operation::Search] {
            let contract = contract(source, operation);
            let schema = &value[contract.name];
            assert!(
                contract.schema_matches(schema),
                "{source:?}: {}",
                contract.name
            );
            let mut args = Map::new();
            for (p, ty) in contract.fixed {
                args.insert(
                    (*p).into(),
                    match ty {
                        S => json!(if *p == "scope" { "" } else { "test" }),
                        I => json!(1),
                        B => json!(false),
                    },
                );
            }
            if let Some(query) = contract.query {
                args.insert(query.into(), json!("one game"));
            }
            assert!(
                contract.arguments_match(&args, schema),
                "{source:?}: {} {args:?}",
                contract.name
            );
            for extra in [
                "file_id",
                "url",
                "headers",
                "token",
                "download_dir",
                "status",
            ] {
                args.insert(extra.into(), json!("must not be sent"));
                assert!(!contract.arguments_match(&args, schema));
                args.remove(extra);
            }
            let mut drift = schema.clone();
            drift["required"] = json!(["token"]);
            assert!(!contract.schema_matches(&drift));
            drift = schema.clone();
            drift["properties"][contract.fixed[0].0] = json!({"type":"array"});
            assert!(!contract.schema_matches(&drift));
        }
    }
}

#[test]
fn browse_refuses_personal_scope_or_adult_opt_in() {
    let val: Value = serde_json::from_str(include_str!("glossmod_http.json")).unwrap();
    let request = json!({"scope":"mine","offset":1,"limit":20,"game":"one"});
    assert!(!contract(Source::GlossMod, Operation::Browse)
        .arguments_match(request.as_object().unwrap(), &val["list_works"]));
    let val: Value = serde_json::from_str(include_str!("nexusmods.json")).unwrap();
    let request = json!({"adult_content":true,"offset":1,"count":20,"game_domain":"one"});
    assert!(!contract(Source::Nexus, Operation::Browse)
        .arguments_match(request.as_object().unwrap(), &val["find_mods"]));
}

#[test]
fn browse_decodes_only_known_catalog_frames_and_hides_errors() {
    let encoded = json!({"content":[{"type":"text","text":"{\"data\":[]}"}]});
    assert_eq!(decode(encoded).unwrap(), json!({"data":[]}));
    let structured = json!({"structuredContent":{"items":[]},"content":[{"type":"resource","uri":"file:///path"}]});
    assert_eq!(decode(structured).unwrap(), json!({"items":[]}));
    let untrusted = json!({"content":[{"type":"resource","uri":"file:///path"}]});
    assert!(decode(untrusted).is_err());
    let error = decode(json!({"isError":true,"content":[{"type":"text","text":"mod.io HTTP 403 secret server details"}]})).unwrap_err();
    assert!(matches!(&error, AppError::Denied(_)));
    assert!(!error.to_string().contains("secret server details"));
}

#[test]
fn browse_credential_revision_is_opaque_and_tracks_same_timestamp_rotations() {
    let dir = tempfile::tempdir().unwrap();
    let servers = McpServers::new(crate::db::Db::open(&dir.path().join("browse.db")).unwrap());
    servers.seed_presets().unwrap();
    let config = servers.get("glossmod_http").unwrap();
    let absent = credential_revision(&servers, &config, true).unwrap();
    let sql = "INSERT INTO secrets_ref VALUES(?1,'fixture','fixture','same-time') ON CONFLICT(key) DO UPDATE SET updated_at='same-time'";
    servers.db.conn().execute(sql, ["mcp.other.key"]).unwrap();
    assert!(credential_revision(&servers, &config, false).unwrap() == absent);
    let key = config.bearer_secret_ref.clone().unwrap();
    servers.db.conn().execute(sql, [&key]).unwrap();
    let first = credential_revision(&servers, &config, false).unwrap();
    assert!(first != absent);
    servers.db.conn().execute(sql, [&key]).unwrap();
    let second = credential_revision(&servers, &config, false).unwrap();
    assert!(second != first);
    servers
        .db
        .conn()
        .execute("DELETE FROM secrets_ref WHERE key=?1", [&key])
        .unwrap();
    assert!(credential_revision(&servers, &config, false).unwrap() != absent);
}

#[test]
fn browse_stock_identity_rejects_custom_bindings_endpoints_and_sandboxes() {
    let dir = tempfile::tempdir().unwrap();
    let servers = McpServers::new(crate::db::Db::open(&dir.path().join("browse.db")).unwrap());
    servers.seed_presets().unwrap();
    for source in Source::ALL {
        let mut config = servers.get(source.id()).unwrap();
        assert!(config.is_stock_browse_source());
        config
            .env_secret_refs
            .insert("UNEXPECTED".into(), "mcp.unexpected".into());
        assert!(!config.is_stock_browse_source());
    }
    assert!(public_modio_base("https://u-123.modapi.io/v1/"));
    assert!(public_modio_base("https://api.mod.io/v1"));
    for url in [
        "https://g-123.test.mod.io/v1",
        "https://api.mod.io.test.invalid/v1",
        "https://api.mod.io/v1?token=foo",
        "https://user@api.mod.io/v1",
    ] {
        assert!(!public_modio_base(url), "{url}");
    }
}

#[tokio::test]
async fn browse_loopback_read_redacts_and_cancels_without_disconnect() {
    use axum::{
        body::Bytes,
        http::{header, StatusCode},
        response::IntoResponse,
        routing::post,
        Router,
    };
    use std::time::Duration;
    use tokio::sync::Notify;
    let called = Arc::new(Notify::new());
    let wire_cancelled = Arc::new(Notify::new());
    let app = Router::new().route("/mcp", post({
        let called = called.clone();
        let wire_cancelled = wire_cancelled.clone();
        move |body: Bytes| {
            let msg: Value = serde_json::from_slice(&body).unwrap();
            let called = called.clone();
            let wire_cancelled = wire_cancelled.clone();
            async move {
                let method = msg["method"].as_str().unwrap();
                let result = match method {
                    "initialize" => json!({"protocolVersion":msg["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"browse-fixture","version":"1"}}),
                    "notifications/initialized" => return StatusCode::ACCEPTED.into_response(),
                    "notifications/cancelled" => {
                        wire_cancelled.notify_one();
                        return StatusCode::ACCEPTED.into_response();
                    }
                    "tools/call" => {
                        if msg["params"]["arguments"]["q"] == "slow" {
                            called.notify_one();
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                        if msg["params"]["arguments"]["q"] == "escaped" {
                            json!({"content":[{"type":"text","text":r#"{"items":[{"title":"\u0062rowse-fixture-key-secret"}]}"#}]})
                        } else {
                            json!({"structuredContent":{"items":[{"title":"browse-fixture-key-secret"}]},"content":[]})
                        }
                    }
                    other => panic!("unexpected MCP method: {other}"),
                };
                ([(header::CONTENT_TYPE, "application/json")], json!({"id":msg["id"],"jsonrpc":"2.0","result":result}).to_string()).into_response()
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Db::open(&dir.path().join("browse.db")).unwrap();
    let settings = crate::settings::Settings::with_secret_store(
        db.clone(),
        Arc::new(crate::mcp::tests::MockSecrets::default()),
    );
    let servers = McpServers::new(db);
    let mut config = ServerConfig::http("browse_fixture", "Fixture", &format!("http://{addr}/mcp"));
    config.bearer_secret_ref = Some("mcp.browse_fixture.key".into());
    config.enabled = true;
    servers.create(&config).unwrap();
    settings
        .write_secret(
            config.bearer_secret_ref.as_deref().unwrap(),
            "browse-fixture-key-secret",
        )
        .unwrap();
    let cancel = CancellationToken::new();
    let mut client = McpClient::connect(
        servers,
        &settings,
        &config.id,
        super::super::ConnectionConsent::after_user_confirmation(&config, false).unwrap(),
        &super::super::McpRuntime::default(),
        &cancel,
    )
    .await
    .unwrap();
    assert!(
        client.ensure_browse_current().is_err(),
        "fixture cannot pass the actual stock gate"
    );
    client.fixture_read_transport();
    let result = client
        .call_browse_tool("list_games", json!({"q":"fast"}), &cancel)
        .await
        .unwrap();
    assert_eq!(
        result["structuredContent"]["items"][0]["title"],
        "[REDACTED]"
    );
    let escaped = client
        .call_browse_tool("list_games", json!({"q":"escaped"}), &cancel)
        .await
        .unwrap();
    let decoded = client
        .redact_browse_catalog(decode(escaped).unwrap())
        .unwrap();
    assert_eq!(decoded["items"][0]["title"], "[REDACTED]");
    let read = client.call_browse_tool("list_games", json!({"q":"slow"}), &cancel);
    let cancel_read = async {
        called.notified().await;
        cancel.cancel();
    };
    let (result, _) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(read, cancel_read)
    })
    .await
    .unwrap();
    assert!(matches!(result, Err(AppError::Cancelled)));
    tokio::time::timeout(Duration::from_secs(3), wire_cancelled.notified())
        .await
        .unwrap();
    assert!(client.ensure_browse_current().is_ok());
    settings
        .write_secret(
            config.bearer_secret_ref.as_deref().unwrap(),
            "fixture-new-key-not-read-by-browse",
        )
        .unwrap();
    assert!(matches!(
        client.ensure_browse_current(),
        Err(AppError::Denied(_))
    ));
    server.abort();
}
