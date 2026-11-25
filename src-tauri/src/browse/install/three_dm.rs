//! The 3DM stock MCP returns a detail document, not a local-file receipt.
//! Admit only the v3 detail endpoint's single first-party latest resource,
//! then hand it to the native downloader through a conversation resolution.
use super::{stock, WorkKey};
use crate::{
    error::AppResult,
    sources::{self, FileHashes, ResolveResult, ResolvedMod, SourceState, SourceStatus},
};
use serde_json::Value;
use std::collections::HashSet;

const ID: &str = "3dm";

pub(super) fn resolution(work: &WorkKey, response: &Value) -> AppResult<ResolveResult> {
    let unavailable = stock::unavailable;
    let envelope = stock::result(response)?;
    let project = stock::number(&work.project_id)?;
    let game = stock::number(&work.provider_game_id)?;
    let details = &envelope["data"];
    if envelope["success"] != true || details["id"] != project || details["game_id"] != game {
        return Err(unavailable());
    }
    let resources = details["mods_resource"]
        .as_array()
        .filter(|rows| !rows.is_empty() && rows.len() <= 1024)
        .ok_or_else(unavailable)?;
    let mut seen = HashSet::new();
    let mut latest = None;
    for row in resources {
        let id = row["id"].as_u64().ok_or_else(unavailable)?;
        stock::number(&id.to_string())?;
        if row["mods_id"] != project || !seen.insert(id) {
            return Err(unavailable());
        }
        match row.get("mods_resource_latest_version") {
            None | Some(Value::Null | Value::Bool(false)) => (),
            Some(Value::Bool(true)) if latest.is_none() => latest = Some((id, row)),
            _ => return Err(unavailable()),
        }
    }
    let (id, file) = latest.ok_or_else(unavailable)?;
    let url = text(file, "mods_resource_url", 8192)?;
    if url.contains(['\\', '%']) || url.split('/').any(|p| matches!(p, "." | "..")) {
        return Err(unavailable());
    }
    let url = sources::validate_download_url(url).map_err(|_| unavailable())?;
    if url.host_str() != Some("dmod.3dmgame.com")
        || url.port().is_some()
        || url.query().is_some()
        || !(url.path().starts_with("/static/upload/resource/")
            || url.path().starts_with("/static/upload/resource2/"))
        || url.path().split('/').skip(1).any(|p| {
            p.is_empty()
                || !p
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
    {
        return Err(unavailable());
    }
    let filename = text(file, "mods_resource_name", 180)?;
    crate::download::safe_filename(filename).map_err(|_| unavailable())?;
    Ok(ResolveResult {
        status: SourceStatus::new(ID, SourceState::Available, ""),
        download: Some(ResolvedMod {
            source_id: ID.into(),
            project_id: work.project_id.clone(),
            version_id: id.to_string(),
            name: text(details, "mods_title", 512)?.into(),
            version: text(file, "mods_resource_version", 128)?.into(),
            download_url: url.to_string(),
            filename: filename.into(),
            size: None,
            hashes: FileHashes::default(),
            homepage_url: None,
            warnings: vec!["3DM 未提供精确文件大小或哈希；请在安装前审查归档。".into()],
        }),
        homepage_url: None,
        dependencies: None,
    })
}

fn text<'a>(value: &'a Value, name: &str, max: usize) -> AppResult<&'a str> {
    value[name]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control))
        .ok_or_else(stock::unavailable)
}
