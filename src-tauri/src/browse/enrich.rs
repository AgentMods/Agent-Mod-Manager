//! Optional, read-only enrichment of public metadata from authorized browse snapshots.
//! This module is not an IPC command; callers resolve provider/work identities first.
use crate::{
    agent::client::{ModelClient, ModelConfig},
    app_locale,
    db::Db,
    error::AppError,
    settings::Settings,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashSet, future::Future, time::Duration};
use tokio::sync::{Semaphore, SemaphorePermit};
use tokio_util::sync::CancellationToken;

mod cache;
mod taxonomy;

pub(crate) use taxonomy::ModType;

const ITEM_RULES: &str = include_str!("enrich/items.txt");
const QUERY_RULES: &str = include_str!("enrich/query.txt");
const MAX_ITEMS: usize = 24;
const MAX_TAGS: usize = 8;
const MAX_RAW_TAGS: usize = 32;
const MAX_RAW_TAG_BYTES: usize = 512;
const MAX_TAG_BYTES: usize = 160;
const MAX_ID_BYTES: usize = 512;
const MAX_TITLE_BYTES: usize = 480;
const MAX_SUMMARY_BYTES: usize = 1200;
const MAX_KIND_BYTES: usize = 160;
const MAX_QUERY_BYTES: usize = 240;
// Also bound pre-sanitization costs. Provider contracts independently cap their results.
const MAX_RAW_TEXT_BYTES: usize = 64 * 1024;
const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_REPLY_BYTES: usize = 64 * 1024;
const MAX_REASONING_BYTES: usize = 128 * 1024;
const MODEL_TIMEOUT: Duration = Duration::from_secs(13);
const MODEL_QUEUE_TIMEOUT: Duration = Duration::from_secs(6);
const QUERY_TIMEOUT: Duration = Duration::from_secs(5);
static MODEL_SLOTS: Semaphore = Semaphore::const_new(2);
static MODEL_QUEUE_SLOTS: Semaphore = Semaphore::const_new(4);

