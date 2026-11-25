//! Desktop-only discovery through explicitly authorized, current stock MCP connections.
mod aggregation;
pub(crate) mod enrich;
pub(crate) mod install;
mod links;
mod matching;
mod providers;

use crate::agent::runtime::AgentRuntime;
use crate::app::AppServices;
use crate::db::Db;
use crate::error::{AppError, AppResult, NativeMessage};
use crate::mcp::McpServers;
use crate::mcp_connections::McpConnections;
use crate::settings::Settings;
use aggregation::{Candidate, Fetch, Issue, IssueCode, MatchedPair, MixedPage, Mixer, Pair};
use enrich::{BrowseTag, EnrichedItem, EnrichmentInput, ModType};
use futures_util::{stream, StreamExt};
use matching::{GameChoice, GameScope, Match};
use providers::{ProviderAvailability, ProviderConnection, ProviderPage};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::{Runtime, State, WebviewWindow};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::{watch, Semaphore};
use tokio_util::sync::CancellationToken;

const MAX_WINDOWS: usize = 8;
const MAX_GAMES: usize = 8;
const MAX_SOURCES: usize = 7;
const MAX_QUERY: usize = 240;
const PAGE_SIZE: usize = 24;
const MAX_PAGES: usize = 12;
const MAX_GAME_PAGES: usize = 3;
const MAX_GAME_OPTIONS: usize = 36;
const AUTH_TTL: Duration = Duration::from_secs(30 * 60);
const SEARCH_TTL: Duration = Duration::from_secs(12 * 60);
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(8);
const MODEL_TIMEOUT: Duration = Duration::from_secs(20);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn browse_error(key: &str) -> AppError {
    AppError::Presented(NativeMessage::new(key, []))
}

fn checked_id(id: &str) -> bool {
    id.len() == 36 && uuid::Uuid::parse_str(id).is_ok()
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
enum SourceStatus {
    Available,
    NotConnected,
    PermissionDenied,
    Unsupported,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Source {
    id: &'static str,
    name: &'static str,
    status: SourceStatus,
}

fn source_name(id: &str) -> &'static str {
    match id {
        "glossmod_http" => "Mod客栈",
        "glossmod_3dm" => "3DM",
        "modio" => "mod.io",
        "curseforge" => "CurseForge",
        "gamebanana" => "GameBanana",
        "nexusmods" => "Nexus Mods",
        "thunderstore" => "Thunderstore",
        _ => "",
    }
}

fn source_status(availability: ProviderAvailability) -> SourceStatus {
    match availability {
        ProviderAvailability::Available => SourceStatus::Available,
        ProviderAvailability::NotConnected => SourceStatus::NotConnected,
        ProviderAvailability::PermissionDenied => SourceStatus::PermissionDenied,
        ProviderAvailability::Unsupported => SourceStatus::Unsupported,
    }
}

struct Grant {
    id: String,
    db: Db,
    expires: Instant,
    connections: BTreeMap<&'static str, ProviderConnection>,
}

#[derive(Default)]
struct WindowState {
    discovery: Option<Discovery>,
    grant: Option<Grant>,
    search: Option<Arc<Search>>,
}

#[derive(Default)]
pub(crate) struct BrowseState {
    windows: Mutex<HashMap<String, WindowState>>,
}

impl BrowseState {
    fn windows(&self) -> MutexGuard<'_, HashMap<String, WindowState>> {
        let mut windows = lock(&self.windows);
        let now = Instant::now();
        windows.retain(|_, slot| {
            if slot
                .discovery
                .as_ref()
                .is_some_and(|discovery| now >= discovery.expires)
            {
                slot.discovery = None;
            }
            if slot
                .grant
                .as_ref()
                .is_some_and(|grant| now >= grant.expires)
            {
                slot.grant = None;
            }
            if slot
                .search
                .as_ref()
                .is_some_and(|search| now >= search.expires || search.cancel.is_cancelled())
            {
                if let Some(search) = slot.search.take() {
                    search.cancel.cancel();
                }
            }
            slot.discovery.is_some() || slot.grant.is_some()
        });
        windows
    }

    pub(crate) fn cancel_window(&self, label: &str) {
        if let Some(search) = lock(&self.windows)
            .remove(label)
            .and_then(|slot| slot.search)
        {
            search.cancel.cancel();
        }
    }
}

#[derive(Clone)]
struct Context {
    settings: Settings,
    connections: McpConnections,
}

impl Context {
    fn new(services: &AppServices, settings: &Settings) -> AppResult<Self> {
        if !Arc::ptr_eq(&services.engine.db.0, &settings.db.0) {
            return Err(browse_error("browse.search_expired"));
        }
        Ok(Self {
            settings: settings.clone(),
            connections: services.mcp_connections.clone(),
        })
    }

