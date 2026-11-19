use super::*;
use crate::{
    db::Db,
    error::{AppError, AppResult},
    settings::{SecretStore, Settings},
};
use axum::{
    body::Body,
    extract::{Request, State},
    response::Response,
    Router,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, sync::Notify, task::JoinHandle};

const TEST_KEY: &str = "mock-brave-secret-not-for-output";

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

fn settings_fixture(key: Option<&str>) -> (tempfile::TempDir, Settings) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("settings.sqlite")).unwrap();
    let settings = Settings::with_secret_store(db, Arc::new(MockSecrets::default()));
    if let Some(key) = key {
        settings.write_secret(SECRET_REF, key).unwrap();
    }
    (dir, settings)
}

#[derive(Clone, Debug)]
struct CapturedRequest {
    method: String,
    uri: String,
    token: Option<String>,
    authorization: Option<String>,
    cookie: Option<String>,
}

struct MockState {
    status: u16,
    body: String,
    delay: Duration,
    redirect_url: Option<String>,
    requests: Mutex<Vec<CapturedRequest>>,
    received: Notify,
}

struct MockSearch {
    endpoint: String,
    state: Arc<MockState>,
    task: JoinHandle<()>,
}

impl MockSearch {
    async fn new(status: u16, body: impl Into<String>, delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let endpoint = format!("http://{address}/res/v1/web/search");
        let state = Arc::new(MockState {
            status,
            body: body.into(),
            delay,
            redirect_url: (status == 302).then(|| format!("http://{address}/redirect-target")),
            requests: Mutex::new(Vec::new()),
            received: Notify::new(),
        });
        let router = Router::new().fallback(reply).with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            endpoint,
            state,
            task,
        }
    }

    fn transport(&self) -> SearchHttp {
        SearchHttp {
            client: http::api_client().unwrap(),
            mock_endpoint: Some(self.endpoint.clone()),
        }
    }

    fn requests(&self) -> Vec<CapturedRequest> {
        self.state.requests.lock().unwrap().clone()
    }
}

