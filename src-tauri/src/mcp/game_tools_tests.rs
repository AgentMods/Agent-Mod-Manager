use super::*;
use crate::mcp::client::ConnectionConsent;
use std::path::Path;

fn open(path: &Path) -> McpServers {
    McpServers::new(crate::db::Db::open(path).unwrap())
}

fn plugin(servers: &McpServers, slug: &str) {
    servers.db.conn().execute(
        "INSERT INTO plugins(slug,name,game_name,version,source,install_path,adapter_json,created_at,updated_at)
         VALUES (?1,?1,?1,'1.0.0','external',?1,'{}','now','now')",
        [slug],
    ).unwrap();
}

fn bind(servers: &McpServers, game: &str, slug: &str) {
    servers
        .db
        .conn()
        .execute(
            "INSERT INTO games(id,name,slug,root_path,adapter_slug,created_at,updated_at)
         VALUES (?1,?1,?1,?1,?2,'now','now')",
            params![game, slug],
        )
        .unwrap();
}

fn install(servers: &McpServers, slug: &str, configs: &[ServerConfig]) {
    let mut conn = servers.db.conn();
    let tx = conn.transaction().unwrap();
    super::super::sync_plugin_servers(&tx, slug, configs).unwrap();
    tx.commit().unwrap();
}

fn remote(id: &str) -> ServerConfig {
    ServerConfig::http(id, id, GLOSSMOD_3DM_ENDPOINT)
}

fn local(id: &str) -> ServerConfig {
    ServerConfig::stdio(id, id, "third-party-tool", vec!["--json".into()])
}

fn assert_remembered(servers: &McpServers, id: &str) {
    let config = servers.get(id).unwrap();
    assert!(config.enabled);
    assert!(ConnectionConsent::from_remembered(servers, &config).is_ok());
}

#[test]
fn policy_launch_follows_all_registered_bindings_without_changing_manual_grants() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("manager.db");
    let servers = open(&path);
    let automatic = remote("plugin_adapter_auto");
    let manual = local("plugin_adapter_manual");
    let other = remote("plugin_other_auto");
    plugin(&servers, "adapter");
    plugin(&servers, "other");
    install(&servers, "adapter", &[automatic.clone(), manual.clone()]);
    install(&servers, "other", std::slice::from_ref(&other));
    servers.set_enabled(&manual.id, true).unwrap();
    servers
        .remember_start(&servers.get(&manual.id).unwrap())
        .unwrap();
    servers.sync_game_tools().unwrap();
    assert!(servers.game_tool_requests().unwrap().is_empty());
    assert_eq!(
        servers.startup_requests().unwrap(),
        std::slice::from_ref(&manual.id)
    );

    bind(&servers, "edition-one", "adapter");
    bind(&servers, "edition-two", "adapter");
    servers.sync_game_tools().unwrap();
    assert_eq!(
        servers.game_tool_requests().unwrap(),
        [
            servers.get(&automatic.id).unwrap(),
            servers.get(&manual.id).unwrap()
        ]
    );
    assert!(servers.pending_game_tools().unwrap().is_empty());
    assert_remembered(&servers, &automatic.id);
    assert!(servers
        .list()
        .unwrap()
        .iter()
        .any(|item| item.id == automatic.id && item.enabled));
    let stored: (bool, Option<Vec<u8>>) = servers
        .db
        .conn()
        .query_row(
            "SELECT enabled,startup_digest FROM mcp_servers WHERE id=?1",
            [&automatic.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(stored, (false, None));
    drop(servers);

    let servers = open(&path);
    assert_remembered(&servers, &automatic.id);
    servers
        .db
        .conn()
        .execute("DELETE FROM games WHERE id='edition-one'", [])
        .unwrap();
    servers.sync_game_tools().unwrap();
    assert_remembered(&servers, &automatic.id);
    servers
        .db
        .conn()
        .execute(
            "UPDATE games SET adapter_slug=NULL WHERE id='edition-two'",
            [],
        )
        .unwrap();
    assert!(!servers.get(&automatic.id).unwrap().enabled);
    assert!(servers.startup_digest(&automatic.id).unwrap().is_none());
    assert_remembered(&servers, &manual.id);
    servers.sync_game_tools().unwrap();
    assert!(servers.game_tool_requests().unwrap().is_empty());
    assert_eq!(servers.startup_requests().unwrap(), [manual.id]);
    assert!(!servers.get(&other.id).unwrap().enabled);
}

#[test]
fn consent_and_manual_stop_are_exact_and_persist_across_restarts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("manager.db");
    let servers = open(&path);
    let safe = remote("plugin_adapter_safe");
    let mut stdio = local("plugin_adapter_cmd");
    let mut bearer = remote("plugin_adapter_bearer");
    bearer.bearer_secret_ref = Some("mcp.catalog.bearer".into());
    let loopback = ServerConfig::http("plugin_adapter_local", "Local", "http://127.0.0.1:5000/mcp");
    plugin(&servers, "adapter");
    bind(&servers, "game", "adapter");
    install(
        &servers,
        "adapter",
        &[
            safe.clone(),
            stdio.clone(),
            bearer.clone(),
            loopback.clone(),
        ],
    );
    servers.sync_game_tools().unwrap();
    let pending = servers.pending_game_tools().unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|config| config.id.as_str())
            .collect::<Vec<_>>(),
        [bearer.id.as_str(), stdio.id.as_str(), loopback.id.as_str()]
    );
    let [deny, allow, stale] = pending.as_slice() else {
        panic!("expected three pending requests")
    };
    assert!(servers.set_game_tool_consent(deny, false).is_ok());
    assert!(servers.set_game_tool_consent(deny, true).is_err());
    servers.set_game_tool_consent(allow, true).unwrap();
    servers.set_game_tool_consent(allow, true).unwrap();
    assert_remembered(&servers, &stdio.id);
    servers.forget_start(&safe.id).unwrap();
    assert!(servers.set_game_tool_consent(allow, false).is_err());
    assert_eq!(
        servers.game_tool_requests().unwrap(),
        [servers.get(&stdio.id).unwrap()]
    );
    drop(servers);

    let servers = open(&path);
    servers.sync_game_tools().unwrap();
    assert_eq!(servers.startup_requests().unwrap(), [stdio.id.clone()]);
    assert_eq!(
        servers.pending_game_tools().unwrap(),
        std::slice::from_ref(stale)
    );
    stdio.credential_url = Some("https://mcp.example.test/credentials".into());
    let mut replacement = loopback.clone();
    replacement.url = Some("http://127.0.0.1:6000/mcp".into());
    install(
        &servers,
        "adapter",
        &[safe, stdio.clone(), bearer, replacement],
    );
    assert!(servers.set_game_tool_consent(stale, true).is_err());
    assert!(servers.set_game_tool_consent(allow, true).is_err());
    assert!(servers.startup_requests().unwrap().is_empty());
    assert_eq!(servers.pending_game_tools().unwrap().len(), 2);
    let before_unbind = servers.get(&stdio.id).unwrap();
    servers.db.conn().execute("DELETE FROM games", []).unwrap();
    assert!(servers.set_game_tool_consent(&before_unbind, true).is_err());
}

