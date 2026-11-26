use super::{cache, decode, json_body, model_request, query_value, MODEL_TIMEOUT};
use crate::{browse::links::public_text, db::Db, settings::Settings};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashSet, future::Future};
use tokio_util::sync::CancellationToken;

const ALIAS_RULES: &str = include_str!("game_aliases.txt");
const SUGGESTION_RULES: &str = include_str!("game_suggestion.txt");
const MAX_ALIAS_BYTES: usize = 180;
const MAX_SOURCE_BYTES: usize = 64;
const MAX_GAME_CANDIDATES: usize = 36;
const MAX_GAME_REPLY_BYTES: usize = 2048;

/// Alternate discovery queries, ordered by official title, abbreviation, then base title.
pub(crate) async fn game_aliases(
    settings: &Settings,
    name: &str,
    steam_title: Option<&str>,
    cancel: &CancellationToken,
) -> Vec<String> {
    aliases_with(&settings.db, name, steam_title, cancel, |payload| {
        model_request(settings, ALIAS_RULES, payload, MODEL_TIMEOUT, cancel)
    })
    .await
}

/// Give the zero-based position of a title the model considers unambiguous.
pub(crate) async fn suggest_game(
    settings: &Settings,
    name: &str,
    steam_title: Option<&str>,
    source_name: &str,
    options: &[String],
    cancel: &CancellationToken,
) -> Option<usize> {
    suggest_with(
        &settings.db,
        name,
        steam_title,
        source_name,
        options,
        cancel,
        |payload| model_request(settings, SUGGESTION_RULES, payload, MODEL_TIMEOUT, cancel),
    )
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GameTitles {
    name: String,
    steam_title: Option<String>,
}

impl GameTitles {
    fn new(name: &str, steam_title: Option<&str>) -> Option<Self> {
        Some(Self {
            name: plain_name(name, super::MAX_QUERY_BYTES)?,
            steam_title: match steam_title {
                Some(title) => Some(plain_name(title, super::MAX_QUERY_BYTES)?),
                None => None,
            },
        })
    }
}

fn plain_name(value: &str, max_bytes: usize) -> Option<String> {
    if value.len() > max_bytes {
        return None;
    }
    let value = query_value(value)?;
    // Reject strings that the public metadata sanitizer would alter and keep
    // the remaining punctuation to the forms used by display titles.
    if public_text(&value, max_bytes).as_deref() != Some(value.as_str())
        || !value.chars().all(|ch| {
            ch.is_alphanumeric() || " .,:!?&+'’‘‐‑–—-()[]·™®：！？（）《》・".contains(ch)
        })
        || value.split_whitespace().any(|token| {
            let token = token
                .trim_start_matches(['(', '[', '\''])
                .to_ascii_lowercase();
            [
                "www.",
                "http:",
                "https:",
                "file:",
                "mailto:",
                "data:",
                "javascript:",
            ]
            .iter()
            .any(|prefix| token.starts_with(prefix))
        })
    {
        return None;
    }
    Some(value)
}

fn title_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AliasReply {
    official_titles: Vec<String>,
    abbreviations: Vec<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    base_title: Option<String>,
}

impl AliasReply {
    fn validate(mut self, game: &GameTitles) -> Option<Self> {
        if self.official_titles.len() > 2 || self.abbreviations.len() > 1 {
            return None;
        }
        for title in self
            .official_titles
            .iter_mut()
            .chain(&mut self.abbreviations)
            .chain(&mut self.base_title)
        {
            *title = plain_name(title, MAX_ALIAS_BYTES)?;
        }
        let mut seen = HashSet::from([title_key(&game.name)]);
        if let Some(steam_title) = &game.steam_title {
            seen.insert(title_key(steam_title));
        }
        self.official_titles
            .retain(|title| seen.insert(title_key(title)));
        self.abbreviations
            .retain(|title| seen.insert(title_key(title)));
        self.base_title = self
            .base_title
            .filter(|title| seen.insert(title_key(title)));
        (!self.official_titles.is_empty()
            || !self.abbreviations.is_empty()
            || self.base_title.is_some())
        .then_some(self)
    }

    fn into_titles(self) -> Vec<String> {
        self.official_titles
            .into_iter()
            .chain(self.abbreviations)
            .chain(self.base_title)
            .collect()
    }
}

async fn aliases_with<F: Future<Output = Option<String>>>(
    db: &Db,
    name: &str,
    steam_title: Option<&str>,
    cancel: &CancellationToken,
    ask: impl FnOnce(String) -> F,
) -> Vec<String> {
    let Some(game) = GameTitles::new(name, steam_title) else {
        return Vec::new();
    };
    cached_reply(
        db,
        cache::fingerprint(
            "game-aliases-v1",
            ALIAS_RULES,
            [
                game.name.as_str(),
                game.steam_title.as_deref().unwrap_or_default(),
            ],
        ),
        json!(game).to_string(),
        cancel,
        ask,
        |reply: AliasReply| reply.validate(&game),
    )
    .await
    .map(AliasReply::into_titles)
    .unwrap_or_default()
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SuggestionReply {
    #[serde(deserialize_with = "Option::deserialize")]
    index: Option<usize>,
}

impl SuggestionReply {
    fn validate(self, options: &[String]) -> Option<Self> {
        let title = title_key(options.get(self.index?)?);
        (options
            .iter()
            .filter(|option| title_key(option) == title)
            .take(2)
            .count()
            == 1)
            .then_some(self)
    }
}

async fn suggest_with<F: Future<Output = Option<String>>>(
    db: &Db,
    name: &str,
    steam_title: Option<&str>,
    source_name: &str,
    options: &[String],
    cancel: &CancellationToken,
    ask: impl FnOnce(String) -> F,
) -> Option<usize> {
    if options.is_empty() || options.len() > MAX_GAME_CANDIDATES {
        return None;
    }
    let game = GameTitles::new(name, steam_title)?;
    let source = plain_name(source_name, MAX_SOURCE_BYTES)?;
    // Reject an invalid candidate without dropping it and shifting indices.
    let options = options
        .iter()
        .map(|title| plain_name(title, super::MAX_QUERY_BYTES))
        .collect::<Option<Vec<_>>>()?;
    let payload =
        json!({"name":game.name,"steamTitle":game.steam_title,"source":source,"options":options})
            .to_string();
    cached_reply(
        db,
        cache::fingerprint("game-suggestion-v1", SUGGESTION_RULES, [&payload]),
        payload,
        cancel,
        ask,
        |reply: SuggestionReply| reply.validate(&options),
    )
    .await?
    .index
}

async fn cached_reply<T: DeserializeOwned + Serialize, F: Future<Output = Option<String>>>(
    db: &Db,
    key: String,
    payload: String,
    cancel: &CancellationToken,
    ask: impl FnOnce(String) -> F,
    validate: impl Fn(T) -> Option<T>,
) -> Option<T> {
    if cancel.is_cancelled() {
        return None;
    }
    let cached = cache::read(db, &[Some(key.clone())])
        .ok()
        .and_then(|mut entries| entries.pop().flatten())
        .and_then(&validate);
    if cached.is_some() || cancel.is_cancelled() {
        return cached.filter(|_| !cancel.is_cancelled());
    }
    let reply = ask(payload).await?;
    if reply.len() > MAX_GAME_REPLY_BYTES
        || cancel.is_cancelled()
        || !json_body(&reply).starts_with('{')
    {
        return None;
    }
    let reply = validate(decode(&reply)?)?;
    if cancel.is_cancelled() {
        return None;
    }
    let _ = cache::write(db, &[(key.as_str(), &reply)]);
    (!cancel.is_cancelled()).then_some(reply)
}

#[cfg(test)]
#[path = "game_match_tests.rs"]
mod tests;
