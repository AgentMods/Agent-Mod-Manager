use futures_util::{stream, StreamExt};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::future::Future;

use super::links::{self, public_text, valid_provider_id};
use super::providers::{ProviderConnection, ProviderItem, ProviderPage};
use super::{GameScope, PAGE_SIZE};

const MAX_CALLS_PER_PAGE: usize = 40;
const MAX_CALLS_PER_STREAM: usize = 12;
const MAX_PROVIDER_ITEMS: usize = 32;
const REFILL_PARALLEL: usize = 8;

#[derive(Clone)]
pub(super) struct Pair {
    pub game: GameScope,
    pub source: &'static str,
    pub connection: ProviderConnection,
}

#[derive(Clone)]
pub(super) struct MatchedPair {
    pub pair: Pair,
    pub provider_game_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum IssueCode {
    NoGameMatch,
    PermissionDenied,
    QueryFailed,
    LimitReached,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Issue {
    pub(super) game_id: String,
    pub(super) source_id: &'static str,
    pub(super) code: IssueCode,
}

impl Pair {
    pub fn issue(&self, code: IssueCode) -> Issue {
        Issue {
            game_id: self.game.id.clone(),
            source_id: self.source,
            code,
        }
    }
}

#[derive(Clone)]
pub(super) struct Candidate {
    pub source: &'static str,
    pub project_id: String,
    pub title: String,
    pub author: Option<String>,
    pub summary: Option<String>,
    pub cover_url: Option<String>,
    pub page_url: Option<String>,
    pub kind: Option<String>,
    pub tags: Vec<String>,
    pub targets: BTreeMap<String, String>, // local game ID → provider game ID
}

impl Candidate {
    fn checked(matched: &MatchedPair, item: ProviderItem) -> Option<Self> {
        let source = matched.pair.source;
        if !valid_provider_id(&item.id) {
            return None;
        }
        Some(Self {
            source,
            page_url: item
                .page_url
                .and_then(|raw| links::project(source, &matched.provider_game_id, &item.id, &raw)),
            cover_url: item.cover_url.and_then(|raw| links::cover(source, &raw)),
            title: public_text(&item.title, 220)?,
            author: item.author.and_then(|author| public_text(&author, 128)),
            summary: item.summary.and_then(|summary| public_text(&summary, 1500)),
            kind: item.kind.and_then(|kind| public_text(&kind, 96)),
            tags: item
                .tags
                .into_iter()
                .take(18)
                .filter_map(|tag| public_text(&tag, 72))
                .collect(),
            project_id: item.id,
            targets: BTreeMap::from([(
                matched.pair.game.id.clone(),
                matched.provider_game_id.clone(),
            )]),
        })
    }
}

#[derive(Clone)]
pub(super) struct Fetch {
    pub matched: MatchedPair,
    pub query: String,
    pub cursor: Option<String>,
}

struct Feed {
    matched: MatchedPair,
    query: String,
    cursor: Option<String>,
    visited: BTreeSet<String>,
    ready: VecDeque<Candidate>,
    calls: usize,
    done: bool,
}

impl Feed {
    fn open(matched: MatchedPair, query: String) -> Self {
        Self {
            matched,
            query,
            cursor: None,
            visited: Default::default(),
            ready: Default::default(),
            calls: 0,
            done: false,
        }
    }

    fn ingest(&mut self, result: Result<ProviderPage, IssueCode>) -> Option<IssueCode> {
        let page = match result {
            Ok(page) => page,
            Err(code) => {
                self.done = true;
                return Some(code);
            }
        };
        let overflow = page.items.len() > MAX_PROVIDER_ITEMS;
        self.ready.extend(
            page.items
                .into_iter()
                .take(MAX_PROVIDER_ITEMS)
                .filter_map(|item| Candidate::checked(&self.matched, item)),
        );
        self.cursor = page.next;
        if overflow {
            self.done = true;
            return Some(IssueCode::LimitReached);
        }
        if let Some(cursor) = &self.cursor {
            if !valid_cursor(cursor) || !self.visited.insert(cursor.clone()) {
                self.done = true;
                return Some(IssueCode::QueryFailed);
            }
            if self.calls >= MAX_CALLS_PER_STREAM {
                self.done = true;
                return Some(IssueCode::LimitReached);
            }
        } else {
            self.done = true;
        }
        None
    }
}

pub(super) fn valid_cursor(value: &str) -> bool {
    !value.is_empty() && value.len() <= 2048 && !value.chars().any(char::is_control)
}

pub(super) type ItemKey = (&'static str, String);

#[derive(Default)]
pub(super) struct Mixer {
    feeds: Vec<Feed>,
    turn: usize,
    seen: HashMap<ItemKey, String>,
    pub issues: BTreeSet<Issue>,
}

#[derive(Default)]
pub(super) struct MixedPage {
    pub ids: Vec<String>,
    pub updates: BTreeMap<String, Candidate>,
}

impl Mixer {
    pub fn push(&mut self, matched: MatchedPair, query: String) {
        let old_len = self.feeds.len();
        self.feeds.push(Feed::open(matched, query));
        // With no active original feeds, leave the turn at the first translated
        // feed even when the old ring was exhausted at a nonzero index.
        if !self.feeds[..old_len]
            .iter()
            .any(|feed| !feed.done || !feed.ready.is_empty())
        {
            self.turn = old_len;
        }
    }

    /// Start the next pass at the first lane being appended rather than at a
    /// still-live original lane that just produced an empty result page.
    pub fn prioritize_next_feed(&mut self) {
        self.turn = self.feeds.len();
    }

    pub fn retire_unavailable(&mut self, available: &BTreeSet<&'static str>) {
        for feed in &mut self.feeds {
            if !available.contains(feed.matched.pair.source) {
                feed.done = true;
                feed.ready.clear();
                self.issues
                    .insert(feed.matched.pair.issue(IssueCode::PermissionDenied));
            }
        }
    }

    pub fn has_more(&self) -> bool {
        self.feeds
            .iter()
            .any(|feed| !feed.done || !feed.ready.is_empty())
    }

    fn cyclic(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.feeds.len()).map(|step| (step + self.turn) % self.feeds.len())
    }

    pub async fn page<F, Fut>(&mut self, mut fetch: F) -> MixedPage
    where
        F: FnMut(Fetch) -> Fut,
        Fut: Future<Output = Result<ProviderPage, IssueCode>>,
    {
        let mut remaining = MAX_CALLS_PER_PAGE;
        let mut page = MixedPage::default();
        while page.ids.len() < PAGE_SIZE {
            let next = self.cyclic().find(|&index| {
                let feed = &self.feeds[index];
                !feed.ready.is_empty() || (!feed.done && remaining != 0)
            });
            let Some(next) = next else { break };
            if let Some(candidate) = self.feeds[next].ready.pop_front() {
                self.turn = (next + 1) % self.feeds.len();
                let key = (candidate.source, candidate.project_id.clone());
                let id = self.seen.entry(key).or_insert_with(|| {
                    let id = uuid::Uuid::new_v4().to_string();
                    page.ids.push(id.clone());
                    id
                });
                page.updates
                    .entry(id.clone())
                    .and_modify(|prior| prior.targets.extend(candidate.targets.clone()))
                    .or_insert(candidate);
                continue;
            }
            // Dispatch a bounded refill wave, then consume buffers in ring order
            // regardless of completion order or how fast each provider responded.
            let indexes = self
                .cyclic()
                .filter(|&index| {
                    let feed = &self.feeds[index];
                    !feed.done && feed.ready.is_empty()
                })
                .take(remaining.min(REFILL_PARALLEL))
                .collect::<Vec<_>>();
            remaining -= indexes.len();
            let pending = indexes
                .into_iter()
                .map(|index| {
                    let feed = &mut self.feeds[index];
                    feed.calls += 1;
                    let future = fetch(Fetch {
                        matched: feed.matched.clone(),
                        query: feed.query.clone(),
                        cursor: feed.cursor.clone(),
                    });
                    async move { (index, future.await) }
                })
                .collect::<Vec<_>>();
            let results = stream::iter(pending)
                .buffer_unordered(REFILL_PARALLEL)
                .collect::<Vec<_>>()
                .await;
            for (index, result) in results {
                let feed = &mut self.feeds[index];
                if let Some(code) = feed.ingest(result) {
                    self.issues.insert(feed.matched.pair.issue(code));
                }
            }
            if self.feeds[next].ready.is_empty() {
                self.turn = (next + 1) % self.feeds.len();
            }
        }
        page
    }
}
