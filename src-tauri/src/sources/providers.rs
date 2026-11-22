use super::{http::*, *};
use reqwest::{header::HeaderValue, Client};
use serde_json::{json, Value};
use std::collections::HashSet;

#[path = "dependency_graph.rs"]
pub(super) mod dependency_graph;

#[path = "nexus_search.rs"]
mod nexus_search;
#[path = "three_dm.rs"]
mod three_dm;

// Primary references (read-only operations only):
// https://docs.modrinth.com/api/operations/searchprojects/
// https://docs.modrinth.com/api/operations/getprojectversions/
// https://docs.curseforge.com/rest-api/#search-mods
// https://github.com/Nexus-Mods/node-nexus-api/blob/master/src/Nexus.ts
// https://github.com/thunderstore-io/Thunderstore/blob/master/django/thunderstore/api/urls.py
// https://github.com/thunderstore-io/Thunderstore/blob/master/django/thunderstore/api/cyberstorm/views/package_listing_list.py

struct Provider {
    id: &'static str,
    client: Client,
    key: Result<Option<HeaderValue>, ()>,
    #[cfg(test)]
    mock: Option<contract_tests::MockApi>,
}

pub(super) fn builtins(keys: SourceKeys) -> AppResult<Vec<Arc<dyn ModSource>>> {
    let client = api_client()?;
    Ok(vec![
        Arc::new(Provider {
            id: "modrinth",
            client: client.clone(),
            key: Ok(None),
            #[cfg(test)]
            mock: None,
        }),
        Arc::new(Provider {
            id: "thunderstore",
            client: client.clone(),
            key: Ok(None),
            #[cfg(test)]
            mock: None,
        }),
        Arc::new(Provider {
            id: "curseforge",
            client: client.clone(),
            key: secret_header(keys.curseforge).map_err(|_| ()),
            #[cfg(test)]
            mock: None,
        }),
        Arc::new(Provider {
            id: "nexus",
            client: client.clone(),
            key: secret_header(keys.nexus).map_err(|_| ()),
            #[cfg(test)]
            mock: None,
        }),
        Arc::new(Provider {
            id: "3dm",
            client,
            key: secret_header(keys.three_dm).map_err(|_| ()),
            #[cfg(test)]
            mock: None,
        }),
    ])
}

fn s(v: &Value, key: &str) -> AppResult<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| AppError::Other(format!("来源缺少字段 {key}")))
}
fn opt(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
fn optional_hash(v: &Value, key: &str) -> AppResult<Option<String>> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(hash)) => Ok(Some(hash.clone())),
        _ => Err(AppError::Invalid("来源 hash 必须是十六进制字符串".into())),
    }
}

fn modrinth_hashes(file: &Value) -> AppResult<FileHashes> {
    match file.get("hashes") {
        None | Some(Value::Null) => Ok(FileHashes::default()),
        Some(hashes) if hashes.is_object() => Ok(FileHashes {
            sha512: optional_hash(hashes, "sha512")?,
            sha1: optional_hash(hashes, "sha1")?,
            ..Default::default()
        }),
        _ => Err(AppError::Invalid("来源 hashes 格式无效".into())),
    }
}

fn modrinth_dependencies(version: &Value) -> Option<Vec<ModDependency>> {
    let value = version.get("dependencies")?;
    if value.as_array()?.len() > MAX_MODRINTH_DEPENDENCIES {
        return None;
    }
    let dependencies = Vec::<ModDependency>::deserialize(value).ok()?;
    // Do not silently drop malformed entries: a dropped required dependency
    // must not turn into a verified empty list or a seemingly complete one.
    valid_modrinth_dependencies(&dependencies).then_some(dependencies)
}

fn thunderstore_dependencies(version: &Value) -> Option<Vec<ModDependency>> {
    let values = version.get("dependencies")?.as_array()?;
    if values.len() > MAX_MODRINTH_DEPENDENCIES {
        return None;
    }
    values
        .iter()
        .map(|dependency| {
            // An unavailable declaration is not safe to present as a solved
            // dependency, even when the package coordinate is well formed.
            if dependency["is_removed"] == true
                || dependency["is_unavailable"] == true
                || dependency["is_active"] == false
            {
                return None;
            }
            let namespace = segment(dependency["namespace"].as_str()?).ok()?;
            let name = segment(dependency["name"].as_str()?).ok()?;
            let version = segment(dependency["version_number"].as_str()?).ok()?;
            Some(ModDependency {
                dependency_type: DependencyType::Required,
                project_id: Some(format!("{namespace}/{name}")),
                version_id: Some(version.to_owned()),
                file_name: None,
            })
        })
        .collect()
}

fn modrinth_matches(version: &Value, game_version: Option<&str>, loader: Option<&str>) -> bool {
    [("game_versions", game_version), ("loaders", loader)]
        .into_iter()
        .all(|(key, filter)| {
            filter.is_none_or(|filter| {
                version[key]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(filter)))
            })
        })
}