#[test]
fn third_party_https_requires_consent_and_only_runs_while_bound() {
    let temp = tempfile::tempdir().unwrap();
    let servers = open(&temp.path().join("manager.db"));
    let external = ServerConfig::http(
        "plugin_adapter_external",
        "External",
        "https://mcp.example.test/mcp",
    );
    plugin(&servers, "adapter");
    bind(&servers, "game", "adapter");
    install(&servers, "adapter", std::slice::from_ref(&external));
    servers.sync_game_tools().unwrap();
    assert!(servers.game_tool_requests().unwrap().is_empty());
    let mut pending = servers.pending_game_tools().unwrap();
    assert_eq!(pending.len(), 1);
    let pending = pending.pop().unwrap();
    assert_eq!(pending.id, external.id);
    servers.set_game_tool_consent(&pending, true).unwrap();
    assert_remembered(&servers, &external.id);

    servers
        .db
        .conn()
        .execute("UPDATE games SET adapter_slug=NULL WHERE id='game'", [])
        .unwrap();
    servers.sync_game_tools().unwrap();
    assert!(servers.startup_requests().unwrap().is_empty());
    assert!(servers.pending_game_tools().unwrap().is_empty());
    assert!(!servers.get(&external.id).unwrap().enabled);
    servers
        .db
        .conn()
        .execute(
            "UPDATE games SET adapter_slug='adapter' WHERE id='game'",
            [],
        )
        .unwrap();
    assert!(servers.pending_game_tools().unwrap().is_empty());
    assert_remembered(&servers, &external.id);
}

