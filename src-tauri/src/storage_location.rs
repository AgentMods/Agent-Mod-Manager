//! Desktop-only storage relocation. The location database and encrypted credentials
//! live in the default private directory, independent of the selected data root.
use crate::{
    error::{AppError, AppResult},
    safe_fs::{self, FileIdentity},
    util,
};
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

const DATA_FOLDER: &str = "Agent Mod Manager";
const MANAGER_DB: &str = "manager.db";

pub(crate) struct StorageLocation {
    default_root: PathBuf,
    // Exclusive locking keeps a restarted process from migrating files before
    // the previous process has exited and released its file handles.
    control: Mutex<Connection>,
}

#[derive(Serialize, Deserialize)]
struct Migration {
    target: PathBuf,
    parent_identity: FileIdentity,
    ready: Option<ReadyCopy>,
}

#[derive(Serialize, Deserialize)]
struct ReadyCopy {
    stage: PathBuf,
    identity: FileIdentity,
}

impl StorageLocation {
    pub(crate) fn new(default_root: PathBuf) -> AppResult<Self> {
        fs::create_dir_all(&default_root)?;
        let default_root = default_root.canonicalize()?;
        let private = default_root.join("private");
        crate::encrypted_store::private_directory(&private)?;
        let path = private.join("storage-location.db");
        if !absent(&path)? {
            require_file(&path)?;
        }
        let control = Connection::open(path)?;
        control.busy_timeout(Duration::from_secs(15))?;
        control.pragma_update(None, "synchronous", "FULL")?;
        control.pragma_update(None, "locking_mode", "EXCLUSIVE")?;
        control.execute_batch(
            "BEGIN EXCLUSIVE;
             CREATE TABLE IF NOT EXISTS location (
                 id INTEGER PRIMARY KEY CHECK(id = 1), active TEXT, pending TEXT, error TEXT
             );
             INSERT OR IGNORE INTO location(id) VALUES (1);
             COMMIT;",
        )?;
        Ok(Self {
            default_root,
            control: Mutex::new(control),
        })
    }

    #[cfg(not(test))]
    pub(crate) fn default_root(&self) -> &Path {
        &self.default_root
    }