fn modrinth_version_summary(version: &Value, id: &str) -> AppResult<ModVersionSummary> {
    fn text(value: &Value, max: usize) -> AppResult<&str> {
        value
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control))
            .ok_or_else(|| AppError::Other("来源版本元数据格式无效".into()))
    }
    fn tokens(value: &Value, max: usize) -> AppResult<Vec<String>> {
        let items = array(value)?;
        if items.len() > max {
            return Err(AppError::Other("来源版本兼容列表过长".into()));
        }
        items
            .iter()
            .map(|v| text(v, 64).map(str::to_owned))
            .collect()
    }
    let release_channel = match version["version_type"].as_str() {
        Some("release") => ReleaseChannel::Release,
        Some("beta") => ReleaseChannel::Beta,
        Some("alpha") => ReleaseChannel::Alpha,
        _ => return Err(AppError::Other("来源版本发布频道未知".into())),
    };
    let published_at = chrono::DateTime::parse_from_rfc3339(text(&version["date_published"], 64)?)
        .map_err(|_| AppError::Other("来源版本发布日期无效".into()))?;
    Ok(ModVersionSummary {
        version_id: id.into(),
        version: text(&version["version_number"], 128)?.into(),
        name: text(&version["name"], 256)?.into(),
        release_channel,
        published_at: published_at.to_rfc3339(),
        game_versions: tokens(&version["game_versions"], 512)?,
        loaders: tokens(&version["loaders"], 32)?,
    })
}

fn thunderstore_version_summary(version: &Value) -> AppResult<ModVersionSummary> {
    fn text(value: &Value, max: usize) -> AppResult<&str> {
        value
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control))
            .ok_or_else(|| AppError::Other("Thunderstore 版本元数据格式无效".into()))
    }

    let version_id = segment(text(&version["version_number"], 128)?)?.to_owned();
    let published_at =
        chrono::DateTime::parse_from_rfc3339(text(&version["datetime_created"], 64)?)
            .map_err(|_| AppError::Other("Thunderstore 版本发布日期无效".into()))?;
    Ok(ModVersionSummary {
        version_id: version_id.clone(),
        version: version_id.clone(),
        name: version_id,
        release_channel: ReleaseChannel::Release,
        published_at: published_at.to_rfc3339(),
        // Thunderstore exposes package versions, not game-version or loader
        // compatibility declarations, in this endpoint. Do not infer either.
        game_versions: Vec::new(),
        loaders: Vec::new(),
    })
}

fn curseforge_hashes(file: &Value) -> AppResult<FileHashes> {
    let mut hashes = FileHashes::default();
    let Some(values) = file.get("hashes").filter(|v| !v.is_null()) else {
        return Ok(hashes);
    };
    for hash in array(values)? {
        // Official HashAlgo: 1 = SHA-1, 2 = MD5. Do not guess other algorithms.
        let target = match hash["algo"].as_u64() {
            Some(1) => &mut hashes.sha1,
            Some(2) => &mut hashes.md5,
            _ => return Err(AppError::Invalid("来源 hash 算法无效或不支持".into())),
        };
        if target.is_some() {
            return Err(AppError::Invalid("来源提供重复的 hash 算法".into()));
        }
        *target = Some(s(hash, "value")?);
    }
    Ok(hashes)
}

fn opt_url(v: &Value, key: &str) -> Option<String> {
    opt(v, key).filter(|url| validate_download_url(url).is_ok())
}
fn array(v: &Value) -> AppResult<&Vec<Value>> {
    v.as_array()
        .ok_or_else(|| AppError::Other("来源列表格式无效".into()))
}
fn num(v: &Value, key: &str) -> AppResult<String> {
    v.get(key)
        .and_then(Value::as_u64)
        .map(|n| n.to_string())
        .ok_or_else(|| AppError::Other(format!("来源缺少数字字段 {key}")))
}

impl Provider {
    async fn fetch_json(&self, request: reqwest::RequestBuilder) -> AppResult<Value> {
        // The test double is absent from production builds and cannot change
        // endpoint selection, forward credentials or fall through to the network.
        #[cfg(test)]
        if let Some(mock) = &self.mock {
            return mock.respond(request);
        }
        json(request).await
    }

    fn get(&self, url: String) -> reqwest::RequestBuilder {
        self.authenticate(self.client.get(url))
    }

    fn authenticate(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.key {
            Ok(Some(key)) if self.id == "curseforge" => request.header("x-api-key", key.clone()),
            Ok(Some(key)) if self.id == "nexus" => request
                .header("apikey", key.clone())
                .header("Application-Name", "Agent Mod Manager")
                .header("Application-Version", env!("CARGO_PKG_VERSION")),
            Ok(Some(key)) if self.id == "3dm" => request.header("Authorization", key.clone()),
            _ => request,
        }
    }
    fn state(&self, state: SourceState, message: &str) -> SourceStatus {
        SourceStatus::new(self.id, state, message)
    }
    fn empty(&self, state: SourceState, message: &str) -> SearchResult {
        SearchResult {
            mods: Vec::new(),
            sources: vec![self.state(state, message)],
        }
    }
    fn unresolved(
        &self,
        state: SourceState,
        message: &str,
        homepage_url: Option<String>,
    ) -> ResolveResult {
        ResolveResult {
            status: self.state(state, message),
            download: None,
            homepage_url,
            dependencies: None,
        }
    }
    fn resolved(&self, mut download: ResolvedMod) -> AppResult<ResolveResult> {
        download.hashes.validate()?;
        validate_download_url(&download.download_url)?;
        download.homepage_url = download
            .homepage_url
            .filter(|url| validate_download_url(url).is_ok());
        Ok(ResolveResult {
            status: self.status(),
            homepage_url: download.homepage_url.clone(),
            download: Some(download),
            dependencies: None,
        })
    }
    fn missing(&self) -> bool {
        matches!(self.id, "curseforge" | "nexus" | "3dm") && matches!(self.key, Ok(None))
    }

