use super::*;
use axum::{http::header, routing::get, Router};
use serde_json::json;
use tauri::test::{mock_builder, mock_context, noop_assets};
use tauri::utils::config::BundleType;

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
        .manage(AppUpdates::new(Phase::Unsupported("disabled")))
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
fn signing_key_and_endpoint_are_required() {
    assert!(!configured(None));
    assert!(!configured(Some(&json!({
        "pubkey": "  ", "endpoints": ["https://example.invalid/latest.json"]
    }))));
    assert!(!configured(Some(
        &json!({"pubkey": "key", "endpoints": []})
    )));
    assert!(configured(Some(&json!({
        "pubkey": "key", "endpoints": ["https://example.invalid/latest.json"]
    }))));
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
    let state = AppUpdates::new(Phase::Idle);
    let starts = std::thread::scope(|scope| {
        let startup = scope.spawn(|| state.phase().begin_check());
        let manual = scope.spawn(|| state.phase().begin_check());
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
    let manifest = json!({
        "version": version,
        "platforms": {
            "fixture-installer": { "url": format!("{origin}/artifact"), "signature": SIGNATURE }
        }
    });
    let router = Router::new()
        .route(
            "/latest.json",
            get(move || async move {
                (
                    [(header::CONTENT_TYPE, "application/json")],
                    manifest.to_string(),
                )
            }),
        )
        .route("/artifact", get(move || async move { payload }));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let mut context = mock_context(noop_assets());
    context.config_mut().plugins.0.insert(
        "updater".into(),
        json!({
            "pubkey": PUBKEY,
            "endpoints": [format!("{origin}/latest.json")],
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
    let state = AppUpdates::new(Phase::Checking);
    let updater = app
        .updater_builder()
        .no_proxy()
        .timeout(CHECK_TIMEOUT)
        .build();
    state.check_and_download(updater).await;
    server.abort();
    state
}

#[tokio::test]
async fn stages_verified_bytes_only_once_and_only_for_normal_exit() {
    for normal_exit in [true, false] {
        let state = download_from_fixture("9.9.9", PAYLOAD).await;
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