    pub(crate) fn startup_root(&self) -> AppResult<PathBuf> {
        let conn = self.control.lock().unwrap();
        let (active, pending): (Option<String>, Option<String>) = conn.query_row(
            "SELECT active, pending FROM location WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let source = active
            .as_deref()
            .map(Path::new)
            .unwrap_or(&self.default_root);
        // A missing configured directory must never cause an empty database to be created.
        if active.is_some() {
            require_file(&source.join(MANAGER_DB))?;
        }
        let Some(pending) = pending else {
            return Ok(source.into());
        };
        let _writes = crate::transaction::write_lock();
        let mut migration: Migration = serde_json::from_str(&pending)?;
        match self.finish_migration(&conn, source, &mut migration) {
            Ok(()) => Ok(migration.target),
            Err(error) => {
                conn.execute(
                    "UPDATE location SET pending=NULL, error=?1 WHERE id=1",
                    [error.to_string()],
                )?;
                Ok(source.into())
            }
        }
    }

    pub(crate) fn failure(&self) -> AppResult<Option<String>> {
        Ok(self.control.lock().unwrap().query_row(
            "SELECT error FROM location WHERE id=1",
            [],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn schedule(
        &self,
        engine: &crate::transaction::FileEngine,
        parent: &Path,
    ) -> AppResult<()> {
        if !parent.is_absolute() {
            return Err(AppError::Invalid("请提供绝对目录路径".into()));
        }
        let parent = parent.canonicalize()?;
        if !parent.is_dir() {
            return Err(AppError::Invalid("请选择文件夹".into()));
        }
        let migration = Migration {
            target: parent.join(DATA_FOLDER),
            parent_identity: identity(&parent)?,
            ready: None,
        };
        self.validate_destination(&engine.paths.root, &migration)?;
        validate_database(&engine.db.conn(), &engine.paths.root, &migration.target)?;
        let updated = self.control.lock().unwrap().execute(
            "UPDATE location SET pending=?1, error=NULL WHERE id=1 AND pending IS NULL",
            [serde_json::to_string(&migration)?],
        )?;
        if updated == 0 {
            return Err(AppError::Conflict("已有存储迁移正在等待重启".into()));
        }
        Ok(())
    }

    fn validate_destination(&self, source: &Path, migration: &Migration) -> AppResult<()> {
        let target = &migration.target;
        let parent = target
            .parent()
            .ok_or_else(|| AppError::Invalid("无效迁移路径".into()))?;
        if !parent.is_dir() || identity(parent)? != migration.parent_identity {
            return Err(AppError::Conflict("所选文件夹已改变，请重新选择".into()));
        }
        if crate::paths::overlaps(target, source)
            || crate::paths::overlaps(target, &self.default_root)
        {
            return Err(AppError::Invalid(
                "新目录与现有管理器目录重叠，请另选位置".into(),
            ));
        }
        if migration.ready.is_none() && !absent(target)? {
            return Err(AppError::Conflict(format!(
                "所选位置已包含 {} 文件夹，请选择其他位置",
                DATA_FOLDER
            )));
        }
        Ok(())
    }

    fn finish_migration(
        &self,
        control: &Connection,
        source: &Path,
        migration: &mut Migration,
    ) -> AppResult<()> {
        self.validate_destination(source, migration)?;
        let target = &migration.target;
        let parent = target.parent().unwrap();
        if migration.ready.is_none() {
            let source_db = open_manager_db(source)?;
            validate_database(&source_db, source, target)?;
            checkpoint(&source_db)?;
            drop(source_db);

            let mut builder = tempfile::Builder::new();
            builder.prefix(".amm-storage-");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                builder.permissions(fs::Permissions::from_mode(0o700));
            }
            let stage = builder.tempdir_in(parent)?;
            let stage_identity = identity(stage.path())?;
            copy_storage(source, stage.path(), target)?;
            if identity(stage.path())? != stage_identity
                || identity(parent)? != migration.parent_identity
            {
                return Err(AppError::Conflict("迁移暂存目录或所选文件夹已改变".into()));
            }
            migration.ready = Some(ReadyCopy {
                stage: stage.path().into(),
                identity: stage_identity,
            });
            control.execute(
                "UPDATE location SET pending=?1 WHERE id=1",
                [serde_json::to_string(&migration)?],
            )?;
            let _ = stage.keep();
        }
        let ready = migration.ready.as_ref().unwrap();
        if absent(target)? {
            safe_fs::rename_checked(&ready.stage, target, &ready.identity)?;
        } else if identity(target)? != ready.identity || !absent(&ready.stage)? {
            return Err(AppError::Conflict("迁移目标已被占用".into()));
        }
        sync_directory(parent)?;
        require_file(&target.join(MANAGER_DB))?;
        let target = utf8_path(target)?;
        control.execute(
            "UPDATE location SET active=?1, pending=NULL, error=NULL WHERE id=1",
            [target],
        )?;
        Ok(())
    }
}

fn identity(path: &Path) -> AppResult<FileIdentity> {
    safe_fs::identity(path)?.ok_or_else(|| AppError::Invalid("目录缺失或无法识别".into()))
}

fn absent(path: &Path) -> AppResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

fn require_file(path: &Path) -> AppResult<()> {
    let file = safe_fs::open_regular(path)?;
    safe_fs::verify_regular_at(&file, path)
}

fn utf8_path(path: &Path) -> AppResult<&str> {
    path.to_str()
        .ok_or_else(|| AppError::Invalid("存储路径必须为有效的 Unicode 文本".into()))
}

fn validate_database(conn: &Connection, source: &Path, target: &Path) -> AppResult<()> {
    utf8_path(target)?;
    let interrupted: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM file_transactions
         WHERE status IN ('applying', 'rolling_back', 'recovery_failed'))",
        [],
        |row| row.get(0),
    )?;
    if interrupted {
        return Err(AppError::Conflict(
            "请先完成中断事务的恢复，再迁移存储".into(),
        ));
    }
    let mut roots =
        conn.prepare("SELECT root_path FROM games UNION SELECT path FROM game_user_roots")?;
    for entry in roots.query_map([], |row| row.get::<_, String>(0))? {
        let path = entry?;
        let root = crate::paths::canonicalize_lenient(Path::new(&path))?;
        if crate::paths::overlaps(source, &root) || crate::paths::overlaps(target, &root) {
            return Err(AppError::Invalid(
                "存储目录不能与已登记的游戏目录重叠".into(),
            ));
        }
    }
    Ok(())
}

fn open_manager_db(root: &Path) -> AppResult<Connection> {
    let path = root.join(MANAGER_DB);
    require_file(&path)?;
    for name in ["manager.db-wal", "manager.db-shm", "manager.db-journal"] {
        let sidecar = root.join(name);
        if !absent(&sidecar)? {
            require_file(&sidecar)?;
        }
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    conn.busy_timeout(Duration::from_secs(8))?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    Ok(conn)
}

fn checkpoint(conn: &Connection) -> AppResult<()> {
    let busy: u32 = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
    if busy != 0 {
        return Err(AppError::Conflict("数据库仍在使用，暂时无法迁移".into()));
    }
    Ok(())
}

fn copy_storage(source: &Path, stage: &Path, target: &Path) -> AppResult<()> {
    let entries = walkdir::WalkDir::new(source)
        .min_depth(1)
        .follow_links(false)
        .into_iter();
    for entry in entries.filter_entry(|entry| {
        entry.depth() != 1
            || !matches!(
                entry.file_name().to_str(),
                Some("private" | "manager.db-wal" | "manager.db-shm" | "manager.db-journal")
            )
    }) {
        let entry = entry?;
        let relative = entry
            .path()
            .strip_prefix(source)
            .expect("walk is rooted at source");
        let output = stage.join(relative);
        if entry.file_type().is_dir() {
            safe_fs::create_dir(&output)?;
        } else if entry.file_type().is_file() {
            copy_storage_file(entry.path(), relative, &output, target)?;
        } else {
            return Err(AppError::Denied(format!(
                "数据目录含有链接或特殊文件: {}",
                relative.display()
            )));
        }
    }
    let mut db = open_manager_db(stage)?;
    rebase_database(&mut db, source, stage, target)?;
    let check: String = db.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if check != "ok" {
        return Err(AppError::Conflict("新数据库未通过完整性检查".into()));
    }
    checkpoint(&db)?;
    drop(db);
    #[cfg(unix)]
    for entry in walkdir::WalkDir::new(stage)
        .contents_first(true)
        .follow_links(false)
    {
        let entry = entry?;
        if entry.file_type().is_dir() {
            sync_directory(entry.path())?;
        }
    }
    Ok(())
}

fn copy_storage_file(input: &Path, relative: &Path, output: &Path, target: &Path) -> AppResult<()> {
    // Snapshots cross-check their own stored location against the database row.
    let snapshot_manifest = relative
        .strip_prefix(Path::new("backups").join("snapshots"))
        .is_ok_and(|path| path.components().count() == 2 && path.ends_with("manifest.json"));
    let expected_hash = if snapshot_manifest {
        let file = safe_fs::open_regular(input)?;
        safe_fs::verify_regular_at(&file, input)?;
        let mut snapshot: crate::snapshot::Snapshot = serde_json::from_reader(&file)?;
        safe_fs::verify_regular_at(&file, input)?;
        let source_directory = input.parent().unwrap();
        if snapshot.path != source_directory {
            return Err(AppError::Conflict(
                "快照清单与原目录不一致，无法迁移".into(),
            ));
        }
        snapshot.path = target.join(relative.parent().unwrap());
        let contents = serde_json::to_vec_pretty(&snapshot)?;
        let mut output_file = safe_fs::create_new(output)?;
        output_file.write_all(&contents)?;
        output_file.sync_all()?;
        util::sha256_reader(&mut contents.as_slice())?
    } else {
        let expected_hash = util::sha256_file(input)?;
        util::atomic_copy(input, output)?;
        expected_hash
    };
    if util::sha256_file(output)? != expected_hash {
        return Err(AppError::Conflict(format!(
            "复制校验失败: {}",
            relative.display()
        )));
    }
    Ok(())
}

fn sync_directory(path: &Path) -> AppResult<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

// Path data used by application features. Free-form message text, approval/audit
// history and signed package declarations retain their original contents.
const PATH_COLUMNS: &[(&str, &str)] = &[
    ("games", "cover_path"),
    ("download_artifacts", "path"),
    ("trash_items", "original_path"),
    ("trash_items", "trash_path"),
    ("snapshots", "path"),
    ("plugins", "install_path"),
    ("skills", "location"),
    ("skills_deployments", "install_path"),
    ("plugin_drafts", "path"),
    ("plugin_deployments", "install_path"),
    ("file_transaction_entries", "src"),
    ("file_transaction_entries", "dst"),
    ("file_transaction_entries", "backup_path"),
];

const JSON_COLUMNS: &[(&str, &str)] = &[
    ("download_artifacts", "artifact_json"),
    ("installs", "files_json"),
    ("installs", "metadata_json"),
    ("skills_deployments", "record_json"),
    ("file_transactions", "plan_json"),
    ("file_transactions", "journal_json"),
];

fn rebase_database(
    db: &mut Connection,
    source: &Path,
    stage: &Path,
    target: &Path,
) -> AppResult<()> {
    let tx = db.transaction()?;
    crate::mcp::config::relocate_playwright_storage(&tx, source, target)?;
    for &(table, column) in PATH_COLUMNS.iter().chain(JSON_COLUMNS) {
        let json = JSON_COLUMNS.contains(&(table, column));
        let mut read = tx.prepare(&format!(
            "SELECT rowid, {column} FROM {table} WHERE {column} IS NOT NULL"
        ))?;
        let mut updates = Vec::new();
        let mut rows = read.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let text: String = row.get(1)?;
            if json {
                let mut value: Value = serde_json::from_str(&text)?;
                let identities_changed = column == "journal_json"
                    && rebase_journal_identities(&mut value, source, stage)?;
                if rebase_paths(&mut value, source, target)? || identities_changed {
                    updates.push((id, serde_json::to_string(&value)?));
                }
            } else if let Some(relative) = storage_relative(&text, source) {
                updates.push((id, utf8_path(&target.join(relative))?.to_owned()));
            }
        }
        let mut write = tx.prepare(&format!("UPDATE {table} SET {column}=?1 WHERE rowid=?2"))?;
        for (id, text) in updates {
            write.execute(params![text, id])?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn storage_relative<'a>(text: &'a str, source: &Path) -> Option<&'a Path> {
    let path = Path::new(text);
    let relative = path.strip_prefix(source).ok()?;
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
        || relative.starts_with("private")
    {
        return None;
    }
    Some(relative)
}

fn rebase_paths(value: &mut Value, source: &Path, target: &Path) -> AppResult<bool> {
    let mut changed = false;
    match value {
        Value::Array(items) => {
            for item in items {
                changed |= rebase_paths(item, source, target)?;
            }
        }
        Value::Object(fields) => {
            for item in fields.values_mut() {
                changed |= rebase_paths(item, source, target)?;
            }
        }
        Value::String(text) => {
            if let Some(relative) = storage_relative(text, source) {
                *text = utf8_path(&target.join(relative))?.to_owned();
                changed = true;
            }
        }
        _ => {}
    }
    Ok(changed)
}

fn rebase_journal_identities(journal: &mut Value, source: &Path, stage: &Path) -> AppResult<bool> {
    let mut changed = false;
    for entry in journal.as_array_mut().into_iter().flatten() {
        for step in entry["renames"].as_array_mut().into_iter().flatten() {
            let Some(relative) = step["to"]
                .as_str()
                .and_then(|path| storage_relative(path, source))
            else {
                continue;
            };
            let expected: FileIdentity = serde_json::from_value(step["identity"].clone())?;
            // Preserve the pre-existing conflict when a file was already replaced.
            if safe_fs::identity(&source.join(relative))?.as_ref() == Some(&expected) {
                step["identity"] = serde_json::to_value(identity(&stage.join(relative))?)?;
                changed = true;
            }
        }
    }
    Ok(changed)
}

#[cfg(test)]
#[path = "storage_location_tests.rs"]
mod tests;
