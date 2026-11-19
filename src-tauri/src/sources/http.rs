use crate::error::{AppError, AppResult};
use reqwest::{header, Client};
use serde::de::DeserializeOwned;
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    time::Duration,
};
use url::Url;

#[path = "http_cache.rs"]
mod cache;

pub const USER_AGENT: &str = concat!("AgentModManager/", env!("CARGO_PKG_VERSION"));

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || a == 0
        || a >= 240
        || (a == 100 && (64..=127).contains(&b))
        || (a == 198 && (b == 18 || b == 19))
        || (a == 192 && b == 0 && c == 0))
}

fn public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return public_v4(v4);
    }
    // Global unicast only; exclude transition, documentation and special ranges.
    let s = ip.segments();
    (s[0] & 0xe000) == 0x2000
        && !(s[0] == 0x2001 && s[1] < 0x200)
        && !(s[0] == 0x2001 && s[1] == 0xdb8)
        && s[0] != 0x2002
}

fn public_ip(ip: IpAddr) -> bool {
    #[cfg(test)]
    if ip.is_loopback() {
        return true;
    }
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => public_v6(ip),
    }
}

/// HTTPS, no userinfo/fragment/local endpoints. Error messages omit the input.
/// DNS is additionally checked and pinned when the downloader connects.
pub fn validate_download_url(raw: &str) -> AppResult<Url> {
    let url = Url::parse(raw).map_err(|_| AppError::Invalid("无效下载 URL".into()))?;
    let scheme_ok = url.scheme() == "https";
    #[cfg(test)]
    let scheme_ok = scheme_ok
        || (url.scheme() == "http"
            && url.host_str().is_some_and(|h| {
                h.trim_matches(['[', ']'])
                    .parse::<IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
            }));
    if !scheme_ok
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return Err(AppError::Denied(
            "下载 URL 必须使用 HTTPS，且不得包含用户凭据或 fragment".into(),
        ));
    }
    let host = url.host_str().unwrap().trim_matches(['[', ']']);
    if host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || !host.contains('.') && !host.contains(':')
        || host.parse::<IpAddr>().is_ok_and(|ip| !public_ip(ip))
    {
        return Err(AppError::Denied("拒绝非公网下载地址".into()));
    }
    Ok(url)
}

pub(crate) async fn pinned_download_client(url: &Url, timeout: Duration) -> AppResult<Client> {
    let host = url.host_str().unwrap_or_default().trim_matches(['[', ']']);
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| AppError::Other("下载主机解析失败".into()))?
        .collect();
    if addresses.is_empty() || addresses.iter().any(|a| !public_ip(a.ip())) {
        return Err(AppError::Denied("下载主机解析到非公网地址".into()));
    }
    Client::builder()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .connect_timeout(Duration::from_secs(15))
        .timeout(timeout)
        .resolve_to_addrs(host, &addresses)
        .build()
        .map_err(|_| AppError::Other("无法创建下载客户端".into()))
}

/// Authentication stays on each provider request, never in client defaults or
/// a cookie jar, so the cache can isolate every credential scope before I/O.
pub fn api_client() -> AppResult<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(30))
        // API credentials must never be forwarded to another origin.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| AppError::Other("无法创建来源客户端".into()))
}

pub fn secret_header(secret: Option<String>) -> AppResult<Option<header::HeaderValue>> {
    secret
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            let mut value = header::HeaderValue::from_str(&s)
                .map_err(|_| AppError::Invalid("来源密钥格式无效".into()))?;
            value.set_sensitive(true);
            Ok(value)
        })
        .transpose()
}

pub async fn json<T: DeserializeOwned>(request: reqwest::RequestBuilder) -> AppResult<T> {
    json_with_state(request, cache::shared()).await
}

async fn json_with_state<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
    state: &cache::HttpState,
) -> AppResult<T> {
    let (client, request) = request.build_split();
    let request = request.map_err(|_| AppError::Invalid("来源请求格式无效".into()))?;
    // Provider clients have no default auth/cookie headers: credentials must be
    // explicit on the request so that every auth/representation scope is hashed.
    let key = cache::request_key(&request);
    if let Some(body) = key.and_then(|key| state.get(key)) {
        return decode(&body);
    }
    let origin = state.origin(request.url())?;
    let _permit = state.acquire(&origin).await?;
    // A concurrent request may have filled the cache while this one waited.
    if let Some(body) = key.and_then(|key| state.get(key)) {
        return decode(&body);
    }
    let response = client
        .execute(request)
        .await
        .map_err(|_| AppError::Other("来源网络请求失败".into()))?;
    let status = response.status();
    if matches!(status.as_u16(), 429 | 503) {
        origin.backoff(response.headers());
    } else if status.is_success() {
        origin.success();
    }
    if !status.is_success() {
        return Err(status_error(status));
    }
    let ttl = key.and_then(|_| cache::response_ttl(response.headers(), state.ttl()));
    let received = std::time::Instant::now();
    // Bound JSON allocations, including compressed responses.
    let mut response = response;
    let mut bytes = Vec::new();
    if response
        .content_length()
        .is_some_and(|length| length > cache::MAX_RESPONSE_BYTES as u64)
    {
        return Err(AppError::Other("来源响应超过大小限制".into()));
    }
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AppError::Other("来源响应中断".into()))?
    {
        if bytes.len().saturating_add(chunk.len()) > cache::MAX_RESPONSE_BYTES {
            return Err(AppError::Other("来源响应超过大小限制".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value = decode(&bytes)?;
    if let (Some(key), Some(ttl)) = (key, ttl) {
        state.insert(key, bytes, ttl.saturating_sub(received.elapsed()));
    }
    Ok(value)
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> AppResult<T> {
    serde_json::from_slice(bytes).map_err(|_| AppError::Other("来源响应格式无效".into()))
}

pub(super) fn status_error(status: reqwest::StatusCode) -> AppError {
    match status.as_u16() {
        401 | 403 => AppError::Denied("来源拒绝访问；请检查密钥及下载权限".into()),
        404 => AppError::NotFound("来源未找到项目或文件".into()),
        429 => AppError::Other("来源请求过多，请稍后重试".into()),
        _ => AppError::Other(format!("来源返回 HTTP {}", status.as_u16())),
    }
}

pub fn segment(value: &str) -> AppResult<&str> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b) || b == b'.')
    {
        return Err(AppError::Invalid("来源标识符无效".into()));
    }
    if value == "." || value == ".." {
        return Err(AppError::Invalid("来源标识符无效".into()));
    }
    Ok(value)
}

pub fn positive_id(value: &str) -> AppResult<&str> {
    if !value.bytes().all(|b| b.is_ascii_digit())
        || value.parse::<u64>().ok().filter(|v| *v > 0).is_none()
    {
        return Err(AppError::Invalid("需要来源的正整数 ID".into()));
    }
    Ok(value)
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
