use crate::db::Db;
use crate::error::AppResult;
use crate::games::{self, Game};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use super::links::{public_text, valid_provider_id};
use super::providers::ProviderGame;

#[derive(Clone)]
pub(super) struct GameScope {
    pub id: String,
    pub name: String,
    slug: String,
    fingerprint: String,
}

impl GameScope {
    pub fn new(game: Game) -> Self {
        let fingerprint = Self::fingerprint(&game);
        Self {
            id: game.id,
            name: game.name,
            slug: game.slug,
            fingerprint,
        }
    }

    fn fingerprint(game: &Game) -> String {
        let mut hash = Sha256::new();
        for field in [
            Some(game.id.as_str()),
            Some(game.name.as_str()),
            Some(game.slug.as_str()),
            Some(game.root_path.as_str()),
            Some(game.platform.as_str()),
            game.steam_app_id.as_deref(),
            game.adapter_slug.as_deref(),
            game.adapter_version.as_deref(),
        ] {
            if let Some(value) = field {
                hash.update([1]);
                hash.update((value.len() as u64).to_be_bytes());
                hash.update(value.as_bytes());
            } else {
                hash.update([0]);
            }
        }
        format!("{:x}", hash.finalize())
    }

    pub fn current_game(&self, db: &Db) -> Option<Game> {
        games::get_game(db, &self.id)
            .ok()
            .filter(|game| Self::fingerprint(game) == self.fingerprint)
    }

