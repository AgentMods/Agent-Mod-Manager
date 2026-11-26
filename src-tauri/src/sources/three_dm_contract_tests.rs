use super::*;

const SEARCH: &str = include_str!("fixtures/three-dm-search.json");
const DETAIL: &str = include_str!("fixtures/three-dm-detail.json");
const SEARCH_PATH: &str = "/api/v3/mods";
const DETAIL_PATH: &str = "/api/v3/mods/1001";

fn detail(body: Value) -> (Provider, MockApi) {
    provider("3dm", vec![Step::ok(DETAIL_PATH, body.to_string())])
}

fn request() -> ResolveRequest {
    resolve("3dm", "1001", "2")
}

#[test]
fn project_page_accepts_only_canonical_native_ids() {
    for id in ["1", "1001", "9007199254740991"] {
        assert_eq!(
            crate::sources::three_dm_project_page(id).unwrap(),
            format!("https://mod.3dmgame.com/mod/{id}")
        );
    }
    for id in [
        "",
        "0",
        "01",
        "+1",
        "-1",
        "1 ",
        " 1",
        "１",
        "1/2",
        "1?key=x",
        "9007199254740992",
    ] {
        assert_eq!(crate::sources::three_dm_project_page(id), None, "{id:?}");
    }
}

fn page(number: u32, ids: &[u64]) -> Step {
    let mut response = value(SEARCH);
    let template = response["data"]["data"][0].clone();
    let rows: Vec<_> = ids
        .iter()
        .map(|id| {
            let mut row = template.clone();
            row["id"] = json!(id);
            row
        })
        .collect();
    response["data"] = json!({"data":rows,"count":4,"page":number,"pageSize":2,"totalPages":2});
    Step::ok(SEARCH_PATH, response.to_string())
}

#[tokio::test]
async fn search_maps_nested_v3_envelope_and_sends_raw_key_only_in_header() {
    let (p, mock) = provider("3dm", vec![Step::ok(SEARCH_PATH, SEARCH)]);
    let mut q = query("3dm", "2");
    q.query = "材质 & patch".into();
    let result = p.search(&q).await.unwrap();
    assert_eq!(result.sources[0].state, SourceState::Limited);
    assert_eq!(result.mods.len(), 1);
    let m = &result.mods[0];
    assert_eq!(
        (&*m.source_id, &*m.project_id, &*m.name),
        ("3dm", "1001", "Fixture Mod")
    );
    assert_eq!(m.game_id.as_deref(), Some("2"));
    assert_eq!(m.author.as_deref(), Some("Fixture Author"));
    assert_eq!(m.downloads, Some(123));
    assert_eq!(
        m.homepage_url.as_deref(),
        Some("https://mod.3dmgame.com/mod/1001")
    );
    assert_eq!(
        m.icon_url.as_deref(),
        Some("https://mod.3dmgame.com/static/upload/mod/fixture.webp")
    );
    assert_eq!(
        mock.params(0),
        BTreeMap::from([
            ("search".into(), q.query),
            ("gameId".into(), "2".into()),
            ("order".into(), "0".into()),
            ("page".into(), "1".into()),
            ("pageSize".into(), "20".into()),
        ])
    );
    mock.assert_finished(1);
}

#[tokio::test]
async fn arbitrary_offset_spans_two_pages_without_repeating_earlier_rows() {
    let (p, mock) = provider("3dm", vec![page(1, &[1001, 1002]), page(2, &[1003, 1004])]);
    let q = SearchQuery {
        offset: 1,
        limit: 2,
        ..Default::default()
    };
    let result = p.search(&q).await.unwrap();
    let ids: Vec<_> = result.mods.iter().map(|m| m.project_id.as_str()).collect();
    assert_eq!(ids, ["1002", "1003"]);
    assert_eq!(mock.params(0)["page"], "1");
    assert_eq!(mock.params(1)["page"], "2");
    assert!(!mock.params(0).contains_key("gameId"));
    mock.assert_finished(2);
}

#[tokio::test]
async fn malformed_pages_or_cross_game_results_are_not_treated_as_success() {
    for (field, val) in [
        ("page", json!(2)),
        ("pageSize", json!(21)),
        ("data", json!({})),
    ] {
        let mut body = value(SEARCH);
        body["data"][field] = val;
        let (p, mock) = provider("3dm", vec![Step::ok(SEARCH_PATH, body.to_string())]);
        assert!(p.search(&query("3dm", "2")).await.is_err());
        mock.assert_finished(1);
    }
    let (p, mock) = provider("3dm", vec![Step::ok(SEARCH_PATH, SEARCH)]);
    assert!(p.search(&query("3dm", "1")).await.is_err());
    mock.assert_finished(1);
}

