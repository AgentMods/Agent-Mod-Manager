mod agent_match_ipc;
mod nexus_live;

use super::*;
use crate::games::get_game;
use aggregation::{Fetch, MatchedPair, Mixer};
use providers::{test_connection, ProviderGame, ProviderItem, ProviderPage};
use serde_json::{json, Value};
use std::future;
use tauri::test::MockRuntime;

pub(super) fn game_db(path: &std::path::Path) -> Db {
    let db = Db::open(path).unwrap();
    db.conn()
        .execute(
            "INSERT INTO games (id,name,slug,root_path,created_at,updated_at)
         VALUES ('game-a','Example Game','example-game','abc','now','now'),
                ('game-b','Other Game','other-game','def','now','now')",
            [],
        )
        .unwrap();
    db
}

pub(super) fn game(db: &Db, id: &str) -> GameScope {
    GameScope::new(get_game(db, id).unwrap())
}

pub(super) fn provider_game(id: &str, name: &str) -> ProviderGame {
    ProviderGame {
        id: id.into(),
        name: name.into(),
    }
}

fn matched(game: GameScope, source: &'static str) -> MatchedPair {
    MatchedPair {
        provider_game_id: format!("provider-{}", game.id),
        pair: Pair {
            game,
            source,
            connection: test_connection(),
        },
    }
}

fn item(id: &str) -> ProviderItem {
    ProviderItem {
        id: id.into(),
        title: format!("Project {id}"),
        author: None,
        summary: None,
        cover_url: None,
        page_url: None,
        kind: None,
        tags: vec![],
    }
}

#[test]
fn exact_ambiguous_and_bounded_game_matches_never_guess() {
    let temp = tempfile::tempdir().unwrap();
    let db = game_db(&temp.path().join("games.db"));
    let local = game(&db, "game-a");
    let options = || {
        vec![
            provider_game("example-game", "Different Display Alias"),
            provider_game("edition-two", "Example Game"),
        ]
    };
    assert!(matches!(
        matching::decide(&local, None, options(), None, true),
        Match::Choices(_)
    ));
    assert!(
        matches!(matching::decide(&local, None, options(), Some("edition-two"), false), Match::Exact(id) if id == "edition-two")
    );
    assert!(matches!(
        matching::decide(
            &local,
            None,
            vec![provider_game("unknown", "Example Game HD")],
            None,
            true
        ),
        Match::Choices(_)
    ));
    assert!(matches!(
        matching::decide(
            &local,
            None,
            vec![provider_game("123", "Example GAME")],
            None,
            false
        ),
        Match::Choices(_)
    ));
    assert!(
        matches!(matching::decide(&local, None, vec![provider_game("123", "Example GAME")], None, true), Match::Exact(id) if id == "123")
    );
    assert!(matches!(
        matching::decide(
            &local,
            None,
            vec![provider_game("../game", "Example Game")],
            None,
            true
        ),
        Match::Missing
    ));
}