    async fn modrinth_search(&self, q: &SearchQuery) -> AppResult<SearchResult> {
        if q.game_ids.get(self.id).is_some_and(|g| g != "minecraft") {
            return Ok(self.empty(SourceState::Unsupported, "Modrinth 仅支持 Minecraft 项目"));
        }
        let mut facets = vec![vec!["project_type:mod".to_string()]];
        if let Some(loader) = &q.loader {
            facets.push(vec![format!("categories:{}", segment(loader)?)]);
        }
        if let Some(version) = &q.game_version {
            facets.push(vec![format!("versions:{}", segment(version)?)]);
        }
        let v: Value = self
            .fetch_json(
                self.get("https://api.modrinth.com/v2/search".into())
                    .query(&[
                        ("query", q.query.clone()),
                        ("offset", q.offset.to_string()),
                        ("limit", q.limit.to_string()),
                        ("facets", serde_json::to_string(&facets)?),
                    ]),
            )
            .await?;
        let mods = array(&v["hits"])?
            .iter()
            .map(|v| {
                let id = s(v, "project_id")?;
                Ok(ModSummary {
                    source_id: self.id.into(),
                    project_id: id.clone(),
                    game_id: Some("minecraft".into()),
                    name: s(v, "title")?,
                    summary: opt(v, "description").unwrap_or_default(),
                    author: opt(v, "author"),
                    homepage_url: Some(format!("https://modrinth.com/mod/{}", segment(&id)?)),
                    icon_url: opt_url(v, "icon_url"),
                    downloads: v["downloads"].as_u64(),
                })
            })
            .collect::<AppResult<_>>()?;
        Ok(SearchResult {
            mods,
            sources: vec![self.status()],
        })
    }

    async fn modrinth_resolve(&self, r: &ResolveRequest) -> AppResult<ResolveResult> {
        if r.game_id.as_deref().is_some_and(|game| game != "minecraft") {
            return Ok(self.unresolved(
                SourceState::Unsupported,
                "Modrinth 仅支持 Minecraft 项目",
                None,
            ));
        }
        let project = (!r.project_id.is_empty())
            .then(|| modrinth_id(&r.project_id))
            .transpose()?;
        let version = r.version_id.as_deref().map(modrinth_id).transpose()?;
        let versions: Value = if let Some(version) = version {
            json!([self.modrinth_exact_version(version).await?])
        } else {
            let project = project
                .ok_or_else(|| AppError::Invalid("需要 Modrinth 项目 ID 或精确版本 ID".into()))?;
            self.modrinth_project_versions(project, r.game_version.as_deref(), r.loader.as_deref())
                .await?
        };
        let v = array(&versions)?
            .iter()
            .find(|v| {
                project.is_none_or(|id| v["project_id"].as_str() == Some(id))
                    && version.is_none_or(|id| v["id"].as_str() == Some(id))
                    && modrinth_matches(v, r.game_version.as_deref(), r.loader.as_deref())
            })
            .ok_or_else(|| {
                AppError::NotFound(
                    "没有符合所请求的项目或版本 ID，以及游戏版本和加载器筛选条件的 Modrinth 文件"
                        .into(),
                )
            })?;
        let project = modrinth_id(v["project_id"].as_str().unwrap_or_default())?;
        let version = modrinth_id(v["id"].as_str().unwrap_or_default())?;
        let files = array(&v["files"])?;
        let f = files
            .iter()
            .find(|f| f["primary"] == true)
            .or_else(|| files.first())
            .ok_or_else(|| AppError::NotFound("版本没有文件".into()))?;
        let mut result = self.resolved(ResolvedMod {
            source_id: self.id.into(),
            project_id: project.into(),
            version_id: version.into(),
            name: s(v, "name")?,
            version: s(v, "version_number")?,
            filename: s(f, "filename")?,
            download_url: s(f, "url")?,
            size: f["size"].as_u64(),
            hashes: modrinth_hashes(f)?,
            homepage_url: Some(format!("https://modrinth.com/mod/{project}")),
            warnings: Vec::new(),
        })?;
        result.dependencies = modrinth_dependencies(v);
        Ok(result)
    }

    async fn modrinth_exact_version(&self, version: &str) -> AppResult<Value> {
        let version = modrinth_id(version)?;
        self.fetch_json(self.get(format!("https://api.modrinth.com/v2/version/{version}")))
            .await
    }

    async fn modrinth_project_versions(
        &self,
        project: &str,
        game_version: Option<&str>,
        loader: Option<&str>,
    ) -> AppResult<Value> {
        let mut request = self.get(format!(
            "https://api.modrinth.com/v2/project/{project}/version"
        ));
        for (key, filter) in [("game_versions", game_version), ("loaders", loader)] {
            if let Some(filter) = filter {
                request = request.query(&[(key, serde_json::to_string(&[filter])?)]);
            }
        }
        self.fetch_json(request).await
    }