#[tokio::test]
async fn denied_envelopes_and_echoed_keys_are_isolated_from_other_providers() {
    for body in [json!({"success":false,"msg":"private remote message"}), {
        let mut echo = value(SEARCH);
        echo["data"]["data"][0]["mods_desc"] = json!(TEST_KEY);
        echo
    }] {
        let (three_dm, three_dm_mock) =
            provider("3dm", vec![Step::ok(SEARCH_PATH, body.to_string())]);
        let (modrinth, modrinth_mock) =
            provider("modrinth", vec![Step::ok("/v2/search", MR_SEARCH)]);
        let sources = Sources {
            providers: vec![Arc::new(three_dm), Arc::new(modrinth)],
        };
        let result = sources.search(&query("3dm", "2")).await.unwrap();
        assert_eq!(result.sources[0].state, SourceState::Unavailable);
        assert_eq!(result.sources[1].state, SourceState::Available);
        assert!(!result.mods.is_empty() && result.mods.iter().all(|m| m.source_id == "modrinth"));
        let encoded = serde_json::to_string(&result).unwrap();
        assert!(!encoded.contains(TEST_KEY) && !encoded.contains("private remote message"));
        three_dm_mock.assert_finished(1);
        modrinth_mock.assert_finished(1);
    }
}

#[tokio::test]
async fn invalid_ids_and_unverifiable_filters_never_send_requests() {
    let (p, mock) = provider("3dm", vec![]);
    for id in ["0", "01001", "../1001", "9007199254740992"] {
        assert!(p.resolve(&resolve("3dm", id, "2")).await.is_err());
        assert!(p.search(&query("3dm", id)).await.is_err());
    }
    assert!(p
        .search(&SearchQuery {
            query: "游".repeat(101),
            ..Default::default()
        })
        .await
        .is_err());
    let q = SearchQuery {
        loader: Some("fabric".into()),
        ..query("3dm", "2")
    };
    assert_eq!(
        p.search(&q).await.unwrap().sources[0].state,
        SourceState::Unsupported
    );
    let r = ResolveRequest {
        game_version: Some("1.0".into()),
        ..request()
    };
    let result = p.resolve(&r).await.unwrap();
    assert_eq!(result.status.state, SourceState::ManualDownload);
    assert!(result.download.is_none() && result.homepage_url.is_some());
    mock.assert_finished(0);
}

#[tokio::test]
async fn latest_direct_resource_can_ignore_matching_mirror_without_guessing_integrity() {
    let mut body = value(DETAIL);
    body["data"]["mods_version"] = json!("9.9");
    body["data"]["mods_original_url"] = json!("https://untrusted.example/download?token=opaque");
    let (p, mock) = detail(body);
    let result = p.resolve(&request()).await.unwrap();
    assert_eq!(result.status.state, SourceState::Limited);
    assert_eq!(
        result.homepage_url.as_deref(),
        Some("https://mod.3dmgame.com/mod/1001")
    );
    let file = result.download.unwrap();
    assert_eq!(
        (
            &*file.source_id,
            &*file.project_id,
            &*file.version_id,
            &*file.version
        ),
        ("3dm", "1001", "2001", "2.0")
    );
    assert_eq!(file.filename, "Fixture.zip");
    assert_eq!(file.homepage_url, result.homepage_url);
    assert!(file.download_url.ends_with("/resource2/202611/fixture.zip"));
    assert_eq!(file.size, None);
    assert_eq!(file.hashes, FileHashes::default());
    assert_eq!(file.warnings.len(), 2);
    assert!(mock.params(0).is_empty());
    mock.assert_finished(1);
}

#[tokio::test]
async fn absent_version_falls_back_only_for_latest_with_matching_identity() {
    let mut body = value(DETAIL);
    body["data"]["mods_resource"][0]
        .as_object_mut()
        .unwrap()
        .remove("mods_resource_version");
    let (p, mock) = detail(body.clone());
    let file = p.resolve(&request()).await.unwrap().download.unwrap();
    assert_eq!(
        (file.version_id.as_str(), file.version.as_str()),
        ("2001", "2.0")
    );
    mock.assert_finished(1);

    // The fallback doesn't make an explicitly different mirror equivalent.
    body["data"]["mods_resource"][1]["mods_resource_version"] = json!("2.1");
    let (p, mock) = detail(body.clone());
    let result = p.resolve(&request()).await.unwrap();
    assert_eq!(result.status.state, SourceState::ManualDownload);
    assert!(result.download.is_none());
    mock.assert_finished(1);

    body["data"]["mods_resource"][2]["mods_resource_version"] = Value::Null;
    let (p, mock) = detail(body);
    let historical = ResolveRequest {
        version_id: Some("2003".into()),
        ..request()
    };
    assert!(matches!(
        p.resolve(&historical).await,
        Err(AppError::Invalid(_))
    ));
    mock.assert_finished(1);
}

