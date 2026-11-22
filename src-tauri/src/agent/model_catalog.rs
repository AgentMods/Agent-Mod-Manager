use crate::{
    error::{AppError, AppResult},
    settings::{parse_model_base_url, valid_model_id, ReasoningEffort},
};
use reqwest::RequestBuilder;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use url::Url;

const METADATA_URL: &str = "https://basellm.github.io/llm-metadata/api/all.json";
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const RUNTIME_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
const CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const FAILED_CACHE_TTL: Duration = Duration::from_secs(60);
const MAX_CACHED_SERVICES: usize = 32;

#[derive(Deserialize)]
struct ServiceModels {
    data: Vec<ServiceModel>,
}

#[derive(Deserialize)]
struct ServiceModel {
    id: String,
    #[serde(flatten)]
    fields: Map<String, Value>,
}

type Catalog = BTreeMap<String, Provider>;

#[derive(Deserialize)]
struct Provider {
    api: Option<String>,
    models: BTreeMap<String, CatalogModel>,
}

#[derive(Deserialize)]
struct CatalogModel {
    limit: Option<Limits>,
    reasoning_options: Option<Vec<Value>>,
}

#[derive(Deserialize)]
struct Limits {
    context: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelMetadata {
    id: String,
    reasoning_levels: Vec<ReasoningEffort>,
}

fn normalized_base(base: &Url) -> &str {
    base.as_str().trim_end_matches('/')
}

async fn service_models(
    base: &Url,
    api_key: Option<&str>,
    timeout: Duration,
) -> AppResult<ServiceModels> {
    let endpoint = format!("{}/models", normalized_base(base));
    let mut request = client()?.get(endpoint).timeout(timeout);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    fetch_json(request, "读取模型列表").await
}

pub(crate) async fn list(base: &Url, api_key: Option<&str>) -> AppResult<Vec<String>> {
    let response = service_models(base, api_key, Duration::from_secs(12)).await?;
    let mut ids: Vec<_> = response
        .data
        .into_iter()
        .map(|model| model.id)
        .filter(|id| valid_model_id(id) && !api_key.is_some_and(|key| id.contains(key)))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

async fn load_catalog(url: &str, timeout: Duration) -> AppResult<Catalog> {
    // The provider URL, credentials, and model IDs are used for local matching only.
    fetch_json(
        client()?.get(url).timeout(timeout),
        "读取 BaseLLM 模型元数据",
    )
    .await
}

pub(crate) async fn reasoning_metadata(
    base_url: &str,
    model_ids: &[String],
) -> AppResult<Vec<ModelMetadata>> {
    reasoning_metadata_from(METADATA_URL, base_url, model_ids).await
}

async fn reasoning_metadata_from(
    catalog_url: &str,
    base_url: &str,
    model_ids: &[String],
) -> AppResult<Vec<ModelMetadata>> {
    let base = parse_model_base_url(base_url)?;
    if model_ids.iter().any(|id| !valid_model_id(id)) {
        return Err(AppError::Invalid("模型元数据查询参数无效".into()));
    }
    let catalog = load_catalog(catalog_url, Duration::from_secs(12)).await?;
    Ok(model_ids
        .iter()
        .filter_map(|id| lookup_reasoning(&catalog, &base, id))
        .collect())
}

fn catalog_models<'a>(
    catalog: &'a Catalog,
    base: &Url,
    id: &str,
    allow_unscoped: bool,
) -> Vec<&'a CatalogModel> {
    let exact: Vec<_> = catalog
        .iter()
        .filter_map(|(key, provider)| provider.models.get(id).map(|model| (key, provider, model)))
        .collect();
    let matching_endpoints: Vec<_> = exact
        .iter()
        .filter(|(key, provider, _)| {
            let api = provider.api.as_deref().or(match key.as_str() {
                "openai" => Some("https://api.openai.com/v1"),
                _ => None,
            });
            api.and_then(|api| Url::parse(api).ok()).is_some_and(|api| {
                let api = normalized_base(&api);
                let base = normalized_base(base);
                api == base || base.strip_suffix("/v1") == Some(api)
            })
        })
        .map(|(_, _, model)| *model)
        .collect();
    if !matching_endpoints.is_empty() {
        matching_endpoints
    } else if let Some(model) = id
        .split_once('/')
        .and_then(|(provider, model)| catalog.get(provider)?.models.get(model))
    {
        vec![model]
    } else if allow_unscoped {
        exact.into_iter().map(|(_, _, model)| model).collect()
    } else {
        Vec::new()
    }
}