impl Drop for MockSearch {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn reply(State(state): State<Arc<MockState>>, request: Request) -> Response {
    let headers = request.headers();
    let capture = CapturedRequest {
        method: request.method().to_string(),
        uri: request.uri().to_string(),
        token: headers
            .get("x-subscription-token")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
        authorization: headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
        cookie: headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
    };
    state.requests.lock().unwrap().push(capture);
    state.received.notify_one();
    tokio::time::sleep(state.delay).await;
    let mut response = Response::builder().status(state.status);
    if let Some(location) = &state.redirect_url {
        response = response.header("location", location);
    }
    response
        .header("content-type", "application/json")
        .body(Body::from(state.body.clone()))
        .unwrap()
}

fn hit(title: impl Into<String>, url: impl Into<String>, description: impl Into<String>) -> Value {
    json!({
        "title": title.into(),
        "url": url.into(),
        "description": description.into()
    })
}

fn response(hits: Vec<Value>) -> String {
    json!({
        "type": "search",
        "web": {"type": "search", "results": hits}
    })
    .to_string()
}

fn query(value: impl Into<String>, count: u8) -> WebSearchQuery {
    WebSearchQuery {
        query: value.into(),
        count,
    }
}

#[test]
fn validates_query_limits_and_public_url_terms_without_false_path_matches() {
    query("x", 1).validate().unwrap();
    query("x".repeat(MAX_QUERY_CHARS), 10).validate().unwrap();
    query("界".repeat(MAX_QUERY_CHARS), 5).validate().unwrap();
    query("word ".repeat(74) + "word", 5).validate().unwrap();
    query("https://docs.rs/serde/latest/serde/", 5)
        .validate()
        .unwrap();
    query("find https://example.com/search?q=/guide/", 5)
        .validate()
        .unwrap();

    for invalid in [
        query("", 1),
        query("x".repeat(MAX_QUERY_CHARS + 1), 5),
        query("word ".repeat(75) + "word", 5),
        query("x", 0),
        query("x", 11),
        query("/Users/alice/private.txt", 5),
        query(r#"C:\Users\alice\private.txt"#, 5),
        query("file:///etc/passwd", 5),
        query("https://localhost/private", 5),
        query("https://127.0.0.1/private", 5),
        query("https://user:password@example.com/private", 5),
    ] {
        assert!(
            invalid.validate().is_err(),
            "unexpectedly accepted: {}",
            invalid.query
        );
    }

    assert!(serde_json::from_value::<WebSearchQuery>(json!({
        "query": "public terms",
        "count": 5,
        "unexpected": true
    }))
    .is_err());
    assert_eq!(
        serde_json::from_value::<WebSearchQuery>(json!({"query":"public terms"}))
            .unwrap()
            .count,
        5
    );
}

#[test]
fn result_urls_only_allow_public_http_urls() {
    assert!(safe_url("https://www.rust-lang.org/learn", None).is_some());
    assert!(safe_url("http://example.org/public/path", None).is_some());
    for url in [
        "file:///etc/passwd",
        "https://localhost/private",
        "https://127.0.0.1/private",
        "https://10.0.0.1/private",
        "https://100.64.0.1/private",
        "https://0.1.2.3/private",
        "https://192.0.2.1/private",
        "https://[::1]/private",
        "https://[fc00::1]/private",
        "https://router.internal/private",
        "https://localhost./private",
        "https://user:password@example.com/private",
    ] {
        assert!(
            safe_url(url, None).is_none(),
            "unexpectedly accepted: {url}"
        );
    }
}

#[tokio::test]
async fn authenticates_only_with_subscription_header_and_sanitizes_echoed_secret() {
    let (_dir, settings) = settings_fixture(Some(TEST_KEY));
    let body = response(vec![hit(
        format!(
            "Key {TEST_KEY}; encoded key {0}",
            "mock%2Dbrave%2Dsecret%2Dnot%2Dfor%2Doutput"
        ),
        "https://www.rust-lang.org/learn",
        format!("A snippet repeats {TEST_KEY}"),
    )]);
    let mock = MockSearch::new(200, body, Duration::ZERO).await;
    let result = search_inner(
        &settings,
        &query("public Rust documentation", 5),
        &CancellationToken::new(),
        Some(&mock.transport()),
    )
    .await
    .unwrap();

    let value = result.into_tool_value().unwrap();
    assert_eq!(value["provider"], "brave");
    assert_eq!(value["status"], "available");
    assert_eq!(value["untrusted"], true);
    assert_eq!(value["results"].as_array().unwrap().len(), 1);
    assert_eq!(value.as_object().unwrap().len(), 6);
    assert_eq!(value["results"][0].as_object().unwrap().len(), 3);
    let serialized = serde_json::to_string(&value).unwrap();
    assert!(!serialized.contains(TEST_KEY));
    assert!(!serialized.contains("mock%2Dbrave"));
    assert_eq!(value["results"][0]["title"], "[REDACTED]");
    assert_eq!(value["results"][0]["snippet"], "[REDACTED]");

    let requests = mock.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "GET");
    assert_eq!(request.token.as_deref(), Some(TEST_KEY));
    assert_eq!(request.authorization, None);
    assert_eq!(request.cookie, None);
    assert!(!request.uri.contains(TEST_KEY));
    assert!(request.uri.starts_with("/res/v1/web/search?"));
    let params: HashMap<_, _> =
        url::form_urlencoded::parse(request.uri.split_once('?').unwrap().1.as_bytes())
            .into_owned()
            .collect();
    assert_eq!(
        params.get("q").map(String::as_str),
        Some("public Rust documentation")
    );
    assert_eq!(params.get("count").map(String::as_str), Some("5"));
    assert_eq!(params.get("result_filter").map(String::as_str), Some("web"));
    assert_eq!(
        params.get("text_decorations").map(String::as_str),
        Some("false")
    );
    assert_eq!(
        params.get("safesearch").map(String::as_str),
        Some("moderate")
    );
    assert!(!params.values().any(|value| value.contains(TEST_KEY)));

    for secret_query in [
        TEST_KEY.to_owned(),
        "mock%2Dbrave%2Dsecret%2Dnot%2Dfor%2Doutput".to_owned(),
        format!("find public games {TEST_KEY} query"),
        "find public games mock%2Dbrave%2Dsecret%2Dnot%2Dfor%2Doutput query".to_owned(),
    ] {
        let result = search_inner(
            &settings,
            &query(secret_query, 5),
            &CancellationToken::new(),
            Some(&mock.transport()),
        )
        .await;
        assert!(matches!(result, Err(AppError::Denied(_))));
    }
    assert_eq!(mock.requests().len(), 1, "credentials must never enter q");
}

#[test]
fn production_request_uses_the_fixed_https_endpoint() {
    let transport = SearchHttp::new().unwrap();
    let request = transport
        .request(&query("public terms", 5), &SearchSecret(TEST_KEY.into()))
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(request.url().scheme(), "https");
    assert_eq!(request.url().as_str().split('?').next(), Some(ENDPOINT));
    assert_eq!(
        request.headers().get("x-subscription-token").unwrap(),
        TEST_KEY
    );
    assert!(request
        .headers()
        .get("x-subscription-token")
        .unwrap()
        .is_sensitive());
}

#[tokio::test]
async fn missing_key_returns_needs_configuration_without_network_request() {
    let (_dir, settings) = settings_fixture(None);
    let mock = MockSearch::new(200, response(vec![]), Duration::ZERO).await;
    let result = search_inner(
        &settings,
        &query("public query", 5),
        &CancellationToken::new(),
        Some(&mock.transport()),
    )
    .await
    .unwrap()
    .into_tool_value()
    .unwrap();

    assert_eq!(result["provider"], "brave");
    assert_eq!(result["status"], "needs_configuration");
    assert_eq!(result["results"], json!([]));
    assert_eq!(mock.requests().len(), 0);
}

#[tokio::test]
async fn search_endpoint_redirect_is_not_followed() {
    let (_dir, settings) = settings_fixture(Some(TEST_KEY));
    let mock = MockSearch::new(302, "redirect", Duration::ZERO).await;
    let result = search_inner(
        &settings,
        &query("redirect handling", 5),
        &CancellationToken::new(),
        Some(&mock.transport()),
    )
    .await
    .unwrap()
    .into_tool_value()
    .unwrap();

    assert_eq!(result["status"], "unavailable");
    assert_eq!(result["results"], json!([]));
    assert_eq!(mock.requests().len(), 1);
}

#[tokio::test]
async fn limits_provider_results_to_the_requested_count() {
    let (_dir, settings) = settings_fixture(Some(TEST_KEY));
    let mock = MockSearch::new(
        200,
        response(vec![
            hit("One", "https://example.org/one", "First"),
            hit("Two", "https://example.org/two", "Second"),
            hit("Three", "https://example.org/three", "Third"),
        ]),
        Duration::ZERO,
    )
    .await;
    let result = search_inner(
        &settings,
        &query("count cap", 1),
        &CancellationToken::new(),
        Some(&mock.transport()),
    )
    .await
    .unwrap()
    .into_tool_value()
    .unwrap();

    assert_eq!(result["status"], "available");
    assert_eq!(result["results"].as_array().unwrap().len(), 1);
    assert_eq!(result["results"][0]["title"], "One");
    assert_eq!(result["truncated"], true);
    assert_eq!(mock.requests().len(), 1);
}

#[tokio::test]
async fn real_http_error_statuses_never_become_success_or_echo_provider_body() {
    for (status, expected) in [
        (401, "needs_configuration"),
        (403, "needs_configuration"),
        (404, "unavailable"),
        (422, "unavailable"),
        (429, "unavailable"),
        (500, "unavailable"),
        (503, "unavailable"),
    ] {
        let (_dir, settings) = settings_fixture(Some(TEST_KEY));
        let mock = MockSearch::new(
            status,
            json!({"error": format!("provider echoed {TEST_KEY}")}).to_string(),
            Duration::ZERO,
        )
        .await;
        let result = search_inner(
            &settings,
            &query(format!("status test {status}"), 5),
            &CancellationToken::new(),
            Some(&mock.transport()),
        )
        .await
        .unwrap()
        .into_tool_value()
        .unwrap();

        assert_eq!(result["status"], expected, "HTTP {status}");
        assert_eq!(result["results"], json!([]), "HTTP {status}");
        assert!(result["untrusted"].as_bool().unwrap());
        assert!(!result.to_string().contains(TEST_KEY));
        assert_eq!(mock.requests().len(), 1, "HTTP {status}");
    }
}

#[tokio::test]
async fn cancellation_interrupts_an_inflight_search() {
    let (_dir, settings) = settings_fixture(Some(TEST_KEY));
    let mock = MockSearch::new(
        200,
        response(vec![hit("Title", "https://example.org/", "Snippet")]),
        Duration::from_secs(5),
    )
    .await;
    let transport = mock.transport();
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let request_started = mock.state.received.notified();
    let search = tokio::spawn(async move {
        search_inner(
            &settings,
            &query("cancel in flight", 5),
            &task_cancel,
            Some(&transport),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), request_started)
        .await
        .expect("mock did not receive request");
    cancel.cancel();
    let result = search.await.unwrap();
    assert!(matches!(result, Err(AppError::Cancelled)));
    assert_eq!(mock.requests().len(), 1);
}

#[tokio::test]
async fn caps_result_shape_and_never_follows_returned_urls() {
    let (_dir, settings) = settings_fixture(Some(TEST_KEY));
    let hits = (0..10)
        .map(|index| {
            let url = if index == 1 {
                format!("https://example.org/0/{}", "p".repeat(1800))
            } else {
                format!("https://example.org/{index}/{}", "p".repeat(1800))
            };
            hit(
                format!("title-{index}-{}", "t".repeat(700)),
                url,
                "s".repeat(4000),
            )
        })
        .collect();
    let mock = MockSearch::new(200, response(hits), Duration::ZERO).await;
    let result = search_inner(
        &settings,
        &query("bounded results", 10),
        &CancellationToken::new(),
        Some(&mock.transport()),
    )
    .await
    .unwrap()
    .into_tool_value()
    .unwrap();

    let results = result["results"].as_array().unwrap();
    assert!(!results.is_empty());
    assert!(results.len() <= 10);
    assert!(result["truncated"].as_bool().unwrap());
    assert!(result["untrusted"].as_bool().unwrap());
    assert!(serde_json::to_vec(&result).unwrap().len() <= MAX_TOOL_RESULT_BYTES);
    let mut seen = HashSet::new();
    for hit in results {
        assert!(hit["title"].as_str().unwrap().len() <= MAX_TITLE_BYTES);
        assert!(hit["snippet"].as_str().unwrap().len() <= MAX_SNIPPET_BYTES);
        assert!(hit["url"].as_str().unwrap().len() <= MAX_URL_BYTES);
        assert!(hit["url"]
            .as_str()
            .unwrap()
            .starts_with("https://example.org/"));
        assert!(seen.insert(hit["url"].as_str().unwrap()));
    }
    assert_eq!(mock.requests().len(), 1, "result URLs must not be fetched");
}

#[tokio::test]
async fn malformed_success_payloads_never_become_available_results() {
    for body in [
        "not json",
        r#"{"type":"error","web":{"type":"search","results":[]}}"#,
        r#"{"type":"search","web":{"type":"error","results":[]}}"#,
        r#"{"type":null,"web":{"type":"search","results":[]}}"#,
        r#"{"type":"search","web":{"type":"search","results":[{"url":"https://example.org/"}]}}"#,
        r#"{"type":"search"}"#,
    ] {
        let (_dir, settings) = settings_fixture(Some(TEST_KEY));
        let mock = MockSearch::new(200, body, Duration::ZERO).await;
        let result = search_inner(
            &settings,
            &query("malformed response case", 5),
            &CancellationToken::new(),
            Some(&mock.transport()),
        )
        .await
        .unwrap()
        .into_tool_value()
        .unwrap();
        assert_eq!(result["status"], "unavailable", "body={body}");
        assert_eq!(result["results"], json!([]), "body={body}");
    }

    let (_dir, settings) = settings_fixture(Some(TEST_KEY));
    let mock = MockSearch::new(200, r#"{"type":"search","web":null}"#, Duration::ZERO).await;
    let result = search_inner(
        &settings,
        &query("valid empty response", 5),
        &CancellationToken::new(),
        Some(&mock.transport()),
    )
    .await
    .unwrap()
    .into_tool_value()
    .unwrap();
    assert_eq!(result["status"], "available");
    assert_eq!(result["results"], json!([]));

    let (_dir, settings) = settings_fixture(Some(TEST_KEY));
    let mock = MockSearch::new(200, r#"{"web":{"results":[]}}"#, Duration::ZERO).await;
    let result = search_inner(
        &settings,
        &query("defaulted response kind", 5),
        &CancellationToken::new(),
        Some(&mock.transport()),
    )
    .await
    .unwrap()
    .into_tool_value()
    .unwrap();
    assert_eq!(result["status"], "available");
    assert_eq!(result["results"], json!([]));
}

#[tokio::test]
async fn cancellation_before_request_is_reported_without_using_credentials() {
    let (_dir, settings) = settings_fixture(Some(TEST_KEY));
    let mock = MockSearch::new(200, response(vec![]), Duration::ZERO).await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = search_inner(
        &settings,
        &query("cancelled", 5),
        &cancel,
        Some(&mock.transport()),
    )
    .await;
    assert!(matches!(result, Err(AppError::Cancelled)));
    assert_eq!(mock.requests().len(), 0);
}
