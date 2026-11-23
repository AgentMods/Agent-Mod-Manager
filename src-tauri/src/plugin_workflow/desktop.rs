//! Desktop-only lifecycle intents. Hold WORKFLOW_LOCK then WRITE_LOCK from
//! inspection through the filesystem journal and the atomic registry commit.
use super::*;

pub(super) const CATALOG_INSTALL_KIND: &str = "catalog_plugin_install";
pub(super) const CATALOG_UPDATE_KIND: &str = "catalog_plugin_update";
pub(crate) const PLUGIN_REMOVE_KIND: &str = "plugin_remove";
const SCRATCH: &str = "plugin-catalog";
const RELEASE_PREFIX: &str = "release-";

pub(crate) fn is_desktop_plugin_kind(kind: &str) -> bool {
    matches!(kind, CATALOG_INSTALL_KIND | CATALOG_UPDATE_KIND | PLUGIN_REMOVE_KIND)
}

pub(super) fn is_catalog_scratch(engine: &FileEngine, source: &Path) -> bool {
    source.parent() == Some(engine.paths.staging().join(SCRATCH).as_path())
        && source
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| {
                n.strip_prefix(RELEASE_PREFIX).is_some_and(|suffix| {
                    suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
                })
            })
}

pub(crate) fn install_catalog_release(
    engine: &FileEngine,
    preview: crate::manager_catalog::CatalogPreview,
) -> AppResult<()> {
    let _workflow = lock();
    let files = transaction::write_lock();
    reconcile_locked(engine)?;
    install_new_release_locked(engine, &files, preview, None)
}

/// Desktop Add authorizes one pinned catalog release for one directory. Never
/// overwrite/re-enable a different local package, even if its slug matches.
pub(crate) fn register_catalog_game(
    engine: &FileEngine,
    preview: crate::manager_catalog::CatalogPreview,
    registration: crate::games::CatalogGameRegistration,
) -> AppResult<crate::games::Game> {
    let _workflow = lock();
    let files = transaction::write_lock();
    let slug = &preview.plugin.slug;
    external_slug(engine, slug)?;
    reconcile_locked(engine)?;
    let target = install_path(engine, slug)?;
    if let Some(state) = registry_state(engine, slug, &target)? {
        let info = registered_plugin(engine, slug)?;
        if !state.plugin_enabled
            || !state.validated
            || info.version != preview.plugin.version
            || verified_installed_content(engine, &info, &target)?.hashes != preview.hashes
        {
            return Err(AppError::Conflict(
                "已安装的适配包已停用、无效或与云端版本不同，不会覆盖".into(),
            ));
        }
        let mut conn = engine.db.conn();
        let tx = conn.transaction()?;
        let game = registration.register(&tx, &engine.paths, slug)?;
        tx.commit()?;
        return Ok(game);
    }
    // The newly created registration is read back under the same write lock.
    let root = registration.root.clone();
    install_new_release_locked(engine, &files, preview, Some(registration))?;
    let conn = engine.db.conn();
    let id: String = conn.query_row(
        "SELECT id FROM games WHERE root_path=?1",
        [root.to_string_lossy()],
        |r| r.get(0),
    )?;
    crate::games::get_game_from(&conn, &id)
}

fn install_new_release_locked(
    engine: &FileEngine,
    files: &MutexGuard<'static, ()>,
    preview: crate::manager_catalog::CatalogPreview,
    registration: Option<crate::games::CatalogGameRegistration>,
) -> AppResult<()> {
    let slug = &preview.plugin.slug;
    external_slug(engine, slug)?;
    let target = install_path(engine, slug)?;
    let installed: Option<String> = engine
        .db
        .conn()
        .query_row("SELECT version FROM plugins WHERE slug=?1", [slug], |r| {
            r.get(0)
        })
        .optional()?;
    if let Some(version) = installed {
        return Err(AppError::Conflict(format!(
            "{slug} 已安装 {version}，不会覆盖现有版本；如需安装其他版本，请先移除适配包"
        )));
    }
    registry_state(engine, slug, &target)?;
    match fs::symlink_metadata(&target) {
        Ok(m) if m.is_dir() && fs::read_dir(&target)?.next().is_none() => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(AppError::Conflict(
                "安装位置已有未登记文件，不会覆盖".into(),
            ))
        }
        Err(e) => return Err(e.into()),
    }
    deploy_catalog_locked(engine, files, preview, &target, registration, None)
}