    fn db(&self) -> &Db {
        &self.settings.db
    }

    fn servers(&self) -> McpServers {
        McpServers::new(self.db().clone())
    }

    async fn lookup(&self, pair: Pair, cancel: &CancellationToken) -> (Pair, Lookup) {
        let request = cancel.child_token();
        let _cancel_on_drop = request.clone().drop_guard();
        let result =
            tokio::time::timeout(Duration::from_secs(16), self.lookup_inner(&pair, &request))
                .await
                .unwrap_or(Lookup::Failed(IssueCode::QueryFailed));
        (pair, result)
    }

    async fn lookup_inner(&self, pair: &Pair, cancel: &CancellationToken) -> Lookup {
        let servers = self.servers();
        let db = self.db();
        let Some(registered) = pair.game.current_game(db) else {
            return Lookup::Failed(IssueCode::PermissionDenied);
        };
        if !current(&self.connections, &servers, pair.source, &pair.connection) {
            return Lookup::Failed(IssueCode::PermissionDenied);
        }
        let Ok(remembered) = matching::remembered(db, &pair.game, pair.source) else {
            return Lookup::Failed(IssueCode::QueryFailed);
        };
        let Some(local_name) = links::public_text(&pair.game.name, MAX_QUERY) else {
            return Lookup::Failed(IssueCode::NoGameMatch);
        };
        // Nexus find_games accepts display names. Its returned domainName is the
        // identifier used later by find_mods, including for translated library names.
        let catalog_title = (pair.source == "nexusmods")
            .then(|| registered.steam_catalog_title())
            .flatten()
            .filter(|title| !title.contains(['/', '\\']) && title.len() <= MAX_QUERY)
            .and_then(|title| links::public_text(&title, MAX_QUERY))
            .filter(|title| !title.eq_ignore_ascii_case(&local_name));
        let mut query = catalog_title.as_deref().unwrap_or(&local_name);
        let mut fallback = catalog_title.as_ref().map(|_| local_name.as_str());
        let mut options = Vec::new();
        let mut next: Option<String> = None;
        let mut visited = BTreeSet::new();
        let mut complete = false;
        for _ in 0..MAX_GAME_PAGES {
            let call = cancel.child_token();
            let _cancel_on_drop = call.clone().drop_guard();
            let listed = tokio::time::timeout(
                PROVIDER_TIMEOUT,
                providers::list_games(
                    &self.connections,
                    &servers,
                    pair.source,
                    &pair.connection,
                    query,
                    next.as_deref(),
                    &call,
                ),
            )
            .await;
            let page = match listed {
                Ok(Ok(page)) => page,
                Ok(Err(error)) => return Lookup::Failed(classify(&error)),
                Err(_) => return Lookup::Failed(IssueCode::QueryFailed),
            };
            let remaining = MAX_GAME_OPTIONS - options.len();
            let overflow = page.games.len() > remaining;
            options.extend(page.games.into_iter().take(remaining));
            next = page.next;
            if overflow {
                break;
            }
            if let Some(cursor) = &next {
                if !aggregation::valid_cursor(cursor) || !visited.insert(cursor.clone()) {
                    break;
                }
            } else if let Some(local_query) = fallback.take().filter(|_| options.is_empty()) {
                query = local_query;
                visited.clear();
            } else {
                complete = true;
                break;
            }
            if options.len() == MAX_GAME_OPTIONS {
                break;
            }
        }
        if !current(&self.connections, &servers, pair.source, &pair.connection) {
            return Lookup::Failed(IssueCode::PermissionDenied);
        }
        let decision = matching::decide(
            &pair.game,
            catalog_title.as_deref(),
            options,
            remembered.as_deref(),
            complete,
        );
        if !complete && matches!(decision, Match::Missing) {
            Lookup::Failed(IssueCode::LimitReached)
        } else {
            Lookup::Resolved(decision)
        }
    }

    async fn fetch(
        &self,
        request: Fetch,
        cancel: &CancellationToken,
    ) -> Result<ProviderPage, IssueCode> {
        let servers = self.servers();
        let pair = &request.matched.pair;
        if !pair.game.current(self.db())
            || !current(&self.connections, &servers, pair.source, &pair.connection)
        {
            return Err(IssueCode::PermissionDenied);
        }
        let call = cancel.child_token();
        let _cancel_on_drop = call.clone().drop_guard();
        let data = tokio::time::timeout(
            PROVIDER_TIMEOUT,
            providers::search(
                &self.connections,
                &servers,
                pair.source,
                &pair.connection,
                Some(&request.matched.provider_game_id),
                &request.query,
                request.cursor.as_deref(),
                &call,
            ),
        )
        .await;
        if !current(&self.connections, &servers, pair.source, &pair.connection) {
            return Err(IssueCode::PermissionDenied);
        }
        match data {
            Ok(result) => result.map_err(|error| classify(&error)),
            Err(_) => Err(IssueCode::QueryFailed),
        }
    }
}

