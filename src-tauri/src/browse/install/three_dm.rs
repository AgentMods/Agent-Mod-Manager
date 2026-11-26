//! The 3DM stock MCP returns a detail document, not a local-file receipt.
//! Admit only the v3 detail endpoint's single first-party latest resource,
//! then hand it to the native downloader through a conversation resolution.
use super::{stock, WorkKey};
use crate::{
    error::AppResult,
    sources::{self, FileHashes, ResolveResult, ResolvedMod, SourceState, SourceStatus},
};
use serde_json::Value;

const ID: &str = "3dm";

pub(super) fn resolution(work: &WorkKey, response: &Value) -> AppResult<ResolveResult> {
    let unavailable = stock::unavailable;
    let envelope = stock::result(response)?;
    let project = stock::number(&work.project_id)?;
    let game = stock::number(&work.provider_game_id)?;
    let homepage_url = sources::three_dm_project_page(&work.project_id).ok_or_else(unavailable)?;
    let details = &envelope["data"];
    if envelope["success"] != true || details["id"] != project || details["game_id"] != game {
        return Err(unavailable());
    }
    let file = sources::select_three_dm_resource(details, project, None)
        .map_err(|_| unavailable())?
        .ok_or_else(unavailable)?;
    if file.version.len() > 128 {
        return Err(unavailable());
    }
    Ok(ResolveResult {
        status: SourceStatus::new(ID, SourceState::Available, ""),
        download: Some(ResolvedMod {
            source_id: ID.into(),
            project_id: work.project_id.clone(),
            version_id: file.id.to_string(),
            name: text(details, "mods_title", 512)?.into(),
            version: file.version.trim().into(),
            download_url: file.url,
            filename: file.filename,
            size: None,
            hashes: FileHashes::default(),
            homepage_url: Some(homepage_url.clone()),
            warnings: vec!["3DM 未提供精确文件大小或哈希；请在安装前审查归档。".into()],
        }),
        homepage_url: Some(homepage_url),
        dependencies: None,
    })
}

fn text<'a>(value: &'a Value, name: &str, max: usize) -> AppResult<&'a str> {
    value[name]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control))
        .ok_or_else(stock::unavailable)
}
