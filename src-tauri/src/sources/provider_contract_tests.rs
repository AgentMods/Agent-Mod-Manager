//! Offline response fixtures exercise the actual search/resolve branches.
//! This module and its transport seam are compiled only with cfg(test).
use super::*;
use std::{collections::VecDeque, sync::Mutex};

#[path = "modrinth_dependency_graph_tests.rs"]
mod modrinth_dependency_graph_tests;
#[path = "modrinth_graph_review_tests.rs"]
mod modrinth_graph_review_tests;
#[path = "modrinth_version_list_tests.rs"]
mod modrinth_version_list_tests;
#[path = "three_dm_contract_tests.rs"]
mod three_dm_contract_tests;

const MR_SEARCH: &str = include_str!("fixtures/modrinth-search.json");
const MR_VERSIONS: &str = include_str!("fixtures/modrinth-versions.json");
const TS_PAGE_1: &str = include_str!("fixtures/thunderstore-search-page-1.json");
const TS_PAGE_2: &str = include_str!("fixtures/thunderstore-search-page-2.json");
const TS_VERSION: &str = include_str!("fixtures/thunderstore-version.json");
const TS_VERSIONS_1: &str = include_str!("fixtures/thunderstore-versions-page-1.json");
const TS_VERSIONS_2: &str = include_str!("fixtures/thunderstore-versions-page-2.json");
const CF_SEARCH: &str = include_str!("fixtures/curseforge-search.json");
const CF_PROJECT: &str = include_str!("fixtures/curseforge-project.json");
const CF_FILES: &str = include_str!("fixtures/curseforge-files.json");
const NX_MOD: &str = include_str!("fixtures/nexus-mod.json");
const NX_FILES: &str = include_str!("fixtures/nexus-files.json");
const NX_LINKS: &str = include_str!("fixtures/nexus-download-links.json");
const NX_SEARCH: &str = include_str!("fixtures/nexus-v2-search.json");
const NX_SEARCH_PATH: &str = "/v2/graphql";
const TEST_KEY: &str = "offline-contract-test-key";

struct Step {
    method: reqwest::Method,
    path: &'static str,
    status: reqwest::StatusCode,
    body: String,
}
impl Step {
    fn ok(path: &'static str, body: impl Into<String>) -> Self {
        Self {
            method: reqwest::Method::GET,
            path,
            status: reqwest::StatusCode::OK,
            body: body.into(),
        }
    }
    fn forbidden(path: &'static str) -> Self {
        Self {
            method: reqwest::Method::GET,
            path,
            status: reqwest::StatusCode::FORBIDDEN,
            body: "remote body must not be exposed: offline-contract-test-key".into(),
        }
    }
    fn post(mut self) -> Self {
        self.method = reqwest::Method::POST;
        self
    }
}
struct MockState {
    steps: VecDeque<Step>,
    requests: Vec<reqwest::Request>,
}
#[derive(Clone)]
pub(super) struct MockApi {
    state: Arc<Mutex<MockState>>,
    host: &'static str,
    secret_header: Option<&'static str>,
}
impl MockApi {
    pub(super) fn respond(&self, builder: reqwest::RequestBuilder) -> AppResult<Value> {
        let request = builder.build().expect("provider request must build");
        assert_eq!(request.url().scheme(), "https");
        assert_eq!(request.url().host_str(), Some(self.host));
        assert!(request.url().username().is_empty() && request.url().password().is_none());
        assert!(!request.url().as_str().contains(TEST_KEY));
        assert!(!request
            .url()
            .query_pairs()
            .any(|(k, _)| matches!(k.as_ref(), "apikey" | "x-api-key")));
        if let Some(name) = self.secret_header {
            let key = request
                .headers()
                .get(name)
                .expect("API key belongs in the provider header");
            assert!(key.is_sensitive());
            assert!(key == TEST_KEY);
        } else {
            assert!(request.headers().get("apikey").is_none());
            assert!(request.headers().get("x-api-key").is_none());
        }
        let mut state = self.state.lock().unwrap();
        // An exhausted/unexpected script fails immediately and NEVER falls
        // through to reqwest/network, even when the provider holds a test key.
        let step = state
            .steps
            .pop_front()
            .expect("unexpected provider request; networking is disabled");
        assert_eq!(request.method(), step.method);
        assert_eq!(request.url().path(), step.path);
        state.requests.push(request);
        if !step.status.is_success() {
            return Err(status_error(step.status));
        }
        serde_json::from_str(&step.body).map_err(|_| AppError::Other("来源响应格式无效".into()))
    }
    fn params(&self, index: usize) -> BTreeMap<String, String> {
        self.state.lock().unwrap().requests[index]
            .url()
            .query_pairs()
            .into_owned()
            .collect()
    }
    fn json_body(&self, index: usize) -> Value {
        let state = self.state.lock().unwrap();
        let body = state.requests[index]
            .body()
            .and_then(|body| body.as_bytes())
            .expect("request must carry a JSON body");
        serde_json::from_slice(body).expect("request body must be JSON")
    }
    fn header(&self, index: usize, name: &str) -> Option<String> {
        self.state.lock().unwrap().requests[index]
            .headers()
            .get(name)
            .map(|value| value.to_str().expect("header must be ASCII").to_owned())
    }
    fn assert_finished(&self, count: usize) {
        let state = self.state.lock().unwrap();
        assert!(state.steps.is_empty(), "unconsumed fixture responses");
        assert_eq!(state.requests.len(), count);
    }
}

