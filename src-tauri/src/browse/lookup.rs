//! Resolve a library game against a single authorized source. Only IDs read
//! from that connection can leave this module as a match.
use super::aggregation::{IssueCode, Pair};
use super::catalog_cache::{CatalogCache, PageKey};
use super::matching::{self, GameScope, Match};
use super::providers::{self, ProviderGame, ProviderGamePage};
use super::{
    classify, current, enrich, links, source_name, Context, Lookup, MAX_QUERY, PROVIDER_TIMEOUT,
};
use crate::mcp::McpServers;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

const PAGES_PER_TERM: usize = 3;
const MAX_READS: usize = 9;
const MAX_OPTIONS: usize = 36;
const READ_BUDGET: Duration = Duration::from_secs(24);

#[derive(Default)]
pub(super) struct GameHints {
    steam_title: OnceCell<Option<String>>,
    aliases: OnceCell<Vec<String>>,
}

struct Directory<'a> {
    ctx: &'a Context,
    catalogs: &'a CatalogCache,
    pair: &'a Pair,
    servers: McpServers,
    hints: &'a GameHints,
    cancel: &'a CancellationToken,
}

impl Directory<'_> {
    fn check(&self) -> Result<(), IssueCode> {
        if self.cancel.is_cancelled()
            || !self.pair.game.current(self.ctx.db())
            || !current(
                &self.ctx.connections,
                &self.servers,
                self.pair.source,
                &self.pair.connection,
            )
        {
            Err(IssueCode::PermissionDenied)
        } else {
            Ok(())
        }
    }
}

pub(super) async fn lookup(
    ctx: &Context,
    pair: &Pair,
    hints: &GameHints,
    search: &super::Search,
) -> (Pair, Lookup) {
    let directory = Directory {
        ctx,
        catalogs: &search.catalogs,
        pair,
        hints,
        servers: ctx.servers(),
        cancel: &search.cancel,
    };
    let work = async {
        directory.check()?;
        let name = links::public_text(&pair.game.name, MAX_QUERY).ok_or(IssueCode::NoGameMatch)?;
        let steam = hints
            .steam_title
            .get_or_init(|| async {
                let game = pair.game.current_game(ctx.db())?;
                tokio::task::spawn_blocking(move || game.steam_catalog_title())
                    .await
                    .ok()
                    .flatten()
                    .and_then(|name| public_game_name(&name))
            })
            .await;
        let remembered = matching::remembered(ctx.db(), &pair.game, pair.source)
            .map_err(|_| IssueCode::QueryFailed)?;
        let result = resolve(
            &directory,
            &pair.game,
            &name,
            steam.as_deref(),
            pair.source,
            remembered.as_deref(),
        )
        .await?;
        directory.check()?;
        Ok(result)
    }
    .await;
    (
        pair.clone(),
        match work {
            Ok(result) => Lookup::Resolved(result),
            Err(code) => Lookup::Failed(code),
        },
    )
}

fn public_game_name(name: &str) -> Option<String> {
    if name.len() > MAX_QUERY || name.contains(['/', '\\']) || name.chars().any(char::is_control) {
        return None;
    }
    links::public_text(name, MAX_QUERY)
}

trait CatalogLookup {
    async fn page(&self, query: &str, cursor: Option<&str>) -> Result<ProviderGamePage, IssueCode>;
    async fn aliases(&self, name: &str, steam: Option<&str>) -> Result<Vec<String>, IssueCode>;
    async fn suggest(
        &self,
        name: &str,
        steam: Option<&str>,
        options: &[String],
    ) -> Result<Option<usize>, IssueCode>;
}

impl CatalogLookup for Directory<'_> {
    async fn page(&self, query: &str, cursor: Option<&str>) -> Result<ProviderGamePage, IssueCode> {
        self.check()?;
        let call = self.cancel.child_token();
        let _cancel_on_drop = call.clone().drop_guard();
        let key = PageKey::new(self.pair.source, None, query, cursor);
        let page = self
            .catalogs
            .games
            .read(
                key,
                providers::list_games(
                    &self.ctx.connections,
                    &self.servers,
                    self.pair.source,
                    &self.pair.connection,
                    query,
                    cursor,
                    &call,
                ),
            )
            .await
            .map_err(|error| classify(&error))?;
        self.check()?;
        Ok(page)
    }

    async fn aliases(&self, name: &str, steam: Option<&str>) -> Result<Vec<String>, IssueCode> {
        self.check()?;
        let aliases = self
            .hints
            .aliases
            .get_or_init(|| enrich::game_aliases(&self.ctx.settings, name, steam, self.cancel))
            .await;
        Ok(aliases.clone())
    }

    async fn suggest(
        &self,
        name: &str,
        steam: Option<&str>,
        options: &[String],
    ) -> Result<Option<usize>, IssueCode> {
        self.check()?;
        Ok(enrich::suggest_game(
            &self.ctx.settings,
            name,
            steam,
            source_name(self.pair.source),
            options,
            self.cancel,
        )
        .await)
    }
}

struct Candidates {
    // Broad or truncated queries can contribute manual choices. Keep them
    // separate so a later narrow alias can still discover a complete match.
    games: BTreeMap<String, ProviderGame>,
    partial: BTreeMap<String, ProviderGame>,
    queried: BTreeSet<String>,
    limited: bool,
    remaining_reads: usize,
    remaining_time: Duration,
}

impl Candidates {
    fn new() -> Self {
        Self {
            games: BTreeMap::new(),
            partial: BTreeMap::new(),
            queried: BTreeSet::new(),
            limited: false,
            remaining_reads: MAX_READS,
            remaining_time: READ_BUDGET,
        }
    }