#[test]
fn nexus_matches_a_localized_registration_only_to_a_unique_launcher_title() {
    use std::fs;
    let temp = tempfile::tempdir().unwrap();
    let db = game_db(&temp.path().join("games.db"));
    let apps = temp.path().join("steamapps");
    let installed = apps.join("common/Phantom Blade Zero");
    fs::create_dir_all(&installed).unwrap();
    let mut game = get_game(&db, "game-a").unwrap();
    game.name = "影之刃零".into();
    game.slug = "pbz".into();
    game.steam_app_id = Some("4115450".into());
    game.root_path = installed.to_str().unwrap().to_owned();
    let manifest = apps.join("appmanifest_4115450.acf");
    let content = r#""AppState" { "appid" "4115450" "name" "Phantom Blade Zero" "installdir" "Phantom Blade Zero" }"#;
    assert!(game.steam_catalog_title().is_none());
    fs::write(&manifest, content).unwrap();
    let title = game.steam_catalog_title().unwrap();
    assert_eq!(title, "Phantom Blade Zero");
    let local = GameScope::new(game.clone());
    let options = vec![provider_game("phantombladezero", &title)];
    assert!(
        matches!(matching::decide(&local, Some(&title), options.clone(), None, true),
        Match::Exact(id) if id == "phantombladezero")
    );
    assert!(matches!(
        matching::decide(&local, Some(&title), options.clone(), None, false),
        Match::Choices(_)
    ));
    let mut ambiguous = options;
    ambiguous.push(provider_game("otheredition", &title));
    assert!(matches!(
        matching::decide(&local, Some(&title), ambiguous, None, true),
        Match::Choices(_)
    ));

    fs::write(&manifest, content.replace("4115450", "123")).unwrap();
    assert!(game.steam_catalog_title().is_none());
    fs::write(
        &manifest,
        content.replace(
            r#""installdir" "Phantom Blade Zero""#,
            r#""installdir" "Other""#,
        ),
    )
    .unwrap();
    assert!(game.steam_catalog_title().is_none());

    game.metadata = json!({"lastIdentification": {"rootPath": game.root_path,
        "sources": [{"kind":"steam", "steamAppId":"4115450", "displayName":"Phantom Blade Zero"}]}});
    assert_eq!(
        game.steam_catalog_title().as_deref(),
        Some("Phantom Blade Zero")
    );
    game.metadata["lastIdentification"]["sources"][0]["steamAppId"] = json!("123");
    assert!(game.steam_catalog_title().is_none());
}