fn lookup_reasoning(catalog: &Catalog, base: &Url, id: &str) -> Option<ModelMetadata> {
    let mut matching = catalog_models(catalog, base, id, true).into_iter();
    let mut levels = reasoning_levels(matching.next()?);
    for model in matching {
        let other = reasoning_levels(model);
        levels.retain(|level| other.contains(level));
    }
    Some(ModelMetadata {
        id: id.to_owned(),
        reasoning_levels: levels,
    })
}

fn parse_window(value: &Value) -> Option<usize> {
    usize::try_from(value.as_u64()?)
        .ok()
        .filter(|&window| window > 0)
}

fn lookup_window(catalog: &Catalog, base: &Url, id: &str) -> Option<usize> {
    // The app's default gateway does not require provider-qualified IDs. Elsewhere
    // a bare ID is not evidence that a custom deployment uses the catalog window.
    let default_gateway = normalized_base(base) == "https://one.gloscai.com/v1";
    let mut entries = catalog_models(catalog, base, id, default_gateway).into_iter();
    let window = |m: &CatalogModel| parse_window(m.limit.as_ref()?.context.as_ref()?);
    let first = window(entries.next()?)?;
    entries
        .all(|model| window(model) == Some(first))
        .then_some(first)
}

fn native_window(fields: &Map<String, Value>) -> Option<usize> {
    // OpenRouter exposes context_length; vLLM exposes its configured max_model_len.
    // Input-, output-, and generation-token limits do not describe a full window.
    [
        "context_window",
        "context_length",
        "max_context_length",
        "max_context_tokens",
        "max_model_len",
    ]
    .into_iter()
    .filter_map(|name| fields.get(name).and_then(parse_window))
    .chain(
        fields
            .get("limit")
            .and_then(|limit| limit.get("context"))
            .and_then(parse_window),
    )
    .min()
}

fn known_window(base: &Url, id: &str) -> Option<usize> {
    // DeepSeek V4 rollout routed the legacy API aliases to V4 Flash with a 1M window.
    // This identifies capacity while the alias is accepted, not model availability.
    // https://api-docs.deepseek.com/news/news260424
    // https://api-docs.deepseek.com/quick_start/pricing/
    let official = matches!(
        normalized_base(base),
        "https://api.deepseek.com" | "https://api.deepseek.com/v1"
    );
    let default_gateway = normalized_base(base) == "https://one.gloscai.com/v1";
    let flash = matches!(id, "deepseek-v4-flash" | "deepseek/deepseek-v4-flash");
    ((official && matches!(id, "deepseek-chat" | "deepseek-reasoner"))
        || ((official || default_gateway) && flash))
        .then_some(1_048_576)
}

type NativeWindows = BTreeMap<String, Option<usize>>;
type ServiceKey = (String, Option<[u8; 32]>);
type Cache<T> = Mutex<Option<(Instant, Option<Arc<T>>)>>;
type ServiceCaches = VecDeque<(ServiceKey, Arc<Cache<NativeWindows>>)>;

#[derive(Default)]
struct WindowResolver {
    services: Mutex<ServiceCaches>,
    catalog: Cache<Catalog>,
}

async fn cached<T>(cache: &Cache<T>, load: impl Future<Output = Option<T>>) -> Option<Arc<T>> {
    // Single-flight per service/catalog. Unrelated services never wait on this I/O.
    let mut entry = cache.lock().await;
    if let Some((expires, value)) = entry.as_ref() {
        if Instant::now() < *expires {
            return value.clone();
        }
    }
    let value = load.await.map(Arc::new);
    let ttl = if value.is_some() {
        CACHE_TTL
    } else {
        FAILED_CACHE_TTL
    };
    *entry = Some((Instant::now() + ttl, value.clone()));
    value
}

