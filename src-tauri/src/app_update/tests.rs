use super::*;
use axum::{http::header, routing::get, Router};
use serde_json::json;
use tauri::test::{mock_builder, mock_context, noop_assets};
use tauri::utils::config::BundleType;

const MANIFEST: &str =
    "https://github.com/AgentMods/Agent-Mod-Manager/releases/latest/download/latest.json";
const FIXTURE_ASSET: &str = "Agent-Mod-Manager_9.9.9_windows-x86_64-setup.exe";
const DOWNLOAD_BASE: &str = "https://github.com/AgentMods/Agent-Mod-Manager/releases/download";
const ROUTES: [&str; 6] = [
    "direct",
    "cdn.gh-proxy.org",
    "gh-proxy.org",
    "ghfast.top",
    "gh-proxy.com",
    "ghproxy.net",
];

fn test_state(phase: Phase) -> AppUpdates {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
        .unwrap();
    AppUpdates::new(phase, Db(Arc::new(Mutex::new(conn))))
}

fn release_source() -> ReleaseSource {
    ReleaseSource::from_config(Some(&json!({
        "pubkey": "key", "endpoints": [MANIFEST], "requireSignedVersion": true
    })))
    .unwrap()
}

#[test]
fn ipc_serializes_statuses_and_omits_unavailable_fields() {
    for (status, expected) in [
        (AppUpdateStatus::Idle, json!({"status": "idle"})),
        (AppUpdateStatus::Checking, json!({"status": "checking"})),
        (
            AppUpdateStatus::Downloading {
                version: "1.0.1".into(),
            },
            json!({"status": "downloading", "version": "1.0.1"}),
        ),
        (
            AppUpdateStatus::Ready {
                version: "1.0.1".into(),
            },
            json!({"status": "ready", "version": "1.0.1"}),
        ),
        (AppUpdateStatus::UpToDate, json!({"status": "upToDate"})),
        (
            AppUpdateStatus::Error {
                message: CHECK_ERROR,
            },
            json!({"status": "error", "message": CHECK_ERROR}),
        ),
        (
            AppUpdateStatus::Unsupported {
                message: "disabled",
            },
            json!({"status": "unsupported", "message": "disabled"}),
        ),
    ] {
        assert_eq!(serde_json::to_value(status).unwrap(), expected);
    }
}