fn normalized_graphql_query(body: &Value) -> String {
    body["query"]
        .as_str()
        .expect("GraphQL request must have a query")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn assert_nexus_search_request(
    mock: &MockApi,
    index: usize,
    game: &str,
    term: &str,
    count: u32,
    offset: u32,
) -> String {
    let body = mock.json_body(index);
    let variables = body["variables"].as_object().expect("variables object");
    assert_eq!(
        variables.keys().map(String::as_str).collect::<Vec<_>>(),
        ["count", "game", "offset", "term"]
    );
    assert_eq!(
        body["variables"],
        json!({
            "game": game,
            "term": term,
            "count": count,
            "offset": offset,
        })
    );
    let query = normalized_graphql_query(&body);
    assert!(
        query.starts_with("query FindMods("),
        "unexpected query: {query}"
    );
    assert!(
        query.contains("$game: String!"),
        "unexpected query: {query}"
    );
    assert!(
        query.contains("$term: String!"),
        "unexpected query: {query}"
    );
    assert!(query.contains("$count: Int!"), "unexpected query: {query}");
    assert!(query.contains("$offset: Int!"), "unexpected query: {query}");
    assert!(
        query.contains("gameDomainName"),
        "unexpected query: {query}"
    );
    assert!(query.contains("EQUALS"), "unexpected query: {query}");
    assert!(query.contains("nameStemmed"), "unexpected query: {query}");
    assert!(query.contains("MATCHES"), "unexpected query: {query}");
    assert!(
        query.contains("viewUploaderHidden: false"),
        "unexpected query: {query}"
    );
    assert!(
        query.to_ascii_uppercase().contains("RELEVANCE"),
        "unexpected query: {query}"
    );
    for field in [
        "modId",
        "name",
        "summary",
        "downloads",
        "author",
        "domainName",
    ] {
        assert!(
            query.contains(field),
            "projection is missing {field}: {query}"
        );
    }
    assert!(
        !query.contains("version"),
        "version must not be projected: {query}"
    );
    assert_eq!(
        mock.header(index, "Application-Name").as_deref(),
        Some("Agent Mod Manager")
    );
    assert_eq!(
        mock.header(index, "Application-Version").as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    query
}
fn provider(id: &'static str, steps: Vec<Step>) -> (Provider, MockApi) {
    let (host, header) = match id {
        "modrinth" => ("api.modrinth.com", None),
        "thunderstore" => ("thunderstore.io", None),
        "curseforge" => ("api.curseforge.com", Some("x-api-key")),
        "nexus" => ("api.nexusmods.com", Some("apikey")),
        "3dm" => ("mod.3dmgame.com", Some("authorization")),
        _ => panic!("unsupported fixture provider"),
    };
    let mock = MockApi {
        state: Arc::new(Mutex::new(MockState {
            steps: steps.into(),
            requests: Vec::new(),
        })),
        host,
        secret_header: header,
    };
    let key = secret_header(header.map(|_| TEST_KEY.to_owned())).map_err(|_| ());
    (
        Provider {
            id,
            // A provider that accidentally bypasses the mock still cannot
            // reach the Internet using this client's loopback-only proxy.
            client: Client::builder()
                .proxy(reqwest::Proxy::all("http://127.0.0.1:0").unwrap())
                .build()
                .unwrap(),
            key,
            mock: Some(mock.clone()),
        },
        mock,
    )
}
fn value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap()
}
fn query(source: &str, game: &str) -> SearchQuery {
    SearchQuery {
        query: "sample".into(),
        game_ids: BTreeMap::from([(source.into(), game.into())]),
        ..Default::default()
    }
}
fn resolve(source: &str, project: &str, game: &str) -> ResolveRequest {
    ResolveRequest {
        source_id: source.into(),
        project_id: project.into(),
        game_id: Some(game.into()),
        ..Default::default()
    }
}
fn modrinth_versions_with_dependencies(dependencies: Option<Value>) -> Value {
    let mut versions = value(MR_VERSIONS);
    versions[0]["dependencies"] = json!([{
        "dependency_type": "required",
        "project_id": "UnselDep",
        "version_id": null,
        "file_name": null
    }]);
    let selected = versions[3].as_object_mut().unwrap();
    if let Some(dependencies) = dependencies {
        selected.insert("dependencies".into(), dependencies);
    } else {
        selected.remove("dependencies");
    }
    versions
}
fn assert_resolve_dependencies(result: &ResolveResult, expected: Value) {
    let result = serde_json::to_value(
        super::resolutions::Resolutions::default()
            .publish(super::resolutions::ResolutionScope::Desktop, result.clone())
            .unwrap(),
    )
    .unwrap();
    assert!(result.get("dependencies").is_some());
    assert_eq!(result["dependencies"], expected);
}

#[tokio::test]
async fn modrinth_search_maps_real_search_envelope_and_encodes_facets() {
    let (p, mock) = provider("modrinth", vec![Step::ok("/v2/search", MR_SEARCH)]);
    let mut q = query("modrinth", "minecraft");
    q.game_version = Some("1.21.1".into());
    q.loader = Some("fabric".into());
    q.offset = 40;
    q.limit = 10;
    let page = p.search(&q).await.unwrap();
    let m = &page.mods[0];
    assert_eq!(
        (m.source_id.as_str(), m.project_id.as_str(), m.name.as_str()),
        ("modrinth", "A1b2C3d4", "Sample Tools")
    );
    assert_eq!(m.author.as_deref(), Some("SampleAuthor"));
    assert_eq!(m.summary, "Tools for Minecraft");
    assert_eq!(m.downloads, Some(12345));
    assert_eq!(m.game_id.as_deref(), Some("minecraft"));
    assert_eq!(
        m.homepage_url.as_deref(),
        Some("https://modrinth.com/mod/A1b2C3d4")
    );
    assert!(m
        .icon_url
        .as_deref()
        .unwrap()
        .starts_with("https://cdn.modrinth.com/"));
    let args = mock.params(0);
    assert_eq!(
        (&args["query"], &args["offset"], &args["limit"]),
        (&"sample".into(), &"40".into(), &"10".into())
    );
    assert_eq!(
        value(&args["facets"]),
        json!([
            ["project_type:mod"],
            ["categories:fabric"],
            ["versions:1.21.1"]
        ])
    );
    mock.assert_finished(1);
}

#[tokio::test]
async fn modrinth_resolve_selects_compatible_project_and_primary_file() {
    let (p, mock) = provider(
        "modrinth",
        vec![Step::ok("/v2/project/A1b2C3d4/version", MR_VERSIONS)],
    );
    let mut r = resolve("modrinth", "A1b2C3d4", "minecraft");
    r.game_version = Some("1.21.1".into());
    r.loader = Some("fabric".into());
    let download = p.resolve(&r).await.unwrap().download.unwrap();
    assert_eq!(
        (
            download.project_id.as_str(),
            download.version_id.as_str(),
            download.version.as_str()
        ),
        ("A1b2C3d4", "Version2", "2.0.0")
    );
    assert_eq!(
        (download.filename.as_str(), download.size),
        ("sample-tools.jar", Some(123456))
    );
    assert_eq!(
        download.hashes.sha512.as_deref(),
        Some("a".repeat(128).as_str())
    );
    assert_eq!(
        download.hashes.sha1.as_deref(),
        Some("b".repeat(40).as_str())
    );
    assert!(download.hashes.sha256.is_none());
    assert!(download.hashes.md5.is_none());
    assert!(download
        .download_url
        .ends_with("/Version2/sample-tools.jar"));
    let args = mock.params(0);
    assert_eq!(value(&args["loaders"]), json!(["fabric"]));
    assert_eq!(value(&args["game_versions"]), json!(["1.21.1"]));
    mock.assert_finished(1);
}

#[tokio::test]
async fn modrinth_resolve_serializes_dependencies_from_only_the_selected_version() {
    let expected = json!([
        {
            "dependency_type": "required",
            "project_id": "Required",
            "version_id": "Pinned01",
            "file_name": null
        },
        {
            "dependency_type": "optional",
            "project_id": "Optional",
            "version_id": null,
            "file_name": "optional.jar"
        },
        {
            "dependency_type": "incompatible",
            "project_id": null,
            "version_id": "Conflict",
            "file_name": null
        },
        {
            "dependency_type": "embedded",
            "project_id": null,
            "version_id": null,
            "file_name": "embedded.jar"
        }
    ]);
    let versions = modrinth_versions_with_dependencies(Some(expected.clone()));
    let (p, mock) = provider(
        "modrinth",
        vec![Step::ok(
            "/v2/project/A1b2C3d4/version",
            versions.to_string(),
        )],
    );
    let mut request = resolve("modrinth", "A1b2C3d4", "minecraft");
    request.game_version = Some("1.21.1".into());
    request.loader = Some("fabric".into());
    let result = p.resolve(&request).await.unwrap();
    assert_eq!(result.download.as_ref().unwrap().version_id, "Version2");
    assert_resolve_dependencies(&result, expected);
    mock.assert_finished(1);
}

#[tokio::test]
async fn modrinth_dependency_unknown_states_are_null_and_explicit_empty_is_empty() {
    let mut request = resolve("modrinth", "A1b2C3d4", "minecraft");
    request.game_version = Some("1.21.1".into());
    request.loader = Some("fabric".into());
    let (p, mock) = provider(
        "modrinth",
        vec![Step::ok(
            "/v2/project/A1b2C3d4/version",
            modrinth_versions_with_dependencies(Some(json!([]))).to_string(),
        )],
    );
    let result = p.resolve(&request).await.unwrap();
    assert_resolve_dependencies(&result, json!([]));
    mock.assert_finished(1);

    let valid = json!({
        "dependency_type": "required",
        "project_id": "ValidDep",
        "version_id": null,
        "file_name": null
    });
    let mut unknown_type_group = vec![valid.clone()];
    unknown_type_group.push(json!({
        "dependency_type": "unknown",
        "project_id": "project"
    }));
    let mut missing_reference_group = vec![valid.clone()];
    missing_reference_group.push(json!({"dependency_type": "required"}));
    let oversized = Value::Array(vec![valid; 129]);
    for (label, dependencies) in [
        ("null", Some(Value::Null)),
        ("malformed object", Some(json!({"not": "an array"}))),
        (
            "unknown type invalidates group",
            Some(json!(unknown_type_group)),
        ),
        (
            "missing reference invalidates group",
            Some(json!(missing_reference_group)),
        ),
        (
            "invalid reference type",
            Some(json!([{"dependency_type":"required","project_id":42}])),
        ),
        (
            "invalid ID",
            Some(json!([{"dependency_type":"required","project_id":"../outside"}])),
        ),
        (
            "filename is never a path",
            Some(json!([{"dependency_type":"embedded","file_name":"../other.jar"}])),
        ),
        ("oversized list", Some(oversized)),
        ("missing field", None),
    ] {
        let body = modrinth_versions_with_dependencies(dependencies).to_string();
        let (p, mock) = provider(
            "modrinth",
            vec![Step::ok("/v2/project/A1b2C3d4/version", body)],
        );
        let result = p.resolve(&request).await.unwrap();
        assert!(result.dependencies.is_none(), "{label}");
        mock.assert_finished(1);
    }
}

#[tokio::test]
async fn modrinth_version_id_only_resolves_and_checks_id_and_compatibility_filters() {
    let mut request = ResolveRequest {
        source_id: "modrinth".into(),
        version_id: Some("Version2".into()),
        game_id: Some("minecraft".into()),
        game_version: Some("1.21.1".into()),
        loader: Some("fabric".into()),
        ..Default::default()
    };
    let (p, mock) = provider(
        "modrinth",
        vec![Step::ok(
            "/v2/version/Version2",
            value(MR_VERSIONS)[3].to_string(),
        )],
    );
    let result = p.resolve(&request).await.unwrap();
    let download = result.download.as_ref().unwrap();
    assert_eq!(
        (download.project_id.as_str(), download.version_id.as_str()),
        ("A1b2C3d4", "Version2")
    );
    assert_resolve_dependencies(&result, json!([]));
    mock.assert_finished(1);

    let mut mismatched_id = value(MR_VERSIONS)[3].clone();
    mismatched_id["id"] = json!("DifferentVersion");
    let mut cases = vec![("version id", mismatched_id, "1.21.1", "fabric")];
    cases.push((
        "game version filter",
        value(MR_VERSIONS)[3].clone(),
        "1.20.1",
        "fabric",
    ));
    cases.push((
        "loader filter",
        value(MR_VERSIONS)[3].clone(),
        "1.21.1",
        "quilt",
    ));
    for (label, version, game_version, loader) in cases {
        request.game_version = Some(game_version.into());
        request.loader = Some(loader.into());
        let (p, mock) = provider(
            "modrinth",
            vec![Step::ok("/v2/version/Version2", version.to_string())],
        );
        assert!(
            matches!(p.resolve(&request).await, Err(AppError::NotFound(_))),
            "{label}"
        );
        mock.assert_finished(1);
    }
}

#[tokio::test]
async fn non_modrinth_sources_still_require_project_id_for_version_resolution() {
    for (source, game) in [
        ("thunderstore", "valheim"),
        ("curseforge", "432"),
        ("nexus", "skyrimspecialedition"),
        ("3dm", "1"),
    ] {
        let (p, mock) = provider(source, Vec::new());
        let request = ResolveRequest {
            source_id: source.into(),
            version_id: Some("700".into()),
            game_id: Some(game.into()),
            ..Default::default()
        };
        assert!(p.resolve(&request).await.is_err(), "{source}");
        mock.assert_finished(0);
    }
}

#[tokio::test]
async fn modrinth_explicit_version_uses_single_version_payload_and_rejects_wrong_project() {
    for project in ["A1b2C3d4", "OtherProject"] {
        let (p, mock) = provider(
            "modrinth",
            vec![Step::ok(
                "/v2/version/Version2",
                value(MR_VERSIONS)[3].to_string(),
            )],
        );
        let mut r = resolve("modrinth", project, "minecraft");
        r.version_id = Some("Version2".into());
        let result = p.resolve(&r).await;
        if project == "A1b2C3d4" {
            assert_eq!(result.unwrap().download.unwrap().version_id, "Version2");
        } else {
            assert!(matches!(result, Err(AppError::NotFound(_))));
        }
        mock.assert_finished(1);
    }
}

#[tokio::test]
async fn modrinth_reports_no_compatible_version_or_no_files() {
    let mut without_files = value(MR_VERSIONS);
    without_files[3]["files"] = json!([]);
    for (response, loader) in [
        (MR_VERSIONS.to_owned(), "quilt"),
        (without_files.to_string(), "fabric"),
    ] {
        let (p, mock) = provider(
            "modrinth",
            vec![Step::ok("/v2/project/A1b2C3d4/version", response)],
        );
        let mut r = resolve("modrinth", "A1b2C3d4", "minecraft");
        r.loader = Some(loader.into());
        r.game_version = Some("1.21.1".into());
        assert!(matches!(p.resolve(&r).await, Err(AppError::NotFound(_))));
        mock.assert_finished(1);
    }
}

#[tokio::test]
async fn thunderstore_search_maps_pages_to_offset_and_keeps_fixed_origin() {
    let mut first = value(TS_PAGE_1);
    first["next"] = json!("https://untrusted.example.org/steal");
    let (p, mock) = provider(
        "thunderstore",
        vec![
            Step::ok("/api/cyberstorm/listing/valheim/", first.to_string()),
            Step::ok("/api/cyberstorm/listing/valheim/", TS_PAGE_2),
        ],
    );
    let mut q = query("thunderstore", "valheim");
    q.offset = 19;
    q.limit = 2;
    let page = p.search(&q).await.unwrap();
    assert_eq!(
        page.mods
            .iter()
            .map(|m| m.project_id.as_str())
            .collect::<Vec<_>>(),
        ["Team/SampleMod19", "Team/FinalMod"]
    );
    assert_eq!(page.mods[0].author.as_deref(), Some("Team"));
    assert_eq!(page.mods[0].downloads, Some(120));
    assert_eq!(page.mods[0].summary, "Sample Valheim mod");
    assert_eq!(
        page.mods[0].homepage_url.as_deref(),
        Some("https://thunderstore.io/c/valheim/p/Team/SampleMod19/")
    );
    assert_eq!(mock.params(0)["page"], "1");
    assert_eq!(mock.params(1)["page"], "2");
    assert_eq!(mock.params(1)["q"], "sample");
    mock.assert_finished(2);
}

#[tokio::test]
async fn thunderstore_version_list_validates_community_pages_and_bounds_filters() {
    let (p, mock) = provider(
        "thunderstore",
        vec![
            Step::ok(
                "/api/cyberstorm/listing/valheim/Team/SampleMod/",
                TS_VERSION,
            ),
            Step::ok(
                "/api/cyberstorm/package/Team/SampleMod/versions/",
                TS_VERSIONS_1,
            ),
            Step::ok(
                "/api/cyberstorm/package/Team/SampleMod/versions/",
                TS_VERSIONS_2,
            ),
        ],
    );
    let request = VersionListRequest {
        source_id: "thunderstore".into(),
        project_id: "Team/SampleMod".into(),
        game_id: Some("valheim".into()),
        game_version: Some("0.218.15".into()),
        loader: Some("bepinex".into()),
        offset: 1,
        limit: 2,
    };
    let result = p.list_versions(&request).await.unwrap();
    assert_eq!(result.status.state, SourceState::Limited);
    assert_eq!(
        result
            .versions
            .iter()
            .map(|version| version.version_id.as_str())
            .collect::<Vec<_>>(),
        ["1.1.0", "1.0.0"]
    );
    assert!(result.versions.iter().all(|version| {
        version.release_channel == ReleaseChannel::Release
            && version.game_versions.is_empty()
            && version.loaders.is_empty()
    }));
    assert!(!result.has_more);
    assert_eq!(mock.params(1)["page"], "1");
    assert_eq!(mock.params(2)["page"], "2");
    mock.assert_finished(3);
}

#[tokio::test]
async fn thunderstore_version_list_rejects_mismatched_community_without_fetching_versions() {
    let mut listing = value(TS_VERSION);
    listing["community_identifier"] = json!("wrong-community");
    let (p, mock) = provider(
        "thunderstore",
        vec![Step::ok(
            "/api/cyberstorm/listing/valheim/Team/SampleMod/",
            listing.to_string(),
        )],
    );
    let request = VersionListRequest {
        source_id: "thunderstore".into(),
        project_id: "Team/SampleMod".into(),
        game_id: Some("valheim".into()),
        game_version: None,
        loader: None,
        offset: 0,
        limit: 20,
    };
    assert!(matches!(
        p.list_versions(&request).await,
        Err(AppError::Conflict(_))
    ));
    mock.assert_finished(1);
}

#[tokio::test]
async fn thunderstore_pinned_version_uses_full_version_name_instead_of_latest() {
    let (p, mock) = provider(
        "thunderstore",
        vec![Step::ok(
            "/api/cyberstorm/listing/valheim/Team/SampleMod/v/1.2.3/",
            TS_VERSION,
        )],
    );
    let mut r = resolve("thunderstore", "Team/SampleMod", "valheim");
    r.version_id = Some("1.2.3".into());
    let result = p.resolve(&r).await.unwrap();
    assert_resolve_dependencies(
        &result,
        json!([{
            "dependency_type": "required",
            "project_id": "denikson/BepInExPack_Valheim",
            "version_id": "5.4.2202",
            "file_name": null
        }]),
    );
    let d = result.download.unwrap();
    assert_eq!(
        (
            d.version.as_str(),
            d.version_id.as_str(),
            d.filename.as_str()
        ),
        ("1.2.3", "1.2.3", "Team-SampleMod-1.2.3.zip")
    );
    assert_eq!(d.size, Some(1234567));
    assert_eq!(d.hashes, FileHashes::default());
    assert_eq!(
        d.download_url,
        "https://thunderstore.io/package/download/Team/SampleMod/1.2.3/"
    );
    assert!(!d.warnings.is_empty());
    mock.assert_finished(1);
}

#[tokio::test]
async fn thunderstore_unavailable_dependency_metadata_stays_unknown() {
    let mut response = value(TS_VERSION);
    response["dependencies"][0]["is_unavailable"] = json!(true);
    let (p, mock) = provider(
        "thunderstore",
        vec![Step::ok(
            "/api/cyberstorm/listing/valheim/Team/SampleMod/v/1.2.3/",
            response.to_string(),
        )],
    );
    let mut request = resolve("thunderstore", "Team/SampleMod", "valheim");
    request.version_id = Some("1.2.3".into());
    let result = p.resolve(&request).await.unwrap();
    assert_resolve_dependencies(&result, Value::Null);
    mock.assert_finished(1);
}

#[tokio::test]
async fn thunderstore_does_not_resolve_inconsistent_community_project_version_or_file() {
    for (field, replacement) in [
        ("community_identifier", "riskofrain2"),
        ("namespace", "OtherTeam"),
        ("name", "OtherMod"),
        ("full_version_name", "Team-SampleMod-2.0.0"),
        (
            "download_url",
            "https://thunderstore.io/package/download/Team/SampleMod/2.0.0/",
        ),
    ] {
        let mut response = value(TS_VERSION);
        response[field] = json!(replacement);
        let (p, mock) = provider(
            "thunderstore",
            vec![Step::ok(
                "/api/cyberstorm/listing/valheim/Team/SampleMod/v/1.2.3/",
                response.to_string(),
            )],
        );
        let mut request = resolve("thunderstore", "Team/SampleMod", "valheim");
        request.version_id = Some("1.2.3".into());
        assert!(
            matches!(p.resolve(&request).await, Err(AppError::Conflict(_))),
            "{field}"
        );
        mock.assert_finished(1);
    }

    let mut response = value(TS_VERSION);
    response
        .as_object_mut()
        .unwrap()
        .remove("full_version_name");
    let (p, mock) = provider(
        "thunderstore",
        vec![Step::ok(
            "/api/cyberstorm/listing/valheim/Team/SampleMod/v/1.2.3/",
            response.to_string(),
        )],
    );
    let mut request = resolve("thunderstore", "Team/SampleMod", "valheim");
    request.version_id = Some("1.2.3".into());
    assert!(p.resolve(&request).await.is_err());
    mock.assert_finished(1);
}

#[tokio::test]
async fn thunderstore_latest_lookup_requires_consistent_version_metadata() {
    let path = "/api/cyberstorm/listing/valheim/Team/SampleMod/";
    let mut consistent = value(TS_VERSION);
    consistent["latest_version_number"] = json!("1.2.3");
    let (p, mock) = provider(
        "thunderstore",
        vec![
            Step::ok(path, consistent.to_string()),
            Step::ok(path, TS_VERSION),
        ],
    );
    let request = resolve("thunderstore", "Team/SampleMod", "valheim");
    assert_eq!(
        p.resolve(&request)
            .await
            .unwrap()
            .download
            .unwrap()
            .version_id,
        "1.2.3"
    );
    assert!(matches!(
        p.resolve(&request).await,
        Err(AppError::Conflict(_))
    ));
    mock.assert_finished(2);
}

#[tokio::test]
async fn thunderstore_distinguishes_lookup_namespace_from_owning_team() {
    let mut response = value(TS_VERSION);
    response["team"]["name"] = json!("Maintainer");
    response["full_version_name"] = json!("Maintainer-SampleMod-1.2.3");
    response["download_url"] =
        json!("https://thunderstore.io/package/download/Maintainer/SampleMod/1.2.3/");
    let (p, mock) = provider(
        "thunderstore",
        vec![Step::ok(
            "/api/cyberstorm/listing/valheim/Team/SampleMod/v/1.2.3/",
            response.to_string(),
        )],
    );
    let mut request = resolve("thunderstore", "Team/SampleMod", "valheim");
    request.version_id = Some("1.2.3".into());
    let download = p.resolve(&request).await.unwrap().download.unwrap();
    assert_eq!(download.project_id, "Team/SampleMod");
    assert_eq!(download.filename, "Maintainer-SampleMod-1.2.3.zip");
    mock.assert_finished(1);
}

#[tokio::test]
async fn curseforge_search_maps_data_envelope_nullable_fields_and_numeric_ids() {
    let (p, mock) = provider("curseforge", vec![Step::ok("/v1/mods/search", CF_SEARCH)]);
    let mut q = query("curseforge", "432");
    q.game_version = Some("1.21.1".into());
    q.offset = 20;
    q.limit = 5;
    let page = p.search(&q).await.unwrap();
    let m = &page.mods[0];
    assert_eq!(
        (m.project_id.as_str(), m.name.as_str()),
        ("67217", "Sample Toolkit")
    );
    assert_eq!(m.game_id.as_deref(), Some("432"));
    assert_eq!(m.author.as_deref(), Some("ToolkitAuthor"));
    assert_eq!(m.downloads, Some(9876));
    assert!(m
        .homepage_url
        .as_ref()
        .unwrap()
        .ends_with("/sample-toolkit"));
    assert!(m.icon_url.is_some());
    let args = mock.params(0);
    assert_eq!(
        (
            args["gameId"].as_str(),
            args["index"].as_str(),
            args["pageSize"].as_str(),
            args["gameVersion"].as_str()
        ),
        ("432", "20", "5", "1.21.1")
    );
    mock.assert_finished(1);
}

#[tokio::test]
async fn curseforge_resolve_filters_project_availability_and_game_version_then_selects_latest() {
    let (p, mock) = provider(
        "curseforge",
        vec![
            Step::ok("/v1/mods/67217", CF_PROJECT),
            Step::ok("/v1/mods/67217/files", CF_FILES),
        ],
    );
    let mut r = resolve("curseforge", "67217", "432");
    r.game_version = Some("1.21.1".into());
    let d = p.resolve(&r).await.unwrap().download.unwrap();
    assert_eq!(
        (
            d.project_id.as_str(),
            d.version_id.as_str(),
            d.name.as_str()
        ),
        ("67217", "9004", "Sample Toolkit")
    );
    assert_eq!(
        (d.filename.as_str(), d.size),
        ("sample-toolkit-2.0.jar", Some(54321))
    );
    assert_eq!(
        d.hashes,
        FileHashes {
            sha1: Some("a".repeat(40)),
            md5: Some("b".repeat(32)),
            ..Default::default()
        }
    );
    assert!(d.warnings.iter().any(|w| w.contains("SHA-1/MD5")));
    assert_eq!(mock.params(1)["pageSize"], "50");
    assert_eq!(mock.params(1)["gameVersion"], "1.21.1");
    mock.assert_finished(2);
}

#[tokio::test]
async fn curseforge_hash_algo_mapping_is_independent_of_order_or_missing_algorithm() {
    for (raw, expected) in [
        (
            json!([{"value":"a".repeat(40),"algo":1}]),
            FileHashes {
                sha1: Some("a".repeat(40)),
                ..Default::default()
            },
        ),
        (
            json!([{"value":"B".repeat(32),"algo":2}]),
            FileHashes {
                md5: Some("B".repeat(32)),
                ..Default::default()
            },
        ),
        (
            json!([{"value":"B".repeat(32),"algo":2},{"value":"A".repeat(40),"algo":1}]),
            FileHashes {
                sha1: Some("A".repeat(40)),
                md5: Some("B".repeat(32)),
                ..Default::default()
            },
        ),
        (json!([]), FileHashes::default()),
        (Value::Null, FileHashes::default()),
    ] {
        let mut file = value(CF_FILES)["data"][4].clone();
        file["hashes"] = raw;
        let (p, mock) = provider(
            "curseforge",
            vec![
                Step::ok("/v1/mods/67217", CF_PROJECT),
                Step::ok(
                    "/v1/mods/67217/files/9004",
                    json!({"data":file}).to_string(),
                ),
            ],
        );
        let mut r = resolve("curseforge", "67217", "432");
        r.version_id = Some("9004".into());
        assert_eq!(
            p.resolve(&r).await.unwrap().download.unwrap().hashes,
            expected
        );
        mock.assert_finished(2);
    }
}

#[tokio::test]
async fn curseforge_malformed_or_duplicate_hashes_are_not_silently_dropped() {
    for raw in [
        json!({"value":"a".repeat(40),"algo":1}),
        json!([{"value":"","algo":1}]),
        json!([{"value":"a".repeat(39),"algo":1}]),
        json!([{"value":"g".repeat(32),"algo":2}]),
        json!([{"value":123,"algo":2}]),
        json!([{"algo":1}]),
        json!([{"value":"a".repeat(40),"algo":"1"}]),
        json!([{"value":"a".repeat(64),"algo":3}]),
        json!([{"value":"a".repeat(40),"algo":1},{"value":"b".repeat(40),"algo":1}]),
        json!([{"value":"a".repeat(32),"algo":2},{"value":"b".repeat(32),"algo":2}]),
    ] {
        let mut file = value(CF_FILES)["data"][4].clone();
        file["hashes"] = raw.clone();
        let (p, mock) = provider(
            "curseforge",
            vec![
                Step::ok("/v1/mods/67217", CF_PROJECT),
                Step::ok(
                    "/v1/mods/67217/files/9004",
                    json!({"data":file}).to_string(),
                ),
            ],
        );
        let mut r = resolve("curseforge", "67217", "432");
        r.version_id = Some("9004".into());
        assert!(p.resolve(&r).await.is_err(), "{raw}");
        mock.assert_finished(2);
    }
}

#[tokio::test]
async fn modrinth_malformed_hashes_are_not_downgraded_to_unverified_downloads() {
    for raw in [
        json!([]),
        json!("invalid"),
        json!({"sha512":"a".repeat(128),"sha1":""}),
        json!({"sha512":"a".repeat(128),"sha1":"b".repeat(39)}),
        json!({"sha512":"a".repeat(128),"sha1":"x".repeat(40)}),
        json!({"sha512":"a".repeat(128),"sha1":123}),
        json!({"sha512":"invalid","sha1":"b".repeat(40)}),
    ] {
        let mut versions = value(MR_VERSIONS);
        for version in versions.as_array_mut().unwrap() {
            for file in version["files"].as_array_mut().unwrap() {
                file["hashes"] = raw.clone();
            }
        }
        let (p, mock) = provider(
            "modrinth",
            vec![Step::ok(
                "/v2/project/A1b2C3d4/version",
                versions.to_string(),
            )],
        );
        assert!(
            p.resolve(&resolve("modrinth", "A1b2C3d4", "minecraft"))
                .await
                .is_err(),
            "{raw}"
        );
        mock.assert_finished(1);
    }
}

#[tokio::test]
async fn curseforge_explicit_file_parses_single_data_object() {
    let response = json!({"data":value(CF_FILES)["data"][4]}).to_string();
    let (p, mock) = provider(
        "curseforge",
        vec![
            Step::ok("/v1/mods/67217", CF_PROJECT),
            Step::ok("/v1/mods/67217/files/9004", response),
        ],
    );
    let mut r = resolve("curseforge", "67217", "432");
    r.version_id = Some("9004".into());
    assert_eq!(
        p.resolve(&r).await.unwrap().download.unwrap().version_id,
        "9004"
    );
    mock.assert_finished(2);
}

#[tokio::test]
async fn curseforge_pinned_file_and_project_must_match_before_issuing_a_resolution() {
    let mut project = value(CF_PROJECT);
    project["data"]["id"] = json!(99);
    let mut file = value(CF_FILES)["data"][4].clone();
    file["id"] = json!(9005);
    let mut r = resolve("curseforge", "67217", "432");
    r.version_id = Some("9004".into());
    for steps in [
        vec![Step::ok("/v1/mods/67217", project.to_string())],
        vec![
            Step::ok("/v1/mods/67217", CF_PROJECT),
            Step::ok(
                "/v1/mods/67217/files/9004",
                json!({"data":file}).to_string(),
            ),
        ],
    ] {
        let count = steps.len();
        let (p, mock) = provider("curseforge", steps);
        assert!(matches!(p.resolve(&r).await, Err(AppError::Conflict(_))));
        mock.assert_finished(count);
    }
}

#[tokio::test]
async fn curseforge_disabled_distribution_stops_before_requesting_files() {
    let mut project = value(CF_PROJECT);
    project["data"]["allowModDistribution"] = json!(false);
    let (p, mock) = provider(
        "curseforge",
        vec![Step::ok("/v1/mods/67217", project.to_string())],
    );
    let result = p
        .resolve(&resolve("curseforge", "67217", "432"))
        .await
        .unwrap();
    assert_eq!(result.status.state, SourceState::ManualDownload);
    assert!(result.download.is_none());
    assert!(result.homepage_url.is_some());
    mock.assert_finished(1);
}

#[tokio::test]
async fn curseforge_null_download_link_is_manual_and_foreign_file_is_rejected() {
    for foreign in [false, true] {
        let mut file = value(CF_FILES)["data"][4].clone();
        if foreign {
            file["modId"] = json!(99);
        } else {
            file["downloadUrl"] = Value::Null;
        }
        let (p, mock) = provider(
            "curseforge",
            vec![
                Step::ok("/v1/mods/67217", CF_PROJECT),
                Step::ok(
                    "/v1/mods/67217/files/9004",
                    json!({"data":file}).to_string(),
                ),
            ],
        );
        let mut r = resolve("curseforge", "67217", "432");
        r.version_id = Some("9004".into());
        let result = p.resolve(&r).await;
        if foreign {
            assert!(matches!(result, Err(AppError::NotFound(_))));
        } else {
            let result = result.unwrap();
            assert_eq!(result.status.state, SourceState::ManualDownload);
            assert!(result.download.is_none());
        }
        mock.assert_finished(2);
    }
}

#[tokio::test]
async fn nexus_search_maps_numeric_lookup_and_keeps_key_out_of_url() {
    let (p, mock) = provider(
        "nexus",
        vec![Step::ok(
            "/v1/games/skyrimspecialedition/mods/123.json",
            NX_MOD,
        )],
    );
    let mut q = query("nexus", "skyrimspecialedition");
    q.query = "123".into();
    let result = p.search(&q).await.unwrap();
    let m = &result.mods[0];
    assert_eq!(
        (m.project_id.as_str(), m.name.as_str(), m.summary.as_str()),
        (
            "skyrimspecialedition/123",
            "Sample Skyrim Tools",
            "Sample tools for Skyrim"
        )
    );
    assert_eq!(m.author.as_deref(), Some("SkyrimAuthor"));
    assert_eq!(m.downloads, Some(4200));
    assert_eq!(
        m.homepage_url.as_deref(),
        Some("https://www.nexusmods.com/skyrimspecialedition/mods/123")
    );
    assert_eq!(result.sources[0].state, SourceState::Limited);
    assert!(mock.params(0).is_empty());
    mock.assert_finished(1);
}

#[tokio::test]
async fn nexus_search_maps_text_results_with_scoped_identity_and_pagination_message() {
    let (p, mock) = provider("nexus", vec![Step::ok(NX_SEARCH_PATH, NX_SEARCH).post()]);
    let mut q = query("nexus", "SKYRIMSPECIALEDITION");
    q.query = "sample".into();
    q.offset = 4;
    q.limit = 2;
    let result = p.search(&q).await.unwrap();

    assert_eq!(result.mods.len(), 2);
    assert_eq!(
        result
            .mods
            .iter()
            .map(|m| (m.project_id.as_str(), m.game_id.as_deref()))
            .collect::<Vec<_>>(),
        [
            ("skyrimspecialedition/123", Some("skyrimspecialedition")),
            ("skyrimspecialedition/456", Some("skyrimspecialedition")),
        ]
    );
    assert_eq!(
        result.mods[0].homepage_url.as_deref(),
        Some("https://www.nexusmods.com/skyrimspecialedition/mods/123")
    );
    assert_eq!(result.mods[0].author.as_deref(), Some("SkyrimAuthor"));
    assert_eq!(result.mods[0].downloads, Some(4200));
    assert_eq!(result.mods[1].author, None);
    assert_eq!(result.mods[1].downloads, None);
    assert!(result.sources[0].message.contains("更多"));
    assert_nexus_search_request(&mock, 0, "skyrimspecialedition", "sample", 2, 4);
    mock.assert_finished(1);
}

#[tokio::test]
async fn nexus_text_search_keeps_fixed_query_and_parameterizes_each_page() {
    let (p, mock) = provider(
        "nexus",
        vec![
            Step::ok(NX_SEARCH_PATH, NX_SEARCH).post(),
            Step::ok(NX_SEARCH_PATH, NX_SEARCH).post(),
        ],
    );
    let mut q = query("nexus", "skyrimspecialedition");
    q.query = "armor".into();
    q.limit = 2;
    let first = p.search(&q).await.unwrap();
    q.offset = 1;
    let second = p.search(&q).await.unwrap();

    assert_eq!(first.mods.len(), 2);
    assert_eq!(second.mods.len(), 2);
    let first_query = assert_nexus_search_request(&mock, 0, "skyrimspecialedition", "armor", 2, 0);
    let second_query = assert_nexus_search_request(&mock, 1, "skyrimspecialedition", "armor", 2, 1);
    assert_eq!(first_query, second_query);
    mock.assert_finished(2);
}

#[tokio::test]
async fn nexus_text_search_rejects_graphql_errors_even_with_partial_data() {
    let body = json!({
        "errors": [{"message": format!("remote secret {TEST_KEY}")}],
        "data": {
            "mods": {
                "totalCount": 1,
                "nodes": [{
                    "modId": 123,
                    "name": "Should not be returned",
                    "summary": "",
                    "game": {"domainName": "skyrimspecialedition"}
                }]
            }
        }
    });
    let (p, mock) = provider(
        "nexus",
        vec![Step::ok(NX_SEARCH_PATH, body.to_string()).post()],
    );
    let q = query("nexus", "skyrimspecialedition");
    let error = p.search(&q).await.unwrap_err();
    assert!(!format!("{error:?}").contains(TEST_KEY));
    mock.assert_finished(1);
}

#[tokio::test]
async fn nexus_text_search_rejects_malformed_cross_game_and_duplicate_nodes() {
    let mut cases = Vec::new();

    let mut malformed = value(NX_SEARCH);
    malformed["data"]["mods"]["nodes"][0]["modId"] = json!("123");
    cases.push(("malformed", malformed));

    let mut cross_game = value(NX_SEARCH);
    cross_game["data"]["mods"]["nodes"][0]["game"]["domainName"] = json!("fallout4");
    cases.push(("cross-game", cross_game));

    let mut duplicate = value(NX_SEARCH);
    duplicate["data"]["mods"]["nodes"][1]["modId"] = json!(123);
    cases.push(("duplicate", duplicate));

    for (label, body) in cases {
        let (p, mock) = provider(
            "nexus",
            vec![Step::ok(NX_SEARCH_PATH, body.to_string()).post()],
        );
        let q = query("nexus", "skyrimspecialedition");
        assert!(p.search(&q).await.is_err(), "{label}");
        mock.assert_finished(1);
    }
}

#[tokio::test]
async fn nexus_text_search_rejects_inconsistent_counts_but_accepts_empty_pages() {
    let mut too_many = value(NX_SEARCH);
    too_many["data"]["mods"]["totalCount"] = json!(8);
    let (p, mock) = provider(
        "nexus",
        vec![Step::ok(NX_SEARCH_PATH, too_many.to_string()).post()],
    );
    let mut q = query("nexus", "skyrimspecialedition");
    q.limit = 1;
    assert!(p.search(&q).await.is_err());
    mock.assert_finished(1);

    let mut past_total = value(NX_SEARCH);
    past_total["data"]["mods"]["totalCount"] = json!(5);
    let (p, mock) = provider(
        "nexus",
        vec![Step::ok(NX_SEARCH_PATH, past_total.to_string()).post()],
    );
    q.limit = 20;
    q.offset = 4;
    assert!(p.search(&q).await.is_err());
    mock.assert_finished(1);

    for total_count in [0, 2] {
        let empty = json!({
            "data": {"mods": {"totalCount": total_count, "nodes": []}}
        });
        let (p, mock) = provider(
            "nexus",
            vec![Step::ok(NX_SEARCH_PATH, empty.to_string()).post()],
        );
        q.offset = if total_count == 0 { 0 } else { 20 };
        let result = p.search(&q).await.unwrap();
        assert!(result.mods.is_empty());
        mock.assert_finished(1);
    }
}

#[tokio::test]
async fn nexus_text_search_skips_network_for_missing_or_unsupported_queries() {
    let cases = [
        ("", None, None, "empty query"),
        ("   ", None, None, "whitespace query"),
        ("0", None, None, "zero ID"),
        ("18446744073709551616", None, None, "overflow ID"),
        ("sample", Some("1.21.1"), None, "game version filter"),
        ("sample", None, Some("fabric"), "loader filter"),
    ];
    for (query_text, game_version, loader, label) in cases {
        let (p, mock) = provider("nexus", Vec::new());
        let mut q = query("nexus", "skyrimspecialedition");
        q.query = query_text.into();
        q.game_version = game_version.map(str::to_owned);
        q.loader = loader.map(str::to_owned);
        let result = p.search(&q).await.unwrap();
        assert!(result.mods.is_empty(), "{label}");
        if game_version.is_some() || loader.is_some() {
            assert_eq!(result.sources[0].state, SourceState::Unsupported, "{label}");
        }
        mock.assert_finished(0);
    }

    let (p, mock) = provider("nexus", Vec::new());
    let mut q = query("nexus", "skyrimspecialedition");
    q.game_ids.remove("nexus");
    let result = p.search(&q).await.unwrap();
    assert_eq!(result.sources[0].state, SourceState::NeedsConfiguration);
    mock.assert_finished(0);
}

#[tokio::test]
async fn nexus_resolve_maps_primary_file_and_uppercase_uri_download_response() {
    let (p, mock) = provider(
        "nexus",
        vec![
            Step::ok(
                "/v1/games/skyrimspecialedition/mods/123/files.json",
                NX_FILES,
            ),
            Step::ok(
                "/v1/games/skyrimspecialedition/mods/123/files/700/download_link.json",
                NX_LINKS,
            ),
        ],
    );
    let d = p
        .resolve(&resolve("nexus", "123", "skyrimspecialedition"))
        .await
        .unwrap()
        .download
        .unwrap();
    assert_eq!(
        (
            d.version_id.as_str(),
            d.version.as_str(),
            d.filename.as_str(),
            d.size
        ),
        ("700", "1.2.3", "sample-tools-700.zip", Some(12345))
    );
    assert!(d.download_url.contains("sample-tools-700.zip?expires="));
    assert_eq!(d.project_id, "skyrimspecialedition/123");
    assert_eq!(d.hashes, FileHashes::default());
    mock.assert_finished(2);
}

#[tokio::test]
async fn nexus_explicit_optional_file_is_resolved_without_choosing_main_file() {
    let mut links = value(NX_LINKS);
    links[0]["URI"] = json!("https://nexus-cdn.example.org/sample-tools-701.zip");
    let (p, mock) = provider(
        "nexus",
        vec![
            Step::ok(
                "/v1/games/skyrimspecialedition/mods/123/files.json",
                NX_FILES,
            ),
            Step::ok(
                "/v1/games/skyrimspecialedition/mods/123/files/701/download_link.json",
                links.to_string(),
            ),
        ],
    );
    let mut r = resolve("nexus", "skyrimspecialedition/123", "skyrimspecialedition");
    r.game_id = None;
    r.version_id = Some("701".into());
    assert_eq!(
        p.resolve(&r).await.unwrap().download.unwrap().filename,
        "sample-tools-701.zip"
    );
    mock.assert_finished(2);
}

#[tokio::test]
async fn nexus_scoped_project_rejects_a_conflicting_game_before_network() {
    let (p, mock) = provider("nexus", Vec::new());
    let request = resolve("nexus", "skyrimspecialedition/123", "fallout4");
    assert!(matches!(
        p.resolve(&request).await,
        Err(AppError::Conflict(_))
    ));
    mock.assert_finished(0);
}

#[tokio::test]
async fn nexus_archived_or_ambiguous_files_stop_before_download_link_request() {
    for archived in [false, true] {
        let mut files = value(NX_FILES);
        let mut r = resolve("nexus", "123", "skyrimspecialedition");
        if archived {
            r.version_id = Some("702".into());
        } else {
            let mut duplicate = files["files"][0].clone();
            duplicate["file_id"] = json!(703);
            files["files"].as_array_mut().unwrap().push(duplicate);
        }
        let (p, mock) = provider(
            "nexus",
            vec![Step::ok(
                "/v1/games/skyrimspecialedition/mods/123/files.json",
                files.to_string(),
            )],
        );
        let result = p.resolve(&r).await.unwrap();
        assert_eq!(result.status.state, SourceState::ManualDownload);
        assert!(result.download.is_none());
        mock.assert_finished(1);
    }
}

#[tokio::test]
async fn nexus_forbidden_or_empty_links_degrade_without_exposing_remote_body() {
    for denied in [false, true] {
        let link = if denied {
            Step::forbidden("/v1/games/skyrimspecialedition/mods/123/files/700/download_link.json")
        } else {
            Step::ok(
                "/v1/games/skyrimspecialedition/mods/123/files/700/download_link.json",
                "[]",
            )
        };
        let (p, mock) = provider(
            "nexus",
            vec![
                Step::ok(
                    "/v1/games/skyrimspecialedition/mods/123/files.json",
                    NX_FILES,
                ),
                link,
            ],
        );
        let result = p
            .resolve(&resolve("nexus", "123", "skyrimspecialedition"))
            .await
            .unwrap();
        assert_eq!(result.status.state, SourceState::ManualDownload);
        assert!(result.download.is_none());
        let public = super::resolutions::Resolutions::default()
            .publish(super::resolutions::ResolutionScope::Desktop, result)
            .unwrap();
        assert!(!serde_json::to_string(&public).unwrap().contains(TEST_KEY));
        mock.assert_finished(2);
    }
}

#[tokio::test]
async fn malformed_response_shapes_are_errors_for_all_four_providers() {
    for (id, game, path, response) in [
        ("modrinth", "minecraft", "/v2/search", "{\"hits\":{}}"),
        (
            "thunderstore",
            "valheim",
            "/api/cyberstorm/listing/valheim/",
            "{\"results\":[{\"namespace\":\"Team\"}]}",
        ),
        (
            "curseforge",
            "432",
            "/v1/mods/search",
            "{\"data\":[{\"id\":\"67217\",\"name\":\"Wrong numeric type\"}]}",
        ),
        (
            "nexus",
            "skyrimspecialedition",
            "/v1/games/skyrimspecialedition/mods/123.json",
            "{\"summary\":\"Missing name\"}",
        ),
    ] {
        let (p, mock) = provider(id, vec![Step::ok(path, response)]);
        let mut q = query(id, game);
        if id == "nexus" {
            q.query = "123".into();
        }
        assert!(p.search(&q).await.is_err(), "{id}");
        mock.assert_finished(1);
    }
}

#[tokio::test]
async fn unsafe_resolved_urls_are_rejected_for_all_four_providers() {
    let unsafe_url = "https://user:password@cdn.example.org/mod.zip";
    let mut mr = value(MR_VERSIONS);
    mr[3]["files"][1]["url"] = json!(unsafe_url);
    let mut ts = value(TS_VERSION);
    ts["download_url"] = json!(unsafe_url);
    ts["latest_version_number"] = json!("1.2.3");
    let mut cf = value(CF_FILES);
    cf["data"][4]["downloadUrl"] = json!(unsafe_url);
    let mut nx = value(NX_LINKS);
    nx[0]["URI"] = json!(unsafe_url);
    let cases = vec![
        (
            "modrinth",
            "A1b2C3d4",
            "minecraft",
            vec![Step::ok("/v2/project/A1b2C3d4/version", mr.to_string())],
        ),
        (
            "thunderstore",
            "Team/SampleMod",
            "valheim",
            vec![Step::ok(
                "/api/cyberstorm/listing/valheim/Team/SampleMod/",
                ts.to_string(),
            )],
        ),
        (
            "curseforge",
            "67217",
            "432",
            vec![
                Step::ok("/v1/mods/67217", CF_PROJECT),
                Step::ok("/v1/mods/67217/files", cf.to_string()),
            ],
        ),
        (
            "nexus",
            "123",
            "skyrimspecialedition",
            vec![
                Step::ok(
                    "/v1/games/skyrimspecialedition/mods/123/files.json",
                    NX_FILES,
                ),
                Step::ok(
                    "/v1/games/skyrimspecialedition/mods/123/files/700/download_link.json",
                    nx.to_string(),
                ),
            ],
        ),
    ];
    for (id, project, game, steps) in cases {
        let count = steps.len();
        let (p, mock) = provider(id, steps);
        let mut r = resolve(id, project, game);
        if id == "modrinth" {
            r.loader = Some("fabric".into());
            r.game_version = Some("1.21.1".into());
        }
        if id == "curseforge" {
            r.game_version = Some("1.21.1".into());
        }
        assert!(
            matches!(p.resolve(&r).await, Err(AppError::Denied(_))),
            "{id}"
        );
        mock.assert_finished(count);
    }
}

#[tokio::test]
async fn aggregation_keeps_successful_fixture_results_when_one_provider_is_malformed() {
    let (mr, mr_mock) = provider("modrinth", vec![Step::ok("/v2/search", MR_SEARCH)]);
    let (ts, ts_mock) = provider(
        "thunderstore",
        vec![Step::ok("/api/cyberstorm/listing/valheim/", TS_PAGE_2)],
    );
    let (cf, cf_mock) = provider("curseforge", vec![Step::ok("/v1/mods/search", CF_SEARCH)]);
    let (nx, nx_mock) = provider(
        "nexus",
        vec![Step::ok(
            "/v1/games/skyrimspecialedition/mods/123.json",
            "{\"name\":null}",
        )],
    );
    let sources = Sources {
        providers: vec![Arc::new(mr), Arc::new(ts), Arc::new(cf), Arc::new(nx)],
    };
    let q = SearchQuery {
        query: "123".into(),
        game_ids: BTreeMap::from([
            ("modrinth".into(), "minecraft".into()),
            ("thunderstore".into(), "valheim".into()),
            ("curseforge".into(), "432".into()),
            ("nexus".into(), "skyrimspecialedition".into()),
        ]),
        ..Default::default()
    };
    let result = sources.search(&q).await.unwrap();
    assert_eq!(
        result
            .mods
            .iter()
            .map(|m| m.source_id.as_str())
            .collect::<Vec<_>>(),
        ["modrinth", "thunderstore", "curseforge"]
    );
    assert_eq!(result.sources[3].state, SourceState::Unavailable);
    assert_eq!(result.sources.len(), 4);
    for mock in [mr_mock, ts_mock, cf_mock, nx_mock] {
        mock.assert_finished(1);
    }
}