    async fn modrinth_list_versions(&self, r: &VersionListRequest) -> AppResult<VersionListResult> {
        let project = modrinth_id(&r.project_id)?;
        let response = self
            .modrinth_project_versions(project, r.game_version.as_deref(), r.loader.as_deref())
            .await?;
        // Filter and deduplicate before the local page boundary. Do not return
        // files, signed URLs, changelogs or other raw provider response fields.
        let mut seen = HashSet::new();
        let mut versions = Vec::new();
        let mut more = false;
        for version in array(&response)? {
            if version["project_id"].as_str() != Some(project)
                || !modrinth_matches(version, r.game_version.as_deref(), r.loader.as_deref())
                || array(&version["files"])?.is_empty()
            {
                continue;
            }
            let id = modrinth_id(version["id"].as_str().unwrap_or_default())?;
            if !seen.insert(id) || seen.len() <= r.offset {
                continue;
            }
            if versions.len() == r.limit {
                more = true;
                break;
            }
            versions.push(modrinth_version_summary(version, id)?);
        }
        Ok(VersionListResult {
            status: self.status(),
            versions,
            has_more: more,
        })
    }

    async fn thunderstore_search(&self, q: &SearchQuery) -> AppResult<SearchResult> {
        let Some(game) = q.game_ids.get(self.id) else {
            return Ok(self.empty(
                SourceState::NeedsConfiguration,
                "需要 Thunderstore community 标识符，如 valheim",
            ));
        };
        let game = segment(game)?;
        let mut mods = Vec::new();
        // Official API uses fixed pages of 20; map our offset/limit exactly.
        let offset = u64::from(q.offset);
        let end = offset + u64::from(q.limit);
        let first = offset / 20;
        let last = (end - 1) / 20;
        for page in first..=last {
            let v: Value = self
                .fetch_json(
                    self.get(format!(
                        "https://thunderstore.io/api/cyberstorm/listing/{game}/"
                    ))
                    .query(&[("q", q.query.clone()), ("page", (page + 1).to_string())]),
                )
                .await?;
            for (index, v) in array(&v["results"])?.iter().enumerate() {
                let absolute = page * 20 + index as u64;
                if absolute < offset || absolute >= end {
                    continue;
                }
                let namespace = s(v, "namespace")?;
                let name = s(v, "name")?;
                mods.push(ModSummary {
                    source_id: self.id.into(),
                    project_id: format!("{namespace}/{name}"),
                    game_id: Some(game.into()),
                    name: name.clone(),
                    summary: opt(v, "description").unwrap_or_default(),
                    author: Some(namespace.clone()),
                    icon_url: opt_url(v, "icon_url"),
                    downloads: v["download_count"].as_u64(),
                    homepage_url: Some(format!(
                        "https://thunderstore.io/c/{game}/p/{}/{}/",
                        segment(&namespace)?,
                        segment(&name)?
                    )),
                });
            }
            if v["next"].is_null() {
                break;
            }
        }
        Ok(SearchResult {
            mods,
            sources: vec![self.state(
                SourceState::Available,
                "公共 API；游戏版本/loader 不受此 API 筛选，需审查依赖兼容性",
            )],
        })
    }

    async fn thunderstore_resolve(&self, r: &ResolveRequest) -> AppResult<ResolveResult> {
        let Some(game) = &r.game_id else {
            return Ok(self.unresolved(
                SourceState::NeedsConfiguration,
                "需要 Thunderstore community 标识符",
                None,
            ));
        };
        let game = segment(game)?;
        let (namespace, name) = r.project_id.split_once('/').ok_or_else(|| {
            AppError::Invalid("Thunderstore project_id 应为 namespace/name".into())
        })?;
        let (namespace, name) = (segment(namespace)?, segment(name)?);
        let mut url =
            format!("https://thunderstore.io/api/cyberstorm/listing/{game}/{namespace}/{name}/");
        if let Some(version) = &r.version_id {
            url.push_str(&format!("v/{}/", segment(version)?));
        }
        let v: Value = self.fetch_json(self.get(url)).await?;
        let mismatch =
            || AppError::Conflict("Thunderstore 返回的社区、项目或版本与请求不符".into());
        if s(&v, "community_identifier")? != game
            || s(&v, "namespace")? != namespace
            || s(&v, "name")? != name
        {
            return Err(mismatch());
        }
        // Cyberstorm looks up a namespace, but the package's full name and
        // legacy download route use the owning team. The latest field may
        // refer to a different release on a historical version response.
        let owner = s(&v["team"], "name")?;
        let owner = segment(&owner)?;
        let prefix = format!("{owner}-{name}-");
        let full_version = s(&v, "full_version_name")?;
        let version = segment(full_version.strip_prefix(&prefix).ok_or_else(mismatch)?)?;
        let selected = match r.version_id.as_ref() {
            Some(pinned) => pinned.clone(),
            None => s(&v, "latest_version_number")?,
        };
        if version != selected {
            return Err(mismatch());
        }
        let download_url = s(&v, "download_url")?;
        validate_download_url(&download_url)?;
        if download_url
            != format!("https://thunderstore.io/package/download/{owner}/{name}/{version}/")
        {
            return Err(AppError::Conflict(
                "Thunderstore 下载路径与选中的项目或版本不符".into(),
            ));
        }
        let mut result = self.resolved(ResolvedMod {
            source_id: self.id.into(),
            project_id: r.project_id.clone(),
            version_id: version.into(),
            name: name.into(),
            version: version.into(),
            filename: format!("{owner}-{name}-{version}.zip"),
            download_url,
            size: v["size"].as_u64(),
            hashes: FileHashes::default(),
            homepage_url: Some(format!(
                "https://thunderstore.io/c/{game}/p/{namespace}/{name}/"
            )),
            warnings: vec!["来源未提供 SHA-256/SHA-512；请审查归档和依赖兼容性".into()],
        })?;
        result.dependencies = thunderstore_dependencies(&v);
        Ok(result)
    }

