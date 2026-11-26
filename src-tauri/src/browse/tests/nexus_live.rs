use super::{browse_invoke, browse_ipc_window};
use crate::agent::runtime::AgentRuntime;
use crate::app::AppServices;
use crate::browse::{looked_up, search_ref, valid_source_for, BrowseState};
use crate::mcp::{tests::MockSecrets, McpRuntime, McpServers};
use crate::settings::Settings;
use crate::storage::AppPaths;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{test::MockRuntime, Manager, WebviewWindow};

const SOURCE: &str = "nexusmods";
const GAME_ID: &str = "pbz-library-game";
const DOMAIN: &str = "phantombladezero";

fn bundled_archive() -> PathBuf {
    let executable = std::env::current_exe().unwrap();
    let profile = executable.parent().unwrap().parent().unwrap();
    [
        profile.join("mcp-runtime/runtime.zip"),
        profile.join("resources/mcp-runtime/runtime.zip"),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/mcp-runtime/runtime.zip"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .expect("prepare the bundled MCP runtime for the current platform before running this test")
}

fn ready_page(window: &WebviewWindow<MockRuntime>, search: &str, page: usize) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let frame = browse_invoke(
            window,
            "browse_get_page",
            json!({"searchId": search, "page": page}),
        )
        .unwrap();
        assert_eq!(frame["page"], page);
        assert_eq!(
            frame["issues"],
            json!([]),
            "Nexus page {page} reported an issue"
        );
        assert_eq!(
            frame["gameChoices"],
            json!([]),
            "expected an automatic Steam title match"
        );
        if frame["status"] == "ready" {
            return frame;
        }
        assert_eq!(frame["status"], "loading");
        assert!(Instant::now() < deadline, "Nexus page {page} timed out");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn assert_projects(
    window: &WebviewWindow<MockRuntime>,
    search_id: &str,
    page: &Value,
    query: &str,
) -> BTreeSet<String> {
    let rows = page["items"].as_array().unwrap();
    assert!(
        !rows.is_empty(),
        "Nexus returned no projects for {query:?} on page {}",
        page["page"]
    );
    // Resolve the IPC handles through the same checks as browse_open_project, up to the OS opener.
    let services = window.state::<AppServices>();
    let state = window.state::<BrowseState>();
    let search = search_ref(&state, &services.engine.db, window.label(), search_id).unwrap();
    assert!(valid_source_for(&services, &search, SOURCE));
    let keyword = query.to_ascii_lowercase();
    let mut keyword_in_title = false;
    let mut project_ids = BTreeSet::new();
    for row in rows {
        assert_eq!(row["sourceId"], SOURCE);
        assert_eq!(row["gameIds"], json!([GAME_ID]));
        assert_eq!(row["canOpenProject"], true);
        let title = row["originalTitle"].as_str().unwrap();
        assert!(!title.trim().is_empty());
        keyword_in_title |= title.to_ascii_lowercase().contains(&keyword);
        let candidate = looked_up(&search, row["id"].as_str().unwrap()).unwrap();
        assert_eq!(
            candidate.targets.get(GAME_ID).map(String::as_str),
            Some(DOMAIN)
        );
        let (game, id) = candidate.project_id.split_once('/').unwrap();
        assert_eq!(game, DOMAIN);
        assert!(id.parse::<u64>().unwrap() > 0);
        assert_eq!(
            candidate.page_url.unwrap(),
            format!("https://www.nexusmods.com/{DOMAIN}/mods/{id}")
        );
        assert!(
            project_ids.insert(candidate.project_id),
            "duplicate Nexus project on a page"
        );
    }
    if !query.is_empty() {
        assert!(
            keyword_in_title,
            "no original title on this page matched {query:?}"
        );
    }
    println!(
        "Nexus query {query:?}, page {}: {} projects",
        page["page"],
        rows.len()
    );
    project_ids
}

fn run_live_browse(root: &Path) {
    let root = root.canonicalize().unwrap();
    let apps = root.join("steamapps");
    let game = apps.join("common/Phantom Blade Zero");
    std::fs::create_dir_all(&game).unwrap();
    std::fs::write(
        apps.join("appmanifest_4115450.acf"),
        r#""AppState"
{
    "appid" "4115450"
    "name" "Phantom Blade Zero"
    "installdir" "Phantom Blade Zero"
    "StateFlags" "4"
}"#,
    )
    .unwrap();

    let paths = AppPaths::new(root.join("data")).unwrap();
    let runtime = McpRuntime::bundled(bundled_archive(), root.join("runtime-cache"));
    let services = AppServices::initialize_paths(paths, runtime).unwrap();
    let db = services.engine.db.clone();
    db.conn()
        .execute(
            "INSERT INTO games (id,name,slug,steam_app_id,root_path,created_at,updated_at)
         VALUES (?1,'影之刃零','pbz','4115450',?2,'now','now')",
            [GAME_ID, game.to_str().unwrap()],
        )
        .unwrap();
    let settings = Settings::with_secret_store(db.clone(), Arc::new(MockSecrets::default()));
    let agent =
        AgentRuntime::new(services.engine.clone(), settings.clone(), Arc::new(|_| {})).unwrap();
    let connections = services.mcp_connections.clone();
    let registry = agent.registry.clone();
    let servers = McpServers::new(db);
    servers.set_enabled(SOURCE, true).unwrap();
    tauri::async_runtime::block_on(connections.connect(
        settings,
        &registry,
        servers.get(SOURCE).unwrap(),
        true,
    ))
    .unwrap();
    let (_app, window) = browse_ipc_window(services, agent);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check_searches(&window)));
    window.state::<BrowseState>().cancel_window(window.label());
    let disconnected =
        tauri::async_runtime::block_on(connections.disconnect(&registry, &servers, SOURCE));
    let destroyed = window.destroy();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    disconnected.unwrap();
    destroyed.unwrap();
}

