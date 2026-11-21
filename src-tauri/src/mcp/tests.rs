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
use sha2::{Digest, Sha256};
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

fn playwright_v2_args() -> Vec<String> {
    [
        "--yes",
        "@playwright/mcp@0.0.85",
        "--headless",
        "--isolated",
        "--no-webmcp",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn assert_playwright_windows_args(args: &[String]) {
    let mut expected = playwright_v2_args();
    expected.push("--browser=msedge".into());
    assert_eq!(args, expected);
}

fn markitdown_args() -> Vec<String> {
    [
        "--with",
        "azure-ai-contentunderstanding==1.2.0b3",
        "markitdown-mcp==0.0.1a8",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn markitdown_legacy_mirror_args() -> Vec<String> {
    [
        "--default-index",
        "https://pypi.tuna.tsinghua.edu.cn/simple",
        "--with",
        "azure-ai-contentunderstanding==1.2.0b3",
        "markitdown-mcp==0.0.1a8",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[test]
fn sqlite_crud_presets_markitdown_inherits_uvx_default_and_secret_references() {
    let (_dir, servers, settings, _) = fixture();
    servers.seed_presets().unwrap();
    servers.seed_presets().unwrap();
    let presets = servers.list().unwrap();
    assert_eq!(presets.len(), 4);
    assert!(presets
        .iter()
        .all(|server| !server.enabled && server.builtin));
    let markitdown = servers.get("markitdown").unwrap();
    assert_eq!(markitdown.command.as_deref(), Some("uvx"));
    assert_eq!(markitdown.args, markitdown_args());
    assert!(markitdown.credential_url.is_none());
    assert!(servers.get("playwright").unwrap().credential_url.is_none());
    let http = servers.get("glossmod_http").unwrap();
    assert_eq!(
        http.url.as_deref(),
        Some("https://www.glossmod.com/api/v1/mcp")
    );
    assert_eq!(
        http.credential_url.as_deref(),
        Some("https://www.glossmod.com/creator/ai")
    );
    let stdio = servers.get("glossmod_3dm").unwrap();
    assert_eq!(stdio.name, "3DM MCP");
    assert_eq!(
        stdio.credential_url.as_deref(),
        Some("https://mod.3dmgame.com/Workshop/Api")
    );
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

const LEGACY_3DM_JSON: &str = concat!(
    r#"{"id":"glossmod_3dm","name":"3DM MCP (glossmod-mcp)","transport":"stdio","command":"uvx","args":["glossmod-mcp"],"url":null,"#,
    r#""env_secret_refs":{"GLOSSMOD_API_KEY":"mcp.glossmod_3dm.api_key"},"bearer_secret_ref":null,"enabled":true,"builtin":true}"#,
);
const LEGACY_HTTP_JSON: &str = concat!(
    r#"{"id":"glossmod_http","name":"Edited Mod provider","transport":"streamable_http","command":null,"args":[],"url":"https://example.com/edited-mcp","#,
    r#""env_secret_refs":{},"bearer_secret_ref":"mcp.glossmod_http.api_key","enabled":true,"builtin":true}"#,
);

#[test]
fn credential_url_is_serde_compatible_and_excluded_from_the_legacy_launch_digest() {
    for legacy_json in [LEGACY_3DM_JSON, LEGACY_HTTP_JSON] {
        let mut config: ServerConfig = serde_json::from_str(legacy_json).unwrap();
        let legacy_digest: [u8; 32] = Sha256::digest(legacy_json.as_bytes()).into();
        assert!(config.credential_url.is_none());
        assert_eq!(serde_json::to_vec(&config).unwrap(), legacy_json.as_bytes());
        assert_eq!(config.confirmation_digest().unwrap(), legacy_digest);

        let mut json = serde_json::to_value(&config).unwrap();
        json["credential_url"] = Value::Null;
        assert_eq!(
            serde_json::from_value::<ServerConfig>(json).unwrap(),
            config
        );

        config.credential_url = Some("https://example.test/account/keys".into());
        assert_eq!(config.confirmation_digest().unwrap(), legacy_digest);
        assert_eq!(
            serde_json::from_value::<ServerConfig>(serde_json::to_value(&config).unwrap()).unwrap(),
            config
        );
        config.enabled = false;
        assert_ne!(config.confirmation_digest().unwrap(), legacy_digest);
    }
}

#[test]
fn credential_url_requires_an_absolute_http_url_without_user_info() {
    for mut config in [
        ServerConfig::stdio("stdio", "stdio", "example", vec![]),
        ServerConfig::http("http", "http", "https://example.com/mcp"),
    ] {
        for url in [
            "https://example.com/keys",
            "http://example.com/keys",
            "HTTPS://[::1]:8443/keys/@me?lang=en#account",
            "https://example.com/user%20portal",
        ] {
            config.credential_url = Some(url.into());
            assert!(config.validate().is_ok(), "should accept {url:?}");
        }
        for url in [
            "",
            "/keys",
            "//example.com/keys",
            "javascript:alert(1)",
            "ftp://example.com/keys",
            "https://",
            "https:///example.com/keys",
            "https:example.com/keys",
            "https:/example.com/keys",
            "https://user@example.com/keys",
            "https://user:pass@example.com/keys",
            "https://:pass@example.com/keys",
            "https://@example.com/keys",
            "https://%75ser@example.com/keys",
            "https://example.com:invalid/keys",
            "https://example.com/with space",
            "https://example.com/with\nnewline",
            r"https://example.com\@another.com/keys",
        ] {
            config.credential_url = Some(url.into());
            assert!(
                matches!(config.validate(), Err(AppError::Invalid(message)) if message.contains("credential URL")),
                "should reject {url:?}"
            );
        }
    }
}

#[test]
fn credential_url_round_trips_through_create_list_update_and_clear() {
    let (_dir, servers, _, ctx) = fixture();
    let mut config = ServerConfig::stdio("custom", "Custom MCP", "example", vec![]);
    config.credential_url = Some("http://example.test/account?lang=en#keys".into());
    servers.create(&config).unwrap();
    assert_eq!(servers.get("custom").unwrap(), config);
    assert_eq!(servers.list().unwrap(), vec![config.clone()]);

    let invalid = ServerConfig {
        id: "invalid".into(),
        credential_url: Some("https://user:secret@example.test/".into()),
        ..config.clone()
    };
    assert!(matches!(
        servers.create(&invalid),
        Err(AppError::Invalid(_))
    ));
    assert!(matches!(servers.get("invalid"), Err(AppError::NotFound(_))));
    let invalid_update = ServerConfig {
        id: config.id.clone(),
        ..invalid
    };
    assert!(matches!(
        servers.update(&invalid_update),
        Err(AppError::Invalid(_))
    ));
    assert_eq!(servers.get("custom").unwrap(), config);

    config.credential_url = Some("https://example.test/new-portal".into());
    servers.update(&config).unwrap();
    drop(servers);
    let reopened = McpServers::new(Db::open(&ctx.engine.paths.db_file()).unwrap());
    assert_eq!(reopened.get("custom").unwrap(), config);
    config.credential_url = None;
    reopened.update(&config).unwrap();
    assert_eq!(reopened.get("custom").unwrap(), config);
}

#[test]
fn credential_url_v6_migration_backfills_builtins_once_and_preserves_remembered_grants() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy-v6.sqlite");
    let servers = McpServers::new(Db::open(&path).unwrap());
    servers.seed_presets().unwrap();
    let custom = ServerConfig::http("custom", "Custom", "https://example.com/mcp");
    servers.create(&custom).unwrap();
    let fixtures = [
        (LEGACY_HTTP_JSON, "https://www.glossmod.com/creator/ai"),
        (LEGACY_3DM_JSON, "https://mod.3dmgame.com/Workshop/Api"),
    ];
    for (json, _) in fixtures {
        let config: ServerConfig = serde_json::from_str(json).unwrap();
        servers.update(&config).unwrap();
        let v6_digest: [u8; 32] = Sha256::digest(json.as_bytes()).into();
        servers
            .db
            .conn()
            .execute(
                "UPDATE mcp_servers SET startup_digest=?2 WHERE id=?1",
                rusqlite::params![config.id, v6_digest],
            )
            .unwrap();
    }
    servers
        .db
        .conn()
        .execute_batch("ALTER TABLE mcp_servers DROP COLUMN credential_url; PRAGMA user_version=6;")
        .unwrap();
    drop(servers);

    let migrated = McpServers::new(Db::open(&path).unwrap());
    assert_eq!(
        migrated.startup_requests().unwrap(),
        ["glossmod_3dm", "glossmod_http"]
    );
    for (json, default_url) in fixtures {
        let mut expected: ServerConfig = serde_json::from_str(json).unwrap();
        expected.credential_url = Some(default_url.into());
        let stored = migrated.get(&expected.id).unwrap();
        let v6_digest = Sha256::digest(json.as_bytes()).to_vec();
        assert_eq!(stored, expected);
        assert_eq!(
            migrated.startup_digest(&stored.id).unwrap(),
            Some(v6_digest)
        );
        assert!(ConnectionConsent::from_remembered(&migrated, &stored).is_ok());
    }
    assert_eq!(migrated.get("custom").unwrap(), custom);
    let mut http = migrated.get("glossmod_http").unwrap();
    http.credential_url = None;
    migrated.update(&http).unwrap();
    let mut stdio = migrated.get("glossmod_3dm").unwrap();
    stdio.credential_url = Some("https://example.com/user-edited-portal".into());
    migrated.update(&stdio).unwrap();
    drop(migrated);

    let reopened = McpServers::new(Db::open(&path).unwrap());
    reopened.seed_presets().unwrap();
    assert_eq!(reopened.get("glossmod_http").unwrap(), http);
    assert_eq!(reopened.get("glossmod_3dm").unwrap(), stdio);
}

#[test]
fn credential_url_v6_migration_does_not_backfill_custom_rows_with_builtin_ids() {
    let (_dir, servers, _, ctx) = fixture();
    let custom = ServerConfig::http("glossmod_http", "Custom", "https://example.com/mcp");
    servers.create(&custom).unwrap();
    servers
        .db
        .conn()
        .execute_batch("ALTER TABLE mcp_servers DROP COLUMN credential_url; PRAGMA user_version=6;")
        .unwrap();
    let reopened = McpServers::new(Db::open(&ctx.engine.paths.db_file()).unwrap());
    reopened.seed_presets().unwrap();
    assert_eq!(reopened.get(&custom.id).unwrap(), custom);
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
    if cfg!(windows) {
        assert_playwright_windows_args(&playwright.args);
    } else {
        assert_eq!(playwright.args, playwright_v2_args());
    }
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
    assert_eq!(markitdown.args, markitdown_args());
    assert!(markitdown.env_secret_refs.is_empty() && markitdown.bearer_secret_ref.is_none());
    assert!(!markitdown.enabled && markitdown.builtin);
    assert!(ConnectionConsent::after_user_confirmation(&markitdown, false).is_err());
}

#[test]
fn markitdown_v2_through_v5_stock_upgrade_clears_consent_once_and_v5_user_edit_persists() {
    let dir = tempfile::tempdir().unwrap();
    for version in 2..=5 {
        let path = dir.path().join(format!("v{version}.sqlite"));
        let servers = McpServers::new(Db::open(&path).unwrap());
        servers.seed_presets().unwrap();
        let mut legacy = servers.get("markitdown").unwrap();
        legacy.args = markitdown_legacy_mirror_args();
        legacy.enabled = true;
        servers.update(&legacy).unwrap();
        servers.remember_start(&legacy).unwrap();
        servers
            .db
            .conn()
            .pragma_update(None, "user_version", version)
            .unwrap();
        drop(servers);

        let servers = McpServers::new(Db::open(&path).unwrap());
        let migrated = servers.get("markitdown").unwrap();
        assert_eq!(migrated.args, markitdown_args(), "schema v{version}");
        assert_eq!(migrated.command, legacy.command, "schema v{version}");
        assert!(migrated.enabled && migrated.builtin, "schema v{version}");
        assert!(
            servers.startup_requests().unwrap().is_empty(),
            "schema v{version}"
        );
        assert!(
            ConnectionConsent::from_remembered(&servers, &migrated).is_err(),
            "schema v{version}"
        );

        // Seeding again must not reapply the migration or restore consent.
        servers.seed_presets().unwrap();
        assert_eq!(servers.get("markitdown").unwrap(), migrated);

        // A user's explicit mirror edit after v5 migration is retained on later startups.
        let (expected, expected_startup) = if version == 5 {
            let mut restored = migrated.clone();
            restored.args = markitdown_legacy_mirror_args();
            servers.update(&restored).unwrap();
            servers.remember_start(&restored).unwrap();
            (restored, vec!["markitdown".to_owned()])
        } else {
            (migrated.clone(), Vec::new())
        };
        drop(servers);
        let reopened = McpServers::new(Db::open(&path).unwrap());
        reopened.seed_presets().unwrap();
        assert_eq!(reopened.get("markitdown").unwrap(), expected);
        assert_eq!(reopened.startup_requests().unwrap(), expected_startup);
        assert_eq!(
            ConnectionConsent::from_remembered(&reopened, &expected).is_ok(),
            version == 5
        );
    }
}

#[test]
fn markitdown_direct_v1_upgrade_keeps_package_only_config_and_revokes_bare_uvx_consent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v1.sqlite");
    let servers = McpServers::new(Db::open(&path).unwrap());
    servers.seed_presets().unwrap();
    let mut legacy = servers.get("markitdown").unwrap();
    legacy.args = markitdown_args();
    legacy.enabled = true;
    servers.update(&legacy).unwrap();
    servers.remember_start(&legacy).unwrap();
    servers
        .db
        .conn()
        .pragma_update(None, "user_version", 1)
        .unwrap();
    drop(servers);

    let servers = McpServers::new(Db::open(&path).unwrap());
    let migrated = servers.get("markitdown").unwrap();
    assert_eq!(migrated.args, markitdown_args());
    assert!(migrated.enabled && migrated.builtin);
    assert!(servers.startup_requests().unwrap().is_empty());
    assert!(ConnectionConsent::from_remembered(&servers, &migrated).is_err());
}

#[test]
fn markitdown_v5_custom_launches_preserve_values_and_existing_consent() {
    let dir = tempfile::tempdir().unwrap();
    for customization in ["custom_mirror", "extra_args", "custom_command"] {
        let path = dir.path().join(format!("{customization}.sqlite"));
        let servers = McpServers::new(Db::open(&path).unwrap());
        servers.seed_presets().unwrap();
        let mut customized = servers.get("markitdown").unwrap();
        customized.args = markitdown_legacy_mirror_args();
        match customization {
            "custom_mirror" => {
                customized.args[1] = "https://pypi.example.test/simple".into();
            }
            "extra_args" => customized.args.push("--no-cache".into()),
            "custom_command" => customized.command = Some("custom-uvx".into()),
            _ => unreachable!(),
        }
        customized.enabled = true;
        servers.update(&customized).unwrap();
        servers.remember_start(&customized).unwrap();
        servers
            .db
            .conn()
            .pragma_update(None, "user_version", 5)
            .unwrap();
        drop(servers);

        let servers = McpServers::new(Db::open(&path).unwrap());
        assert_eq!(
            servers.get("markitdown").unwrap(),
            customized,
            "{customization}"
        );
        assert_eq!(servers.startup_requests().unwrap(), ["markitdown"]);
        assert!(
            ConnectionConsent::from_remembered(&servers, &customized).is_ok(),
            "{customization}"
        );
    }
}

#[test]
fn playwright_v2_to_v3_migration_is_windows_only_invalidates_consent_and_runs_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("playwright-v2.sqlite");
    let servers = McpServers::new(Db::open(&path).unwrap());
    servers.seed_presets().unwrap();
    let mut legacy = servers.get("playwright").unwrap();
    legacy.args = playwright_v2_args();
    legacy.enabled = true;
    servers.update(&legacy).unwrap();
    servers.remember_start(&legacy).unwrap();
    servers
        .db
        .conn()
        .pragma_update(None, "user_version", 2)
        .unwrap();
    drop(servers);

    let servers = McpServers::new(Db::open(&path).unwrap());
    let reopened = servers.get("playwright").unwrap();
    if cfg!(windows) {
        assert_playwright_windows_args(&reopened.args);
        assert!(reopened.enabled && reopened.builtin);
        assert!(servers.startup_requests().unwrap().is_empty());
        assert!(ConnectionConsent::from_remembered(&servers, &reopened).is_err());

        // After the one-time migration, an explicit return to the old arguments is respected.
        let mut opted_out = reopened;
        opted_out.args = playwright_v2_args();
        servers.update(&opted_out).unwrap();
        servers.remember_start(&opted_out).unwrap();
        drop(servers);

        let servers = McpServers::new(Db::open(&path).unwrap());
        servers.seed_presets().unwrap();
        assert_eq!(servers.get("playwright").unwrap(), opted_out);
        assert_eq!(servers.startup_requests().unwrap(), ["playwright"]);
        assert!(ConnectionConsent::from_remembered(&servers, &opted_out).is_ok());
    } else {
        assert_eq!(reopened, legacy);
        assert!(servers.startup_requests().unwrap().is_empty()); // v4 changes the bare launcher.
        assert!(ConnectionConsent::from_remembered(&servers, &reopened).is_err());
    }
}

#[test]
fn playwright_upgrades_preserve_edits_but_only_custom_launchers_retain_consent() {
    let dir = tempfile::tempdir().unwrap();
    for customization in ["name", "command", "args"] {
        let path = dir.path().join(format!("{customization}.sqlite"));
        let servers = McpServers::new(Db::open(&path).unwrap());
        servers.seed_presets().unwrap();
        let mut customized = servers.get("playwright").unwrap();
        customized.args = playwright_v2_args();
        match customization {
            "name" => customized.name = "My Playwright".into(),
            "command" => customized.command = Some("custom-npx.cmd".into()),
            "args" => customized.args.push("--browser=firefox".into()),
            _ => unreachable!(),
        }
        customized.enabled = true;
        servers.update(&customized).unwrap();
        servers.remember_start(&customized).unwrap();
        servers
            .db
            .conn()
            .pragma_update(None, "user_version", 2)
            .unwrap();
        drop(servers);

        let servers = McpServers::new(Db::open(&path).unwrap());
        assert_eq!(servers.get("playwright").unwrap(), customized);
        assert_eq!(
            ConnectionConsent::from_remembered(&servers, &customized).is_ok(),
            customization == "command"
        );
    }
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
        &McpRuntime::default(),
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
    let client = McpClient::connect(
        servers,
        &settings,
        &config.id,
        consent,
        &McpRuntime::default(),
        &context.cancel,
    )
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
    let error = match McpClient::connect(
        servers,
        &settings,
        &config.id,
        consent,
        &McpRuntime::default(),
        &context.cancel,
    )
    .await
    {
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
        &McpRuntime::default(),
        &context.cancel
    )
    .await
    .is_err());
    let consent = ConnectionConsent::after_user_confirmation(&config, true).unwrap();
    config.enabled = true;
    servers.update(&config).unwrap();
    assert!(matches!(
        McpClient::connect(
            servers,
            &settings,
            &config.id,
            consent,
            &McpRuntime::default(),
            &context.cancel
        )
        .await,
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

/// Opt-in native stdio smoke for the exact preset. Uses a disposable DB, does
/// not read application credentials, and never invokes the conversion tool.
#[tokio::test]
#[ignore = "requires the prepared MCP runtime and MarkItDown dependencies (or network access)"]
async fn live_markitdown_preset_initializes_and_lists_conversion_tool() {
    let (dir, servers, settings, context) = fixture();
    servers.seed_presets().unwrap();
    servers.set_enabled("markitdown", true).unwrap();
    let config = servers.get("markitdown").unwrap();
    let consent = ConnectionConsent::after_user_confirmation(&config, true).unwrap();
    let client = McpClient::connect(
        servers,
        &settings,
        &config.id,
        consent,
        &McpRuntime::bundled(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("resources/mcp-runtime/runtime.zip"),
            dir.path().canonicalize().unwrap().join("runtime-cache"),
        ),
        &context.cancel,
    )
    .await
    .unwrap();
    let tools = client.list_tools(&context.cancel).await;
    client.disconnect_and_wait().await.unwrap();
    assert!(tools
        .unwrap()
        .iter()
        .any(|tool| tool.remote_name == "convert_to_markdown"));
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
        &McpRuntime::default(),
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

#[tokio::test]
async fn desktop_startup_restores_only_successful_connections_and_remembers_stop() {
    use crate::mcp_connections::McpConnections;
    let (_dir, servers, settings, ctx) = fixture();
    let (url, stop, _) = mock_http("echo").await;
    let (invalid_url, stop_invalid, _) = mock_http("duplicate").await;
    let registry = ToolRegistry::default();
    let connections = McpConnections::default();
    for (id, url) in [
        ("good", &url),
        ("requires_key", &url),
        ("never_started", &url),
        ("bad_tools", &invalid_url),
    ] {
        let mut config = ServerConfig::http(id, id, url);
        config.enabled = true;
        if id == "requires_key" {
            config.bearer_secret_ref = Some("mcp.test.key".into());
            servers
                .write_secret(&settings, "mcp.test.key", "secret")
                .unwrap();
        }
        servers.create(&config).unwrap();
        if id != "never_started" {
            assert_eq!(
                connections
                    .connect(settings.clone(), &registry, config, false)
                    .await
                    .is_ok(),
                id != "bad_tools"
            );
        }
    }
    drop((connections, registry, servers, settings));
    let db = Db::open(&ctx.engine.paths.db_file()).unwrap();
    let servers = McpServers::new(db.clone());
    // Simulate a new desktop session, this time without the external key available.
    let settings = Settings::with_secret_store(db, Arc::new(MockSecrets::default()));
    let connections = McpConnections::default();
    let registry = ToolRegistry::default();
    assert_eq!(
        servers.startup_requests().unwrap(),
        ["good", "requires_key"]
    );
    connections
        .restore_requested(&settings, &registry, || {})
        .await;
    let list = connections.list().await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].server_id, "good");
    assert_eq!(list[0].tools.len(), 1);
    assert!(registry.get(&list[0].tools[0]).is_ok());
    // A transient failure does not lose intent, and stopping it even while offline works.
    assert_eq!(
        servers.startup_requests().unwrap(),
        ["good", "requires_key"]
    );
    for id in ["good", "requires_key"] {
        connections
            .disconnect(&registry, &servers, id)
            .await
            .unwrap();
    }
    assert!(servers.startup_requests().unwrap().is_empty());
    assert!(registry.get(&list[0].tools[0]).is_err());
    connections
        .restore_requested(&settings, &registry, || {})
        .await;
    assert!(connections.list().await.is_empty());
    stop.cancel();
    stop_invalid.cancel();
}

#[tokio::test]
async fn startup_stdio_requires_unchanged_acknowledged_configuration() {
    use crate::mcp_connections::McpConnections;
    let (_dir, servers, settings, _) = fixture();
    let fixture_script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/mcp/mock_stdio.py");
    let python = if cfg!(windows) { "py" } else { "python3" };
    let mut config = ServerConfig::stdio(
        "local_tool",
        "mock",
        python,
        vec![fixture_script.to_string_lossy().into_owned()],
    );
    config.enabled = true;
    servers.create(&config).unwrap();
    assert!(matches!(
        McpConnections::default()
            .connect(
                settings.clone(),
                &ToolRegistry::default(),
                config.clone(),
                false
            )
            .await,
        Err(AppError::Denied(_))
    ));
    assert!(servers.startup_requests().unwrap().is_empty());
    assert!(ConnectionConsent::from_remembered(&servers, &config).is_err());
    // Only the bundled, offline fixture is launched. A fresh manager must register its tool again.
    let first = McpConnections::default();
    let registry = ToolRegistry::default();
    first
        .connect(settings.clone(), &registry, config.clone(), true)
        .await
        .unwrap();
    drop((first, registry));
    let restored = McpConnections::default();
    let registry = ToolRegistry::default();
    restored
        .restore_requested(&settings, &registry, || {})
        .await;
    assert_eq!(restored.list().await.len(), 1);
    assert_eq!(registry.definitions().len(), 1);
    drop((restored, registry));
    assert!(ConnectionConsent::from_remembered(&servers, &config).is_ok());
    // Even bypassing the normal desktop edit path cannot transfer consent to different arguments.
    servers
        .db
        .conn()
        .execute(
            "UPDATE mcp_servers SET args_json='[]' WHERE id=?1",
            [&config.id],
        )
        .unwrap();
    assert!(
        ConnectionConsent::from_remembered(&servers, &servers.get(&config.id).unwrap()).is_err()
    );
    servers.update(&config).unwrap();
    assert!(servers.startup_requests().unwrap().is_empty());
    servers.remember_start(&config).unwrap();
    servers.set_enabled(&config.id, false).unwrap();
    servers.set_enabled(&config.id, true).unwrap();
    assert!(servers.startup_requests().unwrap().is_empty());
    servers.remember_start(&config).unwrap();
    servers.remove(&config.id).unwrap();
    servers.create(&config).unwrap();
    assert!(servers.startup_requests().unwrap().is_empty());
}

#[tokio::test]
async fn stopping_during_startup_cancels_without_registering_or_remembering_again() {
    use crate::mcp_connections::McpConnections;
    let (_dir, servers, settings, _) = fixture();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = ServerConfig::http(
        "slow",
        "slow",
        &format!("http://{}/mcp", listener.local_addr().unwrap()),
    );
    config.enabled = true;
    servers.create(&config).unwrap();
    servers.remember_start(&config).unwrap();
    let connections = McpConnections::default();
    let registry = ToolRegistry::default();
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            connections.restore_requested(&settings, &registry, || {}),
            async {
                let (_socket, _) = listener.accept().await.unwrap();
                connections
                    .disconnect(&registry, &servers, "slow")
                    .await
                    .unwrap();
            }
        );
    })
    .await
    .unwrap();
    assert!(servers.startup_requests().unwrap().is_empty());
    assert!(connections.list().await.is_empty());
    assert!(registry.definitions().is_empty());
}

#[test]
fn migrating_enabled_legacy_server_does_not_grant_startup_consent() {
    let (_dir, servers, _, ctx) = fixture();
    let mut config = ServerConfig::stdio("legacy", "legacy", "unused", vec![]);
    config.enabled = true;
    servers.create(&config).unwrap();
    servers
        .db
        .conn()
        .execute("ALTER TABLE mcp_servers DROP COLUMN startup_digest", [])
        .unwrap();
    let reopened = McpServers::new(Db::open(&ctx.engine.paths.db_file()).unwrap());
    assert_eq!(reopened.get("legacy").unwrap(), config);
    assert!(reopened.startup_requests().unwrap().is_empty());
}
/// Exercise the real archive, native MCP command builder and npm-generated child shim offline.
#[tokio::test]
#[ignore = "requires yarn prepare:mcp-runtime for this target"]
async fn bundled_npx_connects_offline_without_system_node() {
    let (dir, servers, settings, ctx) = fixture();
    let base = dir.path().canonicalize().unwrap();
    let runtime = McpRuntime::bundled(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/mcp-runtime/runtime.zip"),
        base.join("runtime"),
    );
    let package = base.join("local package");
    std::fs::create_dir(&package).unwrap();
    std::fs::write(
        package.join("package.json"),
        r#"{"name":"amm-smoke","version":"1.0.0","bin":{"amm-smoke":"server.js"}}"#,
    )
    .unwrap();
    let script = package.join("server.js");
    std::fs::write(&script, r#"#!/usr/bin/env node
const rl = require('node:readline').createInterface({ input: process.stdin });
rl.on('line', line => {
  const msg = JSON.parse(line);
  if (msg.id === undefined) return;
  const result = msg.method === 'initialize'
    ? {protocolVersion:msg.params.protocolVersion, capabilities:{tools:{}}, serverInfo:{name:'fixture',version:'1'}}
    : {tools:[{name:'probe', description:JSON.stringify([process.execPath, process.env.npm_config_registry]), inputSchema:{type:'object'}}]};
  console.log(JSON.stringify({jsonrpc:'2.0', id:msg.id, result}));
});
"#).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut config = ServerConfig::stdio(
        "packaged_node",
        "Packaged Node",
        "npx",
        vec![
            "--yes".into(),
            "--offline".into(),
            format!(
                "--cache={}",
                dunce::simplified(&base).join("npm-cache").display()
            ),
            format!("--package=file:{}", dunce::simplified(&package).display()),
            "--".into(),
            "amm-smoke".into(),
        ],
    );
    config.enabled = true;
    servers.create(&config).unwrap();
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
    let tools = client.list_tools(&ctx.cancel).await.unwrap();
    client.disconnect_and_wait().await.unwrap();
    assert_eq!(tools.len(), 1);
    let [node, registry]: [String; 2] = serde_json::from_str(&tools[0].description).unwrap();
    assert!(std::path::Path::new(&node)
        .canonicalize()
        .unwrap()
        .starts_with(base.join("runtime")));
    assert_eq!(
        registry.trim_end_matches('/'),
        "https://registry.npmmirror.com"
    );
}