impl WindowResolver {
    async fn resolve(
        &self,
        base: &Url,
        api_key: Option<&str>,
        id: &str,
        catalog_url: Option<&str>,
    ) -> Option<usize> {
        if !valid_model_id(id) {
            return None;
        }
        // Entitlements may differ across credentials. Cache by digest, never by the key itself.
        let service_key = (
            normalized_base(base).to_owned(),
            api_key.map(|key| Sha256::digest(key).into()),
        );
        let service = {
            let mut services = self.services.lock().await;
            let cache = services
                .iter()
                .position(|(key, _)| *key == service_key)
                .and_then(|index| services.remove(index))
                .map(|(_, cache)| cache)
                .unwrap_or_default();
            if services.len() == MAX_CACHED_SERVICES {
                services.pop_front();
            }
            services.push_back((service_key, Arc::clone(&cache)));
            cache
        };
        let models = cached(&service, async {
            let response = service_models(base, api_key, RUNTIME_LOOKUP_TIMEOUT)
                .await
                .ok()?;
            let mut windows = NativeWindows::new();
            for model in response.data {
                if !valid_model_id(&model.id) || api_key.is_some_and(|key| model.id.contains(key)) {
                    continue;
                }
                let window = native_window(&model.fields);
                windows
                    .entry(model.id)
                    .and_modify(|previous| {
                        if *previous != window {
                            *previous = None;
                        }
                    })
                    .or_insert(window);
            }
            Some(windows)
        })
        .await;
        if let Some(window) = models
            .as_ref()
            .and_then(|models| models.get(id))
            .copied()
            .flatten()
        {
            return Some(window);
        }
        if let Some(window) = known_window(base, id) {
            return Some(window);
        }
        let catalog_url = catalog_url?;
        let catalog = cached(&self.catalog, async {
            load_catalog(catalog_url, RUNTIME_LOOKUP_TIMEOUT).await.ok()
        })
        .await?;
        lookup_window(&catalog, base, id)
    }
}

/// Best-effort effective model capacity. Unknown windows never prevent a turn.
/// Successful and failed lookups are cached across turns independently of settings.
pub(crate) async fn resolve_context_window(
    base: &Url,
    api_key: Option<&str>,
    model_id: &str,
) -> Option<usize> {
    static RESOLVER: OnceLock<WindowResolver> = OnceLock::new();
    // Agent tests exercise provider metadata with a loopback mock, never the public catalog.
    let catalog = (!cfg!(test)).then_some(METADATA_URL);
    RESOLVER
        .get_or_init(WindowResolver::default)
        .resolve(base, api_key, model_id, catalog)
        .await
}

fn reasoning_levels(model: &CatalogModel) -> Vec<ReasoningEffort> {
    let mut levels: Vec<_> = model
        .reasoning_options
        .iter()
        .flatten()
        .filter(|option| option.get("type").and_then(Value::as_str) == Some("effort"))
        .filter_map(|option| option.get("values")?.as_array())
        .flatten()
        .filter_map(|level| ReasoningEffort::deserialize(level).ok())
        .collect();
    levels.sort_unstable();
    levels.dedup();
    levels
}

fn client() -> AppResult<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(12))
        .build()?)
}