    async fn thunderstore_list_versions(
        &self,
        r: &VersionListRequest,
    ) -> AppResult<VersionListResult> {
        let Some(game) = &r.game_id else {
            return Ok(VersionListResult {
                status: self.state(
                    SourceState::NeedsConfiguration,
                    "需要 Thunderstore community 标识符",
                ),
                versions: Vec::new(),
                has_more: false,
            });
        };
        let game = segment(game)?;
        let (namespace, name) = r.project_id.split_once('/').ok_or_else(|| {
            AppError::Invalid("Thunderstore project_id 应为 namespace/name".into())
        })?;
        let (namespace, name) = (segment(namespace)?, segment(name)?);
        let listing_path =
            format!("https://thunderstore.io/api/cyberstorm/listing/{game}/{namespace}/{name}/");
        let listing: Value = self.fetch_json(self.get(listing_path)).await?;
        if s(&listing, "community_identifier")? != game
            || s(&listing, "namespace")? != namespace
            || s(&listing, "name")? != name
        {
            return Err(AppError::Conflict(
                "Thunderstore 返回的社区或项目与请求不符".into(),
            ));
        }

        // Thunderstore's public endpoint uses fixed 20-item pages. Derive
        // page numbers locally; never follow provider-supplied URLs.
        const PAGE_SIZE: usize = 20;
        let mut page = r.offset / PAGE_SIZE + 1;
        let mut absolute = page.saturating_sub(1) * PAGE_SIZE;
        let mut versions = Vec::with_capacity(r.limit);
        let mut seen = HashSet::new();
        let mut has_more = false;

        loop {
            let path = format!(
                "https://thunderstore.io/api/cyberstorm/package/{namespace}/{name}/versions/"
            );
            let response: Value = self
                .fetch_json(self.get(path).query(&[("page", page.to_string())]))
                .await?;
            let results = response
                .get("results")
                .map(array)
                .transpose()?
                .or_else(|| response.as_array())
                .ok_or_else(|| AppError::Other("Thunderstore 版本列表格式无效".into()))?;
            let next = response
                .get("next")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .is_some();

            let previous_count = seen.len();
            for version in results {
                let summary = thunderstore_version_summary(version)?;
                if !seen.insert(summary.version_id.clone()) {
                    continue;
                }
                let entry_index = absolute;
                absolute = absolute.saturating_add(1);
                if entry_index < r.offset {
                    continue;
                }
                if versions.len() == r.limit {
                    has_more = true;
                    break;
                }
                versions.push(summary);
            }
            if has_more || versions.len() == r.limit {
                has_more = has_more || next;
                break;
            }
            if !next {
                break;
            }
            if previous_count == seen.len() {
                return Err(AppError::Other(
                    "Thunderstore 返回了重复或空的版本分页".into(),
                ));
            }
            page += 1;
        }

        let status = if r.game_version.is_some() || r.loader.is_some() {
            self.state(
                SourceState::Limited,
                "Thunderstore 版本接口不提供游戏版本或加载器筛选；请人工核对兼容性",
            )
        } else {
            self.status()
        };
        Ok(VersionListResult {
            status,
            versions,
            has_more,
        })
    }

    async fn curseforge_search(&self, q: &SearchQuery) -> AppResult<SearchResult> {
        let Some(game) = q.game_ids.get(self.id) else {
            return Ok(self.empty(
                SourceState::NeedsConfiguration,
                "需要 CurseForge 数字 gameId",
            ));
        };
        positive_id(game)?;
        let mut request = self
            .get("https://api.curseforge.com/v1/mods/search".into())
            .query(&[
                ("gameId", game.clone()),
                ("searchFilter", q.query.clone()),
                ("index", q.offset.to_string()),
                ("pageSize", q.limit.to_string()),
            ]);
        if let Some(v) = &q.game_version {
            request = request.query(&[("gameVersion", v)]);
        }
        if q.loader.is_some() {
            return Ok(self.empty(
                SourceState::Unsupported,
                "CurseForge loader 使用专有枚举；请省略 loader 后审查文件兼容性",
            ));
        }
        let v: Value = self.fetch_json(request).await?;
        let mods = array(&v["data"])?
            .iter()
            .map(|v| {
                Ok(ModSummary {
                    source_id: self.id.into(),
                    project_id: num(v, "id")?,
                    game_id: Some(game.clone()),
                    name: s(v, "name")?,
                    summary: opt(v, "summary").unwrap_or_default(),
                    author: v["authors"]
                        .as_array()
                        .and_then(|a| a.first())
                        .and_then(|a| opt(a, "name")),
                    homepage_url: opt_url(&v["links"], "websiteUrl"),
                    icon_url: opt_url(&v["logo"], "thumbnailUrl"),
                    downloads: v["downloadCount"].as_u64(),
                })
            })
            .collect::<AppResult<_>>()?;
        Ok(SearchResult {
            mods,
            sources: vec![self.status()],
        })
    }

