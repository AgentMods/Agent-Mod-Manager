use crate::{
    error::{AppError, AppResult},
    safe_fs::{self, FileIdentity},
    transaction::{write_lock, FileEngine, FileOp, FilePlan, Fingerprint},
    util,
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::MutexGuard,
    time::SystemTime,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub game_id: String,
    pub label: String,
    pub root_path: PathBuf,
    pub path: PathBuf,
    pub file_count: u64,
    pub total_size: u64,
    pub created_at: String,
    pub transaction_id: String,
    pub files: BTreeMap<PathBuf, Fingerprint>,
}

impl Snapshot {
    pub(crate) fn into_user_paths(mut self) -> Self {
        self.path = crate::paths::user_path(&self.path).into_owned();
        self.root_path = crate::paths::user_path(&self.root_path).into_owned();
        self
    }
}

fn registered_path(conn: &Connection, game_id: &str) -> AppResult<PathBuf> {
    let root: String =
        conn.query_row("SELECT root_path FROM games WHERE id=?1", [game_id], |r| {
            r.get(0)
        })?;
    Ok(root.into())
}

fn registered_root(engine: &FileEngine, game_id: &str) -> AppResult<PathBuf> {
    let root = registered_path(&engine.db.conn(), game_id)?;
    if !root.is_absolute()
        || root.starts_with(&engine.paths.root)
        || engine.paths.root.starts_with(&root)
    {
        return Err(AppError::Denied(
            "快照根必须是绝对路径，且与管理器数据目录不能互相包含".into(),
        ));
    }
    engine.paths.stable_directory(&root)?;
    engine.stable_path(&root, true)?;
    Ok(root)
}

fn confirm_registration(conn: &Connection, game_id: &str, root: &Path) -> AppResult<()> {
    if registered_path(conn, game_id)? != root {
        return Err(AppError::Conflict(
            "游戏登记目录已改变，请重新生成快照计划".into(),
        ));
    }
    Ok(())
}

fn changed(path: &Path) -> AppError {
    AppError::Conflict(format!(
        "生成快照计划时路径已变化: {}",
        crate::paths::display_path(path)
    ))
}

fn directory_state(engine: &FileEngine, path: &Path) -> AppResult<(FileIdentity, SystemTime)> {
    engine.paths.stable_directory(path)?;
    let id = safe_fs::identity(path)?.ok_or_else(|| changed(path))?;
    Ok((id, fs::symlink_metadata(path)?.modified()?))
}

fn empty_directory_fingerprint() -> Fingerprint {
    Fingerprint {
        kind: "dir".into(),
        size: 0,
        // Match the transaction fingerprint for a directory with no descendants.
        sha256: hex::encode(Sha256::digest([])),
    }
}

fn regular_fingerprint(path: &Path) -> AppResult<Fingerprint> {
    let file = safe_fs::open_regular(path)?;
    safe_fs::verify_regular_at(&file, path)?;
    let before = file.metadata()?;
    let mut reader = (&file).take(before.len());
    let sha256 = util::sha256_reader(&mut reader)?;
    let after = file.metadata()?;
    safe_fs::verify_regular_at(&file, path)?;
    if reader.limit() != 0
        || after.len() != before.len()
        || after.modified()? != before.modified()?
    {
        return Err(changed(path));
    }
    Ok(Fingerprint {
        kind: "file".into(),
        size: before.len(),
        sha256,
    })
}

struct Inventory {
    leaves: BTreeMap<PathBuf, Fingerprint>,
    directories: Vec<(PathBuf, (FileIdentity, SystemTime))>,
}