async fn fetch_json<T: DeserializeOwned>(request: RequestBuilder, operation: &str) -> AppResult<T> {
    let mut response = request
        .send()
        .await
        .map_err(|_| AppError::Other(format!("{operation}失败，请检查网络和 API 地址")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(AppError::Other(format!("{operation}失败 (HTTP {status})")));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AppError::Other(format!("{operation}中断")))?
    {
        if chunk.len() > MAX_RESPONSE_BYTES - bytes.len() {
            return Err(AppError::Other(format!(
                "{operation}失败，响应超过大小限制"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| AppError::Other(format!("{operation}失败，响应格式无效")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::get,
        Router,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    const CATALOG: &str = include_str!("../../tests/fixtures/model-catalog/basellm.json");
    const NATIVE: &str = include_str!("../../tests/fixtures/model-catalog/provider.json");

    #[test]
    fn basellm_uses_context_not_input_and_matches_provider_scope() {
        let catalog = serde_json::from_str(CATALOG).unwrap();
        let provider = Url::parse("https://first.example/v1/").unwrap();
        assert_eq!(lookup_window(&catalog, &provider, "shared"), Some(128_000));
        assert_eq!(lookup_window(&catalog, &provider, "input-only"), None);
        assert_eq!(
            lookup_reasoning(&catalog, &provider, "shared")
                .unwrap()
                .reasoning_levels,
            vec![ReasoningEffort::Low, ReasoningEffort::High]
        );
        let gateway = Url::parse("https://one.gloscai.com/v1").unwrap();
        assert_eq!(lookup_window(&catalog, &gateway, "shared"), None);
        assert!(lookup_reasoning(&catalog, &gateway, "shared")
            .unwrap()
            .reasoning_levels
            .is_empty());
        assert_eq!(
            lookup_window(&catalog, &gateway, "second/shared"),
            Some(32_000)
        );
        let custom = Url::parse("https://custom.example/v1").unwrap();
        assert_eq!(lookup_window(&catalog, &custom, "unique"), None);
        assert_eq!(lookup_window(&catalog, &provider, "unknown"), None);
    }

    #[test]
    fn official_deepseek_windows_do_not_masquerade_as_custom_provider_aliases() {
        for base in ["https://api.deepseek.com", "https://api.deepseek.com/v1/"] {
            let base = Url::parse(base).unwrap();
            for id in [
                "deepseek/deepseek-v4-flash",
                "deepseek-v4-flash",
                "deepseek-chat",
                "deepseek-reasoner",
            ] {
                assert_eq!(known_window(&base, id), Some(1_048_576));
            }
        }
        let gateway = Url::parse("https://one.gloscai.com/v1/").unwrap();
        assert_eq!(
            known_window(&gateway, "deepseek/deepseek-v4-flash"),
            Some(1_048_576)
        );
        let unrelated = Url::parse("https://unrelated.example/v1").unwrap();
        assert_eq!(known_window(&unrelated, "deepseek-v4-flash"), None);
        assert_eq!(known_window(&unrelated, "deepseek-chat"), None);
    }

    #[tokio::test]
    async fn provider_listing_and_reasoning_lookup_preserve_large_model_lists() {
        let ids: Vec<_> = (0..206)
            .map(|index| format!("fixture-{index:03}"))
            .collect();
        let list_body = serde_json::json!({"data": ids.iter().rev().map(|id| serde_json::json!({"id":id})).collect::<Vec<_>>()}).to_string();
        let catalog_models: BTreeMap<_, _> = ids.iter().map(|id| {
            (id, serde_json::json!({"reasoning_options":[{"type":"effort","values":["medium"]}]}))
        }).collect();
        let catalog_body = serde_json::json!({"fixture":{"models":catalog_models}}).to_string();
        let router = Router::new()
            .route("/v1/models", get(move || async move { list_body }))
            .route("/catalog", get(move || async move { catalog_body }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let base = Url::parse(&format!("http://{addr}/v1/")).unwrap();
        assert_eq!(list(&base, None).await.unwrap(), ids);
        let rows = reasoning_metadata_from(&format!("http://{addr}/catalog"), base.as_str(), &ids)
            .await
            .unwrap();
        assert_eq!(
            rows.iter().map(|row| &row.id).collect::<Vec<_>>(),
            ids.iter().collect::<Vec<_>>()
        );
        assert!(rows
            .iter()
            .all(|row| row.reasoning_levels == [ReasoningEffort::Medium]));
        server.abort();
    }

    #[tokio::test]
    async fn runtime_prefers_authenticated_native_windows_and_caches_results_and_failures() {
        let service_reads = Arc::new(AtomicUsize::new(0));
        let catalog_reads = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route(
                "/v1/models",
                get({
                    let reads = Arc::clone(&service_reads);
                    move |headers: HeaderMap| {
                        reads.fetch_add(1, Ordering::Relaxed);
                        async move {
                            if headers
                                .get("authorization")
                                .is_some_and(|value| value == "Bearer fixture-key")
                            {
                                (StatusCode::OK, NATIVE).into_response()
                            } else {
                                StatusCode::UNAUTHORIZED.into_response()
                            }
                        }
                    }
                }),
            )
            .route(
                "/catalog",
                get({
                    let reads = Arc::clone(&catalog_reads);
                    move || {
                        reads.fetch_add(1, Ordering::Relaxed);
                        async { CATALOG }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let base = Url::parse(&format!("http://{addr}/v1/")).unwrap();
        let catalog = format!("http://{addr}/catalog");
        let resolve = WindowResolver::default();
        let fetch = |key, id| resolve.resolve(&base, key, id, Some(&catalog));
        let auth = Some("fixture-key");
        assert_eq!(fetch(auth, "second/shared").await, Some(65_536));
        assert_eq!(fetch(auth, "local").await, Some(24_576));
        assert_eq!(catalog_reads.load(Ordering::Relaxed), 0);
        assert_eq!(fetch(auth, "invalid").await, None);
        assert_eq!(fetch(auth, "duplicate").await, None);
        assert_eq!(service_reads.load(Ordering::Relaxed), 1);
        assert_eq!(fetch(None, "second/shared").await, Some(32_000));
        assert_eq!(fetch(None, "local").await, None);
        assert_eq!(service_reads.load(Ordering::Relaxed), 2);
        assert_eq!(catalog_reads.load(Ordering::Relaxed), 1);
        server.abort();
    }
}