fn classify(error: &AppError) -> IssueCode {
    match error {
        AppError::Denied(_) => IssueCode::PermissionDenied,
        _ => IssueCode::QueryFailed,
    }
}

enum Lookup {
    Resolved(Match),
    Failed(IssueCode),
}

fn current(
    connections: &McpConnections,
    servers: &McpServers,
    source: &str,
    expected: &ProviderConnection,
) -> bool {
    providers::connection(connections, servers, source).is_ok_and(|actual| actual == *expected)
}

struct Search {
    id: String,
    db: Db,
    expires: Instant,
    query: String,
    pairs: Vec<Pair>,
    translations: watch::Receiver<Option<QueryVariants>>,
    enrichment_slots: Semaphore,
    cancel: CancellationToken,
    inner: Mutex<SearchInner>,
}

#[derive(Default)]
struct SearchInner {
    progress: Option<Progress>,
    choices: Vec<GameChoice>,
    issues: BTreeSet<Issue>,
    pages: Vec<Frame>,
    items: BTreeMap<String, Candidate>,
    loading: bool,
}

#[derive(Default)]
struct Progress {
    matched: Vec<MatchedPair>,
    mixer: Mixer,
    translations_added: bool,
}

struct Frame {
    ids: Vec<String>,
    has_next: bool,
    translated: Option<(String, Vec<EnrichedItem>)>,
    enriching: Option<String>,
}

impl Progress {
    async fn initialize(ctx: &Context, search: &Search) -> (Self, Vec<GameChoice>) {
        let mut results = stream::iter(
            search
                .pairs
                .iter()
                .cloned()
                .map(|pair| ctx.lookup(pair, &search.cancel)),
        )
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await;
        // Network completion order must not affect which feed gets the first slot.
        results.sort_by(|a, b| (&a.0.game.id, a.0.source).cmp(&(&b.0.game.id, b.0.source)));
        let mut progress = Self::default();
        let mut choices = Vec::new();
        for (pair, result) in results {
            match result {
                Lookup::Resolved(Match::Exact(provider_game_id)) => {
                    let matched = MatchedPair {
                        pair,
                        provider_game_id,
                    };
                    progress.mixer.push(matched.clone(), search.query.clone());
                    progress.matched.push(matched);
                }
                Lookup::Resolved(Match::Choices(options)) => choices.push(GameChoice {
                    game_id: pair.game.id,
                    source_id: pair.source,
                    options,
                }),
                Lookup::Resolved(Match::Missing) => {
                    progress
                        .mixer
                        .issues
                        .insert(pair.issue(IssueCode::NoGameMatch));
                }
                Lookup::Failed(code) => {
                    progress.mixer.issues.insert(pair.issue(code));
                }
            }
        }
        (progress, choices)
    }

    fn add_variants(&mut self, variants: Option<&QueryVariants>, prioritize: bool) -> bool {
        let Some(variants) = variants else {
            return false;
        };
        if self.translations_added {
            return false;
        }
        self.translations_added = true;
        let mut added = false;
        for matched in &self.matched {
            if let Some(query) = variants.for_source(matched.pair.source) {
                if prioritize && !added {
                    self.mixer.prioritize_next_feed();
                }
                self.mixer.push(matched.clone(), query.to_owned());
                added = true;
            }
        }
        added
    }

    async fn mix_page<F, Fut>(
        &mut self,
        page: usize,
        mut variants: watch::Receiver<Option<QueryVariants>>,
        mut fetch: F,
    ) -> MixedPage
    where
        F: FnMut(Fetch) -> Fut,
        Fut: Future<Output = Result<ProviderPage, IssueCode>>,
    {
        if page > 0 && !self.translations_added && !self.matched.is_empty() {
            if !self.mixer.has_more() && variants.borrow().is_none() {
                let _ = variants.changed().await;
            }
            self.add_variants(variants.borrow().as_ref(), false);
        }

        let original = self.mixer.page(&mut fetch).await;
        // Never wait for the model if the first original page can be shown.
        // If it is empty, run one additional bounded pass with the translated
        // lanes first so translated hits can be shown without asking for page 1.
        if page == 0 && original.ids.is_empty() && !self.matched.is_empty() {
            if variants.borrow().is_none() {
                // The translation producer already has its own timeout, and
                // the search can cancel this whole operation at any point.
                let _ = variants.changed().await;
            }
            if self.add_variants(variants.borrow().as_ref(), true) {
                return self.mixer.page(&mut fetch).await;
            }
        }
        self.add_variants(variants.borrow().as_ref(), false);
        original
    }
}