    pub fn current(&self, db: &Db) -> bool {
        self.current_game(db).is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GameOption {
    pub(super) id: String,
    pub(super) name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GameChoice {
    pub(super) game_id: String,
    pub(super) source_id: &'static str,
    pub(super) options: Vec<GameOption>,
}

pub(super) enum Match {
    Exact(String),
    Choices(Vec<GameOption>),
    Missing,
}

fn normalized_name(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub(super) fn same_title(left: &str, right: &str) -> bool {
    normalized_name(left) == normalized_name(right)
}

fn normalized_slug(value: &str) -> String {
    value
        .split(['-', '_', ' '])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .to_ascii_lowercase()
}

const LEGACY_LABELS: [&str; 4] = ["传承版", "傳承版", "传统版", "傳統版"];
const ENHANCED_LABELS: [&str; 4] = ["增强版", "增強版", "加强版", "加強版"];

/// A discovery term for catalogs that index a base name such as “GTA5”.
/// Removing the edition is never evidence for auto-selecting the result.
pub(super) fn base_search_name(name: &str) -> Option<&str> {
    LEGACY_LABELS
        .iter()
        .chain(&ENHANCED_LABELS)
        .find_map(|label| {
            let name = name
                .strip_suffix(label)?
                .trim_end_matches([' ', '-', '·', ':', '：']);
            name.chars().any(char::is_alphanumeric).then_some(name)
        })
}

// Localized edition names are a common reason to ask the model. Preserve
// explicit edition evidence even if a guess or a matching slug drops it.
fn edition_compatible(local: &str, steam: Option<&str>, candidate: &str) -> bool {
    let local = local.to_lowercase();
    let steam = steam.unwrap_or_default().to_lowercase();
    let candidate = candidate.to_lowercase();
    let mut required = [false; 2];
    let mut observed = [false; 2];
    for (index, (word, explicit, localized)) in [
        (
            "legacy",
            ["(legacy)", "- legacy", "legacy edition"],
            LEGACY_LABELS,
        ),
        (
            "enhanced",
            ["(enhanced)", "- enhanced", "enhanced edition"],
            ENHANCED_LABELS,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let translated = |name: &str| localized.iter().any(|label| name.contains(label));
        let marked = |name: &str| {
            // A word such as “Legacy” can also be part of the base game's name.
            // A release suffix after an installment number (e.g. “V Legacy”)
            // and explicit edition labels provide stronger evidence.
            let numbered = name
                .strip_suffix(word)
                .and_then(|prefix| prefix.strip_suffix(' '))
                .and_then(|prefix| prefix.split_whitespace().next_back())
                .is_some_and(|part| {
                    part.ends_with(|ch: char| ch.is_ascii_digit())
                        || matches!(
                            part,
                            "i" | "ii" | "iii" | "iv" | "v" | "vi" | "vii" | "viii" | "ix" | "x"
                        )
                });
            translated(name) || explicit.iter().any(|label| name.contains(label)) || numbered
        };
        required[index] = marked(&local) || marked(&steam);
        observed[index] = translated(&candidate)
            || candidate
                .split(|ch: char| !ch.is_ascii_alphanumeric())
                .any(|part| part == word);
    }
    match required {
        [false, false] => true,
        [true, true] => false,
        _ => required == observed,
    }
}

pub(super) fn use_suggestion(
    game: &GameScope,
    steam: Option<&str>,
    options: Vec<GameOption>,
    index: Option<usize>,
) -> Match {
    if let Some(selected) = index.and_then(|i| options.get(i)) {
        let label = normalized_name(&selected.name);
        let unique = options
            .iter()
            .filter(|other| normalized_name(&other.name) == label)
            .count()
            == 1;
        if unique && edition_compatible(&game.name, steam, &selected.name) {
            return Match::Exact(selected.id.clone());
        }
    }
    Match::Choices(options)
}

pub(super) fn decide(
    game: &GameScope,
    catalog_title: Option<&str>,
    games: Vec<ProviderGame>,
    remembered: Option<&str>,
    complete: bool,
) -> Match {
    let mut options = BTreeMap::new();
    for candidate in games {
        if !valid_provider_id(&candidate.id) {
            continue;
        }
        if let Some(name) = public_text(&candidate.name, 180) {
            options.entry(candidate.id).or_insert(name);
        }
    }
    if let Some(id) = remembered.filter(|id| options.contains_key(*id)) {
        return Match::Exact(id.to_owned());
    }
    let name = normalized_name(&game.name);
    let alias = catalog_title.map(normalized_name);
    let slug = normalized_slug(&game.slug);
    let exact = options
        .iter()
        .filter(|(id, candidate_name)| {
            let candidate_name = normalized_name(candidate_name);
            edition_compatible(&game.name, catalog_title, &candidate_name)
                && (candidate_name == name
                    || alias.as_ref() == Some(&candidate_name)
                    || (!slug.is_empty() && normalized_slug(id) == slug))
        })
        .map(|(id, _)| id)
        .take(2)
        .collect::<Vec<_>>();
    if let (true, [id]) = (complete, exact.as_slice()) {
        return Match::Exact((*id).to_owned());
    }
    if options.is_empty() {
        Match::Missing
    } else {
        Match::Choices(
            options
                .into_iter()
                .map(|(id, name)| GameOption { id, name })
                .collect(),
        )
    }
}

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS desktop_browse_game_matches (
    game_id TEXT NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    source_id TEXT NOT NULL,
    game_fingerprint TEXT NOT NULL,
    provider_game_id TEXT NOT NULL,
    PRIMARY KEY (game_id, source_id)
)";

pub(super) fn remembered(db: &Db, game: &GameScope, source: &str) -> AppResult<Option<String>> {
    let conn = db.conn();
    conn.execute_batch(SCHEMA)?;
    Ok(conn
        .query_row(
            "SELECT provider_game_id FROM desktop_browse_game_matches
             WHERE game_id=?1 AND source_id=?2 AND game_fingerprint=?3",
            params![game.id, source, game.fingerprint],
            |row| row.get(0),
        )
        .optional()?)
}

pub(super) fn remember(
    db: &Db,
    game: &GameScope,
    source: &str,
    provider_id: &str,
) -> AppResult<bool> {
    let conn = db.conn();
    if GameScope::fingerprint(&games::get_game_from(&conn, &game.id)?) != game.fingerprint {
        return Ok(false);
    }
    conn.execute_batch(SCHEMA)?;
    conn.execute(
        "INSERT INTO desktop_browse_game_matches
         (game_id, source_id, game_fingerprint, provider_game_id) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (game_id, source_id) DO UPDATE SET
         game_fingerprint=excluded.game_fingerprint, provider_game_id=excluded.provider_game_id",
        params![game.id, source, game.fingerprint, provider_id],
    )?;
    Ok(true)
}