    async fn curseforge_resolve(&self, r: &ResolveRequest) -> AppResult<ResolveResult> {
        let id = positive_id(&r.project_id)?.trim_start_matches('0');
        let pinned = r
            .version_id
            .as_deref()
            .map(positive_id)
            .transpose()?
            .map(|id| id.trim_start_matches('0'));
        let project: Value = self
            .fetch_json(self.get(format!("https://api.curseforge.com/v1/mods/{id}")))
            .await?;
        if num(&project["data"], "id")? != id {
            return Err(AppError::Conflict(
                "CurseForge 返回的项目 ID 与请求不符".into(),
            ));
        }
        let home = opt_url(&project["data"]["links"], "websiteUrl");
        if project["data"]["allowModDistribution"] == false {
            return Ok(self.unresolved(
                SourceState::ManualDownload,
                "作者禁用第三方分发，请在来源网站下载",
                home,
            ));
        }
        if r.loader.is_some() {
            return Ok(self.unresolved(
                SourceState::Unsupported,
                "暂不支持 CurseForge loader 枚举筛选",
                home,
            ));
        }
        let not_found = || AppError::NotFound("没有匹配的可下载 CurseForge 文件".into());
        let matches = |file: &Value| {
            file["isAvailable"] == true
                && file["modId"]
                    .as_u64()
                    .is_some_and(|project| project.to_string() == id)
                && r.game_version.as_ref().is_none_or(|v| {
                    file["gameVersions"].as_array().is_some_and(|versions| {
                        versions.iter().any(|version| version.as_str() == Some(v))
                    })
                })
        };
        let selected = if let Some(file_id) = pinned {
            let mut v: Value = self
                .fetch_json(self.get(format!(
                    "https://api.curseforge.com/v1/mods/{id}/files/{file_id}"
                )))
                .await?;
            if num(&v["data"], "id")? != file_id {
                return Err(AppError::Conflict(
                    "CurseForge 返回的文件 ID 与请求不符".into(),
                ));
            }
            if !matches(&v["data"]) {
                return Err(not_found());
            }
            v["data"].take()
        } else {
            let mut index = 0u64;
            loop {
                let mut req = self
                    .get(format!("https://api.curseforge.com/v1/mods/{id}/files"))
                    .query(&[("pageSize", 50u64), ("index", index)]);
                if let Some(version) = &r.game_version {
                    req = req.query(&[("gameVersion", version)]);
                }
                let response: Value = self.fetch_json(req).await?;
                let files = array(&response["data"])?;
                if let Some(file) = files
                    .iter()
                    .filter(|file| matches(file))
                    .max_by_key(|file| file["fileDate"].as_str().unwrap_or_default())
                {
                    break file.clone();
                }
                let Some(paging) = response.get("pagination") else {
                    return Err(not_found());
                };
                let count = files.len() as u64;
                let invalid = || AppError::Other("CurseForge 返回的文件分页信息无效".into());
                let total = paging["totalCount"].as_u64().ok_or_else(invalid)?;
                if paging["index"].as_u64() != Some(index)
                    || paging["resultCount"].as_u64() != Some(count)
                    || count > 50
                {
                    return Err(invalid());
                }
                index = index.checked_add(count).ok_or_else(invalid)?;
                if index >= total {
                    return Err(not_found());
                }
                if count == 0 {
                    return Err(invalid());
                }
            }
        };
        let file = &selected;
        let fid = num(file, "id")?;
        let url = match opt(file, "downloadUrl") {
            Some(url) => url,
            None => {
                return Ok(self.unresolved(
                    SourceState::ManualDownload,
                    "来源没有提供第三方下载链接，请在网站下载",
                    home,
                ))
            }
        };
        self.resolved(ResolvedMod {
            source_id: self.id.into(),
            project_id: id.into(),
            version_id: fid,
            name: s(&project["data"], "name")?,
            version: s(file, "displayName")?,
            filename: s(file, "fileName")?,
            download_url: url,
            size: file["fileLength"].as_u64(),
            hashes: curseforge_hashes(file)?,
            homepage_url: home,
            warnings: vec![
                "CurseForge 仅提供弱 SHA-1/MD5 完整性校验，不证明来源真实性或文件安全；请人工审查归档".into(),
            ],
        })
    }