impl Inventory {
    fn read(engine: &FileEngine, root: &PathBuf, retained: &[PathBuf]) -> AppResult<Self> {
        if retained.iter().any(|p| root.starts_with(p)) {
            return Err(AppError::Denied("快照不能扫描事务保留路径".into()));
        }
        let allowed = std::slice::from_ref(root);
        let mut todo = vec![root.clone()];
        let mut leaves = BTreeMap::new();
        let mut directories = Vec::new();
        while let Some(dir) = todo.pop() {
            let state = directory_state(engine, &dir)?;
            let mut has_children = false;
            for entry in safe_fs::directory_entries(&dir, &state.0)? {
                let (name, _) = entry?;
                has_children = true;
                let path = dir.join(name);
                if retained.iter().any(|p| path.starts_with(p)) {
                    continue;
                }
                if path.parent() != Some(dir.as_path())
                    || engine.paths.guard(&path, allowed)? != path
                {
                    return Err(changed(&path));
                }
                let meta = fs::symlink_metadata(&path)?;
                if meta.is_dir() {
                    todo.push(path);
                } else if meta.is_file() {
                    leaves.insert(relative_to(root, &path)?, regular_fingerprint(&path)?);
                } else {
                    return Err(AppError::Denied(format!(
                        "快照不支持链接或特殊文件: {}",
                        crate::paths::display_path(&path)
                    )));
                }
            }
            if !has_children && dir != *root {
                leaves.insert(relative_to(root, &dir)?, empty_directory_fingerprint());
            }
            directories.push((dir, state));
        }
        Ok(Self {
            leaves,
            directories,
        })
    }

    fn confirm_directories(&self, engine: &FileEngine) -> AppResult<()> {
        for (path, before) in &self.directories {
            if &directory_state(engine, path)? != before {
                return Err(changed(path));
            }
        }
        Ok(())
    }
}

fn relative_to(root: &Path, path: &Path) -> AppResult<PathBuf> {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .map_err(|_| AppError::PathEscape(crate::paths::display_path(path)))
}

fn validate_relative(relative: &Path) -> AppResult<()> {
    let text = relative
        .to_str()
        .ok_or_else(|| AppError::Invalid("快照路径必须是有效的 UTF-8".into()))?;
    if crate::paths::safe_join(Path::new(""), text)? != relative {
        return Err(AppError::Invalid(format!(
            "快照路径在恢复时会被改写: {}",
            relative.display()
        )));
    }
    Ok(())
}

fn check_backup_path(engine: &FileEngine, path: &Path) -> AppResult<()> {
    if !path.starts_with(engine.paths.backups()) {
        return Err(AppError::PathEscape(crate::paths::display_path(path)));
    }
    engine.stable_path(path, true)?;
    Ok(())
}

fn verify_snapshot_file(engine: &FileEngine, path: &Path, expected: &Fingerprint) -> AppResult<()> {
    check_backup_path(engine, path)?;
    let matches = match expected.kind.as_str() {
        "file" => regular_fingerprint(path)? == *expected,
        "dir" => {
            let state = directory_state(engine, path)?;
            let empty = safe_fs::directory_entries(path, &state.0)?
                .next()
                .transpose()?
                .is_none();
            empty
                && directory_state(engine, path)? == state
                && empty_directory_fingerprint() == *expected
        }
        _ => false,
    };
    if !matches {
        return Err(AppError::Conflict(format!(
            "快照文件校验失败: {}",
            crate::paths::display_path(path)
        )));
    }
    Ok(())
}

struct CheckedOp<'a> {
    op: FileOp,
    before: Option<&'a Fingerprint>,
    source: Option<&'a Fingerprint>,
}

