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
    let details = &envelope["data"];
    if envelope["success"] != true || details["id"] != project || details["game_id"] != game {
        return Err(unavailable());
    }
    let resources = details["mods_resource"]
        .as_array()
        .ok_or_else(unavailable)?;
    let (file, id, url) = sources::select_three_dm_resource(resources, project, None)
        .map_err(|_| unavailable())?
        .ok_or_else(unavailable)?;
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
            download_url: url,
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
