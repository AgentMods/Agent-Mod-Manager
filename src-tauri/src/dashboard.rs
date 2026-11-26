use crate::{db::Db, error::AppResult, install, mod_library, mod_management};
use rusqlite::params;
use serde::Serialize;
use std::collections::HashSet;

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardStats {
    pub managed_game_count: usize,
    pub installed_mod_count: usize,
    pub total_mod_count: usize,
}

/// Read only the columns needed to count the game detail library rows.
pub(crate) fn read_stats(db: &Db) -> AppResult<DashboardStats> {
    let mut conn = db.conn();
    let snapshot = conn.transaction()?;
    let mut stats = DashboardStats::default();
    {
        let state = install::effective_install_state();
        let mut installs = snapshot.prepare(&format!(
            "SELECT {state}, metadata_json, EXISTS (SELECT 1 FROM removed_mod_items removed
                WHERE removed.game_id=installs.game_id AND removed.kind='install'
                    AND removed.item_id=installs.id)
             FROM installs WHERE game_id=?1"
        ))?;
        let mut archives = snapshot.prepare(&mod_library::game_archives_query("id", true))?;
        let mut games = snapshot.prepare("SELECT id,name FROM games")?;
        let mut games = games.query([])?;
        while let Some(game) = games.next()? {
            let game_id: String = game.get(0)?;
            let game_name: String = game.get(1)?;
            stats.managed_game_count += 1;

            // Match buildModRows: every visible install record represents its archive,
            // even when the record no longer owns any deployed files.
            let mut represented_archives = HashSet::new();
            let mut records = installs.query([&game_id])?;
            while let Some(record) = records.next()? {
                let status: String = record.get(0)?;
                if !mod_management::install_is_visible(&status, record.get(2)?) {
                    continue;
                }
                stats.total_mod_count += 1;
                if matches!(status.as_str(), "enabled" | "installed") {
                    stats.installed_mod_count += 1;
                }
                let metadata: String = record.get(1)?;
                let metadata = serde_json::from_str(&metadata)?;
                if let Some(archive_id) = represented_archive(&metadata) {
                    represented_archives.insert(archive_id);
                }
            }

            let mut entries =
                archives.query(params![game_id, mod_library::game_key(&game_name)])?;
            while let Some(entry) = entries.next()? {
                let archive_id: String = entry.get(0)?;
                if !represented_archives.contains(&archive_id) {
                    stats.total_mod_count += 1;
                }
            }
        }
    }
    snapshot.commit()?;
    Ok(stats)
}

fn represented_archive(metadata: &serde_json::Value) -> Option<String> {
    if let Some(id) = metadata.get("libraryEntryId").filter(|id| !id.is_null()) {
        return id.as_str().map(str::to_owned);
    }
    install::recorded_download_origin(metadata).map(|origin| origin.artifact_id)
}

#[cfg(test)]
mod tests;
