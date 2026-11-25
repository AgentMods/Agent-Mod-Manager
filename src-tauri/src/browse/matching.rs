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
    name: String,
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

fn normalized_slug(value: &str) -> String {
    value
        .split(['-', '_', ' '])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .to_ascii_lowercase()
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
            candidate_name == name
                || alias.as_ref() == Some(&candidate_name)
                || (!slug.is_empty() && normalized_slug(id) == slug)
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
