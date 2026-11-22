use super::resolutions::{ResolutionScope, Resolutions};
use super::web_download::WebDownload;
use super::{FileHashes, ResolveRequest, SearchQuery, SourceKeys, SourceState, Sources};
use crate::{app::AppServices, artifacts, download::DownloadPhase, util};
use std::{fs, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test]
#[ignore = "live public HTTPS CDN; temporary inert JAR without catalog provenance"]
async fn opt_in_live_public_archive_has_no_catalog_provenance() {
    const URL: &str =
        "https://cdn.modrinth.com/data/AANobbMI/versions/26nVNc41/sodium-fabric-0.6.9+mc1.21.1.jar";
    const EXPECTED_SHA256: &str =
        "411c4c03167066f0e2e61ba17793b4be13ddd73de5b677c6d402f1d8f4262ac2";
    let request = WebDownload::parse(URL).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let app = AppServices::initialize(temp.path().join("data")).unwrap();
    let artifact = tokio::time::timeout(
        Duration::from_secs(90),
        crate::download::DownloadManager::new(&app.engine.paths)
            .unwrap()
            .download_web(&request, CancellationToken::new(), None),
    )
    .await
    .expect("public archive transfer timed out")
    .unwrap();
    artifacts::record_web(&app.engine, &artifact).unwrap();
    let stored = artifacts::for_path(&app.engine.db, &artifact.path)
        .unwrap()
        .unwrap();
    assert_eq!(stored.source_id, "web");
    assert!(
        stored.origin().is_none() && stored.version_id.is_empty() && stored.project_id.is_empty()
    );
    assert!(
        artifact.requires_review && !artifact.integrity_verified && !artifact.auto_install_allowed
    );
    assert_eq!(artifact.size, 1_297_415);
    assert_eq!(util::sha256_file(&artifact.path).unwrap(), EXPECTED_SHA256);
    assert_eq!(artifact.sha256, EXPECTED_SHA256);
    assert!(app.engine.roots().unwrap().is_empty());
    println!(
        "Reviewed direct HTTPS transport: {} bytes, SHA-256 {}; no catalog origin, no installation",
        artifact.size, artifact.sha256
    );
}

#[tokio::test]
#[ignore = "live anonymous Thunderstore API and CDN; temp directories only; no extraction or execution"]
async fn opt_in_live_thunderstore_download_stays_inert() {
    const PROJECT: &str = "HansenbansenTeam/HudToggle";
    const VERSION: &str = "1.0.1";
    const SIZE: u64 = 100_869;
    let sources = Sources::new(SourceKeys::default()).unwrap();
    let search = sources
        .search(&SearchQuery {
            query: "HudToggle".into(),
            source_ids: vec!["thunderstore".into()],
            game_ids: [("thunderstore".into(), "valheim".into())].into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(search.sources[0].state, SourceState::Available);
    let found = search
        .mods
        .iter()
        .find(|m| m.project_id == PROJECT)
        .expect("exact project in search results");
    let resolved = sources
        .resolve(&ResolveRequest {
            source_id: found.source_id.clone(),
            project_id: found.project_id.clone(),
            version_id: Some(VERSION.into()),
            game_id: found.game_id.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(resolved.status.state, SourceState::Available);
    let resolutions = Resolutions::default();
    let preview = resolutions
        .publish(ResolutionScope::Desktop, resolved)
        .unwrap();
    assert!(!serde_json::to_value(&preview).unwrap()["download"]
        .as_object()
        .unwrap()
        .contains_key("download_url"));
    let download = preview.download.unwrap();
    assert_eq!(download.version_id, VERSION);
    assert_eq!(download.filename, "HansenbansenTeam-HudToggle-1.0.1.zip");
    assert_eq!(download.size, Some(SIZE));
    assert_eq!(download.hashes, FileHashes::default());

    let temp = tempfile::tempdir().unwrap();
    let app = AppServices::initialize(temp.path().join("data")).unwrap();
    let (progress, updates) = std::sync::mpsc::channel();
    let artifact = tokio::time::timeout(
        Duration::from_secs(90),
        app.downloads.run(
            &app.engine,
            &resolutions,
            "live-thunderstore".into(),
            download.resolution_id,
            crate::download_tasks::ClassificationInput {
                game_name: Some("Valheim".into()),
                ..Default::default()
            },
            Arc::new(move |event| {
                let _ = progress.send(event);
            }),
        ),
    )
    .await
    .expect("Thunderstore transfer timed out")
    .unwrap();
    let artifact = artifact.artifact;
    assert!(updates
        .try_iter()
        .any(|p| p.phase == DownloadPhase::Completed && p.downloaded == SIZE));
    assert_eq!(artifact.size, SIZE);
    assert!(artifact.path.starts_with(app.engine.paths.mod_library()));
    assert!(
        !artifact.integrity_verified && artifact.requires_review && !artifact.auto_install_allowed
    );
    assert_eq!(fs::metadata(&artifact.path).unwrap().len(), SIZE);
    assert_eq!(util::sha256_file(&artifact.path).unwrap(), artifact.sha256);
    let records = artifacts::list(&app.engine.db).unwrap();
    let [record] = records.as_slice() else {
        panic!("expected one completed download")
    };
    let origin = record.origin().unwrap();
    assert_eq!(
        (
            origin.source_id.as_str(),
            origin.project_id.as_str(),
            origin.version_id.as_str()
        ),
        ("thunderstore", PROJECT, VERSION)
    );
    assert!(app.engine.roots().unwrap().is_empty());
    let mutations: u64 = app
        .engine
        .db
        .conn()
        .query_row("SELECT count(*) FROM file_transactions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(mutations, 0);
    println!("Thunderstore {PROJECT}/{VERSION}: {SIZE} bytes, local SHA-256 {}; provider supplied no hash; kept inert", artifact.sha256);
}