fn check_searches(window: &WebviewWindow<MockRuntime>) {
    let invoke = |cmd, body| browse_invoke(window, cmd, body).unwrap();
    let sources = invoke("browse_list_sources", json!({}));
    let nexus = sources
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == SOURCE)
        .unwrap();
    assert_eq!(nexus["status"], "available");
    let grant = invoke("browse_authorize", json!({"sourceIds": [SOURCE]}));
    for query in ["", "Parry"] {
        let started = invoke(
            "browse_start_search",
            json!({"authorizationId": grant["authorizationId"], "gameIds": [GAME_ID], "query": query}),
        );
        let search = started["searchId"].as_str().unwrap();
        let first = ready_page(window, search, 0);
        let first_ids = assert_projects(window, search, &first, query);
        if query.is_empty() {
            assert_eq!(first["hasNext"], true);
            let second = ready_page(window, search, 1);
            let second_ids = assert_projects(window, search, &second, query);
            assert!(
                first_ids.is_disjoint(&second_ids),
                "Nexus repeated a project across pages"
            );
        }
        invoke("browse_cancel_search", json!({"searchId": search}));
    }
}

#[test]
#[ignore = "requires the bundled MCP runtime, network access and a populated Phantom Blade Zero Nexus catalogue"]
fn live_nexus_browse_through_desktop_ipc() {
    #[cfg(windows)]
    {
        const WORKER: &str = "AMM_NEXUS_LIVE_IPC_WORKER";
        if std::env::var_os(WORKER).as_deref() == Some(std::ffi::OsStr::new("1")) {
            let fixture = std::env::temp_dir().join("nexus-ipc");
            std::fs::create_dir(&fixture).unwrap();
            run_live_browse(&fixture);
            return;
        }

        // The mock desktop and its async runtime can retain mapped SQLite files until process exit.
        // Own the fixture in the parent so its first deletion happens after that exit.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let root = dunce::simplified(&root);
        let (_, module) = module_path!().split_once("::").unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "--ignored", "--nocapture"])
            .arg(format!("{module}::live_nexus_browse_through_desktop_ipc"))
            .env(WORKER, "1")
            .env("TEMP", root)
            .env("TMP", root)
            .env("TMPDIR", root)
            .status()
            .expect("start the isolated Nexus IPC test worker");
        let ran = root
            .join("nexus-ipc/steamapps/appmanifest_4115450.acf")
            .is_file();
        remove_worker_fixture(dir);
        assert!(status.success(), "Nexus IPC worker exited with {status}");
        assert!(ran, "the worker did not run the Nexus IPC test");
    }
    #[cfg(not(windows))]
    {
        let dir = tempfile::tempdir().unwrap();
        run_live_browse(dir.path());
    }
}

#[cfg(windows)]
fn remove_worker_fixture(dir: tempfile::TempDir) {
    // Windows can release an image or mapped file shortly after its process signals exit.
    let deadline = Instant::now() + Duration::from_secs(10);
    while let Err(err) = std::fs::remove_dir_all(dir.path()) {
        assert!(
            Instant::now() < deadline,
            "failed to remove Nexus IPC fixture {} after worker exit: {err}",
            dir.path().display()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