#[test]
fn remembered_match_is_scoped_and_rejected_after_retargeting() {
    let temp = tempfile::tempdir().unwrap();
    let db = game_db(&temp.path().join("games.db"));
    let local = game(&db, "game-a");
    assert!(matching::remembered(&db, &local, "modio")
        .unwrap()
        .is_none());
    assert!(matching::remember(&db, &local, "modio", "example-game").unwrap());
    assert_eq!(
        matching::remembered(&db, &local, "modio")
            .unwrap()
            .as_deref(),
        Some("example-game")
    );
    assert!(matching::remembered(&db, &local, "curseforge")
        .unwrap()
        .is_none());
    db.conn()
        .execute("UPDATE games SET root_path='other' WHERE id='game-a'", [])
        .unwrap();
    assert!(!local.current(&db));
    assert!(!matching::remember(&db, &local, "modio", "another-id").unwrap());
    assert!(matching::remembered(&db, &game(&db, "game-a"), "modio")
        .unwrap()
        .is_none());
    db.conn()
        .execute("DELETE FROM games WHERE id='game-a'", [])
        .unwrap();
    assert_eq!(
        db.conn()
            .query_row(
                "SELECT count(*) FROM desktop_browse_game_matches",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn page_round_robin_deduplicates_words_and_games_but_not_sources() {
    let temp = tempfile::tempdir().unwrap();
    let db = game_db(&temp.path().join("games.db"));
    let game_a = game(&db, "game-a");
    let game_b = game(&db, "game-b");
    let mut mixer = Mixer::default();
    mixer.push(matched(game_a.clone(), "modio"), "原词".into());
    mixer.push(matched(game_a, "curseforge"), "原词".into());
    mixer.push(matched(game_b, "modio"), "English".into());
    let mut requests = Vec::new();
    let mut fetch = |request: Fetch| {
        requests.push((
            request.matched.pair.source,
            request.query.clone(),
            request.cursor.clone(),
        ));
        let prefix = request.matched.pair.source;
        future::ready(Ok(
            match (prefix, request.query.as_str(), request.cursor.as_deref()) {
                ("modio", "English", None) => ProviderPage {
                    items: vec![item("shared")],
                    next: None,
                },
                ("modio" | "curseforge", "原词", None) => ProviderPage {
                    items: std::iter::once(item("shared"))
                        .chain((0..18).map(|i| item(&format!("mod-{i}"))))
                        .collect(),
                    next: (prefix == "modio").then(|| "2".into()),
                },
                ("modio", "原词", Some("2")) => ProviderPage {
                    items: vec![item("shared"), item("last")],
                    next: None,
                },
                _ => panic!("unexpected read"),
            },
        ))
    };
    let first = mixer.page(&mut fetch).await;
    assert_eq!(first.ids.len(), 24);
    assert_eq!(first.updates[&first.ids[0]].source, "modio");
    assert_eq!(first.updates[&first.ids[1]].source, "curseforge");
    assert_eq!(first.updates[&first.ids[0]].targets.len(), 2);
    assert_eq!(first.updates[&first.ids[0]].project_id, "shared");
    assert_eq!(first.updates[&first.ids[1]].project_id, "shared");
    let snapshot = first.ids.clone();
    assert!(mixer.has_more());
    let second = mixer.page(&mut fetch).await;
    assert_eq!(second.ids.len(), 15);
    assert!(second.ids.iter().all(|id| !snapshot.contains(id)));
    assert_eq!(first.ids, snapshot);
    assert!(!mixer.has_more());
    assert_eq!(requests.len(), 4);
}

#[tokio::test]
async fn exhausted_cursors_and_rejected_sources_are_isolated() {
    let temp = tempfile::tempdir().unwrap();
    let db = game_db(&temp.path().join("games.db"));
    let game = game(&db, "game-a");
    let mut mixer = Mixer::default();
    for source in ["modio", "curseforge", "gamebanana"] {
        mixer.push(matched(game.clone(), source), "".into());
    }
    let mut repeated = 0;
    let page = mixer
        .page(|request| {
            future::ready(match request.matched.pair.source {
                "modio" => Err(IssueCode::PermissionDenied),
                "curseforge" => {
                    repeated += 1;
                    Ok(ProviderPage {
                        items: vec![item("copy")],
                        next: Some("repeat".into()),
                    })
                }
                "gamebanana" => Ok(ProviderPage {
                    items: vec![item("other")],
                    next: None,
                }),
                _ => unreachable!(),
            })
        })
        .await;
    assert_eq!(repeated, 2);
    assert_eq!(page.ids.len(), 2);
    assert!(mixer.issues.contains(
        &matched(game.clone(), "modio")
            .pair
            .issue(IssueCode::PermissionDenied)
    ));
    assert!(mixer.issues.contains(
        &matched(game, "curseforge")
            .pair
            .issue(IssueCode::QueryFailed)
    ));
    assert!(!mixer.has_more());
}

#[tokio::test]
async fn a_revoked_lane_cannot_consume_slots_with_buffered_candidates() {
    let temp = tempfile::tempdir().unwrap();
    let db = game_db(&temp.path().join("games.db"));
    let game = game(&db, "game-a");
    let revoked = matched(game.clone(), "modio");
    let mut mixer = Mixer::default();
    mixer.push(revoked.clone(), String::new());
    mixer.push(matched(game, "curseforge"), String::new());
    let first = mixer
        .page(|_| {
            future::ready(Ok(ProviderPage {
                items: (0..32).map(|n| item(&n.to_string())).collect(),
                next: None,
            }))
        })
        .await;
    assert_eq!(first.ids.len(), PAGE_SIZE);
    mixer.retire_unavailable(&BTreeSet::from(["curseforge"]));
    let second = mixer
        .page(|_| future::ready(Err(IssueCode::QueryFailed)))
        .await;
    assert_eq!(second.ids.len(), 20);
    assert!(second
        .updates
        .values()
        .all(|item| item.source == "curseforge"));
    assert!(mixer
        .issues
        .contains(&revoked.pair.issue(IssueCode::PermissionDenied)));
    assert!(!mixer.has_more());
}

#[tokio::test]
async fn novel_but_nonproductive_cursor_chain_is_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let db = game_db(&temp.path().join("games.db"));
    let pair = matched(game(&db, "game-a"), "modio");
    let mut mixer = Mixer::default();
    mixer.push(pair.clone(), "q".into());
    let mut calls = 0;
    let page = mixer
        .page(|_| {
            calls += 1;
            future::ready(Ok(ProviderPage {
                items: Vec::new(),
                next: Some(calls.to_string()),
            }))
        })
        .await;
    assert!(page.ids.is_empty());
    assert_eq!(calls, 12);
    assert!(!mixer.has_more());
    assert!(mixer
        .issues
        .contains(&pair.pair.issue(IssueCode::LimitReached)));
}

#[test]
fn cannot_open_another_work_or_non_project_link_or_expose_cover_tokens() {
    use links::{cover, project};
    let good = "https://www.nexusmods.com/example/mods/42";
    assert_eq!(
        project("nexusmods", "example", "example/42", good).as_deref(),
        Some(good)
    );
    assert_eq!(
        project(
            "glossmod_3dm",
            "game",
            "42",
            "https://mod.3dmgame.com/mod/42"
        )
        .as_deref(),
        Some("https://mod.3dmgame.com/mod/42")
    );
    for (id, page) in [
        ("42", "https://mod.3dmgame.com/mod/43"),
        ("042", "https://mod.3dmgame.com/mod/042"),
        ("42", "https://mod.3dmgame.com/mod/42?token=secret"),
        (
            "42",
            "https://dmod.3dmgame.com/static/upload/resource/42.zip",
        ),
    ] {
        assert!(project("glossmod_3dm", "game", id, page).is_none());
    }
    for malicious in [
        "http://www.nexusmods.com/example/mods/42",
        "https://evil.example/example/mods/42",
        "https://www.nexusmods.com.evil.example/example/mods/42",
        "https://example.test@www.nexusmods.com/example/mods/42",
        "https://www.nexusmods.com:444/example/mods/42",
        "https://www.nexusmods.com/example/mods/43",
        "https://www.nexusmods.com/example/mods/42?token=secret",
        "https://www.nexusmods.com/example/mods/42#files",
        "https://www.nexusmods.com/another/mods/42",
        "https://www.nexusmods.com/example/mods/42/download",
        "https://www.nexusmods.com/example/mods/42%2Fdownload",
        "file:///tmp/mod.zip",
        "ror2mm://v1/install/thunderstore.io/team/project/1.0.0",
    ] {
        assert!(
            project("nexusmods", "example", "example/42", malicious).is_none(),
            "{malicious}"
        );
    }
    assert!(project("gamebanana", "g", "42", good).is_none());
    let three_dm_cover = "https://mod.3dmgame.com/static/upload/mod/202611/MOD42@webp";
    assert_eq!(
        cover("glossmod_3dm", three_dm_cover).as_deref(),
        Some(three_dm_cover)
    );
    assert!(cover(
        "glossmod_3dm",
        "https://other.test/static/upload/mod/MOD42@webp"
    )
    .is_none());
    assert!(cover(
        "thunderstore",
        "https://gcdn.thunderstore.io.fake.test/icon.png"
    )
    .is_none());
    assert_eq!(
        cover(
            "thunderstore",
            "https://gcdn.thunderstore.io/icon.png?token=secret#fragment"
        )
        .as_deref(),
        Some("https://gcdn.thunderstore.io/icon.png")
    );
    assert!(links::cover(
        "thunderstore",
        "https://ccdn.thunderstore.io/live/repository/icons/Author-Work-1.0.0.png",
    )
    .is_some());
    assert!(links::cover(
        "thunderstore",
        "https://ccdn.thunderstore.io.invalid.test/live/repository/icons/Author-Work-1.0.0.png",
    )
    .is_none());
}

#[test]
fn expired_mismatched_and_cancelled_windows_do_not_resolve_searches() {
    let temp = tempfile::tempdir().unwrap();
    let db = game_db(&temp.path().join("games.db"));
    let other_db = game_db(&temp.path().join("other.db"));
    let state = BrowseState::default();
    let make = |expires| {
        let (_, translations) = watch::channel(None);
        Arc::new(Search {
            catalogs: Arc::default(),
            id: uuid::Uuid::new_v4().to_string(),
            db: db.clone(),
            expires,
            query: String::new(),
            pairs: vec![matched(game(&db, "game-a"), "modio").pair],
            translations,
            enrichment_slots: Semaphore::new(1),
            cancel: CancellationToken::new(),
            inner: Mutex::default(),
        })
    };
    let search = make(Instant::now() + SEARCH_TTL);
    let save = |search: Arc<Search>| {
        lock(&state.windows).insert(
            "main".into(),
            WindowState {
                discovery: Some(Discovery {
                    db: db.clone(),
                    expires: Instant::now() + AUTH_TTL,
                    connections: BTreeMap::new(),
                }),
                grant: None,
                search: Some(search),
            },
        )
    };
    save(search.clone());
    assert!(search_ref(&state, &db, "main", &search.id).is_ok());
    assert!(search_ref(&state, &db, "other", &search.id).is_err());
    assert!(search_ref(&state, &other_db, "main", &search.id).is_err());
    assert!(looked_up(&search, &uuid::Uuid::new_v4().to_string()).is_err());
    db.conn()
        .execute("UPDATE games SET name='Remaster' WHERE id='game-a'", [])
        .unwrap();
    assert!(search_ref(&state, &db, "main", &search.id).is_err());
    assert!(search.cancel.is_cancelled());
    let expired = make(Instant::now() - Duration::from_secs(1));
    save(expired.clone());
    assert!(search_ref(&state, &db, "main", &expired.id).is_err());
    assert!(expired.cancel.is_cancelled());
    let live = make(Instant::now() + SEARCH_TTL);
    save(live.clone());
    state.cancel_window("main");
    assert!(live.cancel.is_cancelled());
    assert!(search_ref(&state, &db, "main", &live.id).is_err());
}

#[test]
fn source_filter_checks_only_the_granted_generation_and_never_adds_a_source() {
    let granted = BTreeMap::from([
        ("modio", test_connection()),
        ("nexusmods", test_connection()),
    ]);
    let mut visited = Vec::new();
    let selection = scoped_connections(&granted, Some(&["modio".into()]), |id, token| {
        assert!(token == granted.get(id).unwrap());
        visited.push(id.to_owned());
        true
    })
    .unwrap();
    assert_eq!(selection.keys().copied().collect::<Vec<_>>(), ["modio"]);
    assert_eq!(visited, ["modio"]);
    assert!(scoped_connections(
        &granted,
        Some(&["modio".into(), "gamebanana".into()]),
        |_, _| { panic!("unauthorized masks must be rejected without connection reads") }
    )
    .is_err());
    let only_changed = |id: &str, _: &ProviderConnection| id != "nexusmods";
    assert!(scoped_connections(&granted, None, only_changed).is_err());
    assert!(scoped_connections(&granted, Some(&["modio".into()]), only_changed).is_ok());
}

#[test]
fn ipc_uses_camel_case_and_withholds_the_provider_work_and_url() {
    let temp = tempfile::tempdir().unwrap();
    let db = game_db(&temp.path().join("games.db"));
    let row = Candidate {
        source: "glossmod_http",
        project_id: "1923".into(),
        title: "Original".into(),
        author: None,
        summary: None,
        cover_url: None,
        page_url: Some("https://www.glossmod.com/mods/1923".into()),
        kind: None,
        tags: vec![],
        targets: BTreeMap::from([(game(&db, "game-a").id, "private-mapping".into())]),
    };
    let page = Page {
        status: PageStatus::Ready,
        page: 0,
        items: vec![BrowseItem::display(
            "opaque-item".into(),
            &row,
            enrich::fallback_item(entry(&row)),
        )],
        issues: vec![matched(game(&db, "game-b"), "modio")
            .pair
            .issue(IssueCode::PermissionDenied)],
        game_choices: vec![],
        has_next: true,
        is_enriching: false,
    };
    let encoded = serde_json::to_value(page).unwrap();
    assert_eq!(encoded["status"], "ready");
    assert_eq!(encoded["issues"][0]["code"], "permissionDenied");
    assert_eq!(encoded["items"][0]["sourceId"], "glossmod_http");
    assert_eq!(encoded["items"][0]["originalTitle"], "Original");
    assert_eq!(encoded["items"][0]["canOpenProject"], true);
    assert_eq!(encoded["items"][0]["gameIds"][0], "game-a");
    assert!(!encoded.to_string().contains("private-mapping"));
    assert!(!encoded.to_string().contains("1923"));
}

fn browse_ipc_window(
    services: AppServices,
    agent: AgentRuntime,
) -> (tauri::App<MockRuntime>, WebviewWindow<MockRuntime>) {
    use tauri::test;
    // Match desktop startup: the managed AgentRuntime owns the credential-backed settings.
    let app = test::mock_builder()
        .manage(services)
        .manage(agent)
        .manage(BrowseState::default())
        .invoke_handler(tauri::generate_handler![
            browse_list_sources,
            browse_authorize,
            browse_start_search,
            browse_get_page,
            browse_select_game_match,
            browse_open_project,
            browse_cancel_search,
            install::browse_install_task,
            install::start_browse_install,
            install::list_browse_install_tasks,
        ])
        .build(test::mock_context(test::noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    (app, window)
}

fn browse_invoke(
    window: &WebviewWindow<MockRuntime>,
    cmd: &str,
    body: Value,
) -> Result<Value, Value> {
    use tauri::{ipc, test, webview::InvokeRequest};
    test::get_ipc_response(
        window,
        InvokeRequest {
            cmd: cmd.into(),
            callback: ipc::CallbackFn(1),
            error: ipc::CallbackFn(2),
            url: (if cfg!(windows) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            })
            .parse()
            .unwrap(),
            body: ipc::InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: test::INVOKE_KEY.into(),
        },
    )
    .map(|response| response.deserialize().unwrap())
}

#[test]
fn ipc_source_discovery_and_denied_requests_never_start_a_catalog() {
    use crate::settings::SecretStore;

    struct NoSecrets;
    impl SecretStore for NoSecrets {
        fn read(&self, _: &str) -> AppResult<Option<String>> {
            panic!("unauthorized requests cannot read user secrets")
        }

        fn write(&self, _: &str, _: &str) -> AppResult<()> {
            panic!("browse cannot write user secrets")
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let services = AppServices::initialize(dir.path().join("data")).unwrap();
    services
        .engine
        .db
        .conn()
        .execute(
            "INSERT INTO games (id,name,slug,root_path,created_at,updated_at)
             VALUES ('game-a','Example Game','example-game','abc','now','now')",
            [],
        )
        .unwrap();
    let settings = Settings::with_secret_store(services.engine.db.clone(), Arc::new(NoSecrets));
    let agent = crate::agent::runtime::AgentRuntime::new(
        services.engine.clone(),
        settings,
        Arc::new(|_| {}),
    )
    .unwrap();
    let (_app, window) = browse_ipc_window(services, agent);
    let invoke = |cmd, body| browse_invoke(&window, cmd, body);

    let denied = json!({"sourceIds": ["glossmod_http"]});
    let no_grant = "browse.authorization_changed";
    assert_eq!(
        invoke("browse_authorize", denied.clone()).unwrap_err()["key"],
        no_grant
    );
    let listed = invoke("browse_list_sources", json!({})).unwrap();
    let rows = listed.as_array().unwrap();
    assert_eq!(rows.len(), MAX_SOURCES);
    assert!(rows.iter().all(|row| row["status"] == "notConnected"));
    assert!(rows.iter().all(|row| row.as_object().unwrap().len() == 3));
    assert_eq!(
        invoke("browse_authorize", denied).unwrap_err()["key"],
        no_grant
    );
    let forged_id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        invoke("list_browse_install_tasks", json!({})).unwrap(),
        json!([])
    );
    for command in ["browse_install_task", "start_browse_install"] {
        assert_eq!(
            invoke(
                command,
                json!({"searchId": forged_id, "itemId": forged_id, "gameId": "game-a"})
            )
            .unwrap_err()["key"],
            "browse.search_expired"
        );
    }
    assert_eq!(
        invoke(
            "browse_start_search",
            json!({"authorizationId": forged_id, "gameIds": ["game-a"], "query": "test"})
        )
        .unwrap_err()["key"],
        no_grant
    );
    assert_eq!(
        invoke(
            "browse_select_game_match",
            json!({"searchId": forged_id, "gameId": "game-a", "sourceId": "glossmod_http", "providerGameId": "1"})
        )
        .unwrap_err()["key"],
        "browse.search_expired"
    );
    assert_eq!(
        invoke("browse_get_page", json!({"searchId": forged_id, "page": 0})).unwrap_err()["key"],
        "browse.search_expired"
    );
    assert_eq!(
        invoke(
            "browse_open_project",
            json!({"searchId": forged_id, "itemId": forged_id, "url": "https://invalid.test"})
        )
        .unwrap_err()["key"],
        "browse.search_expired"
    );
    assert_eq!(
        invoke("browse_cancel_search", json!({"searchId": forged_id})).unwrap(),
        Value::Null
    );
}

/// Public, read-only smoke for the desktop grant, search and page command path.
#[test]
#[ignore = "requires the public 3DM MCP endpoint and a populated Phantom Blade Zero catalogue"]
fn live_three_dm_browse_through_desktop_ipc() {
    const SOURCE: &str = "glossmod_3dm";
    let dir = tempfile::tempdir().unwrap();
    let services = AppServices::initialize(dir.path().join("data")).unwrap();
    let db = services.engine.db.clone();
    db.conn()
        .execute(
            "INSERT INTO games (id,name,slug,root_path,created_at,updated_at)
         VALUES ('game-a','影之刃零','phantom-blade-zero','abc','now','now')",
            [],
        )
        .unwrap();
    let settings = Settings::with_secret_store(
        db.clone(),
        Arc::new(crate::mcp::tests::MockSecrets::default()),
    );
    let agent =
        AgentRuntime::new(services.engine.clone(), settings.clone(), Arc::new(|_| {})).unwrap();
    let servers = McpServers::new(db);
    let mut config = servers.get(SOURCE).unwrap();
    config.enabled = true;
    config.bearer_secret_ref = Some("mcp.personal.3dm_key".into());
    servers.update(&config).unwrap();
    let connections = services.mcp_connections.clone();
    let registry = agent.registry.clone();
    tauri::async_runtime::block_on(connections.connect(settings, &registry, config, false))
        .unwrap();
    let (_app, window) = browse_ipc_window(services, agent);
    let invoke = |cmd, body| browse_invoke(&window, cmd, body).unwrap();
    let sources = invoke("browse_list_sources", json!({}));
    let source = sources
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == SOURCE)
        .unwrap();
    assert_eq!(source["status"], "available");
    let grant = invoke("browse_authorize", json!({"sourceIds": [SOURCE]}));
    let ready_page = |search: &Value, page| {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let frame = invoke("browse_get_page", json!({"searchId": search, "page": page}));
            if frame["status"] == "ready" {
                assert_eq!(frame["issues"], json!([]));
                let rows = frame["items"].as_array().unwrap();
                assert!(
                    !rows.is_empty(),
                    "3DM returned an empty page for the smoke fixture"
                );
                assert!(rows
                    .iter()
                    .all(|r| r["sourceId"] == SOURCE && r["canOpenProject"] == true));
                println!("3DM page {page}: {} projects", rows.len());
                break frame;
            }
            assert!(Instant::now() < deadline, "3DM page timed out");
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    for query in ["", "服装"] {
        let search = invoke(
            "browse_start_search",
            json!({"authorizationId":grant["authorizationId"], "gameIds":["game-a"], "query":query}),
        );
        let first = ready_page(&search["searchId"], 0);
        if query.is_empty() {
            assert_eq!(first["hasNext"], true);
            let second = ready_page(&search["searchId"], 1);
            assert_ne!(first["items"][0]["id"], second["items"][0]["id"]);
        }
        invoke(
            "browse_cancel_search",
            json!({"searchId":search["searchId"]}),
        );
    }
    tauri::async_runtime::block_on(connections.disconnect(&registry, &servers, SOURCE)).unwrap();
}

#[tokio::test]
async fn an_empty_original_first_page_automatically_waits_and_uses_the_translation() {
    let temp = tempfile::tempdir().unwrap();
    let db = game_db(&temp.path().join("games.db"));
    let pair = matched(game(&db, "game-a"), "nexusmods");
    let mut progress = Progress {
        matched: vec![pair.clone()],
        ..Progress::default()
    };
    progress.mixer.push(pair, "原词".into());
    let (translations, variants) = watch::channel(None);
    let (started, original_requested) = tokio::sync::oneshot::channel();
    let mut started = Some(started);
    let mut requests = Vec::new();
    let page = progress.mix_page(0, variants, |request| {
        requests.push(request.query.clone());
        let items = match request.query.as_str() {
            "原词" => {
                started.take().unwrap().send(()).unwrap();
                vec![]
            }
            "English" => vec![item("translation-only")],
            _ => panic!("unexpected query"),
        };
        future::ready(Ok(ProviderPage { items, next: None }))
    });
    let deliver = async {
        original_requested.await.unwrap();
        translations
            .send(Some(QueryVariants {
                chinese: None,
                english: Some("English".into()),
            }))
            .unwrap_or_else(|_| panic!("page 0 must wait for pending translations"));
    };
    let (page, ()) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(page, deliver)
    })
    .await
    .unwrap();
    assert_eq!(requests, ["原词", "English"]);
    assert_eq!(page.ids.len(), 1);
    assert_eq!(page.updates[&page.ids[0]].project_id, "translation-only");
    assert!(!progress.mixer.has_more());
}

#[tokio::test]
async fn a_nonempty_original_first_page_does_not_wait_or_query_the_translation() {
    let temp = tempfile::tempdir().unwrap();
    let db = game_db(&temp.path().join("games.db"));
    let pair = matched(game(&db, "game-a"), "nexusmods");
    let mut progress = Progress {
        matched: vec![pair.clone()],
        ..Progress::default()
    };
    progress.mixer.push(pair, "原词".into());
    let (translations, variants) = watch::channel(None);
    let original = tokio::time::timeout(
        Duration::from_secs(1),
        progress.mix_page(0, variants.clone(), |request| {
            assert_eq!(request.query, "原词");
            future::ready(Ok(ProviderPage {
                items: vec![item("original-only")],
                next: None,
            }))
        }),
    )
    .await
    .unwrap();
    assert_eq!(original.ids.len(), 1);
    assert_eq!(
        original.updates[&original.ids[0]].project_id,
        "original-only"
    );
    assert!(!progress.translations_added);

    translations
        .send(Some(QueryVariants {
            chinese: None,
            english: Some("English".into()),
        }))
        .unwrap_or_else(|_| panic!("continuation must remain available"));
    let alternate = progress
        .mix_page(1, variants, |request| {
            assert_eq!(request.query, "English");
            assert!(request.cursor.is_none());
            future::ready(Ok(ProviderPage {
                items: vec![item("original-only"), item("translation-only")],
                next: None,
            }))
        })
        .await;
    assert_eq!(alternate.ids.len(), 1);
    assert_eq!(
        alternate.updates[&alternate.ids[0]].project_id,
        "translation-only"
    );
    assert!(!progress.mixer.has_more());
}
