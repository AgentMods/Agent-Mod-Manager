use super::*;
use axum::{
    body::Body,
    extract::{Request, State},
    response::Response,
    Router,
};
use cache::{Config, HttpState};
use serde_json::{json, Value};
use sha2::Digest;
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Instant, SystemTime},
};
use tokio::{net::TcpListener, task::JoinHandle};

#[derive(Clone)]
struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
    delay: Duration,
}

impl Reply {
    fn ok(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            headers: vec![("content-type", "application/json".into())],
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    fn status(status: u16) -> Self {
        Self {
            status,
            ..Self::ok("remote error with fake-test-secret must not escape")
        }
    }

    fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    fn delay(mut self, duration: Duration) -> Self {
        self.delay = duration;
        self
    }
}

struct MockState {
    scripts: Mutex<HashMap<String, VecDeque<Reply>>>,
    requests: Mutex<Vec<(String, Instant)>>,
    active: AtomicUsize,
    peak: AtomicUsize,
    default: Reply,
}

struct Mock {
    endpoint: String,
    state: Arc<MockState>,
    task: JoinHandle<()>,
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Mock {
    async fn new(default: Reply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(MockState {
            scripts: Mutex::new(HashMap::new()),
            requests: Mutex::new(Vec::new()),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            default,
        });
        let router = Router::new().fallback(respond).with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            endpoint,
            state,
            task,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.endpoint)
    }

    fn script(&self, path: &str, replies: Vec<Reply>) {
        self.state
            .scripts
            .lock()
            .unwrap()
            .insert(path.into(), replies.into());
    }

    fn count(&self, path: &str) -> usize {
        self.state
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p == path)
            .count()
    }
}

async fn respond(State(state): State<Arc<MockState>>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    state
        .requests
        .lock()
        .unwrap()
        .push((path.clone(), Instant::now()));
    let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
    state.peak.fetch_max(active, Ordering::SeqCst);
    let reply = state
        .scripts
        .lock()
        .unwrap()
        .get_mut(&path)
        .and_then(VecDeque::pop_front)
        .unwrap_or_else(|| state.default.clone());
    tokio::time::sleep(reply.delay).await;
    state.active.fetch_sub(1, Ordering::SeqCst);
    let mut response = Response::builder().status(reply.status);
    for (name, value) in reply.headers {
        response = response.header(name, value);
    }
    response.body(Body::from(reply.body)).unwrap()
}

fn config() -> Config {
    Config {
        pacing: Duration::ZERO,
        wait_timeout: Duration::from_secs(3),
        fallback_backoff: Duration::from_millis(25),
        ..Config::default()
    }
}

