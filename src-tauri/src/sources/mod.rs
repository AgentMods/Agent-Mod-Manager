//! Read-only provider integration. Secrets enter through SourceKeys, never DTOs.
//! API references and provider-specific restrictions live beside each adapter.
mod http;
mod providers;
pub mod resolutions;
pub(crate) mod web_download;
pub mod web_search;
pub use providers::dependency_graph::{DependencyGraph, DependencyGraphRequest};
pub(crate) use providers::three_dm::select_resource as select_three_dm_resource;

#[cfg(test)]
mod live_provider_tests;

use crate::error::{AppError, AppResult};
use async_trait::async_trait;
use futures_util::future::join_all;
pub(crate) use http::pinned_download_client;
pub use http::validate_download_url;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    Available,
    MissingKey,
    InvalidKey,
    Unsupported,
    NeedsConfiguration,
    ManualDownload,
    Limited,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceStatus {
    pub source_id: String,
    pub state: SourceState,
    pub message: String,
}

impl SourceStatus {
    pub(crate) fn new(id: &str, state: SourceState, message: &str) -> Self {
        Self {
            source_id: id.into(),
            state,
            message: message.into(),
        }
    }
}

/// Do not derive Debug/Serialize: these are in-memory credentials supplied by
/// the application's secret store, not settings suitable for an agent prompt.
#[derive(Default)]
pub struct SourceKeys {
    pub curseforge: Option<String>,
    pub nexus: Option<String>,
    pub three_dm: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchQuery {
    pub query: String,
    /// Provider IDs -> provider-native game IDs. Examples: modrinth=minecraft,
    /// thunderstore=valheim, curseforge=432, nexus=skyrimspecialedition,
    /// 3dm=2. A Steam App ID is not a provider-native game ID.
    pub game_ids: BTreeMap<String, String>,
    pub game_version: Option<String>,
    pub loader: Option<String>,
    pub offset: u32,
    pub limit: u32,
    /// Empty means all providers. Unknown providers return an explicit status.
    pub source_ids: Vec<String>,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            query: String::new(),
            game_ids: BTreeMap::new(),
            game_version: None,
            loader: None,
            offset: 0,
            limit: 20,
            source_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModSummary {
    pub source_id: String,
    pub project_id: String,
    pub game_id: Option<String>,
    pub name: String,
    pub summary: String,
    pub author: Option<String>,
    pub homepage_url: Option<String>,
    pub icon_url: Option<String>,
    pub downloads: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub mods: Vec<ModSummary>,
    pub sources: Vec<SourceStatus>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ResolveRequest {
    pub source_id: String,
    /// May be omitted only for an exact Modrinth version lookup.
    pub project_id: String,
    pub version_id: Option<String>,
    pub game_id: Option<String>,
    pub game_version: Option<String>,
    pub loader: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionListRequest {
    pub source_id: String,
    pub project_id: String,
    /// Provider-native game ID, not an installed game's local identifier.
    pub game_id: Option<String>,
    pub game_version: Option<String>,
    pub loader: Option<String>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "version_page_size")]
    pub limit: usize,
}

fn version_page_size() -> usize {
    20
}

fn valid_version_filter(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseChannel {
    Release,
    Beta,
    Alpha,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModVersionSummary {
    pub version_id: String,
    pub version: String,
    pub name: String,
    pub release_channel: ReleaseChannel,
    pub published_at: String,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionListResult {
    pub status: SourceStatus,
    pub versions: Vec<ModVersionSummary>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct FileHashes {
    pub sha256: Option<String>,
    pub sha512: Option<String>,
    /// Legacy integrity checks only; neither algorithm proves authenticity.
    pub sha1: Option<String>,
    pub md5: Option<String>,
}

impl FileHashes {
    pub(crate) fn validate(&self) -> AppResult<()> {
        for (hash, length) in [
            (&self.sha256, 64),
            (&self.sha512, 128),
            (&self.sha1, 40),
            (&self.md5, 32),
        ] {
            if hash
                .as_ref()
                .is_some_and(|s| s.len() != length || !s.bytes().all(|c| c.is_ascii_hexdigit()))
            {
                return Err(AppError::Invalid("来源 hash 格式无效".into()));
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct ResolvedMod {
    pub source_id: String,
    pub project_id: String,
    pub version_id: String,
    pub name: String,
    pub version: String,
    /// May contain a short-lived CDN signature. Never log this value.
    pub download_url: String,
    pub filename: String,
    pub size: Option<u64>,
    pub hashes: FileHashes,
    pub homepage_url: Option<String>,
    pub warnings: Vec<String>,
}

impl std::fmt::Debug for ResolvedMod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedMod")
            .field("source_id", &self.source_id)
            .field("project_id", &self.project_id)
            .field("version_id", &self.version_id)
            .field("download_url", &"[redacted]")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyType {
    Required,
    Optional,
    Incompatible,
    Embedded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModDependency {
    pub dependency_type: DependencyType,
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    /// A display-only reference; never use it as a path or download URL.
    pub file_name: Option<String>,
}

pub(crate) const MAX_MODRINTH_DEPENDENCIES: usize = 128;

pub(crate) fn modrinth_id(value: &str) -> AppResult<&str> {
    if !value.is_empty() && value.len() <= 32 && value.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Ok(value)
    } else {
        Err(AppError::Invalid("Modrinth 项目和版本须使用来源 ID".into()))
    }
}

pub(crate) fn valid_modrinth_dependencies(dependencies: &[ModDependency]) -> bool {
    dependencies.len() <= MAX_MODRINTH_DEPENDENCIES
        && dependencies.iter().all(|d| {
            (d.project_id.is_some() || d.version_id.is_some() || d.file_name.is_some())
                && [d.project_id.as_deref(), d.version_id.as_deref()]
                    .into_iter()
                    .flatten()
                    .all(|id| modrinth_id(id).is_ok())
                && d.file_name.as_ref().is_none_or(|name| {
                    !name.trim().is_empty()
                        && name.len() <= 255
                        && !matches!(name.as_str(), "." | "..")
                        && !name.contains(['/', '\\', ':'])
                        && !name.chars().any(char::is_control)
                })
        })
}

#[derive(Debug, Clone)]
pub struct ResolveResult {
    pub status: SourceStatus,
    pub download: Option<ResolvedMod>,
    pub homepage_url: Option<String>,
    /// Direct declarations for the selected version, not a solved graph.
    /// None means unavailable/unverified; Some(empty) means explicitly empty.
    pub dependencies: Option<Vec<ModDependency>>,
}

#[async_trait]
pub trait ModSource: Send + Sync {
    fn id(&self) -> &str;
    fn status(&self) -> SourceStatus;
    async fn search(&self, query: &SearchQuery) -> AppResult<SearchResult>;
    async fn resolve(&self, request: &ResolveRequest) -> AppResult<ResolveResult>;
    async fn list_versions(&self, request: &VersionListRequest) -> AppResult<VersionListResult>;
    async fn dependency_graph(
        &self,
        _request: &DependencyGraphRequest,
        _cancel: &CancellationToken,
    ) -> AppResult<DependencyGraph> {
        Err(AppError::Unsupported("此来源不支持依赖图".into()))
    }
}

#[derive(Clone)]
pub struct Sources {
    providers: Vec<Arc<dyn ModSource>>,
}

impl Sources {
    pub fn new(keys: SourceKeys) -> AppResult<Self> {
        Ok(Self {
            providers: providers::builtins(keys)?,
        })
    }

    pub fn statuses(&self) -> Vec<SourceStatus> {
        self.providers.iter().map(|p| p.status()).collect()
    }

    /// Failures are isolated: a missing key or outage never hides other results.
    pub async fn search(&self, query: &SearchQuery) -> AppResult<SearchResult> {
        if query.limit == 0 || query.limit > 100 {
            return Err(AppError::Invalid("搜索分页大小必须为 1..100".into()));
        }
        let selected: Vec<_> = self
            .providers
            .iter()
            .filter(|p| {
                query.source_ids.is_empty() || query.source_ids.iter().any(|id| id == p.id())
            })
            .collect();
        let responses = join_all(selected.iter().map(|p| p.search(query))).await;
        let mut result = SearchResult {
            mods: Vec::new(),
            sources: Vec::new(),
        };
        for (provider, response) in selected.into_iter().zip(responses) {
            match response {
                Ok(mut page) => {
                    result.mods.append(&mut page.mods);
                    result.sources.append(&mut page.sources);
                }
                // Never forward arbitrary remote bodies/URLs into an agent/log.
                Err(_) => result.sources.push(SourceStatus::new(
                    provider.id(),
                    SourceState::Unavailable,
                    "源请求失败；请检查网络、密钥权限或稍后重试",
                )),
            }
        }
        for id in &query.source_ids {
            if !self.providers.iter().any(|p| p.id() == id) {
                result
                    .sources
                    .push(SourceStatus::new(id, SourceState::Unsupported, "未知来源"));
            }
        }
        Ok(result)
    }

    fn provider(&self, id: &str) -> AppResult<&dyn ModSource> {
        self.providers
            .iter()
            .find(|p| p.id() == id)
            .map(Arc::as_ref)
            .ok_or_else(|| AppError::Unsupported("未知 Mod 来源".into()))
    }

    pub async fn resolve(&self, request: &ResolveRequest) -> AppResult<ResolveResult> {
        self.provider(&request.source_id)?.resolve(request).await
    }

    pub async fn list_versions(
        &self,
        request: &VersionListRequest,
    ) -> AppResult<VersionListResult> {
        if request.limit == 0 || request.limit > 50 {
            return Err(AppError::Invalid("版本列表分页大小必须为 1..50".into()));
        }
        // Only bounded provider filter tokens ever leave the machine.
        if [&request.game_version, &request.loader]
            .into_iter()
            .flatten()
            .any(|v| !valid_version_filter(v))
        {
            return Err(AppError::Invalid("游戏版本或加载器筛选格式无效".into()));
        }
        self.provider(&request.source_id)?
            .list_versions(request)
            .await
    }

    pub async fn modrinth_dependency_graph(
        &self,
        request: &DependencyGraphRequest,
        cancel: &CancellationToken,
    ) -> AppResult<DependencyGraph> {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        modrinth_id(&request.version_id)?;
        if let Some(project) = &request.project_id {
            modrinth_id(project)?;
        }
        if !valid_version_filter(&request.game_version) || !valid_version_filter(&request.loader) {
            return Err(AppError::Invalid(
                "依赖图需要明确的 Minecraft 版本和加载器".into(),
            ));
        }
        request.validate_pins()?;
        self.provider("modrinth")?
            .dependency_graph(request, cancel)
            .await
    }
}
