//! 3DM API v3, separate from Mod 客栈 and from user-owned MCP connections.
//! Primary contract: GlossMod/gloss-mod-mcp/glossmod_mcp/server.py;
//! response fields checked with the public https://mod.3dmgame.com/mcp
//! get-mod-list/get-mod-detail read tools on 2026-11-18. No download was made.
use super::*;
use std::collections::BTreeSet;

const API: &str = "https://mod.3dmgame.com/api/v3";
const SITE: &str = "https://mod.3dmgame.com";
const MAX_ID: u64 = 9_007_199_254_740_991;

fn invalid() -> AppError {
    AppError::Invalid("3DM 响应字段或关联关系无效".into())
}

fn id(value: &Value) -> AppResult<u64> {
    value
        .as_u64()
        .filter(|n| *n > 0 && *n <= MAX_ID)
        .ok_or_else(invalid)
}

fn request_id(value: &str) -> AppResult<u64> {
    positive_id(value)?;
    value
        .parse::<u64>()
        .ok()
        .filter(|n| *n <= MAX_ID)
        .ok_or_else(invalid)
}

fn text(value: &Value, field: &str, max: usize) -> AppResult<String> {
    let value = value[field].as_str().ok_or_else(invalid)?;
    if value.trim().is_empty() || value.chars().count() > max || value.contains('\0') {
        return Err(invalid());
    }
    Ok(value.to_owned())
}

fn optional_text(value: &Value, field: &str, max: usize) -> AppResult<Option<String>> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.is_empty() => Ok(None),
        _ => text(value, field, max).map(Some),
    }
}

fn has_credential(value: &Value, secret: &str) -> bool {
    match value {
        Value::String(s) => s.contains(secret),
        Value::Array(items) => items.iter().any(|v| has_credential(v, secret)),
        Value::Object(items) => items
            .iter()
            .any(|(k, v)| k.contains(secret) || has_credential(v, secret)),
        value => value.to_string().contains(secret),
    }
}

fn icon(raw: Option<String>) -> Option<String> {
    let raw = raw?;
    let url = if raw.starts_with("/static/upload/mod/") && !raw.contains(['\\', '?', '#']) {
        format!("{SITE}{raw}")
    } else {
        raw
    };
    validate_download_url(&url).ok().map(|u| u.to_string())
}

// Only the observed first-party file service is a verified direct-download
// contract. HTTPS alone does not make a cloud-share or HTML page an archive.
fn direct_url(raw: &str) -> Option<String> {
    if raw.contains(['\\', '%']) || raw.split('/').any(|part| matches!(part, "." | "..")) {
        return None;
    }
    let url = validate_download_url(raw).ok()?;
    if url.host_str() != Some("dmod.3dmgame.com")
        || url.port().is_some()
        || url.query().is_some()
        || !(url.path().starts_with("/static/upload/resource/")
            || url.path().starts_with("/static/upload/resource2/"))
        || url.path().split('/').skip(1).any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
    {
        return None;
    }
    Some(url.to_string())
}

impl Provider {
    async fn three_dm_data(&self, request: reqwest::RequestBuilder) -> AppResult<Value> {
        let value = self.fetch_json(request).await?;
        // Error messages never include remote bodies. Also withhold successful
        // responses echoing the request key before any metadata enters a DTO.
        if let Ok(Some(key)) = &self.key {
            let key = key.to_str().map_err(|_| invalid())?;
            if has_credential(&value, key) {
                return Err(AppError::Denied("3DM 响应被凭据过滤器拦截".into()));
            }
        }
        match value["success"].as_bool() {
            Some(true) => value
                .get("data")
                .filter(|v| !v.is_null())
                .cloned()
                .ok_or_else(invalid),
            Some(false) => Err(AppError::Denied(
                "3DM 拒绝请求；请检查 API Key 权限或稍后重试".into(),
            )),
            None => Err(invalid()),
        }
    }

    async fn three_dm_page(
        &self,
        q: &SearchQuery,
        page: u64,
        game: Option<u64>,
    ) -> AppResult<Vec<ModSummary>> {
        let mut request = self.get(format!("{API}/mods")).query(&[
            ("page", page.to_string()),
            ("pageSize", q.limit.to_string()),
            ("order", "0".into()),
            ("search", q.query.clone()),
        ]);
        if let Some(game) = game {
            request = request.query(&[("gameId", game)]);
        }
        let data = self.three_dm_data(request).await?;
        if data["page"].as_u64() != Some(page) || data["pageSize"].as_u64() != Some(q.limit as u64)
        {
            return Err(invalid());
        }
        let rows = array(&data["data"])?;
        if rows.len() > q.limit as usize {
            return Err(invalid());
        }
        rows.iter()
            .map(|row| {
                let project = id(&row["id"])?;
                let game_id = id(&row["game_id"])?;
                if game.is_some_and(|expected| expected != game_id) {
                    return Err(invalid());
                }
                Ok(ModSummary {
                    source_id: "3dm".into(),
                    project_id: project.to_string(),
                    game_id: Some(game_id.to_string()),
                    name: text(row, "mods_title", 512)?,
                    summary: optional_text(row, "mods_desc", 4096)?.unwrap_or_default(),
                    author: optional_text(row, "mods_author", 256)?,
                    homepage_url: Some(format!("{SITE}/mod/{project}")),
                    icon_url: icon(optional_text(row, "mods_image_url", 2048)?),
                    downloads: match row.get("mods_download_cnt") {
                        None | Some(Value::Null) => None,
                        Some(v) => Some(v.as_u64().ok_or_else(invalid)?),
                    },
                })
            })
            .collect()
    }

