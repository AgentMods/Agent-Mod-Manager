use super::{read_conversation, serde_column, Conversation, History};
use crate::{
    error::{AppError, AppResult},
    util::now,
};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use std::collections::HashSet;

pub(super) struct Workspace(Option<Vec<String>>);

impl Workspace {
    pub(super) fn validated(conn: &Connection, requested: Option<&[String]>) -> AppResult<Self> {
        let Some(requested) = requested else {
            return Ok(Self(None));
        };
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        let mut exists = conn.prepare("SELECT EXISTS(SELECT 1 FROM games WHERE id=?1)")?;
        for id in requested {
            if !seen.insert(id) {
                continue;
            }
            if id.trim().is_empty() || !exists.query_row([id], |row| row.get::<_, bool>(0))? {
                return Err(AppError::Invalid(format!("工作区包含无效游戏 ID: {id}")));
            }
            ids.push(id.clone());
        }
        Ok(Self(Some(ids)))
    }

    pub(super) fn single_game_id(&self) -> Option<&str> {
        match self.0.as_deref() {
            Some([one]) => Some(one),
            _ => None,
        }
    }

    pub(super) fn encode(&self) -> AppResult<Option<String>> {
        self.0
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(Into::into)
    }
}

pub(super) fn from_row(row: &Row<'_>) -> rusqlite::Result<Option<Vec<String>>> {
    let workspace = serde_column(row, row.as_ref().column_index("workspace_game_ids_json")?)?;
    if workspace.is_some() {
        return Ok(workspace);
    }
    // Also understand legacy rows written with only the compatibility column.
    let game_id: Option<String> = row.get("game_id")?;
    Ok(game_id.map(|id| vec![id]))
}

pub(crate) fn migrate_workspaces(tx: &Transaction<'_>) -> AppResult<()> {
    let has_workspace: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('conversations') WHERE name='workspace_game_ids_json')",
        [],
        |row| row.get(0),
    )?;
    if !has_workspace {
        tx.execute(
            "ALTER TABLE conversations ADD COLUMN workspace_game_ids_json TEXT
             CHECK(workspace_game_ids_json IS NULL OR json_type(workspace_game_ids_json)='array')",
            [],
        )?;
    }
    // Do not widen stale legacy bindings to the global workspace. Preserve
    // already-written explicit workspaces if a migration fixture lowers the version.
    tx.execute(
        "UPDATE conversations SET
            workspace_game_ids_json=CASE WHEN EXISTS(SELECT 1 FROM games WHERE id=conversations.game_id)
                THEN json_array(game_id) ELSE '[]' END,
            game_id=(SELECT id FROM games WHERE id=conversations.game_id)
         WHERE game_id IS NOT NULL AND workspace_game_ids_json IS NULL",
        [],
    )?;
    Ok(())
}

/// Called in the game removal transaction. Removing a selected game may leave
/// no selected games, but must not turn an explicit selection into dynamic all.
pub(crate) fn remove_game_from_workspaces(tx: &Transaction<'_>, id: &str) -> AppResult<()> {
    tx.execute(
        "WITH affected AS MATERIALIZED (
            SELECT c.id, (
                SELECT json_group_array(j.value ORDER BY j.key)
                FROM json_each(c.workspace_game_ids_json) j WHERE j.value<>?1
            ) AS game_ids
            FROM conversations c
            WHERE c.game_id=?1 OR EXISTS (
                SELECT 1 FROM json_each(c.workspace_game_ids_json) WHERE value=?1
            )
         )
         UPDATE conversations AS c SET
            workspace_game_ids_json=affected.game_ids,
            game_id=CASE WHEN json_array_length(affected.game_ids)=1
                THEN json_extract(affected.game_ids,'$[0]') END
         FROM affected WHERE c.id=affected.id",
        [id],
    )?;
    Ok(())
}

impl History {
    /// Read only the persisted selection, without fetching messages. None is
    /// dynamic all, Some([]) is no games, and Some(ids) is an explicit subset.
    pub fn conversation_workspace(&self, id: &str) -> AppResult<Option<Vec<String>>> {
        self.db
            .conn()
            .query_row(
                "SELECT game_id,workspace_game_ids_json FROM conversations WHERE id=?1",
                [id],
                from_row,
            )
            .optional()?
            .ok_or_else(|| AppError::NotFound("会话".into()))
    }

    pub fn set_conversation_workspace(
        &self,
        id: &str,
        game_ids: Option<&[String]>,
    ) -> AppResult<Conversation> {
        let mut conn = self.db.conn();
        // Serialize with turn start, archive and game deletion, across Db
        // clones and independent SQLite connections. Check before writing.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (archived, running): (bool, bool) = tx.query_row(
            "SELECT archived, EXISTS(SELECT 1 FROM agent_turns WHERE conversation_id=?1 AND status='running')
             FROM conversations WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?.ok_or_else(|| AppError::NotFound("会话".into()))?;
        if archived {
            return Err(AppError::Denied("请先恢复归档会话".into()));
        }
        if running {
            return Err(AppError::Conflict(
                "请先结束此对话中的任务，再切换工作区".into(),
            ));
        }
        let workspace = Workspace::validated(&tx, game_ids)?;
        tx.execute(
            "UPDATE conversations SET workspace_game_ids_json=?2,game_id=?3,updated_at=?4
             WHERE id=?1 AND (workspace_game_ids_json IS NOT ?2 OR game_id IS NOT ?3)",
            params![id, workspace.encode()?, workspace.single_game_id(), now()],
        )?;
        let conversation = read_conversation(&tx, id)?;
        tx.commit()?;
        Ok(conversation)
    }
}

#[cfg(test)]
#[path = "history_workspace_tests.rs"]
mod tests;
