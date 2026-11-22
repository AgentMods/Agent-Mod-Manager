//! Search Bing in an isolated Playwright browser using the application-owned runtime.
use crate::{
    error::{AppError, AppResult},
    mcp::McpRuntime,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    net::IpAddr,
    process::Stdio,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

pub const PROVIDER: &str = "bing";
pub const ENDPOINT: &str = "https://www.bing.com/search";
// Limits on individual transfers, not on the length of an active result stream.
const MAX_BROWSER_REQUEST: usize = 256 * 1024;
const MAX_TOOL_PAGE_BYTES: usize = 64 * 1024;
const PAGE_ENVELOPE_BYTES: usize = 1024;
const MAX_BROWSER_ROWS: usize = 24;
const MAX_JS_INTEGER: u64 = (1 << 53) - 1;
const CURSOR_IDLE_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_IDLE_CURSORS: usize = 16;
const MAX_TITLE_BYTES: usize = 512;
const MAX_SNIPPET_BYTES: usize = 1536;
const MAX_URL_BYTES: usize = 2048;
const MAX_SOURCE_TEXT_BYTES: usize = 16 * 1024;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_BROWSER_OUTPUT: usize = 256 * 1024;

pub(crate) fn default_count() -> usize {
    5
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebSearchQuery {
    pub query: String,
    #[serde(default = "default_count")]
    pub count: usize,
}

impl WebSearchQuery {
    pub fn validate(&self) -> AppResult<()> {
        if self.query.trim().is_empty()
            || self.query.chars().any(unsafe_character)
            || self.count == 0
        {
            return Err(AppError::Invalid(
                "搜索词不能为空或包含控制字符，结果数量须为正整数".into(),
            ));
        }
        // Reserve space for the JSON request metadata; never shorten a submitted query.
        if self.query.len() > MAX_BROWSER_REQUEST - 128
            || serde_json::to_vec(&self.query)?.len() > MAX_BROWSER_REQUEST - 128
        {
            return Err(AppError::Invalid("搜索词超过浏览器传输大小限制".into()));
        }
        if private_path(&self.query) {
            return Err(AppError::Denied(
                "搜索词中不能包含本地路径或私人文件内容".into(),
            ));
        }
        for candidate in web_urls().find_iter(&self.query) {
            if public_url(candidate.as_str().trim_end_matches([')', '}'])).is_none() {
                return Err(AppError::Denied(
                    "搜索词中不能包含本地或带凭据的网址".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchStatus {
    Available,
    NeedsConfiguration,
    Unavailable,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WebSearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Clone, Serialize)]
pub struct WebSearchResult {
    pub provider: &'static str,
    pub status: WebSearchStatus,
    pub message: &'static str,
    pub results: Vec<WebSearchHit>,
    pub truncated: bool,
    pub untrusted: bool,
}

impl WebSearchResult {
    fn empty(status: WebSearchStatus, message: &'static str) -> Self {
        Self {
            provider: PROVIDER,
            status,
            message,
            results: Vec::new(),
            truncated: false,
            untrusted: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct BingPosition {
    first: u64,
    row_offset: u64,
}

impl BingPosition {
    const START: Self = Self {
        first: 1,
        row_offset: 0,
    };

    fn permits(self, next: Self) -> bool {
        next.first <= MAX_JS_INTEGER
            && next.row_offset <= MAX_JS_INTEGER
            && ((next.first > self.first && next.row_offset == 0)
                || (next.first == self.first
                    && next.row_offset > self.row_offset
                    && next.row_offset - self.row_offset <= MAX_BROWSER_ROWS as u64))
    }
}

#[derive(Serialize)]
struct BrowserRequest<'a> {
    query: &'a str,
    count: usize,
    position: BingPosition,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BrowserReason {
    InvalidPagination,
    QueryRejected,
}

#[derive(Deserialize)]
struct BrowserResponse {
    status: WebSearchStatus,
    results: Vec<WebSearchHit>,
    truncated: bool,
    #[serde(default)]
    next: Option<BingPosition>,
    #[serde(default)]
    reason: Option<BrowserReason>,
}

fn clean_text(text: &str, max: usize) -> String {
    if text.len() > MAX_SOURCE_TEXT_BYTES {
        return "[OVERSIZED_TEXT_OMITTED]".into();
    }
    if private_path(text) {
        return "[PRIVATE_PATH_OMITTED]".into();
    }
    let mut output = String::new();
    for character in text.chars().filter(|ch| !unsafe_character(*ch)) {
        if output.len() + character.len_utf8() > max {
            break;
        }
        output.push(character);
    }
    output.trim().to_owned()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PageEnd {
    MoreResults,
    BingExhausted,
    InvalidPagination,
    QueryRejected,
    BingUnavailable,
    NeedsConfiguration,
    PageTimeout,
}

impl PageEnd {
    fn message(self) -> &'static str {
        match self {
            Self::MoreResults => "Bing 网页摘要仅供参考。可使用 next_cursor 继续读取。",
            Self::BingExhausted => {
                "当前 Bing 结果页没有可继续读取的链接。网页摘要仅供参考，请核实来源和下载地址。"
            }
            Self::InvalidPagination => {
                "Bing 的续页链接无效或指回已请求的位置，不能确认是否还有更多结果。"
            }
            Self::QueryRejected => {
                "Bing 拒绝了搜索请求（可能是编码后的搜索网址过长）；未自动缩短搜索词。"
            }
            Self::BingUnavailable => {
                "未能读取 Bing 搜索结果。请检查网络后重试。若 Bing 要求验证码或更改了结果页面，请稍后再试。"
            }
            Self::NeedsConfiguration => {
                "无法启动内置 Playwright 搜索；请检查应用组件和已安装的浏览器。"
            }
            Self::PageTimeout => "本次 Bing 续页超时；如有 next_cursor，可重试尚未读取的位置。",
        }
    }
}

#[derive(Serialize)]
struct SearchPage {
    #[serde(flatten)]
    result: WebSearchResult,
    next_cursor: Option<Uuid>,
    page_end: PageEnd,
}

struct SearchSession {
    position: Option<BingPosition>,
    pending: VecDeque<WebSearchHit>,
    // Retain only fingerprints of emitted links, not previous pages or private inputs.
    seen: HashSet<[u8; 32]>,
    started: bool,
    terminal: PageEnd,
    pending_text_cut: bool,
}

impl Default for SearchSession {
    fn default() -> Self {
        Self {
            position: Some(BingPosition::START),
            pending: VecDeque::new(),
            seen: HashSet::new(),
            started: false,
            terminal: PageEnd::BingExhausted,
            pending_text_cut: false,
        }
    }
}

struct IdleCursor {
    query_digest: [u8; 32],
    touched: Instant,
    session: SearchSession,
}

/// Opaque in-process continuations for the desktop tool. At most one caller can
/// consume each cursor; a cancelled request closes its chain and its browser.
#[derive(Default)]
pub(crate) struct SearchSessions(Mutex<HashMap<Uuid, IdleCursor>>);

impl SearchSessions {
    fn take(&self, cursor: Uuid, query_digest: [u8; 32]) -> AppResult<SearchSession> {
        let mut cursors = self.0.lock().unwrap();
        cursors.retain(|_, cursor| cursor.touched.elapsed() < CURSOR_IDLE_TTL);
        if !cursors
            .get(&cursor)
            .is_some_and(|entry| entry.query_digest == query_digest)
        {
            return Err(AppError::NotFound(
                "搜索游标已结束、过期或不属于此搜索词，请重新搜索".into(),
            ));
        }
        Ok(cursors.remove(&cursor).unwrap().session)
    }

    fn save(&self, query_digest: [u8; 32], session: SearchSession) -> Uuid {
        let mut cursors = self.0.lock().unwrap();
        cursors.retain(|_, cursor| cursor.touched.elapsed() < CURSOR_IDLE_TTL);
        if cursors.len() >= MAX_IDLE_CURSORS {
            if let Some(oldest) = cursors
                .iter()
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(id, _)| *id)
            {
                cursors.remove(&oldest);
            }
        }
        let cursor = Uuid::new_v4();
        cursors.insert(
            cursor,
            IdleCursor {
                query_digest,
                touched: Instant::now(),
                session,
            },
        );
        cursor
    }

    pub(crate) async fn search_page(
        &self,
        runtime: &McpRuntime,
        query: &WebSearchQuery,
        cursor: Option<Uuid>,
        cancel: &CancellationToken,
    ) -> AppResult<Value> {
        self.search_using(query, cursor, cancel, |position, cancel| {
            fetch_page(runtime, &query.query, position, cancel)
        })
        .await
    }

    async fn search_using<F, Fut>(
        &self,
        query: &WebSearchQuery,
        cursor: Option<Uuid>,
        cancel: &CancellationToken,
        fetch: F,
    ) -> AppResult<Value>
    where
        F: FnMut(BingPosition, CancellationToken) -> Fut,
        Fut: Future<Output = AppResult<BrowserResponse>>,
    {
        check_cancel(cancel)?;
        query.validate()?;
        let digest = Sha256::digest(query.query.trim().as_bytes()).into();
        let mut session = match cursor {
            Some(cursor) => self.take(cursor, digest)?,
            None => SearchSession::default(),
        };
        let mut page = collect_page(&mut session, query.count, cancel, fetch).await?;
        if !session.pending.is_empty() || (session.started && session.position.is_some()) {
            page.next_cursor = Some(self.save(digest, session));
            page.result.truncated = true;
        }
        Ok(serde_json::to_value(page)?)
    }
}

fn check_cancel(cancel: &CancellationToken) -> AppResult<()> {
    if cancel.is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(())
    }
}

async fn collect_page<F, Fut>(
    session: &mut SearchSession,
    count: usize,
    cancel: &CancellationToken,
    mut fetch: F,
) -> AppResult<SearchPage>
where
    F: FnMut(BingPosition, CancellationToken) -> Fut,
    Fut: Future<Output = AppResult<BrowserResponse>>,
{
    let mut result = WebSearchResult::empty(WebSearchStatus::Available, "");
    let mut bytes = PAGE_ENVELOPE_BYTES;
    let deadline = tokio::time::Instant::now() + SEARCH_TIMEOUT;
    let page_end = loop {
        check_cancel(cancel)?;
        if let Some(hit) = session.pending.front() {
            let key =
                Sha256::digest(hit.url.split('#').next().unwrap_or_default().as_bytes()).into();
            if session.seen.contains(&key) {
                session.pending.pop_front();
                continue;
            }
            let hit_bytes = serde_json::to_vec(hit)?.len() + 1; // JSON field escapes and comma.
            if result.results.len() == count || bytes + hit_bytes > MAX_TOOL_PAGE_BYTES {
                break PageEnd::MoreResults;
            }
            bytes += hit_bytes;
            session.seen.insert(key);
            result.results.push(session.pending.pop_front().unwrap());
            result.truncated |= session.pending_text_cut;
            continue;
        }
        let Some(position) = session.position else {
            break session.terminal;
        };
        if result.results.len() == count {
            break PageEnd::MoreResults;
        }
        if tokio::time::Instant::now() >= deadline {
            break PageEnd::PageTimeout;
        }
        let fetch_cancel = cancel.child_token();
        let receive = fetch(position, fetch_cancel.clone());
        tokio::pin!(receive);
        let response = tokio::select! {
            biased;
            response = &mut receive => response,
            _ = tokio::time::sleep_until(deadline) => {
                fetch_cancel.cancel();
                // Give the helper EOF and let it close the browser before reporting timeout.
                let _ = receive.await;
                check_cancel(cancel)?;
                break PageEnd::PageTimeout;
            }
        };
        check_cancel(cancel)?;
        let response = match response {
            Ok(response) => response,
            Err(AppError::Cancelled) => return Err(AppError::Cancelled),
            Err(_) => break PageEnd::BingUnavailable,
        };
        match response.status {
            WebSearchStatus::NeedsConfiguration => break PageEnd::NeedsConfiguration,
            WebSearchStatus::Unavailable => {
                if matches!(response.reason, Some(BrowserReason::QueryRejected)) {
                    session.position = None;
                    break PageEnd::QueryRejected;
                }
                break PageEnd::BingUnavailable;
            }
            WebSearchStatus::Available => {}
        }
        if response.results.len() > MAX_BROWSER_ROWS {
            break PageEnd::BingUnavailable;
        }
        session.started = true;
        session.pending_text_cut = response.truncated;
        (session.position, session.terminal) = match (response.next, response.reason) {
            (Some(next), None) if position.permits(next) => (Some(next), PageEnd::MoreResults),
            (None, None) => (None, PageEnd::BingExhausted),
            _ => (None, PageEnd::InvalidPagination),
        };
        for hit in response.results {
            let Some(url) = safe_url(&hit.url) else {
                result.truncated = true;
                continue;
            };
            let title = clean_text(&hit.title, MAX_TITLE_BYTES);
            let snippet = clean_text(&hit.snippet, MAX_SNIPPET_BYTES);
            session.pending_text_cut |= title != hit.title || snippet != hit.snippet;
            if !title.is_empty() {
                session.pending.push_back(WebSearchHit {
                    title,
                    url,
                    snippet,
                });
            }
        }
    };
    result.message = page_end.message();
    result.truncated |=
        page_end != PageEnd::BingExhausted && (!result.results.is_empty() || session.started);
    if result.results.is_empty() {
        result.status = match page_end {
            PageEnd::MoreResults | PageEnd::BingExhausted => WebSearchStatus::Available,
            PageEnd::NeedsConfiguration => WebSearchStatus::NeedsConfiguration,
            _ => WebSearchStatus::Unavailable,
        };
    }
    Ok(SearchPage {
        result,
        next_cursor: None,
        page_end,
    })
}

// The game-identity caller still requests a single bounded preview and uses the
// original Rust result shape. The web_search tool uses SearchSessions instead.
pub async fn search(
    runtime: &McpRuntime,
    query: &WebSearchQuery,
    cancel: &CancellationToken,
) -> AppResult<WebSearchResult> {
    check_cancel(cancel)?;
    query.validate()?;
    let mut session = SearchSession::default();
    Ok(
        collect_page(&mut session, query.count, cancel, |position, cancel| {
            fetch_page(runtime, &query.query, position, cancel)
        })
        .await?
        .result,
    )
}

async fn fetch_page(
    runtime: &McpRuntime,
    query: &str,
    position: BingPosition,
    cancel: CancellationToken,
) -> AppResult<BrowserResponse> {
    let command = match runtime.bing_search_command(&cancel).await {
        Ok(cmd) => cmd,
        Err(AppError::Cancelled) => return Err(AppError::Cancelled),
        Err(_) => {
            return Ok(BrowserResponse {
                status: WebSearchStatus::NeedsConfiguration,
                results: Vec::new(),
                truncated: false,
                next: None,
                reason: None,
            })
        }
    };
    run_browser(
        command,
        &BrowserRequest {
            query,
            count: MAX_BROWSER_ROWS,
            position,
        },
        &cancel,
    )
    .await
}

async fn run_browser(
    mut command: Command,
    request: &BrowserRequest<'_>,
    cancel: &CancellationToken,
) -> AppResult<BrowserResponse> {
    let mut request = serde_json::to_vec(request)?;
    if request.len() > MAX_BROWSER_REQUEST {
        return Err(AppError::Invalid("搜索词超过浏览器传输大小限制".into()));
    }
    request.push(b'\n');
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let exchange = async {
        stdin.write_all(&request).await?;
        let mut bytes = Vec::new();
        stdout
            .take((MAX_BROWSER_OUTPUT + 1) as u64)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() > MAX_BROWSER_OUTPUT || !child.wait().await?.success() {
            return Err(AppError::Other("搜索进程返回异常".into()));
        }
        Ok(serde_json::from_slice(&bytes)?)
    };
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(AppError::Cancelled),
        result = tokio::time::timeout(SEARCH_TIMEOUT, exchange) => {
            result.unwrap_or_else(|_| Err(AppError::Other("Bing 搜索超时".into())))
        }
    };
    // The helper treats EOF as cancellation and closes Playwright and its browser.
    drop(stdin);
    if result.is_err()
        && tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .is_err()
    {
        let _ = child.kill().await;
    }
    result
}

fn unsafe_character(ch: char) -> bool {
    ch.is_control() || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

fn web_urls() -> &'static Regex {
    static URL: OnceLock<Regex> = OnceLock::new();
    URL.get_or_init(|| Regex::new(r#"(?i)https?://[^\s\x22'<>]*"#).expect("constant URL regex"))
}

fn private_path(text: &str) -> bool {
    static PATH: OnceLock<Regex> = OnceLock::new();
    let text = web_urls().replace_all(text, " ");
    PATH.get_or_init(|| {
        Regex::new(r#"(?i)(?:file://|[a-z]:[\\/]|\\\\|(?:^|[\s\x22'(<=>])(?:~[\\/]|/[^\s/]))"#)
            .expect("constant path regex")
    })
    .is_match(&text)
}

fn public_ipv4(ip: std::net::Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || ip.is_unspecified()
        || a == 0
        || a >= 240
        || (a == 100 && (64..=127).contains(&b))
        || (a == 198 && (b == 18 || b == 19))
        || (a == 192 && b == 0 && c == 0))
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_ipv4(ip),
        IpAddr::V6(ip) => {
            if let Some(ip) = ip.to_ipv4_mapped() {
                return public_ipv4(ip);
            }
            let segments = ip.segments();
            (segments[0] & 0xe000) == 0x2000
                && !(segments[0] == 0x2001 && segments[1] < 0x200)
                && !(segments[0] == 0x2001 && segments[1] == 0xdb8)
                && segments[0] != 0x2002
        }
    }
}

fn safe_url(raw: &str) -> Option<String> {
    if raw.len() > MAX_URL_BYTES {
        return None;
    }
    let url = public_url(raw)?;
    (url.as_str().len() <= MAX_URL_BYTES).then(|| url.into())
}

fn public_url(raw: &str) -> Option<Url> {
    if raw
        .chars()
        .any(|ch| unsafe_character(ch) || ch.is_whitespace())
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let host = url
        .host_str()?
        .trim_matches(['[', ']'])
        .trim_end_matches('.');
    if host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".lan")
        || host.ends_with(".home.arpa")
        || host.ends_with(".test")
        || host.ends_with(".example")
        || host.ends_with(".invalid")
        || !host.contains('.') && !host.contains(':')
    {
        return None;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if !public_ip(ip) {
            return None;
        }
    }
    Some(url)
}

#[cfg(test)]
#[path = "web_search_tests.rs"]
mod tests;