#[test]
fn ipc_returns_unsupported_without_an_updater_plugin() {
    let app = mock_builder()
        .manage(test_state(Phase::Unsupported("disabled")))
        .invoke_handler(tauri::generate_handler![
            get_app_update_status,
            check_app_update
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    for command in ["get_app_update_status", "check_app_update"] {
        tauri::test::assert_ipc_response(
            &webview,
            tauri::webview::InvokeRequest {
                cmd: command.into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: if cfg!(windows) {
                    "http://tauri.localhost"
                } else {
                    "tauri://localhost"
                }
                .parse()
                .unwrap(),
                body: Default::default(),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.into(),
            },
            Ok(json!({"status": "unsupported", "message": "disabled"})),
        );
    }
}

#[test]
fn route_ipc_returns_exact_persisted_value_and_rejects_unknown_routes() {
    let app = mock_builder()
        .manage(test_state(Phase::Unsupported("disabled")))
        .invoke_handler(tauri::generate_handler![
            get_app_update_route,
            set_app_update_route
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let request = |cmd: &str, body| tauri::webview::InvokeRequest {
        cmd: cmd.into(),
        callback: tauri::ipc::CallbackFn(0),
        error: tauri::ipc::CallbackFn(1),
        url: if cfg!(windows) {
            "http://tauri.localhost"
        } else {
            "tauri://localhost"
        }
        .parse()
        .unwrap(),
        body: tauri::ipc::InvokeBody::Json(body),
        headers: Default::default(),
        invoke_key: tauri::test::INVOKE_KEY.into(),
    };
    tauri::test::assert_ipc_response(
        &webview,
        request("get_app_update_route", json!({})),
        Ok(json!("cdn.gh-proxy.org")),
    );
    for route in ROUTES {
        tauri::test::assert_ipc_response(
            &webview,
            request("set_app_update_route", json!({"route": route})),
            Ok(json!(route)),
        );
    }
    for invalid in [
        "",
        "https://ghproxy.net",
        "GHPROXY.NET",
        "ghproxy.net ",
        "ghproxy.net.example",
        "ghproxy.net:443",
        "ghproxy.net/path",
    ] {
        let response = tauri::test::get_ipc_response(
            &webview,
            request("set_app_update_route", json!({"route": invalid})),
        )
        .unwrap_err();
        assert_eq!(response["key"], "app_update.route_unsupported");
    }
    tauri::test::assert_ipc_response(
        &webview,
        request("get_app_update_route", json!({})),
        Ok(json!("ghproxy.net")),
    );
}

#[test]
fn startup_and_retries_snapshot_the_persisted_route_without_interrupting_work() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("settings.sqlite3");
    {
        let db = Db::open(&db_path).unwrap();
        assert_eq!(UpdateRoute::load(&db).unwrap().as_str(), "cdn.gh-proxy.org");
        UpdateRoute::save(&db, "gh-proxy.org").unwrap();
    }
    let state = AppUpdates::new(Phase::Idle, Db::open(&db_path).unwrap());
    let (route, status) = state.claim_check();
    assert_eq!(status, AppUpdateStatus::Checking);
    let route = route.unwrap();
    UpdateRoute::save(&state.db, "ghfast.top").unwrap();
    assert_eq!(state.claim_check(), (None, AppUpdateStatus::Checking));
    *state.phase() = Phase::Downloading {
        version: "9.9.9".into(),
    };
    UpdateRoute::save(&state.db, "ghproxy.net").unwrap();
    assert_eq!(state.claim_check().0, None);
    assert_eq!(route.as_str(), "gh-proxy.org");
    let source = release_source();
    let asset = format!("{DOWNLOAD_BASE}/v9.9.9/{FIXTURE_ASSET}")
        .parse()
        .unwrap();
    assert_eq!(
        source.artifact(route, &asset, "9.9.9").unwrap().host_str(),
        Some("gh-proxy.org")
    );
    *state.phase() = Phase::Error(DOWNLOAD_ERROR);
    assert_eq!(state.claim_check().0.unwrap().as_str(), "ghproxy.net");
}

#[test]
fn an_invalid_saved_route_defaults_to_cdn_mirror() {
    let state = test_state(Phase::Idle);
    for invalid in ["broken json", "42", "\"unlisted.example\""] {
        state
            .db
            .conn()
            .execute(
                "INSERT OR REPLACE INTO settings(key,value) VALUES('app_update_route',?1)",
                [invalid],
            )
            .unwrap();
        assert_eq!(
            UpdateRoute::load(&state.db).unwrap().as_str(),
            "cdn.gh-proxy.org"
        );
    }
}

#[test]
fn signing_key_signed_version_and_single_canonical_github_endpoint_are_required() {
    assert!(ReleaseSource::from_config(None).is_none());
    for config in [
        json!({"pubkey": "  ", "endpoints": [MANIFEST], "requireSignedVersion": true}),
        json!({"pubkey": "key", "endpoints": [MANIFEST]}),
        json!({"pubkey": "key", "endpoints": [], "requireSignedVersion": true}),
        json!({"pubkey": "key", "endpoints": [MANIFEST, MANIFEST], "requireSignedVersion": true}),
    ] {
        assert!(ReleaseSource::from_config(Some(&config)).is_none());
    }
    for url in [
        "https://mirror.example/Owner/Repo/releases/latest/download/latest.json",
        "https://github.com/Owner/Repo/releases/download/v9.9.9/latest.json",
        "https://github.com/Owner/Repo/extra/releases/latest/download/latest.json",
        "https://github.com/Owner/Repo/releases/latest/download/latest.json?extra=1",
    ] {
        let config = json!({"pubkey": "key", "endpoints": [url], "requireSignedVersion": true});
        assert!(
            ReleaseSource::from_config(Some(&config)).is_none(),
            "accepted {url}"
        );
    }
    let fork = "https://github.com/Owner/Fork/releases/latest/download/latest.json";
    let config = json!({"pubkey": "key", "endpoints": [fork], "requireSignedVersion": true});
    let source = ReleaseSource::from_config(Some(&config)).unwrap();
    let official_asset = format!("{DOWNLOAD_BASE}/v9.9.9/{FIXTURE_ASSET}")
        .parse()
        .unwrap();
    assert!(source
        .artifact(UpdateRoute::default(), &official_asset, "9.9.9")
        .is_none());
    let fork_asset = "https://github.com/Owner/Fork/releases/download/v9.9.9/fixture.exe"
        .parse()
        .unwrap();
    assert!(source
        .artifact(UpdateRoute::default(), &fork_asset, "9.9.9")
        .is_some());
}

#[test]
fn constructs_exact_urls_for_each_route_and_rejects_unbound_artifacts() {
    let source = release_source();
    let db = test_state(Phase::Idle).db;
    let asset = format!("{DOWNLOAD_BASE}/v9.9.9/{FIXTURE_ASSET}");
    for value in ROUTES {
        let route = UpdateRoute::save(&db, value).unwrap();
        let expected = |origin: &str| {
            if value == "direct" {
                origin.to_owned()
            } else {
                format!("https://{value}/{origin}")
            }
        };
        assert_eq!(route.apply(&source.manifest).as_str(), expected(MANIFEST));
        assert_eq!(
            source
                .artifact(route, &asset.parse().unwrap(), "9.9.9")
                .unwrap()
                .as_str(),
            expected(&asset)
        );
        for attack in [
            asset.replacen("https:", "http:", 1),
            asset.replace("github.com", "github.com.example"),
            asset.replace("github.com", "user@github.com"),
            asset.replace("github.com", "github.com:444"),
            asset.replace("/AgentMods/", "/SomeoneElse/"),
            asset.replace("/Agent-Mod-Manager/", "/Agent-Mod-Manager-Other/"),
            asset.replace("/v9.9.9/", "/v9.9.10/"),
            asset.replace("/v9.9.9/", "/9.9.9/"),
            asset.replace(FIXTURE_ASSET, "folder/payload.exe"),
            asset.replace(FIXTURE_ASSET, "folder%2Fpayload.exe"),
            format!("{asset}?extra=1"),
            format!("{asset}#extra"),
            format!("https://ghproxy.net/{asset}"),
        ] {
            assert!(
                source
                    .artifact(route, &attack.parse().unwrap(), "9.9.9")
                    .is_none(),
                "accepted {attack}"
            );
        }
    }
}

#[test]
fn only_follows_same_route_release_redirects_and_direct_github_cdn() {
    let source = release_source();
    let direct = UpdateRoute::DIRECT;
    let mirror = UpdateRoute::default();
    let tagged: Url = format!("{DOWNLOAD_BASE}/v9.9.9/latest.json")
        .parse()
        .unwrap();
    let asset: Url = format!("{DOWNLOAD_BASE}/v9.9.9/{FIXTURE_ASSET}")
        .parse()
        .unwrap();
    let cdn: Url = "https://release-assets.githubusercontent.com/github-production-release-asset/123/signed-asset?sig=test".parse().unwrap();
    assert!(source.allows_redirect(direct, std::slice::from_ref(&source.manifest), &tagged));
    assert!(source.allows_redirect(direct, &[source.manifest.clone(), tagged.clone()], &cdn));
    assert!(source.allows_redirect(direct, std::slice::from_ref(&asset), &cdn));
    assert!(source.allows_redirect(
        mirror,
        &[mirror.apply(&source.manifest)],
        &mirror.apply(&tagged)
    ));
    assert!(!source.allows_redirect(mirror, &[mirror.apply(&source.manifest)], &tagged));
    assert!(!source.allows_redirect(mirror, &[mirror.apply(&asset)], &cdn));
    assert!(!source.allows_redirect(mirror, &[mirror.apply(&asset)], &mirror.apply(&tagged)));
    for invalid in [
        "https://127.0.0.1/latest.json",
        "https://release-assets.githubusercontent.com.evil/github-production-release-asset/123/latest.json",
        "https://release-assets.githubusercontent.com/not-a-release-asset/123/latest.json",
        "https://github.com/Another/Project/releases/download/v9.9.9/latest.json",
        "https://gh-proxy.com/https://github.com/AgentMods/Agent-Mod-Manager/releases/download/v9.9.9/latest.json",
    ] {
        let invalid = invalid.parse().unwrap();
        assert!(!source.allows_redirect(direct, std::slice::from_ref(&source.manifest), &invalid));
        assert!(!source.allows_redirect(mirror, &[mirror.apply(&source.manifest)], &invalid));
    }
}

#[test]
fn selects_exact_installer_targets_and_rejects_unbundled_apps() {
    for (bundle, platform, suffix) in [
        (BundleType::Nsis, "windows-x86_64", "nsis"),
        (BundleType::Msi, "windows-x86_64", "msi"),
        (BundleType::AppImage, "linux-x86_64", "appimage"),
        (BundleType::Deb, "linux-x86_64", "deb"),
        (BundleType::App, "darwin-aarch64", "app"),
        (BundleType::App, "darwin-x86_64", "app"),
    ] {
        assert_eq!(
            manifest_target(platform, Some(bundle), true, true),
            Some(format!("{platform}-{suffix}"))
        );
    }
    assert_eq!(manifest_target("windows-x86_64", None, false, false), None);
    assert_eq!(
        manifest_target("linux-x86_64", Some(BundleType::AppImage), false, false),
        None
    );
    assert_eq!(
        manifest_target("darwin-aarch64", Some(BundleType::App), false, false),
        None
    );
    assert_eq!(
        manifest_target("linux-x86_64", Some(BundleType::Rpm), false, false),
        None
    );
}

#[test]
fn deduplicates_checks_and_does_not_install_an_incomplete_download() {
    let state = test_state(Phase::Idle);
    let starts = std::thread::scope(|scope| {
        let startup = scope.spawn(|| state.claim_check().0.is_some());
        let manual = scope.spawn(|| state.claim_check().0.is_some());
        [startup.join().unwrap(), manual.join().unwrap()]
    });
    assert_ne!(starts[0], starts[1]);

    let mut phase = state.phase();
    assert_eq!(phase.status(), AppUpdateStatus::Checking);
    assert!(!phase.begin_check());
    *phase = Phase::Downloading {
        version: "1.0.1".into(),
    };
    assert!(!phase.begin_check());
    assert!(phase.take_at_exit(true).is_none());
    assert!(!phase.begin_check());
    assert!(!Phase::Unsupported("disabled").begin_check());
}

// Generated with Tauri's signer against the literal fixture bytes, with
// --app-version 9.9.9. The one-off private key is not retained.
const PAYLOAD: &str = "signed test update";
const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDI5MDYyMUQwMjA2NjY3MTEKUldRUloyWWcwQ0VHS1poQ1Vwekh4YTVLaWFKc0YwWmhYbzRGZGlHQmJlNytHMThzR0pNY3JsbXgK";
const SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRUloyWWcwQ0VHS1N4MCtQamdobTRMdVlJWEtZRGlWaFRtMW96MyttYWR4eTFhY0xHSEtvMkFtWXc5MzdpaDBieC9YaHNGblJiTmUxeGpDQU1tdjFrTzRNYjVIdkYyQ1FnPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk1NDQ5NDA3CWZpbGU6YXBwLXVwZGF0ZS1maXh0dXJlLTVkMDk4YjJlOGExYjQ0M2FiMzMyNjViMjg4ZDk2ODZiLmJpbgl2ZXJzaW9uOjkuOS45CmNrZ0o4Y1N2bGdpTFlnNVJGVlhBdE0xUERLK2FXUjBsUEpQbmI0Y2JnREZPMHJXNDZNYXVtdTR5dm1BTkJ5Q0JhY25pS0c2ZXloNVBQVktyMEp0dkRRPT0K";

async fn download_from_fixture(version: &'static str, payload: &'static str) -> AppUpdates {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let source = release_source();
    let state = test_state(Phase::Idle);
    UpdateRoute::save(&state.db, "ghproxy.net").unwrap();
    let (route, _) = state.claim_check();
    let route = route.unwrap();
    let asset_url: Url = format!("{DOWNLOAD_BASE}/v{version}/{FIXTURE_ASSET}")
        .parse()
        .unwrap();
    let manifest_path = route.apply(&source.manifest).path().to_owned();
    let asset_path = route.apply(&asset_url).path().to_owned();
    let manifest = json!({
        "version": version,
        "platforms": {
            "fixture-installer": { "url": asset_url.as_str(), "signature": SIGNATURE }
        }
    });
    let router = Router::new()
        .route(
            &manifest_path,
            get(move || async move {
                (
                    [(header::CONTENT_TYPE, "application/json")],
                    manifest.to_string(),
                )
            }),
        )
        .route(&asset_path, get(move || async move { payload }));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let mut context = mock_context(noop_assets());
    context.config_mut().plugins.0.insert(
        "updater".into(),
        json!({
            "pubkey": PUBKEY,
            "endpoints": [format!("{origin}{manifest_path}")],
            "requireSignedVersion": true,
            "dangerousInsecureTransportProtocol": true
        }),
    );
    let app = mock_builder().build(context).unwrap();
    app.handle()
        .plugin(
            tauri_plugin_updater::Builder::new()
                .target("fixture-installer")
                .build(),
        )
        .unwrap();
    // Saving a different route while the check is active must not switch its download.
    UpdateRoute::save(&state.db, "gh-proxy.com").unwrap();
    let updater = app
        .updater_builder()
        .no_proxy()
        .timeout(CHECK_TIMEOUT)
        .build();
    state
        .check_and_download(updater, |update| {
            // Keep the real route path and signature verification while connecting to loopback.
            let url = source.artifact(route, &update.download_url, &update.version)?;
            assert_eq!(url.host_str(), Some("ghproxy.net"));
            format!("{origin}{}", url.path()).parse().ok()
        })
        .await;
    server.abort();
    state
}

#[tokio::test]
async fn stages_verified_bytes_only_once_and_only_for_normal_exit() {
    for normal_exit in [true, false] {
        let state = download_from_fixture("9.9.9", PAYLOAD).await;
        UpdateRoute::save(&state.db, "direct").unwrap();
        assert_eq!(state.claim_check().0, None);
        assert_eq!(UpdateRoute::load(&state.db).unwrap().as_str(), "direct");
        let mut phase = state.phase();
        assert_eq!(
            phase.status(),
            AppUpdateStatus::Ready {
                version: "9.9.9".into(),
            }
        );
        assert!(!phase.begin_check());
        assert_eq!(
            phase
                .take_at_exit(normal_exit)
                .map(|staged| staged.read_payload().unwrap().1),
            normal_exit.then(|| PAYLOAD.as_bytes().to_vec())
        );
        assert!(phase.take_at_exit(true).is_none());
    }
}

#[tokio::test]
async fn discards_a_staged_file_if_exit_wins_the_race() {
    let state = download_from_fixture("9.9.9", PAYLOAD).await;
    let mut phase = state.phase();
    let late_result = std::mem::replace(
        &mut *phase,
        Phase::Downloading {
            version: "9.9.9".into(),
        },
    );
    assert!(matches!(late_result, Phase::Ready(_)));
    assert!(phase.take_at_exit(true).is_none());
    phase.finish_download(late_result);
    assert!(matches!(*phase, Phase::Exiting));
    assert!(phase.take_at_exit(true).is_none());
}

#[test]
fn staging_reports_write_errors() {
    let fixture = tempfile::NamedTempFile::new().unwrap();
    let read_only = File::open(fixture.path()).unwrap();
    assert!(write_staged_file(read_only, PAYLOAD.as_bytes()).is_err());
}

#[tokio::test]
async fn rejects_tampered_bytes_and_manifest_version_and_allows_retry() {
    for (version, payload) in [("9.9.9", "modified test update"), ("9.9.10", PAYLOAD)] {
        let state = download_from_fixture(version, payload).await;
        let mut phase = state.phase();
        assert_eq!(
            phase.status(),
            AppUpdateStatus::Error {
                message: DOWNLOAD_ERROR,
            }
        );
        assert!(phase.begin_check());
        assert!(phase.take_at_exit(true).is_none());
    }
}

#[tokio::test]
async fn reports_up_to_date_and_allows_another_check() {
    let state = download_from_fixture("0.1.0", PAYLOAD).await;
    let mut phase = state.phase();
    assert_eq!(phase.status(), AppUpdateStatus::UpToDate);
    assert!(phase.begin_check());
}
