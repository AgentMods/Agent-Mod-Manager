//! Desktop list membership is independent of the durable install and rollback journals.
use crate::{
    error::{AppError, AppResult},
    games, install,
    transaction::{self, FileEngine},
    util,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Debug, Deserialize)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum Target {
    Plan(String),
    Record(String),
    Archive(String),
}

const ARTIFACT_ID: &str = "COALESCE(json_extract(metadata_json,'$.libraryEntryId'),json_extract(metadata_json,'$.downloadOrigin.artifactId'))";

pub(crate) fn inactive(status: &str) -> bool {
    matches!(status, "uninstalled" | "rolled_back" | "purged")
}

// After a journal restore, the active install and its original archive belong
// in management again, even after a later ordinary uninstall.
pub(crate) fn restore_membership(conn: &Connection) -> AppResult<()> {
    conn.execute(&format!("DELETE FROM removed_mod_items AS archive WHERE archive.kind='archive' AND EXISTS (
        SELECT 1 FROM removed_mod_items record JOIN installs i ON i.id=record.item_id AND i.game_id=record.game_id
        WHERE record.kind='install' AND record.game_id=archive.game_id AND i.status IN ('enabled','installed','partially_restored')
        AND ({ARTIFACT_ID})=archive.item_id)"), [])?;
    conn.execute("DELETE FROM removed_mod_items AS record WHERE kind='install' AND EXISTS (
        SELECT 1 FROM installs i WHERE i.id=record.item_id AND i.game_id=record.game_id AND i.status IN ('enabled','installed','partially_restored'))", [])?;
    Ok(())
}

pub(crate) fn visible_installs(
    engine: &FileEngine,
    game_id: &str,
) -> AppResult<Vec<install::InstalledMod>> {
    let records = install::list_installs(engine, game_id)?;
    let conn = engine.db.conn();
    let mut stmt =
        conn.prepare("SELECT item_id FROM removed_mod_items WHERE game_id=?1 AND kind='install'")?;
    let removed = stmt
        .query_map([game_id], |r| r.get(0))?
        .collect::<Result<HashSet<String>, _>>()?;
    // Restoring an uninstall can make a formerly removed record own game files again.
    Ok(records
        .into_iter()
        .filter(|r| !removed.contains(&r.id) || !(inactive(&r.status) || r.status == "planned"))
        .collect())
}

pub(crate) fn require_archive_visible(conn: &Connection, game: &str, id: &str) -> AppResult<()> {
    let hidden: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM removed_mod_items
         WHERE game_id=?1 AND kind='archive' AND item_id=?2)",
        params![game, id],
        |r| r.get(0),
    )?;
    if hidden {
        return Err(AppError::NotFound("此归档已从游戏的管理列表删除".into()));
    }
    Ok(())
}

fn hide(conn: &Connection, game: &str, kind: &str, id: &str) -> AppResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO removed_mod_items(game_id,kind,item_id) VALUES (?1,?2,?3)",
        params![game, kind, id],
    )?;
    Ok(())
}

fn discard_preview(conn: &Connection, game: &str, id: &str) -> AppResult<()> {
    let mut stmt = conn.prepare(
        "SELECT f.id,f.status FROM installs i
         JOIN install_transactions a ON a.install_id=i.id
         JOIN file_transactions f ON f.id=a.transaction_id
         WHERE i.game_id=?1 AND i.id=?2 AND a.action='install'
         AND (f.game_id IS NULL OR f.game_id=?1)",
    )?;
    let plans = stmt
        .query_map(params![game, id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let [(plan, status)] = plans.as_slice() else {
        return Err(AppError::Conflict("无法核对该安装计划，请刷新列表".into()));
    };
    if !matches!(status.as_str(), "planned" | "failed") {
        return Err(AppError::Conflict("安装计划状态已变化，请刷新列表".into()));
    }
    if status == "planned" {
        // A prepared write waiting on a live Agent has an approval in its chat.
        // Leave its one-shot pending response with the existing approval UI.
        let waiting: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM approvals WHERE status='pending'
             AND (json_extract(payload_json,'$.id')=?1 OR json_extract(payload_json,'$.plan.id')=?1))", [plan], |r| r.get(0))?;
        if waiting {
            return Err(AppError::Conflict(
                "此计划仍在会话中等待审批，请先在对应会话拒绝，再删除记录".into(),
            ));
        }
        let changed = conn.execute("UPDATE file_transactions SET status='failed',finished_at=?2,error='用户取消未执行的安装计划'
             WHERE id=?1 AND status='planned' AND json_array_length(journal_json)=0
             AND NOT EXISTS(SELECT 1 FROM file_transaction_journal WHERE transaction_id=?1)
             AND NOT EXISTS(SELECT 1 FROM file_transaction_entries WHERE transaction_id=?1)",params![plan,util::now()])?;
        if changed != 1 {
            return Err(AppError::Conflict(
                "该计划已有执行记录，无法作为待审批项删除".into(),
            ));
        }
    }
    hide(conn, game, "install", id)
}

pub fn remove(engine: &FileEngine, game_id: &str, target: &Target) -> AppResult<()> {
    let _files = transaction::write_lock();
    let mut conn = engine.db.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let game = games::get_game_from(&tx, game_id)?;
    let state = format!("CASE WHEN EXISTS(SELECT 1 FROM install_transactions a WHERE a.install_id=installs.id) THEN ({}) ELSE status END", install::INSTALL_STATE);
    let artifact = match target {
        Target::Plan(id) => {
            discard_preview(&tx, game_id, id)?;
            return Ok(tx.commit()?);
        }
        Target::Record(id) => {
            let (status, artifact): (String, Option<String>) = tx
                .query_row(
                    &format!(
                        "SELECT {state},{ARTIFACT_ID} FROM installs WHERE game_id=?1 AND id=?2"
                    ),
                    params![game_id, id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?
                .ok_or_else(|| AppError::NotFound("此游戏没有这条安装记录".into()))?;
            if !inactive(&status) {
                return Err(AppError::Conflict(
                    "请先完成卸载或回滚，再从管理列表删除".into(),
                ));
            }
            hide(&tx, game_id, "install", id)?;
            artifact
        }
        Target::Archive(id) => Some(id.clone()),
    };
    if let Some(artifact) = artifact {
        let belongs: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM mod_library_entries WHERE id=?1 AND state='ready'
             AND (game_id=?2 OR (game_id IS NULL AND game_name_key=?3)))",
            params![artifact, game_id, crate::mod_library::game_key(&game.name)],
            |r| r.get(0),
        )?;
        if belongs {
            let mut stmt = tx.prepare(&format!(
                "SELECT id,{state},EXISTS(SELECT 1 FROM removed_mod_items h
                 WHERE h.game_id=?1 AND h.kind='install' AND h.item_id=installs.id)
                 FROM installs WHERE game_id=?1 AND {ARTIFACT_ID}=?2"
            ))?;
            let related = stmt
                .query_map(params![game_id, artifact], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, bool>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let busy = related
                .iter()
                .any(|(_, state, hidden)| !(inactive(state) || *hidden && state == "planned"));
            if !busy {
                for (id, _, _) in related {
                    hide(&tx, game_id, "install", &id)?;
                }
                hide(&tx, game_id, "archive", &artifact)?;
            } else if matches!(target, Target::Archive(_)) {
                return Err(AppError::Conflict(
                    "此归档仍有关联安装或待处理计划，请先卸载或取消计划".into(),
                ));
            }
        } else if matches!(target, Target::Archive(_)) {
            return Err(AppError::NotFound("此游戏没有该归档".into()));
        }
    }
    Ok(tx.commit()?)
}
