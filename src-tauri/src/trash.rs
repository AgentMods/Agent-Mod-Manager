use crate::{
    db::Db,
    error::{AppError, AppResult},
    storage::AppPaths,
    transaction::{write_lock, FileEngine, FileOp, FilePlan},
    util,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrashItem {
    pub id: String,
    pub original_path: String,
    pub trash_path: String,
    pub transaction_id: String,
    pub size: u64,
    pub is_dir: bool,
    pub game_id: Option<String>,
    pub label: String,
    pub reason: String,
    pub created_at: String,
    pub restored_at: Option<String>,
    pub exists: bool,
    #[serde(default)]
    pub restore_requires_rollback: bool,
}

impl TrashItem {
    pub(crate) fn into_user_paths(mut self) -> Self {
        crate::paths::set_user_path_text(&mut self.original_path);
        crate::paths::set_user_path_text(&mut self.trash_path);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrashRequest {
    pub paths: Vec<String>,
    #[serde(default)]
    pub game_id: Option<String>,
    #[serde(default)]
    pub reason: String,
}

pub(crate) struct TrashDraft {
    pub(crate) operation: FileOp,
    manifest: TrashItem,
}

impl TrashDraft {
    pub(crate) fn id(&self) -> &str {
        &self.manifest.id
    }
}

/// Build trash moves and metadata without recording a plan or acquiring the write lock.
/// Call before taking the DB connection; path validation reads through the engine.
pub(crate) fn prepare_trash(engine: &FileEngine, req: &TrashRequest) -> AppResult<Vec<TrashDraft>> {
    draft_trash(engine, req, resolve_sources(engine, req)?)
}

fn resolve_sources(engine: &FileEngine, req: &TrashRequest) -> AppResult<Vec<PathBuf>> {
    let mut sources = Vec::with_capacity(req.paths.len());
    if req.paths.is_empty() {
        return Ok(sources);
    }
    let roots = engine.roots()?;
    let retained = engine.retained_paths()?;
    for raw in &req.paths {
        let path = Path::new(raw);
        if fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(AppError::Denied("不能移动符号链接".into()));
        }
        let src = engine.paths.guard(path, &roots)?;
        if retained.iter().any(|p| src.starts_with(p)) {
            return Err(AppError::Denied("不能移走事务保留目录".into()));
        }
        if engine.paths.root.starts_with(&src)
            || roots.iter().any(|r| r.starts_with(&src))
            || src.starts_with(engine.paths.backups())
            || src.starts_with(engine.paths.trash())
            || src == engine.paths.db_file()
            || src == engine.paths.root.join("manager.db-wal")
            || src == engine.paths.root.join("manager.db-shm")
        {
            return Err(AppError::Denied(
                "不能移动管理器内部数据或已登记的根目录".into(),
            ));
        }
        sources.push(src);
    }
    Ok(sources)
}

fn draft_trash(
    engine: &FileEngine,
    req: &TrashRequest,
    sources: Vec<PathBuf>,
) -> AppResult<Vec<TrashDraft>> {
    let mut drafts = Vec::with_capacity(sources.len());
    for (raw, src) in req.paths.iter().zip(sources) {
        let fp = crate::transaction::fingerprint(&src)?
            .ok_or_else(|| AppError::NotFound(raw.clone()))?;
        let id = util::new_id("trash");
        let payload = engine.paths.trash_entry(&id).join("payload");
        let label = src.file_name().unwrap().to_string_lossy().to_string();
        drafts.push(TrashDraft {
            manifest: TrashItem {
                id,
                original_path: src.display().to_string(),
                trash_path: payload.display().to_string(),
                transaction_id: String::new(),
                game_id: req.game_id.clone(),
                label,
                reason: req.reason.clone(),
                created_at: util::now(),
                size: fp.size,
                is_dir: fp.kind == "dir",
                restored_at: None,
                exists: false,
                restore_requires_rollback: false,
            },
            operation: FileOp::Move { src, dst: payload },
        });
    }
    Ok(drafts)
}

/// Record trash metadata for a plan already inserted by the caller.
pub(crate) fn record_prepared(
    engine: &FileEngine,
    tx: &Connection,
    plan_id: &str,
    drafts: Vec<TrashDraft>,
) -> AppResult<()> {
    for TrashDraft { mut manifest, .. } in drafts {
        manifest.transaction_id = plan_id.into();
        // Metadata precedes any file move. Incomplete plans stay hidden from UI.
        let folder = engine.paths.trash_entry(&manifest.id);
        util::atomic_write(
            &folder.join("manifest.json"),
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
        tx.execute("INSERT INTO trash_items (id,original_path,trash_path,size,is_dir,game_id,transaction_id,label,reason,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![manifest.id,manifest.original_path,manifest.trash_path,manifest.size as i64,manifest.is_dir,manifest.game_id,plan_id,manifest.label,manifest.reason,manifest.created_at])?;
    }
    Ok(())
}

/// Return a preview. The approval coordinator must approve before execution.
pub fn plan_trash(engine: &FileEngine, req: &TrashRequest) -> AppResult<FilePlan> {
    let guard = write_lock();
    let sources = resolve_sources(engine, req)?;
    if !sources.is_empty() {
        validate_sources(engine, sources.iter().map(PathBuf::as_path))?;
    }
    let drafts = draft_trash(engine, req, sources)?;
    let ops = drafts.iter().map(|draft| draft.operation.clone()).collect();
    let plan = engine.build_locked_plan(&guard, ops, "移入回收站".into(), true)?;
    let mut conn = engine.db.conn();
    let tx = conn.transaction()?;
    FileEngine::persist_plan(&tx, &plan, true)?;
    record_prepared(engine, &tx, &plan.id, drafts)?;
    tx.commit()?;
    Ok(plan)
}

fn validate_sources<'a>(
    engine: &FileEngine,
    paths: impl Iterator<Item = &'a Path> + Clone,
) -> AppResult<()> {
    let library = engine.paths.mod_library();
    if let Some(path) = paths
        .clone()
        .find(|path| crate::paths::overlaps(&library, path))
    {
        return Err(AppError::Denied(format!(
            "不能通过通用回收站移走 Mods 库目录或库文件：{}。请通过 Mods 库管理入口操作。",
            crate::paths::display_path(path)
        )));
    }
    crate::install::verify_unowned_paths(engine, paths)
}

/// Recheck trash moves under the file write lock, including legacy plans.
/// Incoming uninstall and replacement moves pass the dedicated install validation.
pub(crate) fn validate_apply(engine: &FileEngine, plan: &FilePlan) -> AppResult<()> {
    let trash = engine.paths.trash();
    let mut sources = plan
        .entries
        .iter()
        .filter_map(|entry| match &entry.operation {
            FileOp::Move { src, dst } if crate::paths::contains_path(&trash, dst) => {
                Some(src.as_path())
            }
            _ => None,
        })
        .peekable();
    if sources.peek().is_some() {
        let install_action: bool = engine.db.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM install_transactions t WHERE t.transaction_id=?1 AND (
                t.action='uninstall' OR (t.action='install' AND EXISTS(
                    SELECT 1 FROM install_replacements r WHERE r.transaction_id=t.transaction_id
                    AND r.replacement_install_id=t.install_id))))",
            [&plan.id],
            |row| row.get(0),
        )?;
        if !install_action {
            validate_sources(engine, sources)?;
        }
    }
    validate_restore_apply(engine, plan, &trash)
}

fn validate_restore_apply(engine: &FileEngine, plan: &FilePlan, trash: &Path) -> AppResult<()> {
    let mut destinations = plan
        .entries
        .iter()
        .filter_map(|entry| match &entry.operation {
            FileOp::Move { src, dst } if crate::paths::contains_path(trash, src) => {
                Some(dst.as_path())
            }
            _ => None,
        })
        .peekable();
    if destinations.peek().is_none() {
        return Ok(());
    }
    let managed = {
        let conn = engine.db.conn();
        let mut stmt = conn.prepare(
            "SELECT a.install_id,t.original_path FROM trash_restore_links r
             JOIN trash_items t ON t.id=r.item_id
             JOIN install_transactions a ON a.transaction_id=t.transaction_id AND a.action='uninstall'
             JOIN file_transactions u ON u.id=a.transaction_id AND u.status='committed'
             WHERE r.transaction_id=?1",
        )?;
        let rows = stmt.query_map([&plan.id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                PathBuf::from(row.get::<_, String>(1)?),
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    crate::install::validate_trash_restore_ownership(
        engine,
        destinations,
        managed
            .iter()
            .map(|(id, path)| (id.as_str(), path.as_path())),
    )
}

pub fn list_trash(db: &Db, include_restored: bool) -> AppResult<Vec<TrashItem>> {
    let conn = db.conn();
    let mut stmt = conn.prepare("SELECT t.id,t.original_path,t.trash_path,t.size,t.is_dir,t.game_id,t.label,t.reason,t.created_at,t.transaction_id,
        (SELECT f.finished_at FROM trash_restore_links l JOIN file_transactions f ON f.id=l.transaction_id WHERE l.item_id=t.id AND f.status='committed' LIMIT 1),
        EXISTS(SELECT 1 FROM install_replacements r WHERE r.transaction_id=t.transaction_id)
        FROM trash_items t JOIN file_transactions f ON f.id=t.transaction_id WHERE f.status='committed'
        AND NOT EXISTS(SELECT 1 FROM trash_purges p WHERE p.item_id=t.id AND p.status='done') ORDER BY t.created_at DESC")?;
    let rows = stmt.query_map([], |r| {
        let path: String = r.get(2)?;
        Ok(TrashItem {
            id: r.get(0)?,
            original_path: r.get(1)?,
            exists: Path::new(&path).exists(),
            trash_path: path,
            size: r.get::<_, i64>(3)? as u64,
            is_dir: r.get(4)?,
            game_id: r.get(5)?,
            label: r.get(6)?,
            reason: r.get(7)?,
            created_at: r.get(8)?,
            transaction_id: r.get(9)?,
            restored_at: r.get(10)?,
            restore_requires_rollback: r.get(11)?,
        })
    })?;
    Ok(rows
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|r| include_restored || r.restored_at.is_none())
        .collect())
}

pub fn plan_restore(engine: &FileEngine, ids: &[String], overwrite: bool) -> AppResult<FilePlan> {
    let guard = write_lock();
    let items = list_trash(&engine.db, false)?;
    let selected = ids
        .iter()
        .map(|id| {
            items
                .iter()
                .find(|i| &i.id == id)
                .ok_or_else(|| AppError::NotFound(id.clone()))
        })
        .collect::<AppResult<Vec<_>>>()?;
    if selected.iter().any(|item| item.restore_requires_rollback) {
        return Err(AppError::Denied(
            "这些文件属于 Mod 替换事务，请通过回滚替换事务恢复旧文件".into(),
        ));
    }
    let mut ops = Vec::with_capacity(selected.len());
    for item in &selected {
        let src = PathBuf::from(&item.trash_path);
        crate::paths::ensure_within(&src, &[engine.paths.trash()])?;
        let dst = engine
            .paths
            .guard(Path::new(&item.original_path), &engine.roots()?)?;
        if dst.exists() && !overwrite {
            return Err(AppError::Conflict(format!(
                "目标已存在: {}",
                crate::paths::display_path(&dst)
            )));
        }
        ops.push(FileOp::Move { src, dst });
    }
    let managed = {
        let conn = engine.db.conn();
        let mut stmt = conn.prepare(
            "SELECT install_id FROM install_transactions WHERE transaction_id=?1 AND action='uninstall'",
        )?;
        let mut managed = Vec::new();
        for (item, op) in selected.iter().zip(&ops) {
            let owner: Option<String> = stmt
                .query_row([&item.transaction_id], |row| row.get(0))
                .optional()?;
            if let Some(owner) = owner {
                managed.push((owner, op.dst()));
            }
        }
        managed
    };
    crate::install::validate_trash_restore_ownership(
        engine,
        ops.iter().map(FileOp::dst),
        managed.iter().map(|(id, path)| (id.as_str(), *path)),
    )?;
    let plan = engine.build_locked_plan(&guard, ops, "还原回收站文件".into(), true)?;
    let mut conn = engine.db.conn();
    let tx = conn.transaction()?;
    FileEngine::persist_plan(&tx, &plan, true)?;
    for id in ids {
        tx.execute(
            "INSERT INTO trash_restore_links (item_id,transaction_id) VALUES (?1,?2)",
            params![id, plan.id],
        )?;
    }
    tx.commit()?;
    Ok(plan)
}

/// Only desktop UI may call this after an explicit user confirmation.
/// Never expose purge_trash through the Agent or MCP tool registry.
pub fn purge_trash(
    db: &Db,
    paths: &AppPaths,
    ids: &[String],
    confirmation: &str,
) -> AppResult<u64> {
    if confirmation != "PURGE_TRASH" {
        return Err(AppError::Denied("缺少清空回收站确认".into()));
    }
    let _guard = crate::transaction::write_lock();
    paths.stable_directory(&paths.root)?;
    paths.stable_directory(&paths.trash())?;
    if ids.is_empty() || ids.len() > 5000 {
        return Err(AppError::Invalid("清空项目数量应在1至5000之间".into()));
    }
    let items = list_trash(db, true)?;
    let mut selected = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(AppError::Invalid("清空项目ID重复".into()));
        }
        let item = items
            .iter()
            .find(|i| &i.id == id)
            .ok_or_else(|| AppError::NotFound(id.clone()))?;
        let expected = paths.trash_entry(id);
        if !id.starts_with("trash_")
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || Path::new(&item.trash_path) != expected.join("payload")
            || crate::paths::ensure_within(&expected, &[paths.trash()])? != expected
        {
            return Err(AppError::PathEscape(item.trash_path.clone()));
        }
        selected.push((item, expected));
    }
    let mut freed = 0;
    for (item, expected) in selected {
        let payload_exists = Path::new(&item.trash_path).exists();
        db.conn().execute("INSERT INTO trash_purges(item_id,status,confirmed_at) VALUES (?1,'pending',?2) ON CONFLICT(item_id) DO UPDATE SET status='pending'",params![item.id,util::now()])?;
        if expected.exists() {
            fs::remove_dir_all(&expected)?;
        }
        db.conn().execute(
            "UPDATE trash_purges SET status='done',finished_at=?2 WHERE item_id=?1",
            params![item.id, util::now()],
        )?;
        if payload_exists {
            freed += item.size;
        }
    }
    Ok(freed)
}

#[cfg(test)]
#[path = "trash_install_tests.rs"]
mod install_tests;

#[cfg(test)]
mod tests {
    use super::*;
    fn commit(e: &FileEngine, p: &FilePlan) {
        e.approve_plan(&p.id).unwrap();
        e.apply_file_ops(&p.id).unwrap();
    }

    #[test]
    fn drafts_stay_inert_until_recorded_in_the_callers_transaction() {
        let (_d, e, g) = crate::transaction::tests::setup();
        let file = g.join("a");
        fs::write(&file, b"original").unwrap();
        let mut req = TrashRequest {
            paths: Vec::new(),
            game_id: None,
            reason: String::new(),
        };
        assert!(prepare_trash(&e, &req).unwrap().is_empty());
        req.paths.push(file.display().to_string());
        let drafts = prepare_trash(&e, &req).unwrap();
        let draft = &drafts[0];
        let id = draft.manifest.id.clone();
        let folder = e.paths.trash_entry(&id);
        assert!(!folder.exists());
        assert!(file.exists());
        let plan = e
            .plan_internal(vec![draft.operation.clone()], "移入回收站".into())
            .unwrap();

        let mut conn = e.db.conn();
        let tx = conn.transaction().unwrap();
        record_prepared(&e, &tx, &plan.id, drafts).unwrap();
        let persisted_plan: String = tx
            .query_row(
                "SELECT transaction_id FROM trash_items WHERE id=?1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(persisted_plan, plan.id);
        let manifest: TrashItem =
            serde_json::from_slice(&fs::read(folder.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.transaction_id, plan.id);
        let mut legacy_manifest = serde_json::to_value(&manifest).unwrap();
        legacy_manifest
            .as_object_mut()
            .unwrap()
            .remove("restore_requires_rollback");
        let legacy: TrashItem = serde_json::from_value(legacy_manifest).unwrap();
        assert!(!legacy.restore_requires_rollback);
        assert!(file.exists());
        assert!(!folder.join("payload").exists());
        tx.rollback().unwrap();
        let count: u32 = conn
            .query_row("SELECT count(*) FROM trash_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn manifest_named_file_roundtrips_without_metadata_collision() {
        let (_d, e, g) = crate::transaction::tests::setup();
        let file = g.join("manifest.json");
        fs::write(&file, b"user manifest").unwrap();
        let p = plan_trash(
            &e,
            &TrashRequest {
                paths: vec![file.display().to_string()],
                game_id: None,
                reason: String::new(),
            },
        )
        .unwrap();
        commit(&e, &p);
        let items = list_trash(&e.db, false).unwrap();
        assert!(!items[0].restore_requires_rollback);
        assert_eq!(fs::read(&items[0].trash_path).unwrap(), b"user manifest");
        let p = plan_restore(&e, &[items[0].id.clone()], false).unwrap();
        commit(&e, &p);
        assert_eq!(fs::read(file).unwrap(), b"user manifest");
    }

    #[test]
    fn overwrite_restore_has_preview_and_keeps_replaced_contents() {
        let (_d, e, g) = crate::transaction::tests::setup();
        let file = g.join("a");
        fs::write(&file, b"original").unwrap();
        let p = plan_trash(
            &e,
            &TrashRequest {
                paths: vec![file.display().to_string()],
                game_id: None,
                reason: String::new(),
            },
        )
        .unwrap();
        commit(&e, &p);
        let id = list_trash(&e.db, false).unwrap()[0].id.clone();
        fs::write(&file, b"replacement").unwrap();
        assert!(plan_restore(&e, std::slice::from_ref(&id), false).is_err());
        let p = plan_restore(&e, &[id], true).unwrap();
        assert_eq!(p.entries[0].impact, "overwrite");
        assert_eq!(fs::read(&file).unwrap(), b"replacement");
        commit(&e, &p);
        assert_eq!(fs::read(&file).unwrap(), b"original");
        assert_eq!(
            fs::read(e.paths.backup_dir(&p.id).join("0.original")).unwrap(),
            b"replacement"
        );
        e.rollback_transaction(&p.id).unwrap();
        assert_eq!(fs::read(file).unwrap(), b"replacement");
    }

    #[test]
    fn committed_replacement_trash_requires_rollback_and_purge_blocks_it() {
        let (_d, e, g) = crate::transaction::tests::setup();
        let file = g.join("old.jar");
        fs::write(&file, b"old mod").unwrap();
        let plan = plan_trash(
            &e,
            &TrashRequest {
                paths: vec![file.display().to_string()],
                game_id: Some("g".into()),
                reason: "替换 Mod".into(),
            },
        )
        .unwrap();
        {
            let conn = e.db.conn();
            conn.execute_batch(
                "INSERT INTO installs (id,game_id,name,installed_at,updated_at) VALUES
                ('old','g','Old','now','now'), ('new','g','New','now','now');",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO install_replacements(replaced_install_id,replacement_install_id,transaction_id)
                VALUES ('old','new',?1)",
                [&plan.id],
            )
            .unwrap();
        }
        assert!(list_trash(&e.db, false).unwrap().is_empty());
        commit(&e, &plan);
        let item = list_trash(&e.db, false).unwrap().remove(0);
        assert!(item.restore_requires_rollback);
        let ids = std::slice::from_ref(&item.id);
        for overwrite in [false, true] {
            let error = plan_restore(&e, ids, overwrite).unwrap_err();
            assert!(matches!(error, AppError::Denied(reason) if reason.contains("回滚替换事务")));
        }
        assert_eq!(fs::read(&item.trash_path).unwrap(), b"old mod");
        assert!(!file.exists());
        assert!(purge_trash(&e.db, &e.paths, ids, "").is_err());
        assert_eq!(purge_trash(&e.db, &e.paths, ids, "PURGE_TRASH").unwrap(), 7);
        assert!(list_trash(&e.db, true).unwrap().is_empty());
        let error = e.rollback_transaction(&plan.id).unwrap_err();
        assert!(matches!(error, AppError::Conflict(reason) if reason.contains("不能回滚")));
    }

    #[test]
    fn purge_requires_confirmation_and_cannot_remove_game_file() {
        let (_d, e, g) = crate::transaction::tests::setup();
        let file = g.join("a");
        fs::write(&file, b"keep").unwrap();
        let p = plan_trash(
            &e,
            &TrashRequest {
                paths: vec![file.display().to_string()],
                game_id: None,
                reason: String::new(),
            },
        )
        .unwrap();
        commit(&e, &p);
        let item = list_trash(&e.db, false).unwrap().remove(0);
        assert!(purge_trash(&e.db, &e.paths, std::slice::from_ref(&item.id), "").is_err());
        assert!(Path::new(&item.trash_path).exists());
        e.db.conn()
            .execute(
                "UPDATE trash_items SET trash_path=?1 WHERE id=?2",
                params![file.display().to_string(), item.id],
            )
            .unwrap();
        assert!(purge_trash(&e.db, &e.paths, &[item.id], "PURGE_TRASH").is_err());
    }
}
