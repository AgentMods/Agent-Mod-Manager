//! Process-local provider cache and governor, shared across rebuilt Sources.
//! Keys retain only digests; bodies may contain signed URLs and never touch disk.
//! No Debug/Serialize implementations and no background tasks outlive callers.

use crate::error::{AppError, AppResult};
use reqwest::{header, Method, Request};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use url::Url;

pub(super) const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_CACHE_ENTRIES: usize = 128;
const MAX_ENTRY_BYTES: usize = 2 * 1024 * 1024;
const MAX_ORIGINS: usize = 128;
const CACHE_TTL: Duration = Duration::from_secs(15);
const PACING: Duration = Duration::from_millis(200);
const WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const FALLBACK_BACKOFF: Duration = Duration::from_secs(1);
const MAX_FALLBACK_BACKOFF: Duration = Duration::from_secs(30);

type Key = [u8; 32];

pub(super) fn shared() -> &'static HttpState {
    static STATE: OnceLock<HttpState> = OnceLock::new();
    STATE.get_or_init(|| HttpState::new(Config::default()))
}

#[derive(Clone, Copy)]
pub(super) struct Config {
    pub(super) ttl: Duration,
    pub(super) max_bytes: usize,
    pub(super) max_entries: usize,
    pub(super) max_entry_bytes: usize,
    pub(super) max_origins: usize,
    pub(super) per_origin: usize,
    pub(super) global: usize,
    pub(super) pacing: Duration,
    pub(super) wait_timeout: Duration,
    pub(super) fallback_backoff: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ttl: CACHE_TTL,
            max_bytes: MAX_CACHE_BYTES,
            max_entries: MAX_CACHE_ENTRIES,
            max_entry_bytes: MAX_ENTRY_BYTES,
            max_origins: MAX_ORIGINS,
            per_origin: 2,
            global: 8,
            pacing: PACING,
            wait_timeout: WAIT_TIMEOUT,
            fallback_backoff: FALLBACK_BACKOFF,
        }
    }
}

struct Entry {
    body: Arc<[u8]>,
    expires: Instant,
    used: Instant,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<Key, Entry>,
    bytes: usize,
}

impl Cache {
    fn remove(&mut self, key: Key) {
        if let Some(entry) = self.entries.remove(&key) {
            self.bytes -= entry.body.len();
        }
    }

    fn expire(&mut self, now: Instant) {
        self.entries.retain(|_, entry| {
            if entry.expires <= now {
                self.bytes -= entry.body.len();
                false
            } else {
                true
            }
        });
    }
}

pub(super) struct HttpState {
    config: Config,
    cache: Mutex<Cache>,
    origins: Mutex<HashMap<Key, Arc<Origin>>>,
    global: Arc<Semaphore>,
}

impl HttpState {
    pub(super) fn new(config: Config) -> Self {
        Self {
            config,
            cache: Mutex::new(Cache::default()),
            origins: Mutex::new(HashMap::new()),
            global: Arc::new(Semaphore::new(config.global)),
        }
    }

    pub(super) fn ttl(&self) -> Duration {
        self.config.ttl
    }

