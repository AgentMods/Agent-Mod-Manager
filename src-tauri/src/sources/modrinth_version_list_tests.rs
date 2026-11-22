use super::*;

const PATH: &str = "/v2/project/A1b2C3d4/version";

fn request() -> VersionListRequest {
    serde_json::from_value(json!({
        "source_id": "modrinth",
        "project_id": "A1b2C3d4",
        "game_id": "minecraft",
        "game_version": "1.21.1",
        "loader": "fabric"
    }))
    .unwrap()
}

#[tokio::test]
async fn modrinth_version_list_filters_then_pages_and_resolves_an_explicit_older_id() {
    let mut rows = value(MR_VERSIONS).as_array().unwrap().clone();
    let mut older = rows[3].clone();
    older["id"] = json!("OlderBeta");
    older["version_type"] = json!("beta");
    older["date_published"] = json!("2025-12-05T03:12:50.112201Z");
    older["changelog"] = json!("untrusted notes omitted from list");
    let mut empty = older.clone();
    empty["id"] = json!("NoFile");
    empty["files"] = json!([]);
    rows.extend([rows[3].clone(), empty, older.clone()]);
    let body = json!(rows).to_string();
    let (p, mock) = provider(
        "modrinth",
        vec![
            Step::ok(PATH, body.clone()),
            Step::ok(PATH, body.clone()),
            Step::ok(PATH, body),
            Step::ok("/v2/version/OlderBeta", older.to_string()),
        ],
    );
    let sources = Sources {
        providers: vec![Arc::new(p)],
    };
    let mut req = request();
    assert_eq!(req.limit, 20);
    req.limit = 1;
    let first = sources.list_versions(&req).await.unwrap();
    assert!(first.has_more);
    assert_eq!(first.versions[0].version_id, "Version2");
    assert_eq!(first.versions[0].release_channel, ReleaseChannel::Release);
    req.offset = 1;
    let page = sources.list_versions(&req).await.unwrap();
    assert!(!page.has_more);
    assert_eq!(page.versions.len(), 1);
    let entry = &page.versions[0];
    assert_eq!(entry.version_id, "OlderBeta");
    assert_eq!(entry.version, first.versions[0].version);
    assert_eq!(entry.release_channel, ReleaseChannel::Beta);
    assert_eq!(entry.published_at, "2025-12-05T03:12:50.112201+00:00");
    assert_eq!(entry.loaders, ["fabric"]);
    assert_eq!(entry.game_versions, ["1.21.1"]);
    let shaped = serde_json::to_string(&page).unwrap();
    assert!(
        !shaped.contains("files") && !shaped.contains("https:") && !shaped.contains("changelog")
    );
    req.offset = 10_001;
    let after_end = sources.list_versions(&req).await.unwrap();
    assert!(after_end.versions.is_empty() && !after_end.has_more);
    let expected_filters = BTreeMap::from([
        ("game_versions".into(), r#"["1.21.1"]"#.into()),
        ("loaders".into(), r#"["fabric"]"#.into()),
    ]);
    for index in 0..3 {
        assert_eq!(mock.params(index), expected_filters);
    }
    let selected = sources
        .resolve(&ResolveRequest {
            source_id: req.source_id,
            project_id: req.project_id,
            version_id: Some(entry.version_id.clone()),
            game_id: req.game_id,
            game_version: req.game_version,
            loader: req.loader,
        })
        .await
        .unwrap()
        .download
        .unwrap();
    assert_eq!(selected.version_id, "OlderBeta");
    mock.assert_finished(4);
}

#[tokio::test]
async fn modrinth_version_list_distinguishes_no_matches_and_malformed_metadata() {
    let mut malformed = value(MR_VERSIONS);
    malformed[3]["date_published"] = json!("missing");
    let (p, mock) = provider(
        "modrinth",
        vec![
            Step::ok(PATH, MR_VERSIONS),
            Step::ok(PATH, malformed.to_string()),
        ],
    );
    let mut req = request();
    req.loader = Some("quilt".into());
    let empty = p.list_versions(&req).await.unwrap();
    assert_eq!(empty.status.state, SourceState::Available);
    assert!(empty.versions.is_empty() && !empty.has_more);
    req.loader = Some("fabric".into());
    assert!(p.list_versions(&req).await.is_err());
    mock.assert_finished(2);
}

#[tokio::test]
async fn version_list_rejects_bad_bounds_and_filters_without_requests() {
    let (p, mock) = provider("modrinth", vec![]);
    let sources = Sources {
        providers: vec![Arc::new(p)],
    };
    for req in [
        VersionListRequest {
            limit: 0,
            ..request()
        },
        VersionListRequest {
            limit: 51,
            ..request()
        },
        VersionListRequest {
            loader: Some("../../local-token".into()),
            ..request()
        },
        VersionListRequest {
            game_version: Some("x".repeat(65)),
            ..request()
        },
        VersionListRequest {
            project_id: "A1b2/versions".into(),
            ..request()
        },
    ] {
        assert!(matches!(
            sources.list_versions(&req).await,
            Err(AppError::Invalid(_))
        ));
    }
    mock.assert_finished(0);
}

#[tokio::test]
async fn version_list_rejects_unsupported_game_and_unconfigured_thunderstore_without_network() {
    let (modrinth, modrinth_mock) = provider("modrinth", vec![]);
    let (thunderstore, thunderstore_mock) = provider("thunderstore", vec![]);
    let sources = Sources {
        providers: vec![Arc::new(modrinth), Arc::new(thunderstore)],
    };
    let req = VersionListRequest {
        game_id: Some("local-game-id".into()),
        ..request()
    };
    let report = sources.list_versions(&req).await.unwrap();
    assert_eq!(report.status.source_id, req.source_id);
    assert_eq!(report.status.state, SourceState::Unsupported);
    assert!(report.versions.is_empty() && !report.has_more);

    let req = VersionListRequest {
        source_id: "thunderstore".into(),
        game_id: None,
        ..request()
    };
    let report = sources.list_versions(&req).await.unwrap();
    assert_eq!(report.status.source_id, req.source_id);
    assert_eq!(report.status.state, SourceState::NeedsConfiguration);
    assert!(report.versions.is_empty() && !report.has_more);
    modrinth_mock.assert_finished(0);
    thunderstore_mock.assert_finished(0);
}