#[derive(Clone, Default)]
struct QueryVariants {
    chinese: Option<String>,
    english: Option<String>,
}

fn chinese_source(id: &str) -> bool {
    matches!(id, "glossmod_http" | "glossmod_3dm")
}

impl QueryVariants {
    fn for_source(&self, source: &str) -> Option<&str> {
        if chinese_source(source) {
            self.chinese.as_deref()
        } else {
            self.english.as_deref()
        }
    }
}

async fn query_variant(
    ctx: &Context,
    search: &Search,
    enabled: bool,
    locale: &str,
) -> Option<String> {
    if !enabled || search.query.is_empty() {
        return None;
    }
    let translation =
        enrich::translate_query(&ctx.settings, &search.query, locale, &search.cancel).await?;
    links::public_text(&translation, MAX_QUERY)
        .filter(|alternate| alternate.to_lowercase() != search.query.to_lowercase())
}

fn spawn_translations(ctx: Context, search: Arc<Search>, tx: watch::Sender<Option<QueryVariants>>) {
    tauri::async_runtime::spawn(async move {
        let model_work = async {
            let current = available_sources(&ctx.connections, &ctx.servers(), &search);
            let chinese = current.iter().any(|source| chinese_source(source));
            let english = current.iter().any(|source| !chinese_source(source));
            let (chinese, english) = tokio::join!(
                query_variant(&ctx, &search, chinese, "zh-CN"),
                query_variant(&ctx, &search, english, "en"),
            );
            QueryVariants { chinese, english }
        };
        let variants = tokio::select! {
            _ = search.cancel.cancelled() => QueryVariants::default(),
            result = tokio::time::timeout(MODEL_TIMEOUT, model_work) => result.unwrap_or_default(),
        };
        let _ = tx.send(Some(variants));
    });
}

fn search_ref(
    state: &BrowseState,
    db: &Db,
    label: &str,
    search_id: &str,
) -> AppResult<Arc<Search>> {
    if !checked_id(search_id) {
        return Err(browse_error("browse.search_expired"));
    }
    let search = state
        .windows()
        .get(label)
        .and_then(|slot| slot.search.clone())
        .filter(|search| search.id == search_id && Arc::ptr_eq(&search.db.0, &db.0))
        .ok_or_else(|| browse_error("browse.search_expired"))?;
    let mut checked = BTreeSet::new();
    if !search
        .pairs
        .iter()
        .map(|pair| &pair.game)
        .all(|game| !checked.insert(&game.id) || game.current(db))
    {
        search.cancel.cancel();
        return Err(browse_error("browse.search_expired"));
    }
    Ok(search)
}

fn start_page(ctx: Context, search: Arc<Search>, page: usize, progress: Option<Progress>) {
    tauri::async_runtime::spawn(async move {
        let work = async {
            let (mut progress, choices) = match progress {
                Some(progress) => (progress, None),
                None => {
                    let (progress, choices) = Progress::initialize(&ctx, &search).await;
                    (progress, Some(choices))
                }
            };
            // A source may lose its grant between pages, with already-buffered
            // candidates. Retire its lanes before they consume healthy slots.
            let available = available_sources(&ctx.connections, &ctx.servers(), &search);
            progress.mixer.retire_unavailable(&available);
            progress
                .matched
                .retain(|matched| available.contains(matched.pair.source));
            let mixed = progress
                .mix_page(page, search.translations.clone(), |request| {
                    ctx.fetch(request, &search.cancel)
                })
                .await;
            let pending = !progress.translations_added && !progress.matched.is_empty();
            let more = progress.mixer.has_more() || pending;
            let has_next = more && page + 1 < MAX_PAGES;
            if more && !has_next {
                for matched in &progress.matched {
                    progress
                        .mixer
                        .issues
                        .insert(matched.pair.issue(IssueCode::LimitReached));
                }
            }
            let mut inner = lock(&search.inner);
            if search.cancel.is_cancelled() || Instant::now() >= search.expires {
                return;
            }
            if let Some(choices) = choices {
                inner.choices = choices;
            }
            for (id, candidate) in mixed.updates {
                inner
                    .items
                    .entry(id)
                    .and_modify(|prior| prior.targets.extend(candidate.targets.clone()))
                    .or_insert(candidate);
            }
            inner.issues.clone_from(&progress.mixer.issues);
            inner.pages.push(Frame {
                ids: mixed.ids,
                has_next,
                translated: None,
                enriching: None,
            });
            inner.progress = Some(progress);
            inner.loading = false;
        };
        tokio::select! {
            biased;
            _ = search.cancel.cancelled() => {},
            _ = tokio::time::sleep_until(search.expires.into()) => search.cancel.cancel(),
            _ = work => {},
        }
    });
}

