use crate::{
    db::Db,
    error::{AppError, AppResult},
    paths, safe_fs,
    safe_fs::FileIdentity,
    storage::AppPaths,
    util,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, OnceLock},
};

// Stored only by the native desktop Agent planner, never taken from tool input.
const LOCAL_AGENT_FILES: &str = "local_agent_files";

static WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
pub(crate) fn write_lock() -> MutexGuard<'static, ()> {
    lock()
}

fn lock() -> MutexGuard<'static, ()> {
    WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FileOp {
    Copy { src: PathBuf, dst: PathBuf },
    Move { src: PathBuf, dst: PathBuf },
    Mkdir { dst: PathBuf },
    WriteText { dst: PathBuf, content: String },
}

impl FileOp {
    pub fn dst(&self) -> &Path {
        match self {
            Self::Copy { dst, .. }
            | Self::Move { dst, .. }
            | Self::Mkdir { dst }
            | Self::WriteText { dst, .. } => dst,
        }
    }
    pub(crate) fn src(&self) -> Option<&Path> {
        match self {
            Self::Copy { src, .. } | Self::Move { src, .. } => Some(src),
            _ => None,
        }
    }
    fn set_user_paths(&mut self) {
        let dst = match self {
            Self::Copy { src, dst } | Self::Move { src, dst } => {
                *src = paths::user_path(src).into_owned();
                dst
            }
            Self::Mkdir { dst } | Self::WriteText { dst, .. } => dst,
        };
        *dst = paths::user_path(dst).into_owned();
    }
    fn name(&self) -> &'static str {
        match self {
            Self::Copy { .. } => "copy",
            Self::Move { .. } => "move",
            Self::Mkdir { .. } => "mkdir",
            Self::WriteText { .. } => "write_text",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Fingerprint {
    pub kind: String,
    pub size: u64,
    pub sha256: String,
}

pub(crate) fn fingerprint(path: &Path) -> AppResult<Option<Fingerprint>> {
    fingerprint_checked(path, false)
}

fn fingerprint_checked(path: &Path, protect_descendants: bool) -> AppResult<Option<Fingerprint>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if meta.file_type().is_symlink() || (!meta.is_file() && !meta.is_dir()) {
        return Err(AppError::Denied(format!(
            "不支持链接或特殊文件: {}",
            paths::display_path(path)
        )));
    }
    if meta.is_file() {
        return Ok(Some(Fingerprint {
            kind: "file".into(),
            size: meta.len(),
            sha256: util::sha256_file(path)?,
        }));
    }
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    let mut size = 0;
    let mut entries = walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by(|a, b| a.path().cmp(b.path()));
    for entry in entries.into_iter().skip(1) {
        let rel = entry
            .path()
            .strip_prefix(path)
            .map_err(|e| AppError::Invalid(e.to_string()))?;
        if protect_descendants
            && (AppPaths::env_name(entry.file_name())
                || entry.file_name().eq_ignore_ascii_case(".amm-retained"))
        {
            return Err(AppError::Denied(
                "目录含受保护的 .env 文件或事务保留目录".into(),
            ));
        }
        let fp = fingerprint_leaf(entry.path())?;
        digest.update(serde_json::to_vec(&(rel, &fp))?);
        size += fp.size;
    }
    Ok(Some(Fingerprint {
        kind: "dir".into(),
        size,
        sha256: hex::encode(digest.finalize()),
    }))
}

fn fingerprint_leaf(path: &Path) -> AppResult<Fingerprint> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || (!meta.is_file() && !meta.is_dir()) {
        return Err(AppError::Denied(format!(
            "目录包含链接或特殊文件: {}",
            paths::display_path(path)
        )));
    }
    Ok(Fingerprint {
        kind: if meta.is_dir() { "dir" } else { "file" }.into(),
        size: if meta.is_file() { meta.len() } else { 0 },
        sha256: if meta.is_file() {
            util::sha256_file(path)?
        } else {
            String::new()
        },
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedEntry {
    pub operation: FileOp,
    pub before: Option<Fingerprint>,
    pub source: Option<Fingerprint>,
    pub impact: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilePlan {
    pub id: String,
    pub summary: String,
    pub entries: Vec<PlannedEntry>,
    pub total_bytes: u64,
    pub created_at: String,
    #[serde(default)]
    pub requires_review: bool,
}

impl FilePlan {
    /// Format operation paths for tool results, IPC responses, and approval previews.
    pub fn into_user_paths(mut self) -> Self {
        for entry in &mut self.entries {
            entry.operation.set_user_paths();
        }
        self
    }
}

/// Serialize a plan embedded in a display DTO. Stored plans use ordinary serialization.
pub(crate) fn serialize_user_plan<S: serde::Serializer>(
    plan: &FilePlan,
    s: S,
) -> Result<S::Ok, S::Error> {
    #[cfg(windows)]
    let plan = plan.clone().into_user_paths();
    plan.serialize(s)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalEntry {
    entry: PlannedEntry,
    backup: PathBuf,
    undo: PathBuf,
    #[serde(default)]
    source_park: Option<PathBuf>,
    #[serde(default)]
    overwrite_park: Option<PathBuf>,
    created_dirs: Vec<PathBuf>,
    state: String,
    // None denotes a legacy row, not a new entry with no recorded mutations.
    #[serde(default)]
    renames: Option<Vec<JournalRename>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JournalRename {
    from: PathBuf,
    to: PathBuf,
    identity: FileIdentity,
    // New outputs are retained instead of returned to their staging path.
    retain_at: Option<PathBuf>,
    empty_dir_only: bool,
    expected_content: Option<Fingerprint>,
}

impl JournalRename {
    fn move_to(from: PathBuf, to: PathBuf, identity: FileIdentity) -> Self {
        Self {
            from,
            to,
            identity,
            retain_at: None,
            empty_dir_only: false,
            expected_content: None,
        }
    }

    fn with_content(mut self, expected: &Option<Fingerprint>) -> Self {
        self.expected_content.clone_from(expected);
        self
    }
}

fn ownership_conflict(path: &Path) -> AppError {
    AppError::Conflict(format!(
        "不能确认事务文件的身份，已保留原位等待恢复: {}",
        paths::display_path(path)
    ))
}

fn legacy_recovery_conflict() -> AppError {
    AppError::Conflict("旧版事务日志没有记录文件身份，不能自动回滚，请人工检查原文件和备份".into())
}

fn require_identity(path: &Path) -> AppResult<FileIdentity> {
    safe_fs::identity(path)?.ok_or_else(|| ownership_conflict(path))
}

fn check_fingerprint(path: &Path, expected: &Option<Fingerprint>) -> AppResult<()> {
    if fingerprint(path)? != *expected {
        return Err(AppError::Conflict(format!(
            "事务文件内容已变化: {}",
            paths::display_path(path)
        )));
    }
    Ok(())
}

fn create_parent(path: &Path) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::PathEscape(paths::display_path(path)))?;
    safe_fs::create_dir_all(parent)
}

struct ApplyEvidence {
    original: Option<FileIdentity>,
    source: Option<FileIdentity>,
}

#[cfg(test)]
type RenameHook = Box<dyn Fn(&JournalRename) + Send + Sync>;

#[derive(Clone)]
pub struct FileEngine {
    pub db: Db,
    pub paths: Arc<AppPaths>,
    local_agent_files: bool,
    #[cfg(test)]
    fail_before: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    cross_device: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    before_rename: Arc<Mutex<Option<RenameHook>>>,
}

impl FileEngine {
    pub fn new(db: Db, paths: AppPaths) -> Self {
        Self {
            db,
            paths: Arc::new(paths),
            local_agent_files: false,
            #[cfg(test)]
            fail_before: Arc::new(std::sync::atomic::AtomicUsize::new(usize::MAX)),
            #[cfg(test)]
            cross_device: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            #[cfg(test)]
            before_rename: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn for_local_agent(&self) -> Self {
        Self {
            local_agent_files: true,
            ..self.clone()
        }
    }

    /// Apply and undo use the recorded scope, never broader authority just because
    /// today's caller is the desktop Agent. Old/managed plans stay managed.
    fn for_transaction(&self, kind: &str) -> AppResult<Self> {
        if kind == LOCAL_AGENT_FILES && !self.local_agent_files {
            return Err(AppError::Denied(
                "此文件事务只能由桌面 Agent 或本地用户操作".into(),
            ));
        }
        Ok(Self {
            local_agent_files: kind == LOCAL_AGENT_FILES,
            ..self.clone()
        })
    }

    pub(crate) fn resolve_file_path(&self, path: &Path) -> AppResult<PathBuf> {
        self.paths
            .guard_file_path(path, &self.roots()?, self.local_agent_files)
    }

    #[cfg(test)]
    pub(crate) fn inject_failure_before(&self, index: usize) {
        self.fail_before
            .store(index, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn roots(&self) -> AppResult<Vec<PathBuf>> {
        let conn = self.db.conn();
        let mut stmt =
            conn.prepare("SELECT root_path FROM games UNION SELECT path FROM game_user_roots")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0).map(PathBuf::from))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub(crate) fn has_external_paths(&self, plan: &FilePlan) -> AppResult<bool> {
        let roots = self.paths.managed_roots(&self.roots()?);
        Ok(plan.entries.iter().any(|entry| {
            std::iter::once(entry.operation.dst())
                .chain(entry.operation.src())
                .any(|path| !roots.iter().any(|root| path.starts_with(root)))
        }))
    }

    pub fn retained_paths(&self) -> AppResult<Vec<PathBuf>> {
        let mut paths: Vec<PathBuf> = self
            .roots()?
            .iter()
            .map(|r| r.join(".amm-retained"))
            .collect();
        let conn = self.db.conn();
        let mut stmt = conn.prepare("SELECT journal_json FROM file_transactions")?;
        for json in stmt.query_map([], |r| r.get::<_, String>(0))? {
            for j in serde_json::from_str::<Vec<JournalEntry>>(&json?)? {
                paths.push(j.undo);
                paths.extend(j.source_park);
                paths.extend(j.overwrite_park);
            }
        }
        Ok(paths)
    }

    fn retention_location(&self, path: &Path, id: &str, name: &str) -> AppResult<PathBuf> {
        let roots = self.paths.managed_roots(&self.roots()?);
        let root = roots
            .iter()
            .filter(|r| path.starts_with(r))
            .max_by_key(|r| r.components().count());
        match root {
            Some(r) if r == &self.paths.root => Ok(self.paths.backup_dir(id).join(name)),
            Some(r) => Ok(r.join(".amm-retained").join(id).join(name)),
            // Place staging/undo on the target volume, not at a drive root (which
            // need not be writable) or inside a directory the plan will create.
            None if self.local_agent_files => path
                .ancestors()
                .skip(1)
                .find(|p| p.is_dir())
                .map(|p| p.join(".amm-retained").join(id).join(name))
                .ok_or_else(|| AppError::Invalid("外部路径没有可用的父目录".into())),
            None => Err(AppError::PathEscape(paths::display_path(path))),
        }
    }

    fn guard(&self, path: &Path, internal: bool) -> AppResult<PathBuf> {
        let roots = self.roots()?;
        let target = self
            .paths
            .guard_file_path(path, &roots, self.local_agent_files)?;
        if !internal
            && self.local_agent_files
            && !target.starts_with(&self.paths.root)
            && self.paths.overlaps_storage(&target)
        {
            return Err(AppError::Denied(
                "不能移动或覆盖管理器存储目录及其上级目录".into(),
            ));
        }
        if !internal
            && (target.starts_with(self.paths.downloads())
                || target.starts_with(self.paths.staging()))
            && target
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| {
                    let n = n.to_ascii_lowercase();
                    n.ends_with(".part") || n.ends_with(".rejected")
                })
        {
            return Err(AppError::Denied(
                "未完成或隔离的载荷只能通过回收站工具移走，不能部署".into(),
            ));
        }
        if !internal
            && target
                .components()
                .any(|c| c.as_os_str().eq_ignore_ascii_case(".amm-retained"))
        {
            return Err(AppError::Denied("不能修改事务保留目录".into()));
        }
        if !internal && target.starts_with(&self.paths.root) {
            let allowed = [
                self.paths.staging(),
                self.paths.downloads(),
                self.paths.plugins(),
                self.paths.skills(),
            ];
            if !allowed
                .iter()
                .any(|r| target.starts_with(r) && target != *r)
            {
                return Err(AppError::Denied(format!(
                    "管理器内部路径不可由文件工具修改: {}",
                    paths::display_path(&target)
                )));
            }
        }
        if !internal
            && roots
                .iter()
                .filter_map(|r| paths::canonicalize_lenient(r).ok())
                .any(|root| root.starts_with(&target))
        {
            return Err(AppError::Denied("不能移动或覆盖已登记的游戏根目录".into()));
        }
        Ok(target)
    }

    pub(crate) fn stable_path(&self, path: &Path, internal: bool) -> AppResult<PathBuf> {
        let resolved = self.guard(path, internal)?;
        if resolved != path {
            return Err(AppError::Conflict(format!(
                "计划路径的链接或父目录已改变: {}",
                paths::display_path(path)
            )));
        }
        Ok(resolved)
    }

    pub fn plan_file_ops(&self, ops: Vec<FileOp>, summary: String) -> AppResult<FilePlan> {
        self.plan_locked(ops, summary, false)
    }

    pub(crate) fn plan_internal(&self, ops: Vec<FileOp>, summary: String) -> AppResult<FilePlan> {
        self.plan_locked(ops, summary, true)
    }

    fn plan_locked(
        &self,
        ops: Vec<FileOp>,
        summary: String,
        internal: bool,
    ) -> AppResult<FilePlan> {
        let guard = lock();
        let mut plan = self.build_locked_plan(&guard, ops, summary, internal)?;
        if self.local_agent_files
            && !internal
            && (plan
                .entries
                .iter()
                .any(|e| matches!(e.operation, FileOp::WriteText { .. }))
                || self.has_external_paths(&plan)?)
        {
            plan.requires_review = true;
            Self::persist_plan_kind(&self.db.conn(), &plan, LOCAL_AGENT_FILES)?;
        } else {
            Self::persist_plan(&self.db.conn(), &plan, internal)?;
        }
        Ok(plan)
    }

    pub(crate) fn build_locked_plan(
        &self,
        _guard: &MutexGuard<'static, ()>,
        ops: Vec<FileOp>,
        summary: String,
        internal: bool,
    ) -> AppResult<FilePlan> {
        let interrupted:i64=self.db.conn().query_row("SELECT count(*) FROM file_transactions WHERE status IN ('applying','rolling_back','recovery_failed')",[],|r|r.get(0))?;
        if interrupted > 0 {
            return Err(AppError::Denied(
                "存在待恢复的文件事务，请先恢复一致状态".into(),
            ));
        }
        if ops.is_empty() || ops.len() > 100_000 {
            return Err(AppError::Invalid(
                "文件操作数量应在 1 到 100000 之间".into(),
            ));
        }
        let mut entries = Vec::new();
        // Inline text is part of the immutable plan and journal, so bound the total,
        // not just each entry. No staging file is written while planning.
        let mut text_bytes = 0usize;
        let mut total_bytes = 0;
        let mut touched: Vec<PathBuf> = Vec::new();
        for op in ops {
            let inline_source = if let FileOp::WriteText { content, .. } = &op {
                if !self.local_agent_files || internal {
                    return Err(AppError::Denied(
                        "只有桌面 Agent 的通用文件计划支持 write_text".into(),
                    ));
                }
                text_bytes = text_bytes.saturating_add(content.len());
                if text_bytes > 1024 * 1024 {
                    return Err(AppError::Invalid("单个计划的文本内容不能超过 1 MiB".into()));
                }
                Some(Fingerprint {
                    kind: "file".into(),
                    size: content.len() as u64,
                    sha256: util::sha256_reader(&mut content.as_bytes())?,
                })
            } else {
                None
            };
            let dst = self.guard(op.dst(), internal)?;
            let src = op.src().map(|p| self.guard(p, internal)).transpose()?;
            let before = fingerprint(&dst)?;
            let source = src
                .as_ref()
                .map(|s| {
                    fingerprint_checked(s, self.local_agent_files && !internal)?
                        .ok_or_else(|| AppError::NotFound(paths::display_path(s)))
                })
                .transpose()?
                .or(inline_source);
            if let Some(src) = &src {
                if dst.starts_with(src) || src.starts_with(&dst) {
                    return Err(AppError::Conflict("源路径与目标路径不能互相包含".into()));
                }
            }
            if matches!(op, FileOp::Copy { .. })
                && source.as_ref().is_some_and(|s| s.kind != "file")
            {
                return Err(AppError::Invalid("目录复制必须先展开成逐文件计划".into()));
            }
            if matches!(op, FileOp::Mkdir { .. }) && before.is_some() {
                return Err(AppError::Conflict(format!(
                    "目录已经存在: {}",
                    paths::display_path(&dst)
                )));
            }
            if before.as_ref().is_some_and(|s| s.kind == "dir") {
                return Err(AppError::Conflict(
                    "不能覆盖整个目录，请使用逐文件计划".into(),
                ));
            }
            let mut modified = vec![dst.clone()];
            if matches!(op, FileOp::Move { .. }) {
                modified.extend(src.iter().cloned());
            }
            for p in &modified {
                if touched.iter().any(|q| p.starts_with(q) || q.starts_with(p)) {
                    return Err(AppError::Conflict(
                        "同一计划不能重复修改或包含另一个操作路径".into(),
                    ));
                }
            }
            touched.extend(modified);
            let operation = match op {
                FileOp::Copy { .. } => FileOp::Copy {
                    src: src.unwrap(),
                    dst,
                },
                FileOp::Move { .. } => FileOp::Move {
                    src: src.unwrap(),
                    dst,
                },
                FileOp::Mkdir { .. } => FileOp::Mkdir { dst },
                FileOp::WriteText { content, .. } => FileOp::WriteText { dst, content },
            };
            total_bytes += source.as_ref().map_or(0, |s| s.size);
            let impact = if before.is_some() {
                "overwrite"
            } else if matches!(operation, FileOp::Move { .. }) {
                "move"
            } else {
                "add"
            }
            .into();
            entries.push(PlannedEntry {
                operation,
                before,
                source,
                impact,
            });
        }
        // A source may not be written by a different entry in the same plan.
        for entry in &entries {
            if let Some(src) = entry.operation.src() {
                if entries.iter().any(|e| {
                    e.operation.dst().starts_with(src) || src.starts_with(e.operation.dst())
                }) {
                    return Err(AppError::Conflict("复制源不能同时被本计划修改".into()));
                }
            }
        }
        let requires_review = entries.iter().any(|entry| {
            entry.operation.src().is_some_and(|src| {
                src.starts_with(self.paths.downloads()) || src.starts_with(self.paths.staging())
            })
        });
        Ok(FilePlan {
            id: util::new_id("txn"),
            summary,
            entries,
            total_bytes,
            created_at: util::now(),
            requires_review,
        })
    }

    pub(crate) fn persist_plan(
        conn: &rusqlite::Connection,
        plan: &FilePlan,
        internal: bool,
    ) -> AppResult<()> {
        Self::persist_plan_kind(conn, plan, if internal { "internal" } else { "files" })
    }

    fn persist_plan_kind(
        conn: &rusqlite::Connection,
        plan: &FilePlan,
        kind: &str,
    ) -> AppResult<()> {
        conn.execute(
            "INSERT INTO file_transactions (id,kind,status,summary,plan_json,created_at) VALUES (?1,?2,'planned',?3,?4,?5)",
            params![plan.id, kind, plan.summary, serde_json::to_string(plan)?, plan.created_at],
        )?;
        Ok(())
    }

    pub(crate) fn require_review(&self, id: &str) -> AppResult<FilePlan> {
        let _guard = lock();
        let mut plan = self.get_plan(id)?;
        plan.requires_review = true;
        let updated = self.db.conn().execute(
            "UPDATE file_transactions SET plan_json=?1 WHERE id=?2 AND status='planned'",
            params![serde_json::to_string(&plan)?, id],
        )?;
        if updated != 1 {
            return Err(AppError::Conflict(
                "只能为尚未批准的计划设置审查要求".into(),
            ));
        }
        Ok(plan)
    }

    pub fn get_plan(&self, id: &str) -> AppResult<FilePlan> {
        let value: String = self
            .db
            .conn()
            .query_row(
                "SELECT plan_json FROM file_transactions WHERE id=?1 AND (kind != ?2 OR ?3)",
                params![id, LOCAL_AGENT_FILES, self.local_agent_files],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| AppError::NotFound(id.into()))?;
        Ok(serde_json::from_str(&value)?)
    }

    /// Called only by the trusted approval coordinator, never registered as a tool.
    pub fn approve_plan(&self, id: &str) -> AppResult<()> {
        let changed = self.db.conn().execute(
            "UPDATE file_transactions SET status='approved' WHERE id=?1 AND status='planned' AND (kind != ?2 OR ?3)",
            params![id, LOCAL_AGENT_FILES, self.local_agent_files],
        )?;
        if changed != 1 {
            return Err(AppError::Conflict("计划不存在或不再等待审批".into()));
        }
        Ok(())
    }

    pub fn apply_file_ops(&self, id: &str) -> AppResult<FilePlan> {
        self.apply_locked(&lock(), id)
    }

    pub(crate) fn apply_locked(
        &self,
        _guard: &MutexGuard<'static, ()>,
        id: &str,
    ) -> AppResult<FilePlan> {
        let interrupted:i64=self.db.conn().query_row("SELECT count(*) FROM file_transactions WHERE status IN ('applying','rolling_back','recovery_failed')",[],|r|r.get(0))?;
        if interrupted > 0 {
            return Err(AppError::Denied(
                "存在待恢复的文件事务，暂时不能执行新变更".into(),
            ));
        }
        let (status, kind): (String, String) = self.db.conn().query_row(
            "SELECT status,kind FROM file_transactions WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if status != "approved" {
            return Err(AppError::Denied("计划尚未批准，或已执行".into()));
        }
        self.for_transaction(&kind)?
            .apply_approved(id, kind == "internal")
    }

    fn apply_approved(&self, id: &str, internal: bool) -> AppResult<FilePlan> {
        let plan = self.get_plan(id)?;
        crate::install::validate_apply(self, &plan)?;
        crate::plugin_workflow::validate_deployment_source(self, &plan)?;
        let mut evidence = Vec::with_capacity(plan.entries.len());
        for e in &plan.entries {
            self.stable_path(e.operation.dst(), internal)?;
            let original = e
                .before
                .as_ref()
                .map(|_| require_identity(e.operation.dst()))
                .transpose()?;
            if fingerprint(e.operation.dst())? != e.before {
                return Err(AppError::Conflict(
                    "预览之后目标文件已变化，请重新生成计划".into(),
                ));
            }
            let mut source = None;
            if let Some(src) = e.operation.src() {
                self.stable_path(src, internal)?;
                if matches!(e.operation, FileOp::Move { .. }) {
                    source = Some(require_identity(src)?);
                }
                if fingerprint(src)? != e.source {
                    return Err(AppError::Conflict(
                        "预览之后源文件已变化，请重新生成计划".into(),
                    ));
                }
            }
            evidence.push(ApplyEvidence { original, source });
        }
        let mut journal = self.prepare_journal(&plan)?;
        self.save_journal(id, "applying", &journal)?;
        for (index, (entry, evidence)) in journal.iter_mut().zip(&evidence).enumerate() {
            entry.state = "intent".into();
            self.save_entry(id, index, entry)?;
            #[cfg(test)]
            let result = if self.fail_before.load(std::sync::atomic::Ordering::Relaxed) == index {
                Err(AppError::Io(std::io::Error::other(
                    "injected filesystem failure",
                )))
            } else {
                self.execute_entry(id, index, entry, evidence)
            };
            #[cfg(not(test))]
            let result = self.execute_entry(id, index, entry, evidence);
            match result {
                Ok(()) => {
                    entry.state = "done".into();
                    self.save_entry(id, index, entry)?;
                }
                Err(error) => {
                    self.db.conn().execute(
                        "UPDATE file_transactions SET error=?1 WHERE id=?2",
                        params![error.to_string(), id],
                    )?;
                    self.rollback_locked(id, &mut journal)?;
                    return Err(error);
                }
            }
        }
        self.save_journal(id, "committed", &journal)?;
        Ok(plan)
    }

    fn prepare_journal(&self, plan: &FilePlan) -> AppResult<Vec<JournalEntry>> {
        let id = &plan.id;
        let journal: Vec<JournalEntry> = plan
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let mut created_dirs = Vec::new();
                let mut parent = e.operation.dst().parent();
                while let Some(p) = parent {
                    if p.exists() {
                        break;
                    }
                    created_dirs.push(p.to_path_buf());
                    parent = p.parent();
                }
                Ok(JournalEntry {
                    entry: e.clone(),
                    backup: self.paths.backup_dir(id).join(format!("{i}.original")),
                    undo: self.retention_location(e.operation.dst(), id, &format!("{i}-undone"))?,
                    source_park: if matches!(e.operation, FileOp::Move { .. }) {
                        e.operation
                            .src()
                            .map(|src| self.retention_location(src, id, &format!("{i}-source")))
                            .transpose()?
                    } else {
                        None
                    },
                    overwrite_park: e
                        .before
                        .as_ref()
                        .map(|_| {
                            self.retention_location(e.operation.dst(), id, &format!("{i}-original"))
                        })
                        .transpose()?,
                    created_dirs,
                    state: "pending".into(),
                    renames: Some(Vec::new()),
                })
            })
            .collect::<AppResult<Vec<_>>>()?;
        Ok(journal)
    }

    fn execute_entry(
        &self,
        id: &str,
        index: usize,
        journal: &mut JournalEntry,
        evidence: &ApplyEvidence,
    ) -> AppResult<()> {
        let op = journal.entry.operation.clone();
        self.stable_path(op.dst(), true)?;
        self.stable_path(&journal.backup, true)?;
        self.stable_path(&journal.undo, true)?;
        if let Some(src) = op.src() {
            self.stable_path(src, true)?;
        }
        let work = journal.undo.with_extension("work");
        self.stable_path(&work, true)?;
        let mut work_created = false;
        let mut staging = |name: &str| -> AppResult<PathBuf> {
            if !work_created {
                safe_fs::create_dir(&work)?;
                work_created = true;
            }
            Ok(work.join(name))
        };
        for (i, dir) in journal.created_dirs.clone().into_iter().enumerate().rev() {
            self.stable_path(&dir, true)?;
            // Another entry may already have created this parent. Do not claim it twice.
            if dir.is_dir() {
                continue;
            }
            let staged = staging(&format!("parent-{i}"))?;
            safe_fs::create_dir(&staged)?;
            let mut step = JournalRename::move_to(staged.clone(), dir, require_identity(&staged)?);
            step.empty_dir_only = true;
            self.record_rename(id, index, journal, step, false)?;
        }
        if let Some(identity) = &evidence.original {
            let backup = journal.backup.clone();
            create_parent(&backup)?;
            check_fingerprint(op.dst(), &journal.entry.before)?;
            let step = JournalRename::move_to(op.dst().into(), backup.clone(), identity.clone())
                .with_content(&journal.entry.before);
            if !self.record_rename(id, index, journal, step, true)? {
                let park = journal
                    .overwrite_park
                    .clone()
                    .ok_or_else(|| AppError::Invalid("缺少覆盖保留路径".into()))?;
                self.stable_path(&park, true)?;
                create_parent(&park)?;
                let partial = backup.with_extension(util::new_id("partial"));
                util::atomic_copy(op.dst(), &partial)?;
                check_fingerprint(&partial, &journal.entry.before)?;
                // This verified mirror is not used to decide what can be moved back.
                util::move_path(&partial, &backup)?;
                let step = JournalRename::move_to(op.dst().into(), park, identity.clone())
                    .with_content(&journal.entry.before);
                self.record_rename(id, index, journal, step, false)?;
            }
        }
        if let FileOp::Move { src, dst } = &op {
            let identity = evidence
                .source
                .clone()
                .ok_or_else(|| AppError::Conflict("缺少移动源身份".into()))?;
            check_fingerprint(src, &journal.entry.source)?;
            let step = JournalRename::move_to(src.clone(), dst.clone(), identity)
                .with_content(&journal.entry.source);
            if self.record_rename(id, index, journal, step, true)? {
                return Ok(());
            }
        }
        let staged = staging("payload")?;
        match &op {
            FileOp::Copy { src, .. } => util::atomic_copy(src, &staged)?,
            FileOp::Move { src, .. } if src.is_dir() => util::copy_dir(src, &staged)?,
            FileOp::Move { src, .. } => util::atomic_copy(src, &staged)?,
            FileOp::Mkdir { .. } => safe_fs::create_dir(&staged)?,
            FileOp::WriteText { content, .. } => {
                use std::io::Write;
                let mut output = safe_fs::create_new(&staged)?;
                output.write_all(content.as_bytes())?;
                output.sync_all()?;
            }
        }
        let mut step =
            JournalRename::move_to(staged.clone(), op.dst().into(), require_identity(&staged)?);
        if !matches!(op, FileOp::Mkdir { .. }) {
            check_fingerprint(&staged, &journal.entry.source)?;
            if matches!(op, FileOp::Move { .. } | FileOp::WriteText { .. }) {
                step.expected_content.clone_from(&journal.entry.source);
            }
        } else {
            step.empty_dir_only = true;
        }
        step.retain_at = Some(journal.undo.clone());
        self.record_rename(id, index, journal, step, false)?;
        if let FileOp::Move { src, .. } = op {
            let park = journal
                .source_park
                .clone()
                .ok_or_else(|| AppError::Invalid("缺少跨磁盘保留路径".into()))?;
            self.stable_path(&park, true)?;
            create_parent(&park)?;
            check_fingerprint(&src, &journal.entry.source)?;
            let step = JournalRename::move_to(
                src.clone(),
                park,
                evidence
                    .source
                    .clone()
                    .ok_or_else(|| AppError::Conflict("缺少移动源身份".into()))?,
            )
            .with_content(&journal.entry.source);
            self.record_rename(id, index, journal, step, false)?;
        }
        Ok(())
    }

    fn record_rename(
        &self,
        id: &str,
        index: usize,
        journal: &mut JournalEntry,
        step: JournalRename,
        allow_cross_volume: bool,
    ) -> AppResult<bool> {
        self.stable_path(&step.from, true)?;
        self.stable_path(&step.to, true)?;
        journal
            .renames
            .as_mut()
            .expect("new journal")
            .push(step.clone());
        self.save_entry(id, index, journal)?;
        #[cfg(test)]
        if let Some(hook) = self.before_rename.lock().unwrap().as_ref() {
            hook(&step);
        }
        #[cfg(test)]
        let inject_exdev =
            allow_cross_volume && self.cross_device.load(std::sync::atomic::Ordering::Relaxed);
        #[cfg(not(test))]
        let inject_exdev = false;
        let result = if inject_exdev {
            Err(AppError::Io(std::io::Error::from(
                std::io::ErrorKind::CrossesDevices,
            )))
        } else {
            safe_fs::rename_checked(&step.from, &step.to, &step.identity)
        };
        match result {
            Ok(()) => {
                if safe_fs::identity(&step.to)?.as_ref() != Some(&step.identity) {
                    return Err(ownership_conflict(&step.to));
                }
                Ok(true)
            }
            Err(AppError::Io(error))
                if allow_cross_volume && error.kind() == std::io::ErrorKind::CrossesDevices =>
            {
                journal.renames.as_mut().expect("new journal").pop();
                self.save_entry(id, index, journal)?;
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    fn save_entry(&self, id: &str, index: usize, entry: &JournalEntry) -> AppResult<()> {
        let mut conn = self.db.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE file_transactions SET journal_json=json_set(journal_json,?1,json(?2)) WHERE id=?3",
            params![format!("$[{index}]"), serde_json::to_string(entry)?, id],
        )?;
        tx.execute(
            "UPDATE file_transaction_entries SET result=?1 WHERE transaction_id=?2 AND seq=?3",
            params![entry.state, id, index as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn save_journal(&self, id: &str, status: &str, journal: &[JournalEntry]) -> AppResult<()> {
        let mut conn = self.db.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE file_transactions SET status=?1,journal_json=?2,finished_at=?3 WHERE id=?4",
            params![
                status,
                serde_json::to_string(journal)?,
                if matches!(status, "committed" | "rolled_back") {
                    Some(util::now())
                } else {
                    None
                },
                id
            ],
        )?;
        for (i, j) in journal.iter().enumerate() {
            tx.execute(
                "DELETE FROM file_transaction_entries WHERE transaction_id=?1 AND seq=?2",
                params![id, i as i64],
            )?;
            tx.execute("INSERT INTO file_transaction_entries (transaction_id,seq,op,src,dst,backup_path,result) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![id,i as i64,j.entry.operation.name(),j.entry.operation.src().map(|p|p.display().to_string()),j.entry.operation.dst().display().to_string(),j.backup.display().to_string(),j.state])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn rollback_transaction(&self, id: &str) -> AppResult<()> {
        let _guard = lock();
        let purged: bool = self.db.conn().query_row("SELECT EXISTS(SELECT 1 FROM trash_items t JOIN trash_purges p ON p.item_id=t.id WHERE t.transaction_id=?1)",[id],|r|r.get(0))?;
        if purged {
            return Err(AppError::Conflict(
                "此事务包含用户已确认永久清空的回收站项，不能回滚".into(),
            ));
        }
        let (status, kind, json): (String, String, String) = self.db.conn().query_row(
            "SELECT status,kind,journal_json FROM file_transactions WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let engine = self.for_transaction(&kind)?;
        if status == "rolled_back" {
            return Ok(());
        }
        if !matches!(
            status.as_str(),
            "committed" | "applying" | "rolling_back" | "recovery_failed"
        ) {
            return Err(AppError::Conflict("此事务尚未开始执行".into()));
        }
        let mut journal: Vec<JournalEntry> = serde_json::from_str(&json)?;
        if status == "committed" {
            if crate::install::validate_rollback(&engine, id)? {
                engine.validate_replacement_rollback_payloads(&journal)?;
            }
            engine.validate_committed_rollback(&journal)?;
        }
        engine.rollback_locked(id, &mut journal)
    }

    fn validate_committed_rollback(&self, journal: &[JournalEntry]) -> AppResult<()> {
        // Reject stale or out-of-order requests before moving anything or entering recovery.
        let mut vacated: HashSet<&Path> = HashSet::new();
        for entry in journal.iter().rev() {
            let steps = entry
                .renames
                .as_ref()
                .ok_or_else(legacy_recovery_conflict)?;
            for step in steps.iter().rev() {
                let to = self.stable_path(&step.to, true)?;
                let return_to = step.retain_at.as_deref().unwrap_or(&step.from);
                self.stable_path(return_to, true)?;
                if safe_fs::identity(&to)?.as_ref() != Some(&step.identity) {
                    return Err(ownership_conflict(&to));
                }
                if !vacated.contains(return_to) {
                    match fs::symlink_metadata(return_to) {
                        Ok(_) => {
                            return Err(AppError::Conflict(format!(
                                "回滚目标被占用: {}",
                                paths::display_path(return_to)
                            )))
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                if step.empty_dir_only
                    && step.retain_at.is_some()
                    && fs::read_dir(&to)?.next().transpose()?.is_some()
                {
                    return Err(AppError::Conflict(format!(
                        "回滚目录包含新增内容: {}",
                        paths::display_path(&to)
                    )));
                }
                if step.expected_content.is_some() {
                    check_fingerprint(&to, &step.expected_content)?;
                }
                // Only overwritten payload paths are subsequently reused as inverse destinations.
                vacated.insert(&step.to);
            }
        }
        Ok(())
    }

    fn validate_replacement_rollback_payloads(&self, journal: &[JournalEntry]) -> AppResult<()> {
        for entry in journal {
            self.stable_path(&entry.undo, true)?;
            if fingerprint(&entry.undo)?.is_some() {
                return Err(AppError::Conflict("替换事务的回滚保留路径已占用".into()));
            }
            if let Some(before) = &entry.entry.before {
                self.stable_path(&entry.backup, true)?;
                if fingerprint(&entry.backup)?.as_ref() != Some(before) {
                    return Err(AppError::Conflict("替换备份丢失或已修改，不能回滚".into()));
                }
            }
            for (park, expected) in [
                (entry.source_park.as_ref(), entry.entry.source.as_ref()),
                (entry.overwrite_park.as_ref(), entry.entry.before.as_ref()),
            ] {
                if let Some(park) = park {
                    self.stable_path(park, true)?;
                    if fingerprint(park)?
                        .as_ref()
                        .is_some_and(|fp| Some(fp) != expected)
                    {
                        return Err(AppError::Conflict(
                            "跨磁盘事务的原文件保留副本已修改".into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn rollback_locked(&self, id: &str, journal: &mut [JournalEntry]) -> AppResult<()> {
        self.save_journal(id, "rolling_back", journal)?;
        for (i, entry) in journal.iter_mut().enumerate().rev() {
            if matches!(entry.state.as_str(), "pending" | "undone") {
                continue;
            }
            if let Err(error) = self.undo_entry(entry) {
                self.save_journal(id, "recovery_failed", journal)?;
                return Err(error);
            }
            entry.state = "undone".into();
            self.save_entry(id, i, entry)?;
        }
        self.save_journal(id, "rolled_back", journal)
    }

    fn undo_entry(&self, j: &JournalEntry) -> AppResult<()> {
        let steps = j.renames.as_ref().ok_or_else(legacy_recovery_conflict)?;
        for step in steps.iter().rev() {
            let from = self.stable_path(&step.from, true)?;
            let to = self.stable_path(&step.to, true)?;
            let mut untouched_or_restored =
                safe_fs::identity(&from)?.as_ref() == Some(&step.identity);
            let return_to = if let Some(keep) = &step.retain_at {
                let keep = self.stable_path(keep, true)?;
                untouched_or_restored |= safe_fs::identity(&keep)?.as_ref() == Some(&step.identity);
                keep
            } else {
                from
            };
            let moved = safe_fs::identity(&to)?.as_ref() == Some(&step.identity);
            match (untouched_or_restored, moved) {
                (true, false) => continue,
                (false, true) => {}
                _ => return Err(ownership_conflict(&to)),
            }
            // Leave an implicit parent that another process has populated.
            if step.empty_dir_only && fs::read_dir(&to)?.next().transpose()?.is_some() {
                if step.retain_at.is_some() {
                    return Err(AppError::Conflict(format!(
                        "回滚目录包含新增内容: {}",
                        paths::display_path(&to)
                    )));
                }
                continue;
            }
            if step.expected_content.is_some() {
                check_fingerprint(&to, &step.expected_content)?;
            }
            safe_fs::rename_checked(&to, &return_to, &step.identity)?;
            if safe_fs::identity(&return_to)?.as_ref() != Some(&step.identity) {
                return Err(ownership_conflict(&return_to));
            }
        }
        Ok(())
    }

    pub fn recover_interrupted(&self) -> AppResult<Vec<String>> {
        let ids = {
            let conn = self.db.conn();
            let mut stmt = conn.prepare("SELECT id FROM file_transactions WHERE status IN ('applying','rolling_back','recovery_failed') ORDER BY created_at DESC")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let mut recovered = Vec::new();
        for id in ids {
            self.for_local_agent().rollback_transaction(&id)?;
            recovered.push(id);
        }
        Ok(recovered)
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos", windows)))]
#[path = "transaction_path_tests.rs"]
mod path_tests;

#[cfg(test)]
#[path = "transaction_recovery_tests.rs"]
mod recovery_tests;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn setup() -> (tempfile::TempDir, FileEngine, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path().join("data")).unwrap();
        let db = Db::open(&paths.db_file()).unwrap();
        let game = dir.path().join("game");
        fs::create_dir_all(&game).unwrap();
        let game = game.canonicalize().unwrap();
        db.conn().execute("INSERT INTO games (id,name,slug,root_path,created_at,updated_at) VALUES ('g','Game','game',?1,'now','now')",[game.display().to_string()]).unwrap();
        (dir, FileEngine::new(db, paths), game)
    }

    #[test]
    fn user_plan_formats_only_operation_paths_and_preserves_the_stored_plan() {
        use serde_json::json;

        let raw_src = r"\\?\UNC\server\share\input";
        let raw_dst = r"\\?\C:\game\output";
        let make_plan = |src: &str, dst: &str| {
            let entries = [
                json!({"op": "copy", "src": src, "dst": dst}),
                json!({"op": "move", "src": src, "dst": dst}),
                json!({"op": "mkdir", "dst": dst}),
                json!({"op": "write_text", "dst": dst, "content": raw_dst}),
            ]
            .map(|operation| {
                json!({
                    "operation": operation, "impact": raw_src, "before": null,
                    "source": {"kind": "file", "size": 1, "sha256": "a".repeat(64)}
                })
            });
            json!({"id": "txn1", "summary": raw_dst, "entries": entries,
                "total_bytes": 2, "created_at": "now", "requires_review": true})
        };
        let stored = make_plan(raw_src, raw_dst);
        let plan: FilePlan = serde_json::from_value(stored.clone()).unwrap();
        let expected = if cfg!(windows) {
            make_plan(r"\\server\share\input", r"C:\game\output")
        } else {
            stored.clone()
        };
        assert_eq!(json!(plan.clone().into_user_paths()), expected);
        assert_eq!(
            serialize_user_plan(&plan, serde_json::value::Serializer).unwrap(),
            expected
        );
        assert_eq!(json!(plan), stored);
    }

    #[test]
    fn installation_backup_rollback_trash_restore() {
        let (_dir, e, game) = setup();
        let src = e.paths.downloads().join("mod.txt");
        fs::write(&src, b"new").unwrap();
        let dst = game.join("mods/mod.txt");
        let plan = e
            .plan_file_ops(
                vec![FileOp::Copy {
                    src: src.clone(),
                    dst: dst.clone(),
                }],
                "Install".into(),
            )
            .unwrap();
        assert!(e.apply_file_ops(&plan.id).is_err());
        e.approve_plan(&plan.id).unwrap();
        e.apply_file_ops(&plan.id).unwrap();
        fs::write(&src, b"upgrade").unwrap();
        let upgrade = e
            .plan_file_ops(
                vec![FileOp::Copy {
                    src,
                    dst: dst.clone(),
                }],
                "Upgrade".into(),
            )
            .unwrap();
        e.approve_plan(&upgrade.id).unwrap();
        e.apply_file_ops(&upgrade.id).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), b"upgrade");
        assert_eq!(
            fs::read(e.paths.backup_dir(&upgrade.id).join("0.original")).unwrap(),
            b"new"
        );
        e.rollback_transaction(&upgrade.id).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), b"new");
        let trash = crate::trash::plan_trash(
            &e,
            &crate::trash::TrashRequest {
                paths: vec![dst.display().to_string()],
                game_id: Some("g".into()),
                reason: "Uninstall".into(),
            },
        )
        .unwrap();
        assert!(dst.exists());
        e.approve_plan(&trash.id).unwrap();
        e.apply_file_ops(&trash.id).unwrap();
        assert!(!dst.exists());
        let items = crate::trash::list_trash(&e.db, false).unwrap();
        assert_eq!(items.len(), 1);
        let restore = crate::trash::plan_restore(&e, &[items[0].id.clone()], false).unwrap();
        e.approve_plan(&restore.id).unwrap();
        e.apply_file_ops(&restore.id).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), b"new");
        assert!(crate::trash::list_trash(&e.db, false).unwrap().is_empty());
        e.rollback_transaction(&restore.id).unwrap();
        assert_eq!(crate::trash::list_trash(&e.db, false).unwrap().len(), 1);
    }

    #[test]
    fn changes_after_preview_are_rejected() {
        let (_d, e, g) = setup();
        let src = e.paths.downloads().join("a");
        fs::write(&src, b"a").unwrap();
        let dst = g.join("a");
        fs::write(&dst, b"old").unwrap();
        let p = e
            .plan_file_ops(
                vec![FileOp::Copy {
                    src,
                    dst: dst.clone(),
                }],
                "".into(),
            )
            .unwrap();
        e.approve_plan(&p.id).unwrap();
        fs::write(&dst, b"external").unwrap();
        assert!(e.apply_file_ops(&p.id).is_err());
        assert_eq!(fs::read(dst).unwrap(), b"external");
    }

    #[test]
    fn execution_failure_rolls_back_previous_entries() {
        let (_d, e, g) = setup();
        let src = e.paths.downloads().join("a");
        fs::write(&src, b"new").unwrap();
        let first = g.join("a");
        fs::write(&first, b"old").unwrap();
        let blocker = g.join("blocker");
        fs::create_dir_all(&blocker).unwrap();
        let p = e
            .plan_file_ops(
                vec![
                    FileOp::Copy {
                        src: src.clone(),
                        dst: first.clone(),
                    },
                    FileOp::Copy {
                        src,
                        dst: blocker.join("b"),
                    },
                ],
                "".into(),
            )
            .unwrap();
        e.fail_before.store(1, std::sync::atomic::Ordering::Relaxed);
        e.approve_plan(&p.id).unwrap();
        assert!(e.apply_file_ops(&p.id).is_err());
        assert_eq!(fs::read(first).unwrap(), b"old");
        let status: String =
            e.db.conn()
                .query_row(
                    "SELECT status FROM file_transactions WHERE id=?1",
                    [p.id],
                    |r| r.get(0),
                )
                .unwrap();
        assert_eq!(status, "rolled_back");
    }

    #[test]
    fn restart_recovers_write_between_intent_and_completion() {
        let (_d, e, g) = setup();
        let src = e.paths.downloads().join("a");
        fs::write(&src, b"new").unwrap();
        let dst = g.join("a");
        fs::write(&dst, b"old").unwrap();
        let p = e
            .plan_file_ops(
                vec![FileOp::Copy {
                    src,
                    dst: dst.clone(),
                }],
                "".into(),
            )
            .unwrap();
        let mut journal = e.prepare_journal(&p).unwrap();
        journal[0].state = "intent".into();
        e.save_journal(&p.id, "applying", &journal).unwrap();
        let evidence = ApplyEvidence {
            original: Some(require_identity(&dst).unwrap()),
            source: None,
        };
        e.execute_entry(&p.id, 0, &mut journal[0], &evidence)
            .unwrap();
        let restarted = FileEngine::new(
            Db::open(&e.paths.db_file()).unwrap(),
            e.paths.as_ref().clone(),
        );
        assert_eq!(restarted.recover_interrupted().unwrap(), vec![p.id]);
        assert_eq!(fs::read(dst).unwrap(), b"old");
        assert!(restarted.recover_interrupted().unwrap().is_empty());
    }

    #[test]
    fn paths_outside_registry_and_internal_database_are_denied() {
        let (d, e, g) = setup();
        let src = e.paths.downloads().join("a");
        fs::write(&src, b"a").unwrap();
        assert!(e
            .plan_file_ops(
                vec![FileOp::Copy {
                    src: src.clone(),
                    dst: d.path().join("outside")
                }],
                "".into()
            )
            .is_err());
        assert!(e
            .plan_file_ops(
                vec![FileOp::Copy {
                    src,
                    dst: e.paths.db_file()
                }],
                "".into()
            )
            .is_err());
        assert!(crate::trash::plan_trash(
            &e,
            &crate::trash::TrashRequest {
                paths: vec![g.display().to_string()],
                game_id: None,
                reason: String::new()
            }
        )
        .is_err());
    }

    #[test]
    fn rollback_shared_new_directory_retains_payloads() {
        let (_d, e, g) = setup();
        let src = e.paths.downloads().join("a");
        fs::write(&src, b"a").unwrap();
        let p = e
            .plan_file_ops(
                vec![
                    FileOp::Copy {
                        src: src.clone(),
                        dst: g.join("mods/a"),
                    },
                    FileOp::Copy {
                        src,
                        dst: g.join("mods/b"),
                    },
                ],
                "".into(),
            )
            .unwrap();
        e.approve_plan(&p.id).unwrap();
        e.apply_file_ops(&p.id).unwrap();
        e.rollback_transaction(&p.id).unwrap();
        assert!(!g.join("mods").exists());
        // Rollback payloads remain retained on the target filesystem.
        assert_eq!(
            fs::read(g.join(".amm-retained").join(&p.id).join("0-undone")).unwrap(),
            b"a"
        );
        e.rollback_transaction(&p.id).unwrap();
    }

    #[test]
    fn cross_volume_copy_and_move_keep_verified_originals_and_rollback() {
        let (_d, e, g) = setup();
        e.cross_device
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let src = e.paths.downloads().join("a");
        fs::write(&src, b"new").unwrap();
        let dst = g.join("a");
        fs::write(&dst, b"old").unwrap();
        let p = e
            .plan_file_ops(
                vec![FileOp::Move {
                    src: src.clone(),
                    dst: dst.clone(),
                }],
                "Cross volume".into(),
            )
            .unwrap();
        e.approve_plan(&p.id).unwrap();
        e.apply_file_ops(&p.id).unwrap();
        assert!(!src.exists());
        assert_eq!(fs::read(&dst).unwrap(), b"new");
        assert_eq!(
            fs::read(e.paths.backup_dir(&p.id).join("0.original")).unwrap(),
            b"old"
        );
        e.rollback_transaction(&p.id).unwrap();
        assert_eq!(fs::read(src).unwrap(), b"new");
        assert_eq!(fs::read(dst).unwrap(), b"old");
        e.rollback_transaction(&p.id).unwrap();
    }
}