async fn get(state: &HttpState, client: &Client, url: String) -> AppResult<Value> {
    json_with_state(client.get(url), state).await
}

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn process_cache_survives_rebuilt_provider_clients() {
    let mock = Mock::new(Reply::ok(r#"{"ok":true}"#)).await;
    let url = mock.url("/process-cache?signature=fake-signed-url");
    let first: Value = json(api_client().unwrap().get(&url)).await.unwrap();
    let second: Value = json(api_client().unwrap().get(&url)).await.unwrap();
    assert_eq!(first, second);
    assert_eq!(mock.count("/process-cache"), 1);
    // Neither cache keys nor governor keys contain a raw URL or credential.
    let request = api_client()
        .unwrap()
        .get(url)
        .bearer_auth("fake-test-secret")
        .build()
        .unwrap();
    assert_eq!(cache::request_key(&request).unwrap().len(), 32);
}

#[tokio::test]
async fn cache_scope_hashes_all_auth_headers_representation_headers_and_full_url() {
    let mock = Mock::new(Reply::ok(r#"{"ok":true}"#)).await;
    let state = HttpState::new(config());
    let url = mock.url("/scope?signature=fake-one");
    let headers = [
        ("authorization", "Bearer fake-a"),
        ("x-api-key", "fake-b"),
        ("apikey", "fake-c"),
        ("cookie", "session=fake-d"),
        ("x-tenant", "tenant-one"),
        ("accept-language", "en"),
    ];
    for reverse in [false, true] {
        let client = api_client().unwrap();
        let mut request = client.get(&url);
        let mut ordered = headers.to_vec();
        if reverse {
            ordered.reverse();
        }
        for (name, value) in ordered {
            request = request.header(name, value);
        }
        let _: Value = json_with_state(request, &state).await.unwrap();
    }
    assert_eq!(mock.count("/scope"), 1);
    for changed in 0..headers.len() {
        let client = api_client().unwrap();
        let mut request = client.get(&url);
        for (index, (name, value)) in headers.iter().enumerate() {
            request = request.header(
                *name,
                if index == changed {
                    "different-fake-value"
                } else {
                    value
                },
            );
        }
        let _: Value = json_with_state(request, &state).await.unwrap();
    }
    let _: Value = get(
        &state,
        &api_client().unwrap(),
        mock.url("/scope?signature=fake-two"),
    )
    .await
    .unwrap();
    assert_eq!(mock.count("/scope"), headers.len() + 2);
}

#[tokio::test]
async fn http_errors_invalid_json_and_deserialization_errors_are_never_cached() {
    let mock = Mock::new(Reply::ok(r#"{"ok":true}"#)).await;
    let state = HttpState::new(config());
    let client = api_client().unwrap();
    for status in [401, 403, 404, 429, 500, 503] {
        let path = format!("/error-{status}");
        mock.script(
            &path,
            vec![Reply::status(status).header("retry-after", "0")],
        );
        let error = get(&state, &client, mock.url(&path)).await.unwrap_err();
        assert!(!error.to_string().contains("fake-test-secret"));
        get(&state, &client, mock.url(&path)).await.unwrap();
        get(&state, &client, mock.url(&path)).await.unwrap();
        assert_eq!(mock.count(&path), 2);
    }
    mock.script("/invalid", vec![Reply::ok("{invalid JSON")]);
    assert!(get(&state, &client, mock.url("/invalid")).await.is_err());
    get(&state, &client, mock.url("/invalid")).await.unwrap();
    assert_eq!(mock.count("/invalid"), 2);
    mock.script(
        "/shape",
        vec![Reply::ok(r#"{"value":"wrong type"}"#), Reply::ok("3")],
    );
    assert!(
        json_with_state::<u64>(client.get(mock.url("/shape")), &state)
            .await
            .is_err()
    );
    assert_eq!(
        json_with_state::<u64>(client.get(mock.url("/shape")), &state)
            .await
            .unwrap(),
        3
    );
    assert_eq!(mock.count("/shape"), 2);
}

#[tokio::test]
async fn only_get_without_body_is_cached_and_cache_control_is_respected() {
    let mock = Mock::new(Reply::ok(r#"{"ok":true}"#)).await;
    let state = HttpState::new(config());
    let client = api_client().unwrap();
    for _ in 0..2 {
        let _: Value = json_with_state(client.post(mock.url("/post")), &state)
            .await
            .unwrap();
        let _: Value = json_with_state(client.get(mock.url("/body")).body("payload"), &state)
            .await
            .unwrap();
        let _: Value = json_with_state(
            client
                .get(mock.url("/request-no-cache"))
                .header("cache-control", "no-cache"),
            &state,
        )
        .await
        .unwrap();
    }
    for path in ["/post", "/body", "/request-no-cache"] {
        assert_eq!(mock.count(path), 2);
    }
    for (path, header, value) in [
        ("/no-store", "cache-control", "private, no-store"),
        ("/no-cache", "cache-control", "no-cache"),
        ("/stale", "cache-control", "max-age=0"),
        ("/vary", "vary", "*"),
        ("/cookie", "set-cookie", "session=fake"),
    ] {
        mock.script(path, vec![Reply::ok("{}").header(header, value); 2]);
        for _ in 0..2 {
            get(&state, &client, mock.url(path)).await.unwrap();
        }
        assert_eq!(mock.count(path), 2);
    }
}

#[tokio::test]
async fn short_expiry_lru_entry_limit_and_total_byte_limit_are_enforced() {
    let mock = Mock::new(Reply::ok(r#"{"ok":true}"#)).await;
    let state = HttpState::new(Config {
        ttl: Duration::from_millis(500),
        max_entries: 2,
        ..config()
    });
    let client = api_client().unwrap();
    for path in ["/a", "/b", "/a", "/c", "/a", "/c"] {
        get(&state, &client, mock.url(path)).await.unwrap();
    }
    assert_eq!(mock.count("/a"), 1);
    assert_eq!(mock.count("/b"), 1);
    assert_eq!(mock.count("/c"), 1);
    assert_eq!(state.cache_usage(), (2, 22));
    get(&state, &client, mock.url("/b")).await.unwrap();
    assert_eq!(mock.count("/b"), 2);
    tokio::time::sleep(Duration::from_millis(550)).await;
    get(&state, &client, mock.url("/b")).await.unwrap();
    assert_eq!(mock.count("/b"), 3);
    assert_eq!(state.cache_usage(), (1, 11));

    let state = HttpState::new(Config {
        max_bytes: 20,
        max_entries: 128,
        ..config()
    });
    for path in ["/bytes-a", "/bytes-b", "/bytes-a"] {
        get(&state, &client, mock.url(path)).await.unwrap();
    }
    assert_eq!(mock.count("/bytes-a"), 2);
    assert_eq!(state.cache_usage(), (1, 11));
}

#[tokio::test]
async fn oversized_cache_entries_are_not_retained_and_response_reads_remain_bounded() {
    let mock = Mock::new(Reply::ok(json!({"text":"x".repeat(100)}).to_string())).await;
    let state = HttpState::new(Config {
        max_entry_bytes: 64,
        ..config()
    });
    let client = api_client().unwrap();
    for _ in 0..2 {
        get(&state, &client, mock.url("/large-entry"))
            .await
            .unwrap();
    }
    assert_eq!(mock.count("/large-entry"), 2);
    assert_eq!(state.cache_usage(), (0, 0));
    mock.script(
        "/oversized-response",
        vec![Reply::ok(" ".repeat(cache::MAX_RESPONSE_BYTES + 1))],
    );
    let error = get(&state, &client, mock.url("/oversized-response"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("超过大小限制"));
    assert_eq!(state.cache_usage(), (0, 0));
}

#[tokio::test]
async fn retry_after_seconds_is_shared_across_credentials_and_cancelled_waits_free_permits() {
    let mock = Mock::new(Reply::ok("{}")).await;
    mock.script("/rate", vec![Reply::status(429).header("retry-after", "1")]);
    let state = Arc::new(HttpState::new(Config {
        global: 1,
        ..config()
    }));
    let client = api_client().unwrap();
    assert!(get(&state, &client, mock.url("/rate")).await.is_err());
    let start = Instant::now();
    let owner = state.clone();
    let request = client.get(mock.url("/rate")).bearer_auth("other-fake-key");
    let waiting = tokio::spawn(async move { json_with_state::<Value>(request, &owner).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(mock.count("/rate"), 1);
    waiting.abort();
    let _ = waiting.await;
    // This unrelated origin must not wait behind the first origin's cooldown.
    let other = Mock::new(Reply::ok("{}")).await;
    tokio::time::timeout(
        Duration::from_millis(250),
        get(&state, &client, other.url("/other")),
    )
    .await
    .unwrap()
    .unwrap();
    get(&state, &api_client().unwrap(), mock.url("/rate"))
        .await
        .unwrap();
    assert!(start.elapsed() >= Duration::from_millis(950));
    assert_eq!(mock.count("/rate"), 2);
}

#[tokio::test]
async fn retry_after_http_date_and_exponential_fallback_delay_subsequent_requests() {
    let mock = Mock::new(Reply::ok("{}")).await;
    let state = HttpState::new(config());
    let client = api_client().unwrap();
    let date = chrono::Utc::now() + chrono::Duration::seconds(2);
    let header = date.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    mock.script(
        "/date",
        vec![Reply::status(503).header("retry-after", header.clone())],
    );
    assert!(get(&state, &client, mock.url("/date")).await.is_err());
    get(&state, &client, mock.url("/date")).await.unwrap();
    let deadline: SystemTime = chrono::DateTime::parse_from_rfc2822(&header)
        .unwrap()
        .into();
    assert!(SystemTime::now() >= deadline);
    mock.script("/fallback", vec![Reply::status(429), Reply::status(503)]);
    assert!(get(&state, &client, mock.url("/fallback")).await.is_err());
    let first = Instant::now();
    assert!(get(&state, &client, mock.url("/fallback")).await.is_err());
    assert!(first.elapsed() >= Duration::from_millis(20));
    let second = Instant::now();
    get(&state, &client, mock.url("/fallback")).await.unwrap();
    assert!(second.elapsed() >= Duration::from_millis(45));
}

#[tokio::test]
async fn per_origin_concurrency_pacing_and_cancelled_semaphore_waits_are_bounded() {
    let mock = Mock::new(Reply::ok("{}").delay(Duration::from_millis(90))).await;
    let state = Arc::new(HttpState::new(Config {
        per_origin: 2,
        pacing: Duration::from_millis(30),
        ..config()
    }));
    let mut jobs = Vec::new();
    for index in 0..6 {
        let state = state.clone();
        let request = api_client()
            .unwrap()
            .get(mock.url(&format!("/paced-{index}")));
        jobs.push(tokio::spawn(async move {
            json_with_state::<Value>(request, &state).await
        }));
    }
    for job in jobs {
        job.await.unwrap().unwrap();
    }
    assert!(mock.state.peak.load(Ordering::SeqCst) <= 2);
    {
        let requests = mock.state.requests.lock().unwrap();
        assert_eq!(requests.len(), 6);
        for pair in requests.windows(2) {
            assert!(pair[1].1.duration_since(pair[0].1) >= Duration::from_millis(20));
        }
    }

    let state = Arc::new(HttpState::new(Config {
        per_origin: 1,
        ..config()
    }));
    let request = api_client().unwrap().get(mock.url("/first"));
    let owner = state.clone();
    let first = tokio::spawn(async move { json_with_state::<Value>(request, &owner).await });
    until(|| mock.count("/first") == 1).await;
    let request = api_client().unwrap().get(mock.url("/cancelled"));
    let owner = state.clone();
    let cancelled = tokio::spawn(async move { json_with_state::<Value>(request, &owner).await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    cancelled.abort();
    let _ = cancelled.await;
    first.await.unwrap().unwrap();
    get(&state, &api_client().unwrap(), mock.url("/after-cancel"))
        .await
        .unwrap();
    assert_eq!(mock.count("/cancelled"), 0);
}

#[tokio::test]
async fn extreme_retry_after_fails_closed_without_sleeping_or_eviction_bypass() {
    let mock = Mock::new(Reply::status(429).header("retry-after", "184467440737095516160")).await;
    let state = HttpState::new(Config {
        max_origins: 1,
        wait_timeout: Duration::from_millis(50),
        ..config()
    });
    let client = api_client().unwrap();
    assert!(get(&state, &client, mock.url("/forever")).await.is_err());
    tokio::time::timeout(
        Duration::from_millis(100),
        get(&state, &client, mock.url("/forever")),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(mock.count("/forever"), 1);
    let other = Mock::new(Reply::ok("{}")).await;
    assert!(get(&state, &client, other.url("/cannot-reset"))
        .await
        .is_err());
    assert_eq!(other.count("/cannot-reset"), 0);
}

#[test]
fn retry_after_dates_and_response_freshness_parsing_have_no_unbounded_waits() {
    let now = SystemTime::from(
        chrono::DateTime::parse_from_rfc2822("Sun, 06 Nov 1994 08:49:37 GMT").unwrap(),
    );
    for value in [
        "Sun, 06 Nov 1994 08:49:39 GMT",
        "Sunday, 06-Nov-94 08:49:39 GMT",
        "Sun Nov  6 08:49:39 1994",
    ] {
        assert_eq!(cache::retry_after(value, now), Some(Duration::from_secs(2)));
    }
    assert_eq!(cache::retry_after("0", now), Some(Duration::ZERO));
    assert_eq!(cache::retry_after("garbage", now), None);
    assert_eq!(cache::retry_after("-1", now), None);
    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, max-age=5"),
    );
    headers.insert(header::AGE, header::HeaderValue::from_static("2"));
    assert_eq!(
        cache::response_ttl(&headers, Duration::from_secs(15)),
        Some(Duration::from_secs(3))
    );
    headers.insert(
        header::EXPIRES,
        header::HeaderValue::from_static("Sun, 06 Nov 1994 08:49:39 GMT"),
    );
    assert_eq!(cache::response_ttl(&headers, Duration::from_secs(15)), None);
}

#[test]
fn production_cache_limits_cap_entries_and_bytes() {
    let config = Config::default();
    assert_eq!(config.max_entries, 128);
    assert_eq!(config.max_bytes, 32 * 1024 * 1024);
    let state = HttpState::new(config);
    for index in 0..130u16 {
        let key = sha2::Sha256::digest(index.to_be_bytes()).into();
        state.insert(key, b"{}".to_vec(), config.ttl);
    }
    assert_eq!(state.cache_usage(), (128, 256));
    let state = HttpState::new(config);
    for index in 0..18u8 {
        state.insert([index; 32], vec![index; config.max_entry_bytes], config.ttl);
    }
    assert_eq!(state.cache_usage(), (16, 32 * 1024 * 1024));
}

#[tokio::test]
async fn older_inflight_success_cannot_erase_newer_rate_limit() {
    let mock = Mock::new(Reply::ok("{}")).await;
    mock.script(
        "/old-success",
        vec![Reply::ok("{}").delay(Duration::from_millis(80))],
    );
    mock.script(
        "/new-429",
        vec![Reply::status(429).header("retry-after", "1")],
    );
    let state = Arc::new(HttpState::new(config()));
    let request = api_client().unwrap().get(mock.url("/old-success"));
    let owner = state.clone();
    let old = tokio::spawn(async move { json_with_state::<Value>(request, &owner).await });
    until(|| mock.count("/old-success") == 1).await;
    assert!(get(&state, &api_client().unwrap(), mock.url("/new-429"))
        .await
        .is_err());
    old.await.unwrap().unwrap();
    let request = api_client().unwrap().get(mock.url("/after-success"));
    let owner = state.clone();
    let waiting = tokio::spawn(async move { json_with_state::<Value>(request, &owner).await });
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(mock.count("/after-success"), 0);
    waiting.abort();
    let _ = waiting.await;
}

#[tokio::test]
async fn admission_timeout_releases_waiter_and_idle_origin_can_be_evicted() {
    let mock = Mock::new(Reply::ok("{}")).await;
    mock.script(
        "/slow",
        vec![Reply::ok("{}").delay(Duration::from_millis(100))],
    );
    let state = Arc::new(HttpState::new(Config {
        per_origin: 1,
        max_origins: 1,
        wait_timeout: Duration::from_millis(25),
        ..config()
    }));
    let request = api_client().unwrap().get(mock.url("/slow"));
    let owner = state.clone();
    let first = tokio::spawn(async move { json_with_state::<Value>(request, &owner).await });
    until(|| mock.count("/slow") == 1).await;
    assert!(get(&state, &api_client().unwrap(), mock.url("/timed-out"))
        .await
        .is_err());
    assert_eq!(mock.count("/timed-out"), 0);
    first.await.unwrap().unwrap();
    get(&state, &api_client().unwrap(), mock.url("/after-timeout"))
        .await
        .unwrap();
    let other = Mock::new(Reply::ok("{}")).await;
    get(&state, &api_client().unwrap(), other.url("/new-origin"))
        .await
        .unwrap();
    assert_eq!(other.count("/new-origin"), 1);
}

#[tokio::test]
async fn dependency_graph_source_client_only_times_out_on_stalled_reads() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // One response drips regularly for more than a whole idle interval. A
    // second response stops mid-body and must release the reader on its own.
    let idle = Duration::from_millis(300);
    for stalled in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/graph", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(stream.read_u8().await.unwrap());
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 5\r\nConnection: close\r\n\r\n[")
                .await
                .unwrap();
            if stalled {
                std::future::pending::<()>().await;
            }
            for chunk in [b"1", b",", b"2", b"]"] {
                tokio::time::sleep(Duration::from_millis(100)).await;
                stream.write_all(chunk).await.unwrap();
            }
        });
        let client = graph_client_with_idle_timeout(idle).unwrap();
        let state = HttpState::new(config());
        let value = tokio::time::timeout(Duration::from_secs(5), get(&state, &client, url))
            .await
            .expect("test watchdog fired");
        if stalled {
            server.abort();
            assert!(matches!(value, Err(AppError::Other(_))));
        } else {
            server.await.unwrap();
            assert_eq!(value.unwrap(), json!([1, 2]));
        }
    }
}