    fn has_capacity(&self) -> bool {
        self.remaining_reads > 0 && !self.remaining_time.is_zero() && self.games.len() < MAX_OPTIONS
    }

    async fn read(
        &mut self,
        catalog: &impl CatalogLookup,
        term: &str,
        known_names: &[Option<&str>],
    ) -> Result<(), IssueCode> {
        if !self.queried.insert(term.to_lowercase()) {
            return Ok(());
        }
        let mut cursor = None;
        let mut visited = BTreeSet::new();
        let mut rows = Vec::new();
        let mut complete = false;
        let mut failure = None;
        for _ in 0..PAGES_PER_TERM {
            if !self.has_capacity() {
                break;
            }
            self.remaining_reads -= 1;
            let start = Instant::now();
            let page = tokio::time::timeout(
                PROVIDER_TIMEOUT.min(self.remaining_time),
                catalog.page(term, cursor.as_deref()),
            )
            .await;
            self.remaining_time = self.remaining_time.saturating_sub(start.elapsed());
            let page = match page.unwrap_or(Err(IssueCode::QueryFailed)) {
                Ok(page) => page,
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            };
            rows.extend(page.games);
            if rows.len() > MAX_OPTIONS {
                rows.truncate(MAX_OPTIONS);
                break;
            }
            let Some(next) = page.next else {
                complete = true;
                break;
            };
            if !super::aggregation::valid_cursor(&next) || !visited.insert(next.clone()) {
                break;
            }
            cursor = Some(next);
        }
        // Admit the entire completed query or treat it as partial. Deduplicate
        // by source ID as different terms may return the same catalog entry.
        let new_ids: BTreeSet<_> = rows
            .iter()
            .map(|game| &game.id)
            .filter(|id| !self.games.contains_key(*id))
            .collect();
        complete &= self.games.len() + new_ids.len() <= MAX_OPTIONS;
        self.limited |= !complete;
        let options = if complete {
            &mut self.games
        } else {
            &mut self.partial
        };
        let trusted = |title: &str| {
            known_names
                .iter()
                .flatten()
                .any(|known| title.eq_ignore_ascii_case(known))
        };
        for game in rows {
            if let Some(previous) = options.get_mut(&game.id) {
                if trusted(&game.name) && !trusted(&previous.name) {
                    previous.name = game.name;
                }
            } else if options.len() < MAX_OPTIONS {
                options.insert(game.id.clone(), game);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    fn decide(
        &self,
        game: &GameScope,
        steam: Option<&str>,
        remembered: Option<&str>,
        manual: bool,
    ) -> Match {
        let mut rows: Vec<_> = self.games.values().cloned().collect();
        if let Some(id) = remembered {
            if let Some(chosen) = self.partial.get(id) {
                rows.push(chosen.clone());
            }
        }
        if manual {
            rows.extend(
                self.partial
                    .values()
                    .filter(|row| !self.games.contains_key(&row.id))
                    .take(MAX_OPTIONS.saturating_sub(rows.len()))
                    .cloned(),
            );
        }
        matching::decide(game, steam, rows, remembered, !manual)
    }

    fn conflicting_label(&self, id: &str) -> bool {
        self.games.get(id).is_some_and(|selected| {
            self.partial
                .values()
                .any(|other| other.id != id && matching::same_title(&selected.name, &other.name))
        })
    }
}

async fn resolve(
    catalog: &impl CatalogLookup,
    game: &GameScope,
    name: &str,
    steam: Option<&str>,
    source: &str,
    remembered: Option<&str>,
) -> Result<Match, IssueCode> {
    let mut candidates = Candidates::new();
    // Chinese sources commonly index the library spelling. Other sources often
    // index the storefront title; both names remain eligible for an exact hit.
    let known = if matches!(source, "glossmod_http" | "glossmod_3dm") {
        [Some(name), steam]
    } else {
        [steam, Some(name)]
    };
    let mut terms = known
        .into_iter()
        .flatten()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for stage in 0..2 {
        for term in terms.drain(..) {
            if !candidates.has_capacity() {
                break;
            }
            if let Err(error) = candidates.read(catalog, &term, &known).await {
                // Preserve previously observed manual choices if an optional
                // read fails. A revoked grant invalidates every candidate.
                let manual = candidates.decide(game, steam, remembered, true);
                if matches!(manual, Match::Missing) || error == IssueCode::PermissionDenied {
                    return Err(error);
                }
                return Ok(manual);
            }
            if let Match::Exact(id) = candidates.decide(game, steam, remembered, false) {
                if remembered == Some(&id)
                    || (remembered.is_none() && !candidates.conflicting_label(&id))
                {
                    return Ok(Match::Exact(id));
                }
            }
        }
        if stage == 0 && candidates.has_capacity() {
            terms = catalog.aliases(name, steam).await?;
            if let Some(base) = matching::base_search_name(name) {
                terms.push(base.to_owned());
            }
        }
    }
    let mut result = candidates.decide(game, steam, remembered, false);
    if let Match::Choices(options) = result {
        let labels: Vec<_> = options.iter().map(|option| option.name.clone()).collect();
        let suggested = catalog.suggest(name, steam, &labels).await?;
        result = matching::use_suggestion(game, steam, options, suggested);
    }
    if let Match::Exact(id) = &result {
        if remembered == Some(id.as_str()) || !candidates.conflicting_label(id) {
            return Ok(result);
        }
    }
    let manual = candidates.decide(game, steam, remembered, true);
    if matches!(manual, Match::Missing) && (candidates.limited || !candidates.has_capacity()) {
        return Err(IssueCode::LimitReached);
    }
    Ok(manual)
}

#[cfg(test)]
mod tests;