#[test]
fn exact_gta_declarations_and_https_safety_gate() {
    let manifests = [
        include_str!("../../../packages/gta-adapters/gta5-legacy/adapter.json"),
        include_str!("../../../packages/gta-adapters/gta5-enhanced/adapter.json"),
    ];
    for manifest in manifests {
        let adapter: crate::plugins::GameAdapter = serde_json::from_str(manifest).unwrap();
        let original = adapter.mcp_configs().unwrap().pop().unwrap();
        assert_eq!(safe_by_policy(&original), cfg!(windows));
        let changes: [fn(&mut ServerConfig); 6] = [
            |config| config.id.push_str("-other"),
            |config| config.plugin_slug = Some("foreign".into()),
            |config| config.command = Some("uvx.exe".into()),
            |config| config.args.push("--extra".into()),
            |config| config.args[0] = "gta-rpf-mcp".into(),
            |config| {
                config
                    .env_secret_refs
                    .insert("SOME_TOKEN".into(), "mcp.local.token".into());
            },
        ];
        for change in changes {
            let mut changed = original.clone();
            change(&mut changed);
            assert!(!safe_by_policy(&changed));
        }
    }
    for url in [GLOSSMOD_HTTP_ENDPOINT, GLOSSMOD_3DM_ENDPOINT] {
        assert!(safe_by_policy(&ServerConfig::http("test", "Test", url)));
    }
    for url in [
        "https://mcp.example.test/mcp",
        "https://mod.3dmgame.com/mcp/",
        "https://mod.3dmgame.com:8443/mcp",
        "https://mod.3dmgame.com/mcp?key=hidden",
        "http://localhost/mcp",
        "https://localhost./mcp",
        "https://service.local/mcp",
        "https://127.0.0.1/mcp",
        "https://192.168.0.5/mcp",
        "https://[::1]/mcp",
        "https://@mcp.example.test/mcp",
        "https://mcp.example.test/mcp?key=hidden",
    ] {
        assert!(
            !safe_by_policy(&ServerConfig::http("test", "Test", url)),
            "{url}"
        );
    }
}

#[test]
fn v26_manual_grant_migrates_and_survives_game_managed_owner_disable() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("manager.db");
    let servers = open(&path);
    let manual = local("plugin_adapter_manual");
    let auto = remote("plugin_adapter_auto");
    plugin(&servers, "adapter");
    bind(&servers, "game", "adapter");
    install(&servers, "adapter", &[manual.clone(), auto.clone()]);
    servers.set_enabled(&manual.id, true).unwrap();
    servers
        .remember_start(&servers.get(&manual.id).unwrap())
        .unwrap();
    servers
        .db
        .conn()
        .execute_batch("DROP TABLE game_tool_state; PRAGMA user_version=26;")
        .unwrap();
    drop(servers);

    let servers = open(&path);
    assert_eq!(
        servers.startup_requests().unwrap(),
        std::slice::from_ref(&manual.id)
    );
    servers.sync_game_tools().unwrap();
    assert_remembered(&servers, &auto.id);
    {
        let mut conn = servers.db.conn();
        let tx = conn.transaction().unwrap();
        tx.execute("UPDATE plugins SET enabled=0 WHERE slug='adapter'", [])
            .unwrap();
        assert!(super::super::revoke_plugin_servers(&tx, "adapter").unwrap());
        tx.commit().unwrap();
    }
    assert!(servers.startup_requests().unwrap().is_empty());
    assert!(!servers.get(&auto.id).unwrap().enabled);
    assert!(!servers.get(&manual.id).unwrap().enabled);
    assert!(servers.pending_game_tools().unwrap().is_empty());
    servers
        .db
        .conn()
        .execute("UPDATE plugins SET enabled=1 WHERE slug='adapter'", [])
        .unwrap();
    assert_remembered(&servers, &manual.id);
    assert!(!servers.get(&auto.id).unwrap().enabled);
    servers.sync_game_tools().unwrap();
    assert_remembered(&servers, &auto.id);
    servers
        .db
        .conn()
        .execute("UPDATE plugins SET validated=0 WHERE slug='adapter'", [])
        .unwrap();
    assert!(servers.startup_requests().unwrap().is_empty());
    assert!(servers.game_tool_requests().unwrap().is_empty());
    servers
        .db
        .conn()
        .execute("DELETE FROM plugins WHERE slug='adapter'", [])
        .unwrap();
    assert!(matches!(servers.get(&auto.id), Err(AppError::NotFound(_))));
    let remaining: usize = servers
        .db
        .conn()
        .query_row("SELECT count(*) FROM game_tool_state", [], |row| row.get(0))
        .unwrap();
    assert_eq!(remaining, 0);
}

#[test]
fn interrupted_sync_rolls_back_all_intents_and_retry_is_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let servers = open(&temp.path().join("manager.db"));
    let one = remote("plugin_adapter_first");
    let two = remote("plugin_adapter_second");
    plugin(&servers, "adapter");
    bind(&servers, "game", "adapter");
    install(&servers, "adapter", &[one.clone(), two.clone()]);
    servers
        .db
        .conn()
        .execute_batch(
            "CREATE TRIGGER test_sync_failure BEFORE INSERT ON game_tool_state
         WHEN NEW.server_id='plugin_adapter_second'
         BEGIN SELECT RAISE(ABORT, 'test sync failure'); END;",
        )
        .unwrap();
    assert!(servers.sync_game_tools().is_err());
    assert!(servers.startup_requests().unwrap().is_empty());
    servers
        .db
        .conn()
        .execute_batch("DROP TRIGGER test_sync_failure")
        .unwrap();
    servers.sync_game_tools().unwrap();
    assert_eq!(servers.startup_requests().unwrap(), [one.id, two.id]);
    let before = servers.db.conn().total_changes();
    servers.sync_game_tools().unwrap();
    assert_eq!(servers.db.conn().total_changes(), before);
}
