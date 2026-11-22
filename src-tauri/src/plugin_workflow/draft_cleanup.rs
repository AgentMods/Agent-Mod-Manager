use super::*;

/// Called only by the Agent entry points, before their plans are exposed.
/// Keep provenance independent of conversation retention and never infer it from a slug.
pub(crate) fn mark_agent_draft(engine: &FileEngine, id: &str) -> AppResult<()> {
    engine.db.conn().execute(
        "INSERT INTO agent_plugin_drafts(draft_id) VALUES (?1)",
        [id],
    )?;
    Ok(())
}

fn cleanup_candidates(engine: &FileEngine, only_id: Option<&str>) -> AppResult<Vec<String>> {
    let conn = engine.db.conn();
    let mut stmt = conn.prepare(
        "SELECT a.draft_id FROM agent_plugin_drafts a JOIN plugin_drafts d ON d.id=a.draft_id
         JOIN file_transactions w ON w.id=d.write_transaction_id AND w.status='committed'
         WHERE (?1 IS NULL OR a.draft_id=?1) AND NOT EXISTS (SELECT 1 FROM plugin_draft_deletions x
            JOIN file_transactions f ON f.id=x.transaction_id WHERE x.draft_id=d.id AND
            (f.status IN ('committed','applying','rolling_back','recovery_failed') OR
            (f.status='rolled_back' AND f.error IS NULL)))",
    )?;
    // A completed move (even if the user restored it) or an explicit undo is
    // final for automatic cleanup: never immediately re-delete a restored draft.
    let ids = stmt
        .query_map([only_id], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

fn is_consumed(engine: &FileEngine, draft: &DraftInfo) -> AppResult<bool> {
    // Both publication implementations pin the live draft until they finish.
    // Deployments pin its exact source path; another draft of the same slug is unrelated.
    Ok(engine.db.conn().query_row(
        "WITH consumers(state) AS (
           SELECT CASE WHEN f.status='committed' AND EXISTS (
                SELECT 1 FROM plugins WHERE slug=p.slug AND validated=1 AND source='external'
                  AND install_path=p.install_path) THEN 'completed' ELSE f.status END
             FROM plugin_deployments p JOIN file_transactions f ON f.id=p.transaction_id
             WHERE p.slug=?1 AND json_extract(f.plan_json,'$.entries[0].operation.src')=?2
           UNION ALL SELECT state FROM manager_publications WHERE slug=?1
             AND json_extract(preview_json,'$.source.draftId')=?3
             AND json_extract(preview_json,'$.source.writeTransactionId')=?4
           UNION ALL SELECT state FROM plugin_publications
             WHERE json_extract(preview_json,'$.source.draftId')=?3
             AND json_extract(preview_json,'$.source.writeTransactionId')=?4
         ) SELECT EXISTS(SELECT 1 FROM consumers WHERE state='completed') AND NOT EXISTS(
             SELECT 1 FROM consumers WHERE state IN ('planned','approved','applying','rolling_back',
                'recovery_failed','intended','pending','unknown'))",
        params![
            draft.slug,
            draft.path.join("adapter.json").to_string_lossy(),
            draft.id,
            draft.write_transaction_id
        ],
        |row| row.get(0),
    )?)
}

/// Best effort after a durable success. Cleanup is independent of the install
/// or remote receipt; failing a move must not turn either into a failed operation.
pub(crate) fn cleanup_consumed_agent_drafts(engine: &FileEngine, only_id: Option<&str>) {
    let _workflow = lock();
    let files = transaction::write_lock();
    let ids = match cleanup_candidates(engine, only_id) {
        Ok(ids) => ids,
        Err(error) => {
            tracing::warn!(%error, "无法检查 Agent 草稿清理状态");
            return;
        }
    };
    for id in ids {
        let cleanup = || -> AppResult<()> {
            if is_consumed(engine, &get_draft(engine, &id)?)? {
                recycle_draft_locked(engine, &id, &files, true)?;
            }
            Ok(())
        };
        if let Err(error) = cleanup() {
            tracing::warn!(draft_id = %id, %error, "未能自动回收 Agent 草稿");
        }
    }
}