    pub(super) fn get(&self, key: Key) -> Option<Arc<[u8]>> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        cache.expire(now);
        cache.entries.get_mut(&key).map(|entry| {
            entry.used = now;
            entry.body.clone()
        })
    }

    pub(super) fn insert(&self, key: Key, body: Vec<u8>, ttl: Duration) {
        if ttl.is_zero()
            || self.config.max_entries == 0
            || body.len() > self.config.max_entry_bytes.min(self.config.max_bytes)
        {
            return;
        }
        let now = Instant::now();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.expire(now);
        cache.remove(key);
        while cache.entries.len() >= self.config.max_entries
            || cache.bytes.saturating_add(body.len()) > self.config.max_bytes
        {
            let Some(oldest) = cache
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            cache.remove(oldest);
        }
        cache.bytes += body.len();
        cache.entries.insert(
            key,
            Entry {
                body: body.into(),
                expires: now + ttl.min(self.config.ttl),
                used: now,
            },
        );
    }

    pub(super) fn origin(&self, url: &Url) -> AppResult<Arc<Origin>> {
        if !matches!(url.scheme(), "https" | "http") || url.host().is_none() {
            return Err(AppError::Invalid("来源请求地址无效".into()));
        }
        let key: Key = Sha256::digest(url.origin().ascii_serialization().as_bytes()).into();
        let mut origins = self.origins.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(origin) = origins.get(&key) {
            return Ok(origin.clone());
        }
        if origins.len() >= self.config.max_origins {
            let now = Instant::now();
            // Never evict an active/waiting origin or reset an outstanding
            // pacing/backoff deadline to make room for another host.
            let oldest = origins
                .iter()
                .filter(|(_, origin)| Arc::strong_count(origin) == 1)
                .filter_map(|(key, origin)| {
                    let clock = origin.clock.lock().unwrap_or_else(|e| e.into_inner());
                    clock
                        .ready()
                        .is_some_and(|ready| ready <= now)
                        .then_some((*key, clock.last_start))
                })
                .min_by_key(|(_, used)| *used)
                .map(|(key, _)| key);
            if let Some(key) = oldest {
                origins.remove(&key);
            } else {
                return Err(AppError::Other("来源并发主机过多，请稍后重试".into()));
            }
        }
        let origin = Arc::new(Origin {
            permits: Arc::new(Semaphore::new(self.config.per_origin)),
            pacing: self.config.pacing,
            fallback_backoff: self.config.fallback_backoff,
            clock: Mutex::new(Clock {
                next_start: Instant::now(),
                retry: Retry::Ready,
                failures: 0,
                last_start: Instant::now(),
            }),
        });
        origins.insert(key, origin.clone());
        Ok(origin)
    }

    pub(super) async fn acquire(&self, origin: &Arc<Origin>) -> AppResult<Permit> {
        // Timing out or dropping this future releases all acquired permits.
        // We do not reserve future start slots for cancelled waiters.
        tokio::time::timeout(self.config.wait_timeout, async {
            let local = origin
                .permits
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| throttled())?;
            loop {
                let ready = origin
                    .clock
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .ready();
                let Some(ready) = ready else {
                    return Err(throttled());
                };
                // Very long Retry-After values are kept, but never hold a
                // caller waiting longer than the bounded admission timeout.
                if ready.saturating_duration_since(Instant::now()) > self.config.wait_timeout {
                    return Err(throttled());
                }
                tokio::time::sleep_until(ready.into()).await;
                let global = self
                    .global
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| throttled())?;
                {
                    let mut clock = origin.clock.lock().unwrap_or_else(|e| e.into_inner());
                    let now = Instant::now();
                    if clock.ready().is_some_and(|ready| ready <= now) {
                        clock.last_start = now;
                        clock.next_start = now + origin.pacing;
                        return Ok(Permit {
                            _local: local,
                            _global: global,
                        });
                    }
                }
                // A response may have imposed backoff while we waited for the
                // global permit. Other origins must not be held behind it.
                drop(global);
            }
        })
        .await
        .map_err(|_| throttled())?
    }

    #[cfg(test)]
    pub(super) fn cache_usage(&self) -> (usize, usize) {
        let cache = self.cache.lock().unwrap();
        (cache.entries.len(), cache.bytes)
    }
}

pub(super) struct Permit {
    _local: OwnedSemaphorePermit,
    _global: OwnedSemaphorePermit,
}

pub(super) struct Origin {
    permits: Arc<Semaphore>,
    pacing: Duration,
    fallback_backoff: Duration,
    clock: Mutex<Clock>,
}

enum Retry {
    Ready,
    At(Instant),
    // Absurd but syntactically valid delta-seconds can overflow Instant. Fail
    // closed for this origin rather than accidentally retrying immediately.
    Indefinite,
}

struct Clock {
    next_start: Instant,
    retry: Retry,
    failures: u32,
    last_start: Instant,
}

impl Clock {
    fn ready(&self) -> Option<Instant> {
        match self.retry {
            Retry::Ready => Some(self.next_start),
            Retry::At(retry) => Some(retry.max(self.next_start)),
            Retry::Indefinite => None,
        }
    }
}

impl Origin {
    pub(super) fn backoff(&self, headers: &header::HeaderMap) {
        let mut clock = self.clock.lock().unwrap_or_else(|e| e.into_inner());
        let fallback = self
            .fallback_backoff
            .saturating_mul(1u32 << clock.failures.min(5))
            .min(MAX_FALLBACK_BACKOFF);
        clock.failures = clock.failures.saturating_add(1);
        let wall_clock = SystemTime::now();
        let now = Instant::now();
        let delay = headers
            .get_all(header::RETRY_AFTER)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|value| retry_after(value, wall_clock))
            .max()
            .unwrap_or(fallback);
        let Some(deadline) = now.checked_add(delay) else {
            clock.retry = Retry::Indefinite;
            return;
        };
        match clock.retry {
            Retry::Ready => clock.retry = Retry::At(deadline),
            Retry::At(previous) => clock.retry = Retry::At(previous.max(deadline)),
            Retry::Indefinite => {}
        }
    }

    pub(super) fn success(&self) {
        // An older in-flight success must not erase a newer 429's deadline.
        self.clock
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .failures = 0;
    }
}

