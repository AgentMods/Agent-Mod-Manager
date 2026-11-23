//! The IPC wrappers `list_manager_plugin_games` and `prepare_manager_plugin_import` call `search`
//! and `prepare_import` respectively. Release selectors are decimal strings across IPC.
//! Anonymous, read-only Mod客栈 catalog. Only locally constructed release URLs enter the HTTP client.
use crate::{
    error::{AppError, AppResult},
    manager_publication::version_cmp,
    plugin_workflow::{self, CatalogUpdateCandidate, PackageHashes},
    plugins::{self, Platform, PluginMcpServer},
    transaction::{FileEngine, FilePlan},
};
use reqwest::{header::ACCEPT, Client, StatusCode};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::{collections::HashSet, time::Duration};

const ORIGIN: &str = "https://www.glossmod.com";
const SOURCE: &str = "https://www.glossmod.com/manager-plugins";
const GAMES_PATH: &str = "/api/v1/manager-plugins/games";
const CATALOG_LIMIT: usize = 4 * 1024 * 1024;

// Keep database integer IDs exact across IPC, including IDs beyond JavaScript's safe integer range.
fn positive_id(id: &str) -> bool {
    id.parse::<i64>()
        .is_ok_and(|n| n > 0 && n.to_string() == id)
}
fn catalog_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Integer(i64),
        Decimal(String),
    }
    let id = match Id::deserialize(deserializer)? {
        Id::Integer(n) => n.to_string(),
        Id::Decimal(id) => id,
    };
    if !positive_id(&id) {
        return Err(serde::de::Error::custom("目录 ID 必须是正整数"));
    }
    Ok(id)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPlugin {
    #[serde(deserialize_with = "catalog_id")]
    pub id: String,
    #[serde(deserialize_with = "catalog_id")]
    pub release_id: String,
    pub slug: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_name: Option<String>,
    pub version: String,
    pub platforms: Vec<Platform>,
    pub description: String,
    #[serde(deserialize_with = "catalog_id")]
    pub author_id: String,
    pub author_name: String,
    pub steam_app_id: String,
    pub tags: Vec<String>,
    pub hidden: bool,
    pub adapter_url: String,
    pub skill_url: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogGame {
    #[serde(deserialize_with = "catalog_id")]
    pub id: String,
    pub name: String,
    pub slug: String,
    pub aliases: Vec<String>,
    pub steam_app_id: String,
    pub plugins: Vec<CatalogPlugin>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogReleaseSelection {
    pub plugin_id: String,
    pub release_id: String,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSearch {
    pub games: Vec<CatalogGame>,
    pub total_games: usize,
    pub skipped_entries: usize,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseDisplay {
    pub description: String,
    pub cover_url: Option<String>,
}

impl ReleaseDisplay {
    fn from_release(cover_url: Option<String>, skill: &str) -> Option<Self> {
        let description = crate::publication_copy::read_display(skill);
        (description.is_some() || cover_url.is_some()).then(|| Self {
            description: description.unwrap_or_default(),
            cover_url,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPreview {
    pub game_name: String,
    pub plugin: CatalogPlugin,
    pub source: &'static str,
    pub adapter_json: String,
    pub skill_markdown: String,
    pub hashes: PackageHashes,
    pub mcp_servers: Vec<PluginMcpServer>,
    pub installed_version: Option<String>,
    pub display: Option<ReleaseDisplay>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogImport {
    pub preview: CatalogPreview,
    pub draft_id: String,
    #[serde(serialize_with = "crate::transaction::serialize_user_plan")]
    pub plan: FilePlan,
}

fn release_path(release_id: &str, file: &str) -> String {
    format!("/api/v1/manager-plugins/releases/{release_id}/{file}")
}
fn display_text(s: &str, multiline: bool) -> bool {
    !s.trim().is_empty() && !s.chars().any(|c| {
        matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            || (c.is_control() && !(multiline && matches!(c, '\r' | '\n' | '\t')))
    })
}
fn valid_steam_id(id: &str) -> bool {
    id.is_empty()
        || id
            .parse::<u32>()
            .is_ok_and(|n| n > 0 && n.to_string() == id)
}

fn valid_plugin(p: &CatalogPlugin, game_steam_id: &str) -> bool {
    if plugins::validate_slug(&p.slug).is_err()
        || plugins::validate_version(&p.version).is_err()
        || !display_text(&p.name, false)
        || p.game_name.as_deref().is_some_and(|name| {
            !name.is_empty()
                && (name.len() > 256 || name.trim() != name || !display_text(name, false))
        })
        || !display_text(&p.author_name, false)
        || !display_text(&p.description, true)
        || !valid_steam_id(&p.steam_app_id)
        || (!game_steam_id.is_empty()
            && !p.steam_app_id.is_empty()
            && game_steam_id != p.steam_app_id)
        || p.platforms.is_empty()
        || p.platforms.len() > 3
        || p.platforms
            .iter()
            .enumerate()
            .any(|(i, v)| p.platforms[..i].contains(v))
        || p.tags.iter().any(|s| !display_text(s, false))
    {
        return false;
    }
    [(&p.adapter_url, "adapter.json"), (&p.skill_url, "SKILL.md")]
        .iter()
        .all(|(url, file)| {
            let path = release_path(&p.release_id, file);
            url.as_str() == path || url.as_str() == format!("{ORIGIN}{path}")
        })
}

// Treat numeric and decimal-string IDs as one identity, including rows later hidden or rejected.
fn ambiguous_ids<'a>(rows: impl Iterator<Item = &'a Value>, field: &str) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut duplicates = HashSet::new();
    for row in rows {
        let id = match &row[field] {
            Value::Number(n) if n.as_i64().is_some_and(|i| i > 0) => n.to_string(),
            Value::String(s) if positive_id(s) => s.clone(),
            _ => continue,
        };
        if !seen.insert(id.clone()) {
            duplicates.insert(id);
        }
    }
    duplicates
}

struct CatalogSnapshot {
    search: CatalogSearch,
    ambiguous_slugs: HashSet<String>,
}

impl CatalogSnapshot {
    fn update_release(&self, candidate: &CatalogUpdateCandidate) -> Option<(&str, &CatalogPlugin)> {
        // Old installs have no stable ID. Never infer it from a slug present in
        // more than one row, even if only one of those rows survived validation.
        if candidate.plugin_id.is_none() && self.ambiguous_slugs.contains(&candidate.slug) {
            return None;
        }
        self.search.games.iter().find_map(|game| {
            game.plugins
                .iter()
                .find(|plugin| {
                    plugin.slug == candidate.slug
                        && candidate
                            .plugin_id
                            .as_ref()
                            .is_none_or(|id| id == &plugin.id)
                })
                .map(|plugin| (game.name.as_str(), plugin))
        })
    }
}

fn parse_catalog(text: &str) -> AppResult<CatalogSnapshot> {
    #[derive(Deserialize)]
    struct Envelope {
        data: Vec<Value>,
    }
    let envelope: Envelope = serde_json::from_str(text)
        .map_err(|_| AppError::Invalid("云端插件目录响应格式无效".into()))?;
    let ambiguous_games = ambiguous_ids(envelope.data.iter(), "id");
    let raw_plugins = envelope
        .data
        .iter()
        .filter_map(|g| g["plugins"].as_array())
        .flatten();
    let ambiguous_plugins = ambiguous_ids(raw_plugins.clone(), "id");
    let ambiguous_releases = ambiguous_ids(raw_plugins.clone(), "releaseId");
    let mut seen_slugs = HashSet::new();
    let ambiguous_slugs = raw_plugins
        .filter_map(|row| row["slug"].as_str())
        .filter(|slug| !seen_slugs.insert(*slug))
        .map(str::to_owned)
        .collect();
    let mut games = Vec::new();
    let mut skipped_entries = 0;
    for mut row in envelope.data {
        let Some(plugins) = row
            .get_mut("plugins")
            .and_then(Value::as_array_mut)
            .map(std::mem::take)
        else {
            skipped_entries += 1;
            continue;
        };
        let Ok(mut game) = serde_json::from_value::<CatalogGame>(row) else {
            skipped_entries += 1;
            continue;
        };
        if ambiguous_games.contains(&game.id)
            || !display_text(&game.name, false)
            || plugins::validate_slug(&game.slug).is_err()
            || !valid_steam_id(&game.steam_app_id)
            || game.aliases.iter().any(|a| !display_text(a, false))
        {
            skipped_entries += 1;
            continue;
        }
        for entry in plugins {
            if entry.get("hidden").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            match serde_json::from_value::<CatalogPlugin>(entry) {
                Ok(p)
                    if !ambiguous_plugins.contains(&p.id)
                        && !ambiguous_releases.contains(&p.release_id)
                        && valid_plugin(&p, &game.steam_app_id) =>
                {
                    game.plugins.push(p)
                }
                _ => skipped_entries += 1,
            }
        }
        if !game.plugins.is_empty() {
            games.push(game);
        }
    }
    if games.is_empty() && skipped_entries != 0 {
        return Err(AppError::Invalid(format!(
            "云端目录含有 {skipped_entries} 项无效或不兼容的记录，当前没有可用版本"
        )));
    }
    Ok(CatalogSnapshot {
        search: CatalogSearch {
            total_games: games.len(),
            games,
            skipped_entries,
        },
        ambiguous_slugs,
    })
}

fn matches_game(game: &CatalogGame, query: &str) -> bool {
    let matches = |s: &str| s.to_lowercase().contains(query);
    matches(&game.name)
        || matches(&game.slug)
        || game.steam_app_id == query
        || game.aliases.iter().any(|a| matches(a))
        || game
            .plugins
            .iter()
            .any(|p| matches(&p.name) || matches(&p.slug) || p.steam_app_id == query)
}

#[derive(Clone)]
pub(crate) struct CatalogClient {
    http: Client,
    #[cfg(test)]
    fixture_origin: Option<String>,
}
impl CatalogClient {
    pub(crate) fn new() -> AppResult<Self> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(6))
            .build()?;
        Ok(Self {
            http,
            #[cfg(test)]
            fixture_origin: None,
        })
    }
    #[cfg(test)]
    pub(crate) fn for_fixture(origin: &str) -> AppResult<Self> {
        let mut client = Self::new()?;
        client.fixture_origin = Some(origin.trim_end_matches('/').to_owned());
        Ok(client)
    }

    fn origin(&self) -> &str {
        #[cfg(test)]
        if let Some(origin) = &self.fixture_origin {
            return origin;
        }
        ORIGIN
    }
    async fn get(&self, path: &str, limit: usize, accept: &str) -> AppResult<String> {
        let mut response = self
            .http
            .get(format!("{}{path}", self.origin()))
            .header(ACCEPT, accept)
            .send()
            .await?;
        match response.status() {
            StatusCode::OK => (),
            StatusCode::NOT_FOUND | StatusCode::GONE => {
                return Err(AppError::NotFound(
                    "云端插件目录或指定版本的文件不存在，可能已下架".into(),
                ))
            }
            status => {
                return Err(AppError::Other(format!(
                    "读取云端插件失败，HTTP {}",
                    status.as_u16()
                )))
            }
        }
        let too_large = || AppError::Invalid("云端目录或插件文件超过大小限制".into());
        if response
            .content_length()
            .is_some_and(|length| length > limit as u64)
        {
            return Err(too_large());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if chunk.len() > limit - bytes.len() {
                return Err(too_large());
            }
            bytes.extend_from_slice(&chunk);
        }
        String::from_utf8(bytes)
            .map_err(|_| AppError::Invalid("云端目录或插件文件必须是有效的 UTF-8".into()))
    }
    async fn games(&self) -> AppResult<CatalogSearch> {
        Ok(self.snapshot().await?.search)
    }

    async fn snapshot(&self) -> AppResult<CatalogSnapshot> {
        parse_catalog(
            &self
                .get(GAMES_PATH, CATALOG_LIMIT, "application/json")
                .await?,
        )
    }
    pub(crate) async fn search(&self, query: &str) -> AppResult<CatalogSearch> {
        if query.len() > 512 || query.chars().any(char::is_control) {
            return Err(AppError::Invalid("云目录搜索词无效或过长".into()));
        }
        let query = query.trim().to_lowercase();
        let mut result = self.games().await?;
        if !query.is_empty() {
            result.games.retain(|g| matches_game(g, &query));
        }
        Ok(result)
    }
    async fn release_files(&self, release_id: &str) -> AppResult<(String, String)> {
        if !positive_id(release_id) {
            return Err(AppError::Invalid("云插件版本 ID 必须是正整数".into()));
        }
        let adapter_path = release_path(release_id, "adapter.json");
        let skill_path = release_path(release_id, "SKILL.md");
        tokio::try_join!(
            self.get(
                &adapter_path,
                plugin_workflow::ADAPTER_LIMIT,
                "application/json, text/plain"
            ),
            self.get(
                &skill_path,
                plugin_workflow::SKILL_LIMIT,
                "text/markdown, text/plain"
            )
        )
    }

    async fn display(&self, release_id: &str) -> AppResult<Option<ReleaseDisplay>> {
        let (manifest, skill) = self.release_files(release_id).await?;
        let adapter: plugins::GameAdapter = serde_json::from_str(&manifest)?;
        adapter.validate()?;
        plugins::parse_skill(&skill, &adapter.slug)?;
        Ok(ReleaseDisplay::from_release(adapter.cover_url, &skill))
    }

    pub(crate) async fn preview(
        &self,
        engine: &FileEngine,
        plugin_id: &str,
        release_id: &str,
    ) -> AppResult<CatalogPreview> {
        if !positive_id(plugin_id) || !positive_id(release_id) {
            return Err(AppError::Invalid("云插件 ID 与版本 ID 必须是正整数".into()));
        }
        let snapshot = self.games().await?;
        let (game_name, plugin) = snapshot
            .games
            .into_iter()
            .find_map(|g| {
                g.plugins
                    .into_iter()
                    .find(|p| p.id == plugin_id && p.release_id == release_id)
                    .map(|p| (g.name, p))
            })
            .ok_or_else(|| {
                if snapshot.skipped_entries > 0 {
                    AppError::NotFound(format!(
                        "未找到可用的所选版本，目录有 {} 项数据无效或不兼容，请刷新目录",
                        snapshot.skipped_entries
                    ))
                } else {
                    AppError::NotFound("所选云插件版本已更新、已下架或不存在，请刷新目录".into())
                }
            })?;
        let mut preview = self.preview_release(&game_name, plugin).await?;
        preview.installed_version = engine
            .db
            .conn()
            .query_row(
                "SELECT version FROM plugins WHERE slug=?1",
                [&preview.plugin.slug],
                |row| row.get(0),
            )
            .optional()?;
        Ok(preview)
    }

    // A pinned release from an already validated catalog snapshot. Do not look
    // up the directory again between selection and downloading its files.
    async fn preview_release(
        &self,
        game_name: &str,
        plugin: CatalogPlugin,
    ) -> AppResult<CatalogPreview> {
        let (adapter_json, skill_markdown) = self.release_files(&plugin.release_id).await?;
        let package = plugins::parse_package(&plugin.slug, &adapter_json, &skill_markdown)?;
        let display =
            ReleaseDisplay::from_release(package.adapter.cover_url.clone(), &skill_markdown);
        let adapter = package.adapter;
        if adapter.version != plugin.version
            || adapter.name != plugin.name
            || adapter.platforms != plugin.platforms
            || adapter.steam_app_id.as_deref().unwrap_or_default() != plugin.steam_app_id
        {
            return Err(AppError::Conflict(
                "云端目录与指定版本的适配包或 Skill 声明不一致".into(),
            ));
        }
        Ok(CatalogPreview {
            game_name: adapter.game_name.unwrap_or_else(|| game_name.to_owned()),
            plugin,
            source: SOURCE,
            hashes: PackageHashes::of(&adapter_json, &skill_markdown),
            display,
            adapter_json,
            skill_markdown,
            mcp_servers: adapter.mcp_servers,
            installed_version: None,
        })
    }

    async fn update_installed_at_startup(
        &self,
        engine: &FileEngine,
        on_updated: impl Fn(&str),
    ) -> AppResult<()> {
        let candidate_engine = engine.clone();
        let candidates = tokio::task::spawn_blocking(move || {
            plugin_workflow::catalog_update_candidates(&candidate_engine)
        })
        .await
        .map_err(|_| AppError::Other("启动时读取已安装云插件失败".into()))??;
        if candidates.is_empty() {
            return Ok(());
        }
        let snapshot = self.snapshot().await?;
        for candidate in candidates {
            let Some((game_name, plugin)) = snapshot.update_release(&candidate) else {
                continue;
            };
            if let Err(error) = plugins::validate_version(&candidate.version) {
                tracing::warn!(slug = %candidate.slug, %error, "跳过无法比较版本的已安装云插件");
                continue;
            }
            if !version_cmp(&plugin.version, &candidate.version).is_gt() {
                continue;
            }
            let slug = candidate.slug.clone();
            match self
                .update_candidate(engine, candidate, game_name, plugin)
                .await
            {
                Ok(()) => on_updated(&slug),
                Err(error) => tracing::warn!(
                    %slug, release_id = %plugin.release_id, %error, "启动时更新云插件失败"
                ),
            }
        }
        Ok(())
    }

    async fn update_candidate(
        &self,
        engine: &FileEngine,
        candidate: CatalogUpdateCandidate,
        game_name: &str,
        plugin: &CatalogPlugin,
    ) -> AppResult<()> {
        let mut preview = self.preview_release(game_name, plugin.clone()).await?;
        preview.installed_version = Some(candidate.version.clone());
        let engine = engine.clone();
        tokio::task::spawn_blocking(move || {
            plugin_workflow::update_catalog_release(&engine, &candidate, preview)
        })
        .await
        .map_err(|_| AppError::Other("启动时提交云插件更新失败".into()))?
    }

    pub(crate) async fn prepare_import(
        &self,
        engine: &FileEngine,
        plugin_id: &str,
        release_id: &str,
    ) -> AppResult<CatalogImport> {
        let preview = self.preview(engine, plugin_id, release_id).await?;
        let engine = engine.clone();
        tokio::task::spawn_blocking(move || {
            let draft = plugin_workflow::plan_write_draft(
                &engine,
                &preview.plugin.slug,
                &preview.adapter_json,
                &preview.skill_markdown,
            )?;
            plugin_workflow::record_catalog_draft_origin(
                &engine,
                &draft.draft.id,
                &preview.plugin.id,
                &preview.hashes,
            )?;
            Ok(CatalogImport {
                preview,
                draft_id: draft.draft.id,
                plan: draft.plan,
            })
        })
        .await
        .map_err(|_| AppError::Other("准备云插件草稿失败".into()))?
    }

    async fn install(
        &self,
        engine: &FileEngine,
        plugin_id: &str,
        release_id: &str,
    ) -> AppResult<()> {
        let preview = self.preview(engine, plugin_id, release_id).await?;
        let engine = engine.clone();
        tokio::task::spawn_blocking(move || {
            plugin_workflow::install_catalog_release(&engine, preview)
        })
        .await
        .map_err(|_| AppError::Other("安装云插件失败".into()))?
    }
}

pub async fn search(query: &str) -> AppResult<CatalogSearch> {
    CatalogClient::new()?.search(query).await
}
/// Immutable, anonymous same-origin release files. Hidden releases are checked by the provider.
pub async fn release_display(release_id: &str) -> AppResult<Option<ReleaseDisplay>> {
    CatalogClient::new()?.display(release_id).await
}

pub async fn preview(
    engine: &FileEngine,
    plugin_id: &str,
    release_id: &str,
) -> AppResult<CatalogPreview> {
    CatalogClient::new()?
        .preview(engine, plugin_id, release_id)
        .await
}
pub async fn prepare_import(
    engine: &FileEngine,
    plugin_id: &str,
    release_id: &str,
) -> AppResult<CatalogImport> {
    CatalogClient::new()?
        .prepare_import(engine, plugin_id, release_id)
        .await
}

pub async fn install(engine: &FileEngine, plugin_id: &str, release_id: &str) -> AppResult<()> {
    CatalogClient::new()?
        .install(engine, plugin_id, release_id)
        .await
}

/// Refresh installed cloud packages from one anonymous catalog snapshot.
/// Global discovery failures are returned; individual package failures are logged.
pub(crate) async fn update_installed_at_startup(
    engine: &FileEngine,
    on_updated: impl Fn(&str),
) -> AppResult<()> {
    CatalogClient::new()?
        .update_installed_at_startup(engine, on_updated)
        .await
}

#[cfg(test)]
#[path = "manager_catalog_tests.rs"]
mod tests;
