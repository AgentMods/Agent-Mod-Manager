//! Search Bing in an isolated Playwright browser using the application-owned runtime.
use crate::{
    error::{AppError, AppResult},
    mcp::McpRuntime,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, net::IpAddr, process::Stdio, sync::OnceLock, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};
use tokio_util::sync::CancellationToken;
use url::Url;

pub const PROVIDER: &str = "bing";
pub const ENDPOINT: &str = "https://www.bing.com/search";
pub const MAX_QUERY_CHARS: usize = 512;
pub const MAX_TOOL_RESULT_BYTES: usize = 32 * 1024;
const MAX_TITLE_BYTES: usize = 512;
const MAX_SNIPPET_BYTES: usize = 1536;
const MAX_URL_BYTES: usize = 2048;
const MAX_SOURCE_TEXT_BYTES: usize = 16 * 1024;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_BROWSER_OUTPUT: usize = 256 * 1024;

fn default_count() -> u8 {
    5
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebSearchQuery {
    pub query: String,
    #[serde(default = "default_count")]
    pub count: u8,
}

impl WebSearchQuery {
    pub fn validate(&self) -> AppResult<()> {
        if self.query.trim().is_empty()
            || self.query.chars().count() > MAX_QUERY_CHARS
            || self.query.split_whitespace().count() > 75
            || self.query.chars().any(unsafe_character)
            || !(1..=10).contains(&self.count)
        {
            return Err(AppError::Invalid(
                "搜索词须为 1 至 512 个字符，最多 75 个词，结果数量须为 1 至 10 条".into(),
            ));
        }
        if private_path(&self.query) {
            return Err(AppError::Denied(
                "搜索词中不能包含本地路径或私人文件内容".into(),
            ));
        }
        for word in self.query.split_whitespace() {
            let word = word.trim_matches(['\'', '"', '(', ')', '<', '>']);
            if word.split_once("://").is_some_and(|(scheme, _)| {
                scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
            }) && safe_url(word).is_none()
            {
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

    fn bound(&mut self) -> AppResult<()> {
        loop {
            if serde_json::to_vec(self)?.len() <= MAX_TOOL_RESULT_BYTES {
                return Ok(());
            }
            if self.results.pop().is_none() {
                return Err(AppError::Other("联网搜索结果超过大小限制".into()));
            }
            self.truncated = true;
        }
    }

    pub fn into_tool_value(mut self) -> AppResult<Value> {
        self.bound()?;
        Ok(serde_json::to_value(self)?)
    }
}

#[derive(Deserialize)]
struct BrowserResponse {
    status: WebSearchStatus,
    results: Vec<WebSearchHit>,
    truncated: bool,
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

fn unavailable() -> WebSearchResult {
    WebSearchResult::empty(
        WebSearchStatus::Unavailable,
        "未能读取 Bing 搜索结果。请检查网络后重试。若 Bing 要求验证码或更改了结果页面，请稍后再试。",
    )
}

impl BrowserResponse {
    fn into_result(self, count: u8) -> AppResult<WebSearchResult> {
        match self.status {
            WebSearchStatus::NeedsConfiguration => {
                return Ok(WebSearchResult::empty(
                    self.status,
                    "Playwright 无法启动浏览器。请安装 Microsoft Edge、Google Chrome 或 Playwright Chromium 后重试。",
                ));
            }
            WebSearchStatus::Unavailable => return Ok(unavailable()),
            WebSearchStatus::Available => {}
        }
        let mut result =
            WebSearchResult::empty(self.status, "Bing 网页摘要仅供参考，请核实来源和下载地址。");
        result.truncated = self.truncated;
        let mut seen = HashSet::new();
        for hit in self.results {
            if result.results.len() >= usize::from(count) {
                result.truncated = true;
                break;
            }
            let Some(url) = safe_url(&hit.url) else {
                result.truncated = true;
                continue;
            };
            let title = clean_text(&hit.title, MAX_TITLE_BYTES);
            let snippet = clean_text(&hit.snippet, MAX_SNIPPET_BYTES);
            result.truncated |= title != hit.title || snippet != hit.snippet;
            if title.is_empty() || !seen.insert(url.clone()) {
                continue;
            }
            result.results.push(WebSearchHit {
                title,
                url,
                snippet,
            });
        }
        result.bound()?;
        Ok(result)
    }
}

pub async fn search(
    runtime: &McpRuntime,
    query: &WebSearchQuery,
    cancel: &CancellationToken,
) -> AppResult<WebSearchResult> {
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    query.validate()?;
    let command = match runtime.bing_search_command(cancel).await {
        Ok(cmd) => cmd,
        Err(AppError::Cancelled) => return Err(AppError::Cancelled),
        Err(_) => {
            return Ok(WebSearchResult::empty(
                WebSearchStatus::NeedsConfiguration,
                "无法加载内置 Playwright 搜索组件，请重新安装应用或从源码重新构建运行时。",
            ))
        }
    };
    match run_browser(command, query, cancel).await {
        Ok(response) => response.into_result(query.count),
        Err(AppError::Cancelled) => Err(AppError::Cancelled),
        Err(_) => Ok(unavailable()),
    }
}

async fn run_browser(
    mut command: Command,
    query: &WebSearchQuery,
    cancel: &CancellationToken,
) -> AppResult<BrowserResponse> {
    let mut request = serde_json::to_vec(query)?;
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

fn private_path(text: &str) -> bool {
    static URL: OnceLock<Regex> = OnceLock::new();
    static PATH: OnceLock<Regex> = OnceLock::new();
    let text = URL
        .get_or_init(|| Regex::new(r#"(?i)https?://[^\s\x22'<>]+"#).expect("constant URL regex"))
        .replace_all(text, " ");
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
    if raw.len() > MAX_URL_BYTES
        || raw
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
    (url.as_str().len() <= MAX_URL_BYTES).then(|| url.into())
}

#[cfg(test)]
#[path = "web_search_tests.rs"]
mod tests;
