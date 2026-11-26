//! Short-lived normalized MCP responses shared only by searches under one window grant.
use super::providers::{ProviderGamePage, ProviderPage};
use std::collections::VecDeque;
use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(5 * 60);
const PAGE_LIMIT: usize = 64;

#[derive(Default)]
pub(super) struct CatalogCache {
    pub games: Pages<ProviderGamePage>,
    pub mods: Pages<ProviderPage>,
}

#[derive(PartialEq, Eq)]
pub(super) struct PageKey {
    source: &'static str,
    game: Option<String>,
    query: String,
    cursor: Option<String>,
}

impl PageKey {
    pub fn new(
        source: &'static str,
        game: Option<&str>,
        query: &str,
        cursor: Option<&str>,
    ) -> Self {
        Self {
            source,
            game: game.map(str::to_owned),
            query: query.to_owned(),
            cursor: cursor.map(str::to_owned),
        }
    }
}

struct Entry<T> {
    key: PageKey,
    expires: Instant,
    value: T,
}

pub(super) struct Pages<T>(Mutex<VecDeque<Entry<T>>>);

impl<T> Default for Pages<T> {
    fn default() -> Self {
        Self(Mutex::default())
    }
}

impl<T: Clone> Pages<T> {
    fn get(&self, key: &PageKey, now: Instant) -> Option<T> {
        let mut entries = super::lock(&self.0);
        entries.retain(|entry| entry.expires > now);
        entries
            .iter()
            .find(|entry| entry.key == *key)
            .map(|entry| entry.value.clone())
    }

    pub async fn read<E>(
        &self,
        key: PageKey,
        fetch: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        if let Some(value) = self.get(&key, Instant::now()) {
            return Ok(value);
        }
        let value = fetch.await?;
        let mut entries = super::lock(&self.0);
        entries.retain(|entry| entry.key != key);
        if entries.len() == PAGE_LIMIT {
            entries.pop_front();
        }
        entries.push_back(Entry {
            key,
            expires: Instant::now() + TTL,
            value: value.clone(),
        });
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn separates_queries_games_sources_and_pages_and_retries_after_errors_or_expiry() {
        let pages = Pages::default();
        let key = || PageKey::new("nexusmods", Some("gta"), "car", None);
        assert_eq!(
            pages.read(key(), async { Err("offline") }).await,
            Err("offline")
        );
        assert_eq!(pages.read(key(), async { Ok::<_, &str>(1) }).await, Ok(1));
        assert_eq!(pages.read(key(), async { Err("not called") }).await, Ok(1));
        for other in [
            PageKey::new("gamebanana", Some("gta"), "car", None),
            PageKey::new("nexusmods", Some("gta-enhanced"), "car", None),
            PageKey::new("nexusmods", Some("gta"), "cars", None),
            PageKey::new("nexusmods", Some("gta"), "car", Some("1")),
        ] {
            assert!(pages.get(&other, Instant::now()).is_none());
        }
        assert!(pages.get(&key(), Instant::now() + TTL).is_none());
        assert_eq!(pages.read(key(), async { Ok::<_, &str>(2) }).await, Ok(2));
        for i in 0..PAGE_LIMIT {
            let other = PageKey::new("nexusmods", Some("gta"), &i.to_string(), None);
            pages.read(other, async { Ok::<_, &str>(i) }).await.unwrap();
        }
        assert!(pages.get(&key(), Instant::now()).is_none());
    }
}
