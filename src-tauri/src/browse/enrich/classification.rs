use super::{
    cache, clip, decode, request_once, EnrichmentInput, ModType, ModelConfig, MAX_INPUT_BYTES,
    MAX_ITEMS, MODEL_SLOTS,
};
use crate::{db::Db, settings::Settings};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{future::Future, time::Duration};
use tokio_util::sync::CancellationToken;

const CLASSIFICATION_TIMEOUT: Duration = Duration::from_secs(40);
const RULES: &str = include_str!("classification.txt");

pub(crate) async fn classify_items(
    settings: &Settings,
    inputs: &[EnrichmentInput<'_>],
    cancel: &CancellationToken,
) -> Vec<Option<ModType>> {
    classify_with(&settings.db, inputs, cancel, |payload| async move {
        let config = ModelConfig::configured(settings).ok()??;
        if payload.len() > MAX_INPUT_BYTES {
            return None;
        }
        // At most two page workers per active search can wait here. Keep their
        // place behind current requests even if the optional translation queue fills.
        let _permit = tokio::select! {
            biased;
            _ = cancel.cancelled() => return None,
            permit = MODEL_SLOTS.acquire() => permit.ok()?,
        };
        request_once(&config, RULES, payload, CLASSIFICATION_TIMEOUT, cancel).await
    })
    .await
}

// A full 24-item page fits in one request even after JSON escaping. The model
// only needs short public excerpts to choose a key, not a translation per mod.
fn excerpt(value: &str, bytes: usize) -> String {
    clip(value, bytes)
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

fn packet(index: usize, input: &EnrichmentInput<'_>) -> Value {
    json!({
        "index": index,
        "title": excerpt(input.title, 320),
        "summary": input.summary.map(|value| excerpt(value, 400)),
        "kind": input.kind.map(|value| excerpt(value, 80)),
        "tags": input.tags.iter().take(6).map(|value| excerpt(value, 64)).collect::<Vec<_>>(),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClassificationReply {
    items: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClassifiedItem {
    index: usize,
    type_key: ModType,
}

async fn classify_with<F: Future<Output = Option<String>>>(
    db: &Db,
    inputs: &[EnrichmentInput<'_>],
    cancel: &CancellationToken,
    ask: impl FnOnce(String) -> F,
) -> Vec<Option<ModType>> {
    let mut types = vec![None; inputs.len()];
    if inputs.is_empty() || inputs.len() > MAX_ITEMS || cancel.is_cancelled() {
        return types;
    }
    let keys: Vec<_> = inputs
        .iter()
        .map(|input| input.valid().then(|| cache::category_key(input, RULES)))
        .collect();
    if let Ok(hits) = cache::read(db, &keys) {
        types = hits
            .into_iter()
            .map(|hit| hit.filter(|ty| *ty != ModType::Unknown))
            .collect();
    }
    let pending: Vec<_> = inputs
        .iter()
        .enumerate()
        .filter(|(i, _)| keys[*i].is_some() && types[*i].is_none())
        .map(|(i, input)| packet(i, input))
        .collect();
    if pending.is_empty() || cancel.is_cancelled() {
        return types;
    }
    let Some(reply) = ask(json!({"items":pending}).to_string()).await else {
        return types;
    };
    if cancel.is_cancelled() || reply.len() > 8192 {
        return types;
    }
    let Some(reply) = decode::<ClassificationReply>(&reply) else {
        return types;
    };
    // Do not trust response order or let two contradictory rows settle one item.
    // Other rows on the page can still be classified if one row is malformed.
    let mut seen = [false; MAX_ITEMS];
    let mut chosen = [None; MAX_ITEMS];
    for value in reply.items {
        let Some(index) = value["index"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
        else {
            continue;
        };
        if index >= inputs.len() || keys[index].is_none() || types[index].is_some() {
            continue;
        }
        if std::mem::replace(&mut seen[index], true) {
            chosen[index] = None;
            continue;
        }
        if let Ok(row) = serde_json::from_value::<ClassifiedItem>(value) {
            if row.type_key != ModType::Unknown {
                chosen[row.index] = Some(row.type_key);
            }
        }
    }
    let mut writes = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        if let (Some(key), Some(category)) = (key, chosen[index]) {
            types[index] = Some(category);
            writes.push((key.as_str(), category));
        }
    }
    let _ = cache::write(db, &writes);
    types
}

#[cfg(test)]
#[path = "classification_tests.rs"]
mod tests;