    pub(super) async fn three_dm_search(&self, q: &SearchQuery) -> AppResult<SearchResult> {
        if q.query.chars().count() > 100 || q.limit == 0 || q.limit > 100 {
            return Err(AppError::Invalid(
                "3DM 搜索最多 100 字，limit 1..100".into(),
            ));
        }
        let game = q.game_ids.get("3dm").map(|g| request_id(g)).transpose()?;
        if q.game_version.is_some() || q.loader.is_some() {
            return Ok(self.empty(
                SourceState::Unsupported,
                "3DM 未提供可核验的游戏版本/加载器筛选；请移除筛选后人工核对兼容性",
            ));
        }
        let page = u64::from(q.offset / q.limit) + 1;
        let skip = (q.offset % q.limit) as usize;
        let rows = self.three_dm_page(q, page, game).await?;
        let full_page = rows.len() == q.limit as usize;
        let mut mods: Vec<_> = rows.into_iter().skip(skip).collect();
        if skip > 0 && full_page {
            let next = self.three_dm_page(q, page + 1, game).await?;
            mods.extend(next.into_iter().take(skip));
        }
        let mut ids = BTreeSet::new();
        if mods.iter().any(|m| !ids.insert(m.project_id.clone())) {
            return Err(invalid());
        }
        Ok(SearchResult {
            mods,
            sources: vec![self.status()],
        })
    }

    pub(super) async fn three_dm_resolve(&self, r: &ResolveRequest) -> AppResult<ResolveResult> {
        let project = request_id(&r.project_id)?;
        let game = r.game_id.as_deref().map(request_id).transpose()?;
        let resource = r.version_id.as_deref().map(request_id).transpose()?;
        let home = Some(format!("{SITE}/mod/{project}"));
        if r.game_version.is_some() || r.loader.is_some() {
            return Ok(self.unresolved(
                SourceState::ManualDownload,
                "3DM 未提供结构化兼容性信息，不能证明满足所选游戏版本/加载器；请在原始页面核对",
                home,
            ));
        }
        let data = self
            .three_dm_data(self.get(format!("{API}/mods/{project}")))
            .await?;
        if id(&data["id"])? != project || game.is_some_and(|g| data["game_id"].as_u64() != Some(g))
        {
            return Err(invalid());
        }
        id(&data["game_id"])?;
        let name = text(&data, "mods_title", 512)?;
        let rows = match data.get("mods_resource") {
            None | Some(Value::Null) => {
                return Ok(self.unresolved(
                    SourceState::ManualDownload,
                    "来源未提供资源，请在原始页面选择",
                    home,
                ))
            }
            Some(rows) => array(rows)?,
        };
        if rows.len() > 1024 {
            return Err(invalid());
        }
        let mut ids = BTreeSet::new();
        let mut candidates = Vec::new();
        let mut identities = BTreeSet::new();
        for row in rows {
            let file = id(&row["id"])?;
            if id(&row["mods_id"])? != project || !ids.insert(file) {
                return Err(invalid());
            }
            let latest = match row.get("mods_resource_latest_version") {
                None | Some(Value::Null) => false,
                Some(v) => v.as_bool().ok_or_else(invalid)?,
            };
            if resource.map_or(latest, |selected| selected == file) {
                identities.insert((
                    text(row, "mods_resource_name", 512)?,
                    text(row, "mods_resource_version", 128)?,
                ));
                let url = text(row, "mods_resource_url", 8192)?;
                if let Some(url) = direct_url(&url) {
                    candidates.push((row, file, url));
                }
            }
        }
        if resource.is_some_and(|selected| !ids.contains(&selected)) {
            return Err(AppError::NotFound(
                "3DM 未找到指定资源 ID；不会回退到其他文件".into(),
            ));
        }
        if candidates.len() != 1 || identities.len() != 1 {
            return Ok(self.unresolved(
                SourceState::ManualDownload,
                "没有唯一可核验的直链资源，可能是网盘或多个可选文件；请在原始页面选择",
                home,
            ));
        }
        let (row, file, url) = candidates.pop().unwrap();
        let filename = text(row, "mods_resource_name", 180)?;
        crate::download::safe_filename(&filename).map_err(|_| invalid())?;
        self.resolved(ResolvedMod {
            source_id: "3dm".into(),
            project_id: project.to_string(),
            version_id: file.to_string(),
            name,
            version: text(row, "mods_resource_version", 128)?,
            download_url: url,
            filename,
            // mods_resource_size is rounded prose, not an exact byte count.
            size: None,
            hashes: FileHashes::default(),
            homepage_url: home,
            warnings: vec![
                "3DM 未提供可核验的精确字节数及文件 hash；下载后必须人工审查，尚未安装".into(),
                "未验证游戏版本、加载器及前置依赖兼容性；不要仅凭 latest 标记安装".into(),
            ],
        })
    }
}