    async fn nexus_search(&self, q: &SearchQuery) -> AppResult<SearchResult> {
        let Some(game) = q.game_ids.get(self.id) else {
            return Ok(self.empty(
                SourceState::NeedsConfiguration,
                "需要 Nexus 游戏 domain_name",
            ));
        };
        let game = segment(game)?.to_ascii_lowercase();
        let term = q.query.trim();
        if term.is_empty() || term.chars().any(char::is_control) {
            return Ok(self.empty(SourceState::Limited, "Nexus 搜索词为空或包含不可用字符"));
        }
        if !term.bytes().all(|byte| byte.is_ascii_digit()) {
            return self.nexus_text_search(&game, term, q).await;
        }
        if positive_id(term).is_err() {
            return Ok(self.empty(SourceState::Limited, "Nexus 数字 mod ID 无效"));
        }
        let id = positive_id(term)?.trim_start_matches('0');
        if q.offset > 0 {
            return Ok(self.empty(SourceState::Limited, "数字 mod ID 查询只有一项"));
        }
        let v: Value = self
            .fetch_json(self.get(format!(
                "https://api.nexusmods.com/v1/games/{game}/mods/{id}.json"
            )))
            .await?;
        Ok(SearchResult {
            mods: vec![ModSummary {
                source_id: self.id.into(),
                project_id: format!("{game}/{id}"),
                game_id: Some(game.clone()),
                name: s(&v, "name")?,
                summary: opt(&v, "summary").unwrap_or_default(),
                author: opt(&v, "author"),
                homepage_url: Some(format!("https://www.nexusmods.com/{game}/mods/{id}")),
                icon_url: opt_url(&v, "picture_url"),
                downloads: v["mod_downloads"].as_u64(),
            }],
            sources: vec![self.state(SourceState::Limited, "按数字 mod ID 精确查询")],
        })
    }

    async fn nexus_resolve(&self, r: &ResolveRequest) -> AppResult<ResolveResult> {
        let (game, id) = if let Some((domain, id)) = r.project_id.split_once('/') {
            if r.game_id
                .as_ref()
                .is_some_and(|game| !game.eq_ignore_ascii_case(domain))
            {
                return Err(AppError::Conflict(
                    "Nexus 项目所属游戏与请求游戏不一致".into(),
                ));
            }
            (domain, id)
        } else if let Some(game) = &r.game_id {
            (game.as_str(), r.project_id.as_str())
        } else {
            return Ok(self.unresolved(
                SourceState::NeedsConfiguration,
                "需要 Nexus 游戏 domain_name",
                None,
            ));
        };
        let game = segment(game)?.to_ascii_lowercase();
        let id = positive_id(id)?.trim_start_matches('0');
        let home = Some(format!("https://www.nexusmods.com/{game}/mods/{id}"));
        let files: Value = self
            .fetch_json(self.get(format!(
                "https://api.nexusmods.com/v1/games/{game}/mods/{id}/files.json"
            )))
            .await?;
        let candidates = array(&files["files"])?;
        let file = if let Some(fid) = &r.version_id {
            let fid = positive_id(fid)?;
            candidates
                .iter()
                .find(|f| f["file_id"].as_u64().map(|n| n.to_string()).as_deref() == Some(fid))
        } else {
            // Never silently choose an optional/old/archived variant.
            let main: Vec<_> = candidates
                .iter()
                .filter(|f| f["category_id"] == 1 && f["is_primary"] == true)
                .collect();
            if main.len() == 1 {
                Some(main[0])
            } else {
                None
            }
        };
        let Some(file) = file else {
            return Ok(self.unresolved(
                SourceState::ManualDownload,
                "请在官网选择文件，或指定 version_id=file_id",
                home,
            ));
        };
        if matches!(file["category_id"].as_u64(), Some(6 | 7)) {
            return Ok(self.unresolved(SourceState::ManualDownload, "文件已归档或移除", home));
        }
        if r.loader.is_some() || r.game_version.is_some() {
            return Ok(self.unresolved(
                SourceState::Unsupported,
                "Nexus 文件 API 不提供统一游戏版本/loader 兼容性筛选",
                home,
            ));
        }
        let fid = num(file, "file_id")?;
        let links: Value = match self
            .fetch_json(self.get(format!(
            "https://api.nexusmods.com/v1/games/{game}/mods/{id}/files/{fid}/download_link.json"
        )))
            .await
        {
            Ok(v) => v,
            Err(AppError::Denied(_)) => {
                return Ok(self.unresolved(
                    SourceState::ManualDownload,
                    "账户不具备直接下载权限；请通过 Nexus 网站发起下载",
                    home,
                ))
            }
            Err(e) => return Err(e),
        };
        let Some(link) = array(&links)?.first() else {
            return Ok(self.unresolved(SourceState::ManualDownload, "来源未提供下载链接", home));
        };
        self.resolved(ResolvedMod {
            source_id: self.id.into(),
            project_id: format!("{game}/{id}"),
            version_id: fid,
            name: s(file, "name")?,
            version: s(file, "version")?,
            filename: s(file, "file_name")?,
            download_url: s(link, "URI")?,
            size: file["size_in_bytes"].as_u64(),
            hashes: FileHashes::default(),
            homepage_url: home,
            warnings: vec!["来源未提供 SHA-256/SHA-512；下载链接可能过期，请审查归档".into()],
        })
    }
}

