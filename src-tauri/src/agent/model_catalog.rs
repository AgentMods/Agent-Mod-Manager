use crate::{
    error::{AppError, AppResult},
    settings::{
        parse_model_base_url, valid_model_id, ReasoningEffort, MAX_CONFIGURED_MODELS,
        MAX_CONTEXT_BUDGET,
    },
};
use reqwest::RequestBuilder;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};
use url::Url;

const METADATA_URL: &str = "https://basellm.github.io/llm-metadata/api/all.json";
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Deserialize)]
struct ServiceModels {
    data: Vec<ServiceModel>,
}

#[derive(Deserialize)]
struct ServiceModel {
    id: String,
}

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
    context: Option<usize>,
    input: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelMetadata {
    id: String,
    context_budget: Option<usize>,
    reasoning_levels: Vec<ReasoningEffort>,
}

pub(crate) async fn list(base: &Url, api_key: Option<&str>) -> AppResult<Vec<String>> {
    let endpoint = format!("{}/models", base.as_str().trim_end_matches('/'));
    let mut request = client()?.get(endpoint);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let response: ServiceModels = fetch_json(request, "读取模型列表").await?;
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

pub(crate) async fn metadata(
    base_url: &str,
    model_ids: &[String],
) -> AppResult<Vec<ModelMetadata>> {
    let base = parse_model_base_url(base_url)?;
    if model_ids.len() > MAX_CONFIGURED_MODELS || model_ids.iter().any(|id| !valid_model_id(id)) {
        return Err(AppError::Invalid("模型元数据查询参数无效".into()));
    }
    // The provider URL and model IDs are used for local matching only.
    let catalog: BTreeMap<String, Provider> =
        fetch_json(client()?.get(METADATA_URL), "读取 BaseLLM 模型元数据").await?;
    Ok(model_ids
        .iter()
        .filter_map(|id| lookup_metadata(&catalog, &base, id))
        .collect())
}

fn lookup_metadata(
    catalog: &BTreeMap<String, Provider>,
    base: &Url,
    id: &str,
) -> Option<ModelMetadata> {
    let exact: Vec<_> = catalog
        .values()
        .filter_map(|provider| provider.models.get(id).map(|model| (provider, model)))
        .collect();
    let matching_endpoints: Vec<_> = exact
        .iter()
        .filter(|(provider, _)| {
            provider
                .api
                .as_deref()
                .and_then(|api| Url::parse(api).ok())
                .is_some_and(|api| {
                    api.as_str().trim_end_matches('/') == base.as_str().trim_end_matches('/')
                })
        })
        .copied()
        .collect();
    let namespaced = || {
        let (provider_id, model_id) = id.split_once('/')?;
        catalog.get(provider_id)?.models.get(model_id)
    };
    let mut matching = if !matching_endpoints.is_empty() {
        matching_endpoints
            .into_iter()
            .map(|(_, model)| model)
            .collect()
    } else if let Some(model) = namespaced() {
        vec![model]
    } else {
        exact
            .into_iter()
            .map(|(_, model)| model)
            .collect::<Vec<_>>()
    }
    .into_iter();

    let first = matching.next()?;
    let mut context_budget = context_budget(first);
    let mut reasoning_levels = reasoning_levels(first);
    for model in matching {
        if context_budget != self::context_budget(model) {
            context_budget = None;
        }
        let levels = self::reasoning_levels(model);
        reasoning_levels.retain(|level| levels.contains(level));
    }
    Some(ModelMetadata {
        id: id.to_owned(),
        context_budget,
        reasoning_levels,
    })
}

fn context_budget(model: &CatalogModel) -> Option<usize> {
    let limit = model.limit.as_ref()?;
    limit
        .input
        .or(limit.context)
        .filter(|value| (1..=MAX_CONTEXT_BUDGET).contains(value))
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
    use serde_json::json;

    #[test]
    fn metadata_prefers_matching_provider_and_only_exposes_supported_efforts() {
        let catalog = serde_json::from_value(json!({
            "first": {"api": "https://first.example/v1", "models": {"shared": {
                "limit": {"context": 128000, "input": 96000},
                "reasoning_options": [{"type": "effort", "values": ["low", "high", "unknown"]}]
            }}},
            "second": {"api": "https://second.example/v1", "models": {"shared": {
                "limit": {"context": 32000},
                "reasoning_options": [{"type": "toggle"}, {"type": "budget_tokens", "max": 10000}]
            }}}
        }))
        .unwrap();
        let provider = Url::parse("https://first.example/v1/").unwrap();
        let matched = lookup_metadata(&catalog, &provider, "shared").unwrap();
        assert_eq!(matched.context_budget, Some(96000));
        assert_eq!(
            matched.reasoning_levels,
            vec![ReasoningEffort::Low, ReasoningEffort::High]
        );

        let gateway = Url::parse("https://gateway.example/v1").unwrap();
        let ambiguous = lookup_metadata(&catalog, &gateway, "shared").unwrap();
        assert_eq!(ambiguous.context_budget, None);
        assert!(ambiguous.reasoning_levels.is_empty());
        let named = lookup_metadata(&catalog, &gateway, "second/shared").unwrap();
        assert_eq!(named.context_budget, Some(32000));
        assert!(named.reasoning_levels.is_empty());
        assert!(lookup_metadata(&catalog, &gateway, "unknown").is_none());
    }
}