fn entry(candidate: &Candidate) -> EnrichmentInput<'_> {
    EnrichmentInput {
        source: candidate.source,
        item_id: &candidate.project_id,
        title: &candidate.title,
        summary: candidate.summary.as_deref(),
        kind: candidate.kind.as_deref(),
        tags: &candidate.tags,
    }
}

fn start_enrichment(
    ctx: Context,
    search: Arc<Search>,
    page: usize,
    original: Vec<(Candidate, bool)>,
    locale: String,
) {
    tauri::async_runtime::spawn(async move {
        let work = async {
            let _permit = search.enrichment_slots.acquire().await.ok()?;
            let current_sources = available_sources(&ctx.connections, &ctx.servers(), &search);
            let mut presentation = original
                .iter()
                .map(|(row, _)| enrich::fallback_item(entry(row)))
                .collect::<Vec<_>>();
            let visible = original
                .iter()
                .enumerate()
                .filter(|(_, (row, visible))| *visible && current_sources.contains(row.source))
                .map(|(index, (row, _))| (index, entry(row)))
                .collect::<Vec<_>>();
            let input = visible.iter().map(|(_, input)| *input).collect::<Vec<_>>();
            let enriched =
                enrich::enrich_items(&ctx.settings, &input, &locale, &search.cancel).await;
            if enriched.len() == visible.len() {
                for ((index, _), text) in visible.into_iter().zip(enriched) {
                    presentation[index] = text;
                }
            }
            Some(presentation)
        };
        let output = tokio::select! {
            _ = search.cancel.cancelled() => return,
            _ = tokio::time::sleep_until(search.expires.into()) => return,
            result = tokio::time::timeout(MODEL_TIMEOUT, work) => result.ok().flatten(),
        };
        let effective = crate::app_locale::effective(ctx.db());
        let mut inner = lock(&search.inner);
        if let Some(frame) = inner.pages.get_mut(page) {
            if frame.enriching.as_deref() == Some(&locale) {
                frame.enriching = None;
                if effective == locale {
                    let presentation = output
                        .filter(|out| out.len() == original.len())
                        .unwrap_or_else(|| {
                            original
                                .iter()
                                .map(|(row, _)| enrich::fallback_item(entry(row)))
                                .collect()
                        });
                    frame.translated = Some((locale, presentation));
                }
            }
        }
    });
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum PageStatus {
    Loading,
    Ready,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Page {
    status: PageStatus,
    page: usize,
    items: Vec<BrowseItem>,
    issues: Vec<Issue>,
    game_choices: Vec<GameChoice>,
    has_next: bool,
    is_enriching: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowseItem {
    id: String,
    source_id: &'static str,
    game_ids: Vec<String>,
    title: String,
    original_title: String,
    author: Option<String>,
    summary: Option<String>,
    type_key: ModType,
    tags: Vec<BrowseTag>,
    cover_url: Option<String>,
    can_open_project: bool,
}

impl BrowseItem {
    fn display(id: String, original: &Candidate, enriched: EnrichedItem) -> Self {
        Self {
            id,
            source_id: original.source,
            game_ids: original.targets.keys().cloned().collect(),
            title: enriched.title,
            original_title: original.title.clone(),
            author: original.author.clone(),
            summary: enriched.summary,
            type_key: enriched.type_key,
            tags: enriched.tags,
            cover_url: original.cover_url.clone(),
            can_open_project: original.page_url.is_some(),
        }
    }
}

fn available_sources(
    connections: &McpConnections,
    servers: &McpServers,
    search: &Search,
) -> BTreeSet<&'static str> {
    if search.cancel.is_cancelled() || Instant::now() >= search.expires {
        return BTreeSet::new();
    }
    let mut checked = BTreeSet::new();
    search
        .pairs
        .iter()
        .filter(|pair| {
            checked.insert(pair.source)
                && current(connections, servers, pair.source, &pair.connection)
        })
        .map(|pair| pair.source)
        .collect()
}

fn valid_source_for(services: &AppServices, search: &Search, source: &str) -> bool {
    !search.cancel.is_cancelled()
        && Instant::now() < search.expires
        && search
            .pairs
            .iter()
            .find(|pair| pair.source == source)
            .is_some_and(|pair| {
                current(
                    &services.mcp_connections,
                    &McpServers::new(search.db.clone()),
                    source,
                    &pair.connection,
                )
            })
}

fn looked_up(search: &Search, item_id: &str) -> AppResult<Candidate> {
    if checked_id(item_id) {
        if let Some(item) = lock(&search.inner).items.get(item_id).cloned() {
            return Ok(item);
        }
    }
    Err(browse_error("browse.item_unavailable"))
}

pub(crate) struct TrustedWork {
    pub source_id: String,
    pub project_id: String,
    pub provider_game_id: String,
    pub game_id: String,
    pub title: String,
    game_scope: GameScope,
}

pub(crate) async fn resolve_work(
    state: &BrowseState,
    services: &AppServices,
    window: &str,
    search_id: &str,
    item_id: &str,
    game_id: &str,
) -> AppResult<TrustedWork> {
    let search = search_ref(state, &services.engine.db, window, search_id)?;
    let item = looked_up(&search, item_id)?;
    let provider_game_id = item
        .targets
        .get(game_id)
        .ok_or_else(|| browse_error("browse.item_unavailable"))?;
    if !valid_source_for(services, &search, item.source) {
        return Err(browse_error("browse.authorization_changed"));
    }
    Ok(TrustedWork {
        source_id: item.source.into(),
        project_id: item.project_id,
        provider_game_id: provider_game_id.clone(),
        game_id: game_id.to_owned(),
        title: item.title,
        game_scope: search
            .pairs
            .iter()
            .find(|pair| pair.source == item.source && pair.game.id == game_id)
            .ok_or_else(|| browse_error("browse.item_unavailable"))?
            .game
            .clone(),
    })
}

const DISCOVERY_TTL: Duration = Duration::from_secs(3 * 60);

struct Discovery {
    db: Db,
    expires: Instant,
    connections: BTreeMap<&'static str, ProviderConnection>,
}

#[tauri::command]
pub(crate) fn browse_list_sources<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, BrowseState>,
    services: State<'_, AppServices>,
) -> AppResult<Vec<Source>> {
    let db = &services.engine.db;
    let providers = providers::sources(&services.mcp_connections, &McpServers::new(db.clone()));
    let discovery = Discovery {
        db: db.clone(),
        expires: Instant::now() + DISCOVERY_TTL,
        connections: providers
            .iter()
            .filter_map(|source| Some((source.id, source.connection.clone()?)))
            .collect(),
    };
    let mut windows = state.windows();
    if windows.len() >= MAX_WINDOWS && !windows.contains_key(window.label()) {
        return Err(browse_error("browse.request_limit"));
    }
    windows
        .entry(window.label().to_owned())
        .or_default()
        .discovery = Some(discovery);
    Ok(providers
        .into_iter()
        .map(|source| Source {
            id: source.id,
            name: source_name(source.id),
            status: source_status(source.availability),
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Authorization {
    authorization_id: String,
    sources: Vec<Source>,
}

fn checked_list(values: &[String], limit: usize, max_bytes: usize) -> bool {
    !values.is_empty()
        && values.len() <= limit
        && values
            .iter()
            .all(|id| !id.is_empty() && id.len() <= max_bytes)
        && values.iter().collect::<BTreeSet<_>>().len() == values.len()
}

fn scoped_connections(
    granted: &BTreeMap<&'static str, ProviderConnection>,
    requested: Option<&[String]>,
    mut is_current: impl FnMut(&str, &ProviderConnection) -> bool,
) -> AppResult<BTreeMap<&'static str, ProviderConnection>> {
    let expected = requested.map_or(granted.len(), <[String]>::len);
    let selected = granted
        .iter()
        .filter(|(id, _)| requested.is_none_or(|ids| ids.iter().any(|candidate| candidate == **id)))
        .collect::<Vec<_>>();
    // Reject a forged/new source list before even checking a connection.
    if selected.is_empty()
        || selected.len() != expected
        || selected
            .iter()
            .any(|(&source, identity)| !is_current(source, identity))
    {
        return Err(browse_error("browse.authorization_changed"));
    }
    Ok(selected
        .into_iter()
        .map(|(&source, identity)| (source, identity.clone()))
        .collect())
}

#[tauri::command]
pub(crate) fn browse_authorize<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, BrowseState>,
    services: State<'_, AppServices>,
    source_ids: Vec<String>,
) -> AppResult<Authorization> {
    if !checked_list(&source_ids, MAX_SOURCES, 64) {
        return Err(browse_error("browse.request_limit"));
    }
    let db = &services.engine.db;
    let mut windows = state.windows();
    let slot = windows
        .get_mut(window.label())
        .ok_or_else(|| browse_error("browse.authorization_changed"))?;
    let discovery = slot
        .discovery
        .as_ref()
        .filter(|last| Arc::ptr_eq(&db.0, &last.db.0))
        .ok_or_else(|| browse_error("browse.authorization_changed"))?;
    let servers = McpServers::new(db.clone());
    let connections =
        scoped_connections(&discovery.connections, Some(&source_ids), |id, expected| {
            current(&services.mcp_connections, &servers, id, expected)
        })?;
    let authorization_id = uuid::Uuid::new_v4().to_string();
    let sources = connections
        .keys()
        .map(|&id| Source {
            id,
            name: source_name(id),
            status: SourceStatus::Available,
        })
        .collect();
    if let Some(search) = slot.search.take() {
        search.cancel.cancel();
    }
    slot.grant = Some(Grant {
        id: authorization_id.clone(),
        db: db.clone(),
        expires: Instant::now() + AUTH_TTL,
        connections,
    });
    Ok(Authorization {
        authorization_id,
        sources,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Started {
    search_id: String,
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri injects four handles; the remaining four are the stable IPC.
pub(crate) fn browse_start_search<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, BrowseState>,
    services: State<'_, AppServices>,
    agent: State<'_, AgentRuntime>,
    authorization_id: String,
    game_ids: Vec<String>,
    query: String,
    source_ids: Option<Vec<String>>,
) -> AppResult<Started> {
    if !checked_id(&authorization_id) {
        return Err(browse_error("browse.authorization_changed"));
    }
    if !checked_list(&game_ids, MAX_GAMES, 64)
        || query.len() > MAX_QUERY
        || query.chars().any(char::is_control)
        || source_ids
            .as_ref()
            .is_some_and(|ids| !checked_list(ids, MAX_SOURCES, 64))
    {
        return Err(browse_error("browse.request_limit"));
    }
    let ctx = Context::new(&services, &agent.settings)?;
    let games = game_ids
        .into_iter()
        .map(|game_id| crate::games::get_game(ctx.db(), &game_id).map(GameScope::new))
        .collect::<AppResult<Vec<_>>>()?;
    let query = query.trim().to_owned();
    let search_id = uuid::Uuid::new_v4().to_string();
    let (tx, translations) = watch::channel(query.is_empty().then(QueryVariants::default));
    let mut windows = state.windows();
    let slot = windows
        .get_mut(window.label())
        .ok_or_else(|| browse_error("browse.authorization_changed"))?;
    let grant = slot
        .grant
        .as_ref()
        .filter(|grant| grant.id == authorization_id && Arc::ptr_eq(&grant.db.0, &ctx.db().0))
        .ok_or_else(|| browse_error("browse.authorization_changed"))?;
    let servers = ctx.servers();
    let connections = scoped_connections(&grant.connections, source_ids.as_deref(), |_, _| true)?;
    // Keep still-authorized providers usable if a peer disconnected. Each
    // captured connection is checked again by its lanes before any read.
    if !connections
        .iter()
        .any(|(&id, expected)| current(&ctx.connections, &servers, id, expected))
    {
        return Err(browse_error("browse.authorization_changed"));
    }
    let mut pairs = Vec::new();
    for game in games {
        for (&source, connection) in &connections {
            pairs.push(Pair {
                game: game.clone(),
                source,
                connection: connection.clone(),
            });
        }
    }
    let search = Arc::new(Search {
        id: search_id.clone(),
        db: ctx.db().clone(),
        expires: grant.expires.min(Instant::now() + SEARCH_TTL),
        pairs,
        query,
        translations,
        enrichment_slots: Semaphore::new(2),
        cancel: CancellationToken::new(),
        inner: Mutex::new(SearchInner {
            loading: true,
            ..Default::default()
        }),
    });
    if let Some(old) = slot.search.replace(search.clone()) {
        old.cancel.cancel();
    }
    drop(windows);
    if !search.query.is_empty() {
        spawn_translations(ctx.clone(), search.clone(), tx);
    }
    start_page(ctx, search, 0, None);
    Ok(Started { search_id })
}

#[tauri::command]
pub(crate) fn browse_get_page<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, BrowseState>,
    services: State<'_, AppServices>,
    agent: State<'_, AgentRuntime>,
    search_id: String,
    page: usize,
) -> AppResult<Page> {
    if page >= MAX_PAGES {
        return Err(browse_error("browse.request_limit"));
    }
    let ctx = Context::new(&services, &agent.settings)?;
    let search = search_ref(&state, ctx.db(), window.label(), &search_id)?;
    let available = available_sources(&ctx.connections, &ctx.servers(), &search);
    if available.is_empty() {
        search.cancel.cancel();
        return Err(browse_error("browse.authorization_changed"));
    }
    let locale = crate::app_locale::effective(ctx.db());
    let mut inner = lock(&search.inner);
    let mut issues = inner.issues.clone();
    for pair in &search.pairs {
        if !available.contains(pair.source) {
            issues.insert(pair.issue(IssueCode::PermissionDenied));
        }
    }
    let mut result = Page {
        status: PageStatus::Loading,
        page,
        items: Vec::new(),
        issues: issues.into_iter().collect(),
        game_choices: inner
            .choices
            .iter()
            .filter(|choice| available.contains(choice.source_id))
            .cloned()
            .collect(),
        has_next: false,
        is_enriching: false,
    };
    if page < inner.pages.len() {
        let frame = &inner.pages[page];
        let localized = frame
            .translated
            .as_ref()
            .filter(|(language, _)| *language == locale);
        let mut originals = Vec::with_capacity(frame.ids.len());
        for (index, id) in frame.ids.iter().enumerate() {
            let original = &inner.items[id];
            let visible = available.contains(original.source);
            if visible {
                let text = localized
                    .and_then(|(_, text)| text.get(index))
                    .cloned()
                    .unwrap_or_else(|| enrich::fallback_item(entry(original)));
                result
                    .items
                    .push(BrowseItem::display(id.clone(), original, text));
            }
            if localized.is_none() {
                originals.push((original.clone(), visible));
            }
        }
        result.status = PageStatus::Ready;
        result.has_next = frame.has_next;
        result.is_enriching = localized.is_none() && !result.items.is_empty();
        if result.is_enriching && frame.enriching.as_deref() != Some(&locale) {
            inner.pages[page].enriching = Some(locale.clone());
            drop(inner);
            start_enrichment(ctx, search, page, originals, locale);
        }
        return Ok(result);
    }
    if page != inner.pages.len() || !inner.pages.last().is_none_or(|last| last.has_next) {
        return Err(browse_error("browse.page_unavailable"));
    }
    if !inner.loading {
        let progress = inner.progress.take();
        inner.loading = true;
        drop(inner);
        start_page(ctx, search, page, progress);
    }
    Ok(result)
}

#[tauri::command]
pub(crate) fn browse_select_game_match<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, BrowseState>,
    services: State<'_, AppServices>,
    search_id: String,
    game_id: String,
    source_id: String,
    provider_game_id: String,
) -> AppResult<()> {
    let db = &services.engine.db;
    let search = search_ref(&state, db, window.label(), &search_id)?;
    let pair = search
        .pairs
        .iter()
        .find(|pair| pair.source == source_id && pair.game.id == game_id)
        .ok_or_else(|| browse_error("browse.item_unavailable"))?;
    let allowed = lock(&search.inner).choices.iter().any(|choice| {
        choice.source_id == source_id
            && choice.game_id == game_id
            && choice
                .options
                .iter()
                .any(|option| option.id == provider_game_id)
    });
    if !allowed {
        return Err(browse_error("browse.item_unavailable"));
    }
    if !valid_source_for(&services, &search, pair.source) {
        return Err(browse_error("browse.authorization_changed"));
    }
    if !matching::remember(db, &pair.game, pair.source, &provider_game_id)? {
        return Err(browse_error("browse.search_expired"));
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn browse_open_project<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, BrowseState>,
    services: State<'_, AppServices>,
    search_id: String,
    item_id: String,
) -> AppResult<()> {
    use tauri::Manager;
    let search = search_ref(&state, &services.engine.db, window.label(), &search_id)?;
    let item = looked_up(&search, &item_id)?;
    let url = item
        .page_url
        .ok_or_else(|| browse_error("browse.project_page_unavailable"))?;
    if !valid_source_for(&services, &search, item.source) {
        return Err(browse_error("browse.authorization_changed"));
    }
    window
        .app_handle()
        .opener()
        .open_url(url, None::<&str>)
        .map_err(|_| browse_error("browse.project_open_failed"))
}

#[tauri::command]
pub(crate) fn browse_cancel_search<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, BrowseState>,
    search_id: String,
) {
    if let Some(slot) = state.windows().get_mut(window.label()) {
        if slot
            .search
            .as_ref()
            .is_some_and(|active| active.id == search_id)
        {
            if let Some(search) = slot.search.take() {
                search.cancel.cancel();
            }
        }
    }
}

#[cfg(test)]
mod tests;