#[derive(Clone, Copy)]
pub(crate) struct EnrichmentInput<'a> {
    pub source: &'a str,
    pub item_id: &'a str,
    pub title: &'a str,
    pub summary: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub tags: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BrowseTag {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EnrichedItem {
    pub title: String,
    pub summary: Option<String>,
    pub type_key: ModType,
    pub tags: Vec<BrowseTag>,
}

pub(crate) fn fallback_item(input: EnrichmentInput<'_>) -> EnrichedItem {
    let tags = taxonomy::fallback_tags(input.tags);
    EnrichedItem {
        title: input.title.to_owned(),
        summary: input.summary.map(str::to_owned),
        type_key: taxonomy::fallback_type(input.kind, &tags),
        tags,
    }
}

/// `locale` is captured by the backend when scheduling this page. The caller must
/// associate it with the snapshot and avoid serving it after a locale change.
pub(crate) async fn enrich_items(
    settings: &Settings,
    inputs: &[EnrichmentInput<'_>],
    locale: &str,
    cancel: &CancellationToken,
) -> Vec<EnrichedItem> {
    enrich_with(&settings.db, inputs, locale, cancel, |payload| {
        model_request(settings, ITEM_RULES, payload, MODEL_TIMEOUT, cancel)
    })
    .await
}

/// A translated query is an optional extra lane, never a replacement for the
/// explicitly submitted original. Blank, unchanged or invalid results add no lane.
pub(crate) async fn translate_query(
    settings: &Settings,
    original: &str,
    target_locale: &str,
    cancel: &CancellationToken,
) -> Option<String> {
    let app_locale = app_locale::effective(&settings.db);
    translate_with(
        &settings.db,
        original,
        &app_locale,
        target_locale,
        cancel,
        |payload| model_request(settings, QUERY_RULES, payload, QUERY_TIMEOUT, cancel),
    )
    .await
}

fn known_locale(locale: &str) -> bool {
    app_locale::LANGUAGES
        .iter()
        .any(|(code, _)| *code == locale)
}

fn clip(text: &str, bytes: usize) -> &str {
    let end = text.floor_char_boundary(bytes.min(text.len()));
    text[..end].trim()
}

fn fits(text: Option<&str>, bytes: usize) -> bool {
    text.is_none_or(|text| text.len() <= bytes)
}

impl EnrichmentInput<'_> {
    fn valid(&self) -> bool {
        !self.source.is_empty()
            && self.source.len() <= MAX_ID_BYTES
            && !self.item_id.is_empty()
            && self.item_id.len() <= MAX_ID_BYTES
            && fits(Some(self.title), MAX_RAW_TEXT_BYTES)
            && !self.title.trim().is_empty()
            && fits(self.summary, MAX_RAW_TEXT_BYTES)
            && fits(self.kind, MAX_RAW_TEXT_BYTES)
            && self.tags.len() <= MAX_RAW_TAGS
            && self.tags.iter().all(|tag| tag.len() <= MAX_RAW_TAG_BYTES)
    }

    fn packet(&self, index: usize, original: &EnrichedItem) -> Value {
        json!({
            "index":index,
            "title":clip(self.title, MAX_TITLE_BYTES),
            "summary":self.summary.map(|text| clip(text, MAX_SUMMARY_BYTES)),
            "kind":self.kind.map(|text| clip(text, MAX_KIND_BYTES)),
            "tags":original.tags,
        })
    }
}

async fn enrich_with<F: Future<Output = Option<String>>>(
    db: &Db,
    inputs: &[EnrichmentInput<'_>],
    locale: &str,
    cancel: &CancellationToken,
    ask: impl FnOnce(String) -> F,
) -> Vec<EnrichedItem> {
    let mut enriched: Vec<_> = inputs.iter().copied().map(fallback_item).collect();
    if inputs.is_empty() || !known_locale(locale) || cancel.is_cancelled() {
        return enriched;
    }
    let keys: Vec<_> = inputs
        .iter()
        .take(MAX_ITEMS)
        .map(|input| input.valid().then(|| cache::item_key(input, locale)))
        .collect();
    let cached = cache::read::<EnrichedItem>(db, &keys).unwrap_or_default();
    let mut pending = [false; MAX_ITEMS];
    let mut packets = Vec::new();
    // Track the encoded packet sizes (not raw text lengths): control characters
    // in third-party metadata may expand on JSON serialization.
    let mut bytes = 128;
    for (index, key) in keys.iter().enumerate() {
        if key.is_none() {
            continue;
        }
        if let Some(Some(item)) = cached.get(index) {
            enriched[index] = item.clone();
        } else {
            let packet = inputs[index].packet(index, &enriched[index]);
            let size = packet.to_string().len() + 1;
            if bytes + size <= MAX_INPUT_BYTES {
                bytes += size;
                pending[index] = true;
                packets.push(packet);
            }
        }
    }
    if packets.is_empty() || cancel.is_cancelled() {
        return enriched;
    }
    let payload = json!({"targetLocale":locale,"items":packets}).to_string();
    let Some(reply) = ask(payload).await.filter(|_| !cancel.is_cancelled()) else {
        return enriched;
    };
    let Some(reply) = decode::<ItemReply>(&reply) else {
        return enriched;
    };
    let mut seen = HashSet::new();
    if reply
        .items
        .iter()
        .any(|row| !pending.get(row.index).copied().unwrap_or(false) || !seen.insert(row.index))
    {
        return enriched;
    }
    let mut writes = Vec::new();
    for row in reply.items {
        let index = row.index;
        if let Some(item) = row.validate(inputs[index], &enriched[index]) {
            if let Some(key) = &keys[index] {
                writes.push((key.as_str(), item.clone()));
            }
            enriched[index] = item;
        }
    }
    let _ = cache::write(db, &writes);
    enriched
}

fn clean_text(text: &str, limit: usize, multiline: bool) -> Option<String> {
    let value = text.trim();
    if value.is_empty()
        || value.len() > limit
        || value
            .chars()
            .any(|ch| ch.is_control() && !(multiline && matches!(ch, '\n' | '\r' | '\t')))
    {
        None
    } else {
        Some(value.to_owned())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemReply {
    items: Vec<ModelItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelItem {
    index: usize,
    title: String,
    // Require the key even when the provider has no summary. A completely
    // missing field is a model protocol failure, not an absent original summary.
    #[serde(deserialize_with = "Option::deserialize")]
    summary: Option<String>,
    type_key: ModType,
    tags: Vec<BrowseTag>,
}

impl ModelItem {
    fn validate(self, input: EnrichmentInput<'_>, original: &EnrichedItem) -> Option<EnrichedItem> {
        let title = clean_text(&self.title, MAX_TITLE_BYTES, false)?;
        let summary = match (input.summary.filter(|s| !s.trim().is_empty()), self.summary) {
            (Some(_), Some(summary)) => Some(clean_text(&summary, MAX_SUMMARY_BYTES, true)?),
            (None, None) => original.summary.clone(),
            _ => return None,
        };
        if self.tags.len() > MAX_TAGS {
            return None;
        }
        let mut tags = Vec::new();
        for tag in self.tags {
            if !taxonomy::is_stable_tag(&tag.key)
                && !original.tags.iter().any(|raw| raw.key == tag.key)
            {
                return None;
            }
            let label = clean_text(&tag.label, MAX_TAG_BYTES, false)?;
            if !tags.iter().any(|old: &BrowseTag| old.key == tag.key) {
                tags.push(BrowseTag {
                    key: tag.key,
                    label,
                });
            }
        }
        if tags.is_empty() {
            tags.clone_from(&original.tags);
        }
        Some(EnrichedItem {
            title,
            summary,
            type_key: if original.type_key == ModType::Unknown {
                self.type_key
            } else {
                original.type_key
            },
            tags,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryReply {
    query: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CachedQuery {
    alternate: Option<String>,
}

fn query_value(text: &str) -> Option<String> {
    if text.len() > MAX_QUERY_BYTES || text.chars().any(char::is_control) {
        return None;
    }
    let text = clean_text(text, MAX_QUERY_BYTES, false)?;
    // URLs/paths are not translated into a second query lane, and a punctuation
    // only or numeric search has nothing to translate.
    if !text.chars().any(char::is_alphabetic)
        || text.contains(['/', '\\', '<', '>', '`'])
        || text.contains("://")
    {
        return None;
    }
    Some(text)
}

async fn translate_with<F: Future<Output = Option<String>>>(
    db: &Db,
    original: &str,
    app_locale: &str,
    target_locale: &str,
    cancel: &CancellationToken,
    ask: impl FnOnce(String) -> F,
) -> Option<String> {
    let query = query_value(original)?;
    if !known_locale(app_locale) || !known_locale(target_locale) || cancel.is_cancelled() {
        return None;
    }
    let key = cache::query_key(original, app_locale, target_locale);
    let cached = cache::read::<CachedQuery>(db, &[Some(key.clone())])
        .ok()
        .and_then(|mut results| results.pop().flatten());
    let comparable = |text: &str| {
        text.split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
    };
    let distinct = |translated: &str| comparable(translated) != comparable(&query);
    if let Some(cached) = cached {
        return cached
            .alternate
            .as_deref()
            .and_then(query_value)
            .filter(|value| distinct(value));
    }
    let payload = json!({"appLocale":app_locale,"targetLocale":target_locale,"query":query});
    let reply = ask(payload.to_string()).await?;
    if cancel.is_cancelled() {
        return None;
    }
    let reply = decode::<QueryReply>(&reply)?;
    let translated = query_value(&reply.query)?;
    let alternate = distinct(&translated).then_some(translated);
    // A valid no-op is a negative cache hit without storing the original text.
    let _ = cache::write(
        db,
        &[(
            key.as_str(),
            CachedQuery {
                alternate: alternate.clone(),
            },
        )],
    );
    alternate
}

fn json_body(text: &str) -> &str {
    let text = text.trim();
    text.strip_prefix("```json\n")
        .or_else(|| text.strip_prefix("```json\r\n"))
        .or_else(|| text.strip_prefix("```\n"))
        .or_else(|| text.strip_prefix("```\r\n"))
        .and_then(|body| body.strip_suffix("```"))
        .map_or(text, str::trim)
}

fn decode<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    if text.len() > MAX_REPLY_BYTES {
        return None;
    }
    serde_json::from_str(json_body(text)).ok()
}

fn contains_credential(text: &str, key: &str) -> bool {
    fn contains(value: &Value, key: &str) -> bool {
        match value {
            Value::String(text) => text.contains(key),
            Value::Object(fields) => fields
                .iter()
                .any(|(name, child)| name.contains(key) || contains(child, key)),
            Value::Array(values) => values.iter().any(|child| contains(child, key)),
            _ => false,
        }
    }
    !key.is_empty()
        && (text.contains(key) || decode::<Value>(text).is_some_and(|value| contains(&value, key)))
}

async fn model_request(
    settings: &Settings,
    rules: &str,
    payload: String,
    deadline: Duration,
    cancel: &CancellationToken,
) -> Option<String> {
    if payload.len() > MAX_INPUT_BYTES || cancel.is_cancelled() {
        return None;
    }
    let _permit = acquire_model_slot(&MODEL_QUEUE_SLOTS, &MODEL_SLOTS, cancel).await?;
    let config = ModelConfig::configured(settings).ok()??;
    request_once(&config, rules, payload, deadline, cancel).await
}

async fn acquire_model_slot<'a>(
    queue: &Semaphore,
    active: &'a Semaphore,
    cancel: &CancellationToken,
) -> Option<SemaphorePermit<'a>> {
    // The two search-language variants can initially take both active slots;
    // allow the first page to wait briefly without building an unbounded queue.
    let _waiting = queue.try_acquire().ok()?;
    tokio::select! {
        biased;
        _ = cancel.cancelled() => None,
        permit = tokio::time::timeout(MODEL_QUEUE_TIMEOUT, active.acquire()) => permit.ok()?.ok(),
    }
}

async fn request_once(
    config: &ModelConfig,
    rules: &str,
    payload: String,
    deadline: Duration,
    cancel: &CancellationToken,
) -> Option<String> {
    let blocked = |value: &str| {
        config
            .api_key
            .as_deref()
            .is_some_and(|key| contains_credential(value, key))
    };
    if blocked(&payload) || blocked(rules) {
        return None;
    }
    let messages = [
        json!({"role":"system","content":rules}),
        json!({"role":"user","content":payload}),
    ];
    let client = ModelClient::new().ok()?;
    let completion = tokio::time::timeout(
        deadline,
        client.stream_once(config, &messages, cancel, |_, response| {
            if response.content.len() > MAX_REPLY_BYTES
                || response.reasoning.len() > MAX_REASONING_BYTES
            {
                return Err(AppError::Invalid(
                    "Browse model reply exceeded its limit".into(),
                ));
            }
            Ok(())
        }),
    )
    .await
    .ok()?
    .ok()?;
    if cancel.is_cancelled() || blocked(&completion.content) {
        return None;
    }
    Some(completion.content)
}

#[cfg(test)]
mod tests;