fn throttled() -> AppError {
    AppError::Other("来源请求处于限流冷却或并发队列已超时，请稍后重试".into())
}

fn component(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

pub(super) fn request_key(request: &Request) -> Option<Key> {
    if request.method() != Method::GET
        || request.body().is_some()
        || !request_cacheable(request.headers())
    {
        return None;
    }
    let mut hash = Sha256::new();
    component(&mut hash, b"provider-json-v1");
    component(&mut hash, request.url().as_str().as_bytes());
    // Hash every header, not a fixed credential-name list: apikey, x-api-key,
    // Authorization, Cookie and future tenant/representation headers all matter.
    let mut names: Vec<_> = request.headers().keys().collect();
    names.sort_unstable_by(|a, b| a.as_str().cmp(b.as_str()));
    for name in names {
        component(&mut hash, name.as_str().as_bytes());
        let values = request.headers().get_all(name);
        hash.update((values.iter().count() as u64).to_be_bytes());
        for value in values.iter() {
            component(&mut hash, value.as_bytes());
        }
    }
    Some(hash.finalize().into())
}

fn request_cacheable(headers: &header::HeaderMap) -> bool {
    for value in headers.get_all(header::CACHE_CONTROL) {
        let Ok(value) = value.to_str() else {
            return false;
        };
        for part in value.split(',') {
            let (name, value) = part.trim().split_once('=').unwrap_or((part.trim(), ""));
            let name = name.trim();
            if name.eq_ignore_ascii_case("no-cache")
                || name.eq_ignore_ascii_case("no-store")
                || name.eq_ignore_ascii_case("max-age") && value.trim().trim_matches('"') == "0"
            {
                return false;
            }
        }
    }
    !headers.contains_key(header::PRAGMA)
}

pub(super) fn response_ttl(headers: &header::HeaderMap, max: Duration) -> Option<Duration> {
    if headers.contains_key(header::SET_COOKIE)
        || headers.get_all(header::VARY).iter().any(|value| {
            value
                .to_str()
                .map_or(true, |value| value.split(',').any(|v| v.trim() == "*"))
        })
    {
        return None;
    }
    let mut ttl = max;
    for value in headers.get_all(header::CACHE_CONTROL) {
        let value = value.to_str().ok()?;
        for part in value.split(',') {
            let (name, value) = part.trim().split_once('=').unwrap_or((part.trim(), ""));
            let name = name.trim();
            if name.eq_ignore_ascii_case("no-cache") || name.eq_ignore_ascii_case("no-store") {
                return None;
            }
            if name.eq_ignore_ascii_case("max-age") {
                ttl = ttl.min(Duration::from_secs(
                    value.trim().trim_matches('"').parse().ok()?,
                ));
            }
        }
    }
    if let Some(age) = headers.get(header::AGE) {
        ttl = ttl.saturating_sub(Duration::from_secs(age.to_str().ok()?.parse().ok()?));
    }
    if let Some(expires) = headers.get(header::EXPIRES) {
        ttl = ttl.min(
            http_date(expires.to_str().ok()?)?
                .duration_since(SystemTime::now())
                .unwrap_or(Duration::ZERO),
        );
    }
    (!ttl.is_zero()).then_some(ttl)
}

pub(super) fn retry_after(raw: &str, now: SystemTime) -> Option<Duration> {
    let raw = raw.trim();
    if !raw.is_empty() && raw.bytes().all(|byte| byte.is_ascii_digit()) {
        // Overflowing delta-seconds means effectively indefinite, not absent.
        return Some(Duration::from_secs(raw.parse().unwrap_or(u64::MAX)));
    }
    Some(
        http_date(raw)?
            .duration_since(now)
            .unwrap_or(Duration::ZERO),
    )
}

fn http_date(raw: &str) -> Option<SystemTime> {
    let date = chrono::DateTime::parse_from_rfc2822(raw)
        .map(|date| date.with_timezone(&chrono::Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(raw, "%A, %d-%b-%y %H:%M:%S GMT")
                .map(|date| date.and_utc())
        })
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(raw, "%a %b %e %H:%M:%S %Y")
                .map(|date| date.and_utc())
        })
        .ok()?;
    Some(SystemTime::from(date))
}