fn checked_plan(
    engine: &FileEngine,
    guard: &MutexGuard<'static, ()>,
    ops: &[CheckedOp<'_>],
    summary: String,
) -> AppResult<FilePlan> {
    let plan = engine.build_locked_plan(
        guard,
        ops.iter().map(|e| e.op.clone()).collect(),
        summary,
        true,
    )?;
    // Planning resolves paths and fingerprints them again. Persist only if
    // those observations agree with the inventory and the snapshot manifest.
    if plan.entries.len() != ops.len() {
        return Err(AppError::Conflict("快照计划与扫描结果不一致".into()));
    }
    for (actual, expected) in plan.entries.iter().zip(ops) {
        let same_source = match (&actual.operation, &expected.op) {
            (FileOp::Copy { src: a, .. }, FileOp::Copy { src: b, .. })
            | (FileOp::Move { src: a, .. }, FileOp::Move { src: b, .. }) => a == b,
            (FileOp::Mkdir { .. }, FileOp::Mkdir { .. }) => true,
            _ => false,
        };
        if !same_source
            || actual.operation.dst() != expected.op.dst()
            || actual.before.as_ref() != expected.before
            || actual.source.as_ref() != expected.source
        {
            return Err(changed(expected.op.dst()));
        }
    }
    Ok(plan)
}

pub fn plan_create(
    engine: &FileEngine,
    game_id: &str,
    label: &str,
) -> AppResult<(Snapshot, FilePlan)> {
    let guard = write_lock();
    let root = registered_root(engine, game_id)?;
    let inventory = Inventory::read(engine, &root, &engine.retained_paths()?)?;
    let id = util::new_id("snapshot");
    // IDs supplied by a game are never interpolated into filesystem paths.
    let destination = engine.paths.backups().join("snapshots").join(&id);
    let mut ops = Vec::new();
    for (relative, fp) in &inventory.leaves {
        validate_relative(relative)?;
        let dst = destination.join("files").join(relative);
        ops.push(CheckedOp {
            op: if fp.kind == "dir" {
                FileOp::Mkdir { dst }
            } else {
                FileOp::Copy {
                    src: root.join(relative),
                    dst,
                }
            },
            before: None,
            source: (fp.kind == "file").then_some(fp),
        });
    }
    if ops.is_empty() {
        ops.push(CheckedOp {
            op: FileOp::Mkdir {
                dst: destination.join("files"),
            },
            before: None,
            source: None,
        });
    }
    let plan = checked_plan(engine, &guard, &ops, format!("创建快照: {label}"))?;
    inventory.confirm_directories(engine)?;
    let files = inventory.leaves;
    let snapshot = Snapshot {
        id: id.clone(),
        game_id: game_id.into(),
        label: label.into(),
        root_path: root.clone(),
        path: destination.clone(),
        file_count: files.values().filter(|f| f.kind == "file").count() as u64,
        total_size: files.values().map(|f| f.size).sum(),
        created_at: util::now(),
        transaction_id: plan.id.clone(),
        files,
    };
    let mut conn = engine.db.conn();
    let tx = conn.transaction()?;
    confirm_registration(&tx, game_id, &root)?;
    util::atomic_write(
        &destination.join("manifest.json"),
        &serde_json::to_vec_pretty(&snapshot)?,
    )?;
    FileEngine::persist_plan(&tx, &plan, true)?;
    tx.execute("INSERT INTO snapshots (id,game_id,label,root_path,path,file_count,total_size,kind,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'manual',?8)",params![id,game_id,label,root.display().to_string(),destination.display().to_string(),snapshot.file_count as i64,snapshot.total_size as i64,snapshot.created_at])?;
    tx.execute(
        "INSERT INTO snapshot_transactions (snapshot_id,transaction_id) VALUES (?1,?2)",
        params![id, plan.id],
    )?;
    tx.commit()?;
    Ok((snapshot, plan))
}

pub fn list_snapshots(engine: &FileEngine, game_id: &str) -> AppResult<Vec<Snapshot>> {
    let paths = {
        let conn = engine.db.conn();
        let mut stmt=conn.prepare("SELECT s.path FROM snapshots s JOIN snapshot_transactions l ON l.snapshot_id=s.id JOIN file_transactions f ON f.id=l.transaction_id WHERE s.game_id=?1 AND f.status='committed' ORDER BY s.created_at DESC")?;
        let rows = stmt.query_map([game_id], |r| r.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    paths
        .into_iter()
        .map(|p| {
            let directory = PathBuf::from(p);
            let manifest = directory.join("manifest.json");
            check_backup_path(engine, &manifest)?;
            let file = safe_fs::open_regular(&manifest)?;
            safe_fs::verify_regular_at(&file, &manifest)?;
            let snapshot: Snapshot = serde_json::from_reader(std::io::BufReader::new(&file))?;
            safe_fs::verify_regular_at(&file, &manifest)?;
            if snapshot.path != directory || snapshot.game_id != game_id {
                return Err(AppError::Conflict("快照清单与登记记录不一致".into()));
            }
            Ok(snapshot)
        })
        .collect()
}

pub fn plan_restore(engine: &FileEngine, game_id: &str, id: &str) -> AppResult<FilePlan> {
    let guard = write_lock();
    let root = registered_root(engine, game_id)?;
    let snapshot = list_snapshots(engine, game_id)?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| AppError::NotFound(id.into()))?;
    if root != snapshot.root_path {
        return Err(AppError::Conflict(
            "游戏目录已改变，请先确认快照恢复位置".into(),
        ));
    }
    let retained = engine.retained_paths()?;
    let current = Inventory::read(engine, &root, &retained)?;
    let mut ops = Vec::new();
    let restore_id = util::new_id("snapshot-restore");
    let mut replaced = Vec::new();
    // Park additions, including a whole directory if the snapshot expects a
    // file there. Preserve them under backups instead of unlinking them.
    for (relative, fp) in &current.leaves {
        if !snapshot.files.contains_key(relative)
            && !snapshot.files.keys().any(|p| p.starts_with(relative))
        {
            let dst = engine.paths.backups().join(&restore_id).join(relative);
            ops.push(CheckedOp {
                op: FileOp::Move {
                    src: root.join(relative),
                    dst,
                },
                before: None,
                source: Some(fp),
            });
            replaced.push(relative.clone());
        } else if snapshot
            .files
            .get(relative)
            .is_some_and(|old| old.kind != fp.kind)
        {
            return Err(AppError::Conflict(format!(
                "快照路径类型冲突: {}",
                relative.display()
            )));
        }
    }
    for (relative, fp) in &snapshot.files {
        if replaced.iter().any(|p| relative.starts_with(p)) {
            return Err(AppError::Conflict("快照结构与当前目录冲突".into()));
        }
        validate_relative(relative)?;
        let dst = root.join(relative);
        if retained.iter().any(|p| dst.starts_with(p)) {
            return Err(AppError::Conflict(format!(
                "快照不能恢复到事务保留路径: {}",
                relative.display()
            )));
        }
        let source = snapshot.path.join("files").join(relative);
        verify_snapshot_file(engine, &source, fp)?;
        let before = current.leaves.get(relative);
        if before == Some(fp) {
            continue;
        }
        ops.push(CheckedOp {
            op: if fp.kind == "dir" {
                FileOp::Mkdir { dst }
            } else {
                FileOp::Copy { src: source, dst }
            },
            before,
            source: (fp.kind == "file").then_some(fp),
        });
    }
    if ops.is_empty() {
        return Err(AppError::Conflict("游戏目录已经与此快照一致".into()));
    }
    let plan = checked_plan(
        engine,
        &guard,
        &ops,
        format!("恢复快照: {}", snapshot.label),
    )?;
    current.confirm_directories(engine)?;
    let mut conn = engine.db.conn();
    let tx = conn.transaction()?;
    confirm_registration(&tx, game_id, &root)?;
    FileEngine::persist_plan(&tx, &plan, true)?;
    tx.commit()?;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_restores_previous_contents_and_retains_added_files() {
        let (_dir, e, g) = crate::transaction::tests::setup();
        fs::create_dir_all(g.join("nested/empty")).unwrap();
        fs::create_dir_all(g.join(".amm-retained")).unwrap();
        fs::write(g.join(".amm-retained/keep"), b"retained").unwrap();
        fs::write(g.join("a"), b"before").unwrap();
        let (snapshot, plan) = plan_create(&e, "g", "Before update").unwrap();
        assert_eq!(snapshot.files.len(), 2);
        assert_eq!(snapshot.files[Path::new("nested/empty")].kind, "dir");
        assert!(list_snapshots(&e, "g").unwrap().is_empty());
        e.approve_plan(&plan.id).unwrap();
        e.apply_file_ops(&plan.id).unwrap();
        assert_eq!(list_snapshots(&e, "g").unwrap().len(), 1);
        fs::remove_dir_all(g.join("nested")).unwrap();
        fs::write(g.join("a"), b"after").unwrap();
        fs::write(g.join("extra"), b"keep me").unwrap();
        let restore = plan_restore(&e, "g", &snapshot.id).unwrap();
        e.approve_plan(&restore.id).unwrap();
        e.apply_file_ops(&restore.id).unwrap();
        assert_eq!(fs::read(g.join("a")).unwrap(), b"before");
        assert!(!g.join("extra").exists());
        assert!(g.join("nested/empty").is_dir());
        assert_eq!(fs::read(g.join(".amm-retained/keep")).unwrap(), b"retained");
        e.rollback_transaction(&restore.id).unwrap();
        assert_eq!(fs::read(g.join("a")).unwrap(), b"after");
        assert_eq!(fs::read(g.join("extra")).unwrap(), b"keep me");
        assert!(!g.join("nested").exists());
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_create_rejects_a_root_link_to_another_registered_game() {
        let (dir, e, g) = crate::transaction::tests::setup();
        let other = dir.path().join("other");
        fs::create_dir(&other).unwrap();
        fs::write(other.join("mod.txt"), b"other game's data").unwrap();
        let other = crate::games::register_game(&e.db, &other, None).unwrap();
        fs::rename(&g, g.with_file_name("parked-game")).unwrap();
        std::os::unix::fs::symlink(&other.root_path, &g).unwrap();

        assert!(matches!(
            plan_create(&e, "g", "Wrong game"),
            Err(AppError::Denied(_))
        ));
        assert!(!e.paths.backups().join("snapshots").exists());
        let (snapshot, _) = plan_create(&e, &other.id, "Registered game").unwrap();
        assert_eq!(snapshot.root_path, Path::new(&other.root_path));
        assert!(snapshot.files.contains_key(Path::new("mod.txt")));
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_restore_rejects_a_reregistered_link_to_its_old_root() {
        let (_dir, e, g) = crate::transaction::tests::setup();
        fs::write(g.join("mod.txt"), b"old").unwrap();
        let (snapshot, plan) = plan_create(&e, "g", "Before update").unwrap();
        e.approve_plan(&plan.id).unwrap();
        e.apply_file_ops(&plan.id).unwrap();
        fs::write(g.join("mod.txt"), b"new").unwrap();

        let alias = g.with_file_name("alias");
        std::os::unix::fs::symlink(&g, &alias).unwrap();
        e.db.conn()
            .execute(
                "UPDATE games SET root_path=?1 WHERE id='g'",
                [alias.to_str().unwrap()],
            )
            .unwrap();
        crate::games::register_game(&e.db, &g, None).unwrap();

        assert!(matches!(
            plan_restore(&e, "g", &snapshot.id),
            Err(AppError::Denied(_))
        ));
        assert_eq!(fs::read(g.join("mod.txt")).unwrap(), b"new");
        assert_eq!(
            e.db.conn()
                .query_row("SELECT count(*) FROM file_transactions", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn snapshot_plan_rejects_a_source_that_changed_since_inventory() {
        let (_dir, e, g) = crate::transaction::tests::setup();
        let guard = write_lock();
        let src = g.join("mod.txt");
        fs::write(&src, b"old").unwrap();
        let inventory = Inventory::read(&e, &g, &e.retained_paths().unwrap()).unwrap();
        let expected = &inventory.leaves[Path::new("mod.txt")];
        let ops = [CheckedOp {
            op: FileOp::Copy {
                src: src.clone(),
                dst: e.paths.backups().join("mod.txt"),
            },
            before: None,
            source: Some(expected),
        }];
        assert!(checked_plan(&e, &guard, &ops, "Control".into()).is_ok());
        fs::write(&src, b"new").unwrap();
        assert!(matches!(
            checked_plan(&e, &guard, &ops, "Changed".into()),
            Err(AppError::Conflict(_))
        ));
    }

    #[test]
    fn snapshot_inventory_detects_replacement_of_an_empty_directory() {
        let (_dir, e, g) = crate::transaction::tests::setup();
        let empty = g.join("empty");
        fs::create_dir(&empty).unwrap();
        let inventory = Inventory::read(&e, &g, &e.retained_paths().unwrap()).unwrap();
        assert!(inventory.confirm_directories(&e).is_ok());
        fs::rename(&empty, g.join("parked")).unwrap();
        fs::create_dir(&empty).unwrap();
        assert!(matches!(
            inventory.confirm_directories(&e),
            Err(AppError::Conflict(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_create_rejects_a_name_that_restore_would_reinterpret() {
        let (_dir, e, g) = crate::transaction::tests::setup();
        fs::write(g.join(r"mod\file.txt"), b"payload").unwrap();

        assert!(matches!(
            plan_create(&e, "g", "Ambiguous path"),
            Err(AppError::Invalid(_))
        ));
        assert!(!e.paths.backups().join("snapshots").exists());
    }
}