#[tokio::test]
async fn latest_without_valid_version_and_invalid_mirror_are_not_admitted() {
    for (path, replacement) in [
        ("/data/mods_version", Value::Null),
        ("/data/mods_version", json!("2.0\ninvalid")),
        ("/data/mods_version", json!(128u32)),
        ("/data/mods_resource/0/mods_resource_version", json!(false)),
        (
            "/data/mods_resource/0/mods_resource_version",
            json!("2.0\ninvalid"),
        ),
        ("/data/mods_resource/1/mods_resource_version", json!(" ")),
        (
            "/data/mods_resource/1/mods_resource_version",
            json!("v".repeat(129)),
        ),
    ] {
        let mut body = value(DETAIL);
        body["data"]["mods_resource"][0]["mods_resource_version"] = Value::Null;
        *body.pointer_mut(path).unwrap() = replacement;
        let (p, mock) = detail(body);
        assert!(
            matches!(p.resolve(&request()).await, Err(AppError::Invalid(_))),
            "{path}"
        );
        mock.assert_finished(1);
    }
}

#[tokio::test]
async fn exact_resource_id_does_not_fall_back_to_newer_or_mirror() {
    for (id, expected) in [("2003", Some("1.0")), ("2002", None)] {
        let (p, mock) = detail(value(DETAIL));
        let result = p
            .resolve(&ResolveRequest {
                version_id: Some(id.into()),
                ..request()
            })
            .await
            .unwrap();
        assert_eq!(
            result.download.as_ref().map(|d| d.version.as_str()),
            expected
        );
        if let Some(download) = result.download {
            assert_eq!(download.version_id, id);
        } else {
            assert_eq!(result.status.state, SourceState::ManualDownload);
        }
        mock.assert_finished(1);
    }
    let (p, mock) = detail(value(DETAIL));
    assert!(matches!(
        p.resolve(&ResolveRequest {
            version_id: Some("2999".into()),
            ..request()
        })
        .await,
        Err(AppError::NotFound(_))
    ));
    mock.assert_finished(1);
}

#[tokio::test]
async fn different_latest_variants_or_multiple_direct_files_require_user_selection() {
    for field in [
        "mods_resource_name",
        "mods_resource_version",
        "mods_resource_url",
    ] {
        let mut body = value(DETAIL);
        body["data"]["mods_resource"][1][field] = json!(match field {
            "mods_resource_name" => "Optional-patch.zip",
            "mods_resource_version" => "1.9",
            _ => "https://dmod.3dmgame.com/static/upload/resource2/202611/other.zip",
        });
        let (p, mock) = detail(body);
        let result = p.resolve(&request()).await.unwrap();
        assert_eq!(result.status.state, SourceState::ManualDownload);
        assert!(result.download.is_none());
        mock.assert_finished(1);
    }
}

#[tokio::test]
async fn wrong_host_non_file_paths_and_noncanonical_urls_never_resolve() {
    for url in [
        "https://pan.example.org/share/fixture.zip",
        "https://dmod.3dmgame.com.example.org/static/upload/resource/fixture.zip",
        "https://dmod.3dmgame.com/static/upload/mod/fixture.zip",
        "https://dmod.3dmgame.com/static/upload/mod/../resource/fixture.zip",
        "https://dmod.3dmgame.com/static/upload/mod/%2e%2e/resource/fixture.zip",
        "https://dmod.3dmgame.com/static/upload/resource/fixture.zip?token=opaque",
        "https://user:pass@dmod.3dmgame.com/static/upload/resource/fixture.zip",
        "http://dmod.3dmgame.com/static/upload/resource/fixture.zip",
    ] {
        let mut body = value(DETAIL);
        body["data"]["mods_resource"][0]["mods_resource_url"] = json!(url);
        let (p, mock) = detail(body);
        let result = p.resolve(&request()).await.unwrap();
        assert_eq!(result.status.state, SourceState::ManualDownload, "{url}");
        assert!(result.download.is_none());
        mock.assert_finished(1);
    }
}

#[tokio::test]
async fn inconsistent_project_game_resource_ids_and_unsafe_names_are_rejected() {
    for (pointer, replacement) in [
        ("/data/id", json!(1002)),
        ("/data/game_id", json!(3)),
        ("/data/mods_resource/0/mods_id", json!(1002)),
        ("/data/mods_resource/1/id", json!(2001)),
        (
            "/data/mods_resource/0/mods_resource_latest_version",
            json!("true"),
        ),
        (
            "/data/mods_resource/0/mods_resource_name",
            json!("../Fixture.zip"),
        ),
        (
            "/data/mods_resource/0/mods_resource_name",
            json!(format!("{}.zip", "x".repeat(177))),
        ),
    ] {
        let mut body = value(DETAIL);
        *body.pointer_mut(pointer).unwrap() = replacement;
        let (p, mock) = detail(body);
        let r = ResolveRequest {
            version_id: Some("2001".into()),
            ..request()
        };
        assert!(
            matches!(p.resolve(&r).await, Err(AppError::Invalid(_))),
            "{pointer}"
        );
        mock.assert_finished(1);
    }
}