pub(super) fn deploy_catalog_locked(
    engine: &FileEngine,
    files: &MutexGuard<'static, ()>,
    preview: crate::manager_catalog::CatalogPreview,
    target: &Path,
    registration: Option<crate::games::CatalogGameRegistration>,
    previous: Option<PreviousPackage>,
) -> AppResult<()> {
    let slug = &preview.plugin.slug;
    let content = Content {
        adapter_json: preview.adapter_json,
        skill_markdown: preview.skill_markdown,
        hashes: preview.hashes,
    };
    let package = content.package(slug)?;
    ensure_mcp_ids_free(&engine.db.conn(), slug, &package)?;
    let (kind, action) = if previous.is_some() {
        (CATALOG_UPDATE_KIND, "自动更新云插件")
    } else {
        (CATALOG_INSTALL_KIND, "安装云插件")
    };
    let parent = checked_path(engine, &engine.paths.staging().join(SCRATCH))?;
    crate::safe_fs::create_dir_all(&parent)?;
    let scratch = tempfile::Builder::new()
        .prefix(RELEASE_PREFIX)
        .rand_bytes(16)
        .tempdir_in(&parent)?;
    let source = checked_path(engine, scratch.path())?;
    for (name, text) in [
        ("adapter.json", &content.adapter_json),
        ("SKILL.md", &content.skill_markdown),
    ] {
        let path = checked_path(engine, &source.join(name))?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    let mut plan = engine.build_locked_plan(
        files,
        ["adapter.json", "SKILL.md"]
            .into_iter()
            .map(|name| FileOp::Copy {
                src: source.join(name),
                dst: target.join(name),
            })
            .collect(),
        format!("{action} {slug} {}", package.adapter.version),
        false,
    )?;
    plan.requires_review = true;
    verify_copy_plan(&plan, &plan.id, &source, target, &content, previous.is_none())?;
    verify_previous(&plan, target, previous.as_ref())?;
    let hashes = content.hashes.clone();
    let json = serde_json::to_string(&DeploymentRecord {
        schema_version: 1,
        registration,
        content,
        plugin_enabled: previous.as_ref().is_none_or(|p| p.plugin_enabled),
        skill_enabled: previous.as_ref().is_none_or(|p| p.skill_enabled),
        previous,
    })?;
    persist_authorized(engine, &plan, kind, |tx| {
        catalog_updates::record_origin(tx, &plan.id, &preview.plugin.id, &hashes)?;
        tx.execute("INSERT INTO plugin_deployments(transaction_id,slug,package_json,install_path,created_at) VALUES (?1,?2,?3,?4,?5)",
            params![plan.id, slug, json, target.to_string_lossy(), util::now()])?;
        Ok(())
    })?;
    apply_authorized(engine, files, &plan.id)
}

pub(crate) fn remove_installed_plugin(
    engine: &FileEngine,
    slug: &str,
) -> AppResult<PluginRemovalReceipt> {
    let _workflow = lock();
    let files = transaction::write_lock();
    external_slug(engine, slug)?;
    let target = install_path(engine, slug)?;
    reconcile_locked(engine)?;
    ensure_unbound(&engine.db.conn(), slug)?;
    let info = registered_plugin(engine, slug)?;
    let state =
        registry_state(engine, slug, &target)?.ok_or_else(|| invalid("适配包登记不存在"))?;
    if !state.validated {
        return Err(AppError::Conflict(
            "适配包未通过完整性校验，请先恢复原有包文件再移除".into(),
        ));
    }
    // Even though the backup destination needs internal access, never move a
    // directory that contains an independently registered game root.
    engine.stable_path(&target, false)?;
    let content = read_content(engine, &target)?;
    let package = content.package(slug)?;
    if package.adapter.version != info.version
        || !plugins::matches_registered_adapter(&state.adapter_json, &package.adapter)?
    {
        return Err(AppError::Conflict(
            "适配包文件与登记不一致，不会移动未知文件".into(),
        ));
    }
    let backup = checked_path(
        engine,
        &engine
            .paths
            .backups()
            .join("removed-plugins")
            .join(util::new_id("package")),
    )?;
    let plan = engine.build_locked_plan(
        &files,
        vec![FileOp::Move {
            src: target,
            dst: backup,
        }],
        format!("移除适配包 {slug} {}（可回滚）", info.version),
        true,
    )?;
    if plan.entries[0].before.is_some() {
        return Err(AppError::Conflict("适配包备份位置已存在".into()));
    }
    let json = serde_json::to_string(&DeploymentRecord {
        schema_version: 1,
        registration: None,
        content,
        previous: None,
        plugin_enabled: state.plugin_enabled,
        skill_enabled: state.skill_enabled,
    })?;
    persist_authorized(engine, &plan, PLUGIN_REMOVE_KIND, |tx| {
        tx.execute("INSERT INTO plugin_removals(transaction_id,slug,last_deployment_id,package_json,package_updated_at)
            VALUES (?1,?2,(SELECT transaction_id FROM plugin_deployments WHERE slug=?2 ORDER BY rowid DESC LIMIT 1),?3,?4)",
            params![plan.id, slug, json, info.updated_at])?;
        Ok(())
    })?;
    apply_authorized(engine, &files, &plan.id)?;
    Ok(PluginRemovalReceipt {
        transaction_id: plan.id,
    })
}

fn persist_authorized(
    engine: &FileEngine,
    plan: &FilePlan,
    kind: &str,
    record: impl FnOnce(&rusqlite::Transaction<'_>) -> AppResult<()>,
) -> AppResult<()> {
    let mut conn = engine.db.conn();
    let tx = conn.transaction()?;
    FileEngine::persist_plan_kind(&tx, plan, kind)?;
    record(&tx)?;
    tx.execute(
        "UPDATE file_transactions SET status='approved' WHERE id=?1",
        [&plan.id],
    )?;
    tx.commit()?;
    Ok(())
}

fn apply_authorized(
    engine: &FileEngine,
    files: &MutexGuard<'static, ()>,
    id: &str,
) -> AppResult<()> {
    if let Err(error) = engine.apply_desktop_plugin_locked(files, id) {
        // A rejected preflight may never have entered the journal. Do not leave
        // a runnable approved intent if its ephemeral source is about to vanish.
        engine.db.conn().execute(
            "UPDATE file_transactions SET status='failed',error=?2,finished_at=?3 WHERE id=?1 AND status='approved'",
            params![id, error.to_string(), util::now()],
        )?;
        return Err(error);
    }
    Ok(())
}

fn ensure_unbound(conn: &Connection, slug: &str) -> AppResult<()> {
    let game: Option<String> = conn
        .query_row(
            "SELECT name FROM games WHERE adapter_slug=?1 ORDER BY name,id LIMIT 1",
            [slug],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(game) = game {
        return Err(AppError::Conflict(format!(
            "「{game}」仍绑定此适配包，请先在「游戏库 → 对应游戏更多操作 → 选择适配包 → 移除适配包」解绑后重试"
        )));
    }
    Ok(())
}

pub(super) fn ensure_mcp_ids_free(conn: &Connection, slug: &str, package: &GamePackage) -> AppResult<()> {
    for config in package.adapter.mcp_configs()? {
        let collision: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM mcp_servers WHERE id=?1 AND plugin_slug IS NOT ?2)",
            params![config.id, slug],
            |r| r.get(0),
        )?;
        if collision {
            return Err(AppError::Conflict(format!(
                "MCP 工具标识已被其他连接占用，请先解决冲突：{}",
                config.id
            )));
        }
    }
    Ok(())
}

pub(super) fn ensure_deployment_not_retired(engine: &FileEngine, id: &str) -> AppResult<()> {
    let retired: bool = engine.db.conn().query_row(
        "SELECT EXISTS(SELECT 1 FROM plugin_deployments WHERE transaction_id=?1) AND
            NOT EXISTS(SELECT 1 FROM active_plugin_deployments WHERE transaction_id=?1)",
        [id],
        |r| r.get(0),
    )?;
    if retired {
        return Err(AppError::Conflict(
            "所选部署属于已移除的版本；请重新安装或重新预览草稿".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_plugin_rollback(engine: &FileEngine, id: &str, kind: &str) -> AppResult<()> {
    ensure_deployment_not_retired(engine, id)?;
    if kind == CATALOG_UPDATE_KIND {
        return catalog_updates::validate_rollback(engine, id);
    }
    let conn = engine.db.conn();
    if kind == CATALOG_INSTALL_KIND {
        let slug: String = conn.query_row(
            "SELECT slug FROM plugin_deployments WHERE transaction_id=?1",
            [id],
            |r| r.get(0),
        )?;
        ensure_unbound(&conn, &slug)?;
    }
    if kind == PLUGIN_REMOVE_KIND {
        let (slug, newer, json): (String, bool, String) = conn.query_row(
            "SELECT r.slug,
                EXISTS(SELECT 1 FROM plugin_removals n WHERE n.slug=r.slug AND n.sequence>r.sequence AND n.removed=1)
                OR EXISTS(SELECT 1 FROM plugin_deployments d JOIN file_transactions f ON f.id=d.transaction_id
                          WHERE d.slug=r.slug AND f.status='committed' AND d.rowid>COALESCE(last.rowid,0)), r.package_json
             FROM plugin_removals r LEFT JOIN plugin_deployments last ON last.transaction_id=r.last_deployment_id
             WHERE r.transaction_id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let target = engine.paths.plugins().join("games").join(&slug);
        if newer || registry_state_from(&conn, &slug, &target)?.is_some() {
            return Err(AppError::Conflict(
                "已存在更新的适配包安装或移除操作，不能直接恢复旧包".into(),
            ));
        }
        let record: DeploymentRecord = serde_json::from_str(&json)?;
        ensure_mcp_ids_free(&conn, &slug, &record.content.package(&slug)?)?;
    }
    Ok(())
}

/// Called inside the FileEngine's terminal journal write. Files and registry
/// never report a successful commit independently. Startup recovers a journal
/// still in progress before it reconciles the registry.
pub(crate) fn finalize_desktop_transaction(
    engine: &FileEngine,
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    kind: &str,
    status: &str,
    finished_at: &str,
) -> AppResult<Option<String>> {
    if kind == CATALOG_UPDATE_KIND {
        return catalog_updates::finalize(engine, tx, id, status, finished_at);
    }
    if !is_desktop_plugin_kind(kind) {
        return Ok(None);
    }
    let (slug, json, package_updated_at, removed): (String, String, Option<String>, bool) = if kind
        == CATALOG_INSTALL_KIND
    {
        tx.query_row(
            "SELECT slug,package_json,NULL,0 FROM plugin_deployments WHERE transaction_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?
    } else {
        tx.query_row("SELECT slug,package_json,package_updated_at,removed FROM plugin_removals WHERE transaction_id=?1", [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
    };
    let target = install_path(engine, &slug)?;
    let record: DeploymentRecord = serde_json::from_str(&json)?;
    if record.schema_version != 1 || record.previous.is_some() {
        return Err(invalid("桌面适配包事务记录无效"));
    }
    let state = registry_state_from(tx, &slug, &target)?;
    if kind == PLUGIN_REMOVE_KIND && status == "committed" {
        if state.is_none() {
            return Err(AppError::Conflict("适配包登记在移除期间已改变".into()));
        }
        verify_parked_package(engine, tx, id, &target, &record.content)?;
    }
    let register = (kind == CATALOG_INSTALL_KIND && status == "committed")
        || (kind == PLUGIN_REMOVE_KIND && status == "rolled_back" && removed);
    if register {
        if state.is_some() || read_content(engine, &target)?.hashes != record.content.hashes {
            return Err(AppError::Conflict(
                "适配包登记或文件与事务快照不一致".into(),
            ));
        }
        let timestamp = if kind == CATALOG_INSTALL_KIND {
            recorded_time(Some(finished_at)).map(format_update_time)
        } else {
            package_updated_at
        };
        register_in(
            tx,
            &slug,
            &target,
            &record.content,
            (record.plugin_enabled, record.skill_enabled),
            timestamp.as_deref(),
        )?;
        if kind == CATALOG_INSTALL_KIND {
            if let Some(registration) = record.registration {
                registration.register(tx, &engine.paths, &slug)?;
            }
        }
    } else if let Some(state) =
        state.filter(|_| kind == CATALOG_INSTALL_KIND || status == "committed")
    {
        ensure_unbound(tx, &slug)?;
        let package = record.content.package(&slug)?;
        if !plugins::matches_registered_adapter(&state.adapter_json, &package.adapter)? {
            return Err(AppError::Conflict(
                "适配包归属或版本在执行期间已改变".into(),
            ));
        }
        sync_plugin_servers(tx, &slug, &[])?;
        tx.execute(
            "DELETE FROM skills WHERE name=?1 AND source='external' AND location=?2",
            params![slug, target.join("SKILL.md").to_string_lossy()],
        )?;
        tx.execute(
            "DELETE FROM plugins WHERE slug=?1 AND source='external' AND install_path=?2",
            params![slug, target.to_string_lossy()],
        )?;
    }
    if kind == PLUGIN_REMOVE_KIND {
        tx.execute(
            "UPDATE plugin_removals SET removed=?2 WHERE transaction_id=?1",
            params![id, status == "committed"],
        )?;
    }
    Ok(Some(slug))
}

fn verify_parked_package(
    engine: &FileEngine,
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    target: &Path,
    content: &Content,
) -> AppResult<()> {
    let json: String = tx.query_row(
        "SELECT plan_json FROM file_transactions WHERE id=?1",
        [id],
        |row| row.get(0),
    )?;
    let plan: FilePlan = serde_json::from_str(&json)?;
    let [entry] = plan.entries.as_slice() else {
        return Err(invalid("适配包移除必须移动一个完整包目录"));
    };
    let FileOp::Move { src, dst } = &entry.operation else {
        return Err(invalid("适配包移除未使用可回滚的移动操作"));
    };
    if src != target
        || entry.before.is_some()
        || dst.parent() != Some(engine.paths.backups().join("removed-plugins").as_path())
        || read_content(engine, dst)?.hashes != content.hashes
    {
        return Err(AppError::Conflict("移除的适配包与已校验快照不一致".into()));
    }
    Ok(())
}
