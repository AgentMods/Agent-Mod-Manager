//! The catalog is fetched without a write lock. Tie each result to the same installed
//! deployment again under the lock before replacing either file.
use super::*;
use crate::manager_catalog::CatalogPreview;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CatalogUpdateCandidate {
    pub slug: String,
    pub version: String,
    pub deployment_id: String,
    pub plugin_id: Option<String>,
}

pub(super) fn record_origin(conn: &Connection, transaction_id: &str, plugin_id: &str, hashes: &PackageHashes) -> AppResult<()> {
    conn.execute("INSERT INTO catalog_plugin_origins(transaction_id,plugin_id,adapter_sha256,skill_sha256) VALUES (?1,?2,?3,?4)",
        params![transaction_id, plugin_id, hashes.adapter_sha256, hashes.skill_sha256])?;
    Ok(())
}

pub(crate) fn record_catalog_draft_origin(engine: &FileEngine, draft_id: &str, plugin_id: &str, hashes: &PackageHashes) -> AppResult<()> {
    let draft = get_draft(engine, draft_id)?;
    record_origin(&engine.db.conn(), &draft.write_transaction_id, plugin_id, hashes)
}

fn candidates(conn: &Connection, slug: Option<&str>) -> AppResult<Vec<CatalogUpdateCandidate>> {
    // Pick the current generation before filtering on origin. A later local draft
    // or a removal must not inherit an older cloud installation's subscription.
    let mut stmt = conn.prepare("SELECT p.slug,p.version,f.id,o.plugin_id FROM plugins p
        JOIN file_transactions f ON f.id=(
            SELECT d.transaction_id FROM active_plugin_deployments d
            JOIN file_transactions t ON t.id=d.transaction_id
            WHERE d.slug=p.slug AND t.status='committed' ORDER BY d.generation DESC LIMIT 1)
        LEFT JOIN catalog_plugin_origins o ON o.transaction_id=f.id
        WHERE p.source='external' AND p.validated=1 AND (?1 IS NULL OR p.slug=?1)
          AND (o.plugin_id IS NOT NULL OR f.kind=?2) ORDER BY p.slug")?;
    let rows = stmt.query_map(params![slug, desktop::CATALOG_INSTALL_KIND], |r| Ok(CatalogUpdateCandidate {
        slug: r.get(0)?, version: r.get(1)?, deployment_id: r.get(2)?, plugin_id: r.get(3)?,
    }))?;
    Ok(rows.collect::<Result<_,_>>()?)
}

pub(crate) fn catalog_update_candidates(engine: &FileEngine) -> AppResult<Vec<CatalogUpdateCandidate>> {
    candidates(&engine.db.conn(), None)
}

pub(crate) fn update_catalog_release(engine: &FileEngine, candidate: &CatalogUpdateCandidate, preview: CatalogPreview) -> AppResult<()> {
    let _workflow = lock();
    let files = transaction::write_lock();
    let slug = &candidate.slug;
    if preview.plugin.slug != *slug || candidate.plugin_id.as_ref().is_some_and(|id| id != &preview.plugin.id)
        || !crate::manager_publication::version_cmp(&preview.plugin.version, &candidate.version).is_gt()
        || candidates(&engine.db.conn(), Some(slug))?.first() != Some(candidate) {
        return Err(AppError::Conflict("适配包或来源已改变，跳过本次自动更新".into()));
    }
    external_slug(engine, slug)?;
    let target = install_path(engine, slug)?;
    let state = registry_state(engine, slug, &target)?.ok_or_else(|| invalid("适配包登记不存在"))?;
    let previous = read_content(engine, &target)?;
    let old_package = previous.package(slug)?;
    let json: String = engine.db.conn().query_row("SELECT package_json FROM plugin_deployments WHERE transaction_id=?1", [&candidate.deployment_id], |r| r.get(0))?;
    let record: DeploymentRecord = serde_json::from_str(&json)?;
    if previous.hashes != record.content.hashes || old_package.adapter.version != candidate.version
        || !plugins::matches_registered_adapter(&state.adapter_json, &old_package.adapter)? {
        return Err(AppError::Conflict("本地适配包与原部署不一致，跳过本次自动更新".into()));
    }
    desktop::deploy_catalog_locked(engine, &files, preview, &target, None, Some(PreviousPackage {
        content: previous, deployment_id: Some(candidate.deployment_id.clone()),
        plugin_enabled: state.plugin_enabled, skill_enabled: state.skill_enabled,
    }))
}

fn update_record(conn: &Connection, id: &str) -> AppResult<(String, DeploymentRecord)> {
    let (slug, json): (String, String) = conn.query_row("SELECT slug,package_json FROM plugin_deployments WHERE transaction_id=?1", [id], |r| Ok((r.get(0)?,r.get(1)?)))?;
    let record: DeploymentRecord = serde_json::from_str(&json)?;
    if record.schema_version != 1 || record.registration.is_some() || record.previous.as_ref().and_then(|old| old.deployment_id.as_ref()).is_none() {
        return Err(invalid("云适配包更新缺少有效的旧版部署记录"));
    }
    Ok((slug, record))
}

pub(super) fn validate_rollback(engine: &FileEngine, id: &str) -> AppResult<()> {
    let conn = engine.db.conn();
    let (slug, record) = update_record(&conn, id)?;
    let target = install_path(engine, &slug)?;
    // Validate before the first inverse rename, so edits or a newer deployment
    // cannot leave a manual rollback half-applied.
    if candidates(&conn, Some(&slug))?.first().is_none_or(|c| c.deployment_id != id)
        || read_content(engine, &target)?.hashes != record.content.hashes {
        return Err(AppError::Conflict("适配包在更新后已改变，不能回滚此版本".into()));
    }
    let previous = record.previous.as_ref().expect("validated previous deployment");
    desktop::ensure_mcp_ids_free(&conn, &slug, &previous.content.package(&slug)?)
}

pub(super) fn finalize(engine: &FileEngine, tx: &rusqlite::Transaction<'_>, id: &str, status: &str, finished_at: &str) -> AppResult<Option<String>> {
    let (slug, record) = update_record(tx, id)?;
    let target = install_path(engine, &slug)?;
    let state = registry_state_from(tx, &slug, &target)?.ok_or_else(|| invalid("云适配包的本地登记已移除"))?;
    let previous = record.previous.as_ref().expect("validated previous deployment");
    let (content, timestamp) = if status == "committed" {
        (&record.content, recorded_time(Some(finished_at)))
    } else {
        // This also runs during crash recovery when the files were partially copied
        // and the registry still describes the original version.
        let finished: Option<String> = tx.query_row("SELECT finished_at FROM file_transactions WHERE id=?1 AND status='committed'", [&previous.deployment_id], |r| r.get(0))?;
        (&previous.content, recorded_time(finished.as_deref()))
    };
    if read_content(engine, &target)?.hashes != content.hashes {
        return Err(AppError::Conflict("云适配包文件与事务快照不一致".into()));
    }
    register_in(tx, &slug, &target, content, (state.plugin_enabled, state.skill_enabled), timestamp.map(format_update_time).as_deref())?;
    Ok(Some(slug))
}