#[async_trait]
impl ModSource for Provider {
    fn id(&self) -> &str {
        self.id
    }
    fn status(&self) -> SourceStatus {
        if self.key.is_err() {
            self.state(
                SourceState::InvalidKey,
                "来源密钥格式无效；已隔离此来源，不影响其他来源",
            )
        } else if self.missing() {
            self.state(
                SourceState::MissingKey,
                "未配置来源 API Key；已降级，不发送请求",
            )
        } else if self.id == "3dm" {
            self.state(
                SourceState::Limited,
                "3DM API v3 已启用；网盘、多文件及版本兼容性需人工核对",
            )
        } else if self.id == "nexus" {
            self.state(
                SourceState::Limited,
                "支持按游戏搜索 Mod 名称或数字 ID；直接下载受账户权限限制",
            )
        } else {
            self.state(SourceState::Available, "来源已启用")
        }
    }
    async fn search(&self, query: &SearchQuery) -> AppResult<SearchResult> {
        if self.missing() || self.key.is_err() {
            return Ok(SearchResult {
                mods: Vec::new(),
                sources: vec![self.status()],
            });
        }
        match self.id {
            "modrinth" => self.modrinth_search(query).await,
            "thunderstore" => self.thunderstore_search(query).await,
            "curseforge" => self.curseforge_search(query).await,
            "nexus" => self.nexus_search(query).await,
            "3dm" => self.three_dm_search(query).await,
            _ => unreachable!(),
        }
    }
    async fn resolve(&self, request: &ResolveRequest) -> AppResult<ResolveResult> {
        if self.missing() || self.key.is_err() {
            return Ok(ResolveResult {
                status: self.status(),
                download: None,
                homepage_url: None,
                dependencies: None,
            });
        }
        if self.id != "modrinth" && request.project_id.is_empty() {
            return Err(AppError::Invalid("此来源需要项目 ID".into()));
        }
        match self.id {
            "modrinth" => self.modrinth_resolve(request).await,
            "thunderstore" => self.thunderstore_resolve(request).await,
            "curseforge" => self.curseforge_resolve(request).await,
            "nexus" => self.nexus_resolve(request).await,
            "3dm" => self.three_dm_resolve(request).await,
            _ => unreachable!(),
        }
    }
    async fn list_versions(&self, request: &VersionListRequest) -> AppResult<VersionListResult> {
        let unsupported = if self.id != "modrinth" && self.id != "thunderstore" {
            Some("此来源暂不支持列出版本")
        } else if self.id == "modrinth"
            && request
                .game_id
                .as_deref()
                .is_some_and(|game| game != "minecraft")
        {
            Some("Modrinth 仅支持 Minecraft 项目")
        } else {
            None
        };
        if let Some(message) = unsupported {
            return Ok(VersionListResult {
                status: self.state(SourceState::Unsupported, message),
                versions: Vec::new(),
                has_more: false,
            });
        }
        if self.id == "thunderstore" {
            self.thunderstore_list_versions(request).await
        } else {
            self.modrinth_list_versions(request).await
        }
    }

    async fn dependency_graph(
        &self,
        request: &DependencyGraphRequest,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> AppResult<DependencyGraph> {
        if self.id != "modrinth" {
            return Err(AppError::Unsupported("此来源不支持依赖图".into()));
        }
        self.modrinth_graph(request, cancel).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn missing_credentials_degrade_without_network_and_3dm_is_key_gated() {
        let sources = Sources::new(SourceKeys::default()).unwrap();
        let q = SearchQuery {
            source_ids: vec!["curseforge".into(), "nexus".into(), "3dm".into()],
            ..Default::default()
        };
        let page = sources.search(&q).await.unwrap();
        assert!(page.mods.is_empty());
        assert!(page
            .sources
            .iter()
            .all(|s| s.state == SourceState::MissingKey));
        let sources = Sources::new(SourceKeys {
            three_dm: Some("private-test-value".into()),
            ..Default::default()
        })
        .unwrap();
        let statuses = sources.statuses();
        assert_eq!(statuses[4].state, SourceState::Limited);
        assert!(!serde_json::to_string(&statuses)
            .unwrap()
            .contains("private-test-value"));
    }

    #[tokio::test]
    async fn malformed_credentials_disable_only_the_affected_provider() {
        let sources = Sources::new(SourceKeys {
            curseforge: Some("private\nvalue".into()),
            ..Default::default()
        })
        .unwrap();
        let statuses = sources.statuses();
        assert_eq!(statuses[0].state, SourceState::Available);
        assert_eq!(statuses[2].state, SourceState::InvalidKey);
        let page = sources
            .search(&SearchQuery {
                source_ids: vec!["curseforge".into()],
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(page.sources[0].state, SourceState::InvalidKey);
        assert!(!serde_json::to_string(&page).unwrap().contains("private"));
    }
    #[test]
    fn urls_and_ids_fail_closed() {
        for u in [
            "file:///etc/passwd",
            "https://user:pass@cdn.example.org/a",
            "https://127.0.0.2/x",
            "https://10.0.0.1/x",
            "https://[::ffff:10.0.0.1]/x",
            "https://localhost/x",
            "https://example.org/x#secret",
        ] {
            // Loopback is enabled only for HTTP mock servers under cfg(test).
            if !u.contains("127.0.0.2") {
                assert!(validate_download_url(u).is_err(), "{u}");
            }
        }
        assert!(segment("../x").is_err());
        assert!(positive_id("0").is_err());
    }
}

#[cfg(test)]
#[path = "provider_contract_tests.rs"]
mod contract_tests;
