//! Embedded system guidance and the catalog of registered declarative Skills.
//! Approved FileEngine transactions own standalone installation and removal.
//! Call migrate at startup, then reconcile after recovery/apply/rollback.
use crate::{
    db::Db,
    error::{AppError, AppResult},
    paths, plugin_workflow, plugins,
    transaction::{FileEngine, FileOp, FilePlan, Fingerprint},
    trash, util,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

const SKILL_LIMIT: usize = 128 * 1024;
const ADAPTER_LIMIT: usize = 256 * 1024;
const RECORD_LIMIT: usize = 1024 * 1024;
const MANIFEST_LIMIT: usize = 64 * 1024;
const SOURCE: &str = "external";
const SYSTEM_SOURCE: &str = "system";

struct SystemSkill {
    name: &'static str,
    location: &'static str,
    markdown: &'static str,
}

macro_rules! system_skill {
    ($name:literal) => {
        SystemSkill {
            name: $name,
            location: concat!("plugins/skills/", $name, "/SKILL.md"),
            markdown: include_str!(concat!("../../plugins/skills/", $name, "/SKILL.md")),
        }
    };
}

const SYSTEM_SKILLS: &[SystemSkill] = &[
    system_skill!("system-adapter-authoring"),
    system_skill!("system-web-search"),
    system_skill!("system-mod-download"),
];

fn system_skill(name: &str) -> Option<&'static SystemSkill> {
    SYSTEM_SKILLS.iter().find(|skill| skill.name == name)
}

pub(crate) fn is_system_skill(name: &str) -> bool {
    system_skill(name).is_some()
}

static SKILLS_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    SKILLS_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
fn invalid(message: impl Into<String>) -> AppError {
    AppError::Invalid(message.into())
}

/// Additive, idempotent migration; does not rewrite shared skills/plugin tables.
/// desired_enabled is explicit user intent, separate from safety disabling.
pub fn migrate(db: &Db) -> AppResult<()> {
    db.conn().execute_batch(
        "CREATE TABLE IF NOT EXISTS skills_deployments (
           transaction_id TEXT PRIMARY KEY,
           name TEXT NOT NULL,
           record_json TEXT NOT NULL,
           install_path TEXT NOT NULL,
           desired_enabled INTEGER NOT NULL DEFAULT 1 CHECK(desired_enabled IN (0,1)),
           created_at TEXT NOT NULL,
           FOREIGN KEY(transaction_id) REFERENCES file_transactions(id)
         );
         CREATE INDEX IF NOT EXISTS idx_skills_deployments_name ON skills_deployments(name);
         CREATE TABLE IF NOT EXISTS system_skill_preferences (
           name TEXT PRIMARY KEY,
           enabled INTEGER NOT NULL CHECK(enabled IN (0,1))
         );",
    )?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillKind {
    System,
    Plugin,
    Standalone,
    Unknown,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    #[serde(serialize_with = "crate::paths::serialize_user_path")]
    pub location: PathBuf,
    pub source: String,
    pub enabled: bool,
    pub created_at: String,
    pub kind: SkillKind,
    pub valid: bool,
    pub issues: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDetail {
    pub skill: SkillInfo,
    pub markdown: String,
}

impl SystemSkill {
    fn info(&self, enabled: bool) -> AppResult<SkillInfo> {
        let metadata = plugins::parse_skill(self.markdown, self.name)?;
        Ok(SkillInfo {
            name: metadata.name,
            description: metadata.description,
            location: PathBuf::from(self.location),
            source: SYSTEM_SOURCE.into(),
            enabled,
            created_at: String::new(),
            kind: SkillKind::System,
            valid: true,
            issues: vec![],
        })
    }

    fn enabled(&self, db: &Db) -> AppResult<bool> {
        Ok(db
            .conn()
            .query_row(
                "SELECT enabled FROM system_skill_preferences WHERE name=?1",
                [self.name],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(true))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillAction {
    Install,
    Uninstall,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillPreview {
    pub name: String,
    pub description: String,
    #[serde(serialize_with = "crate::paths::serialize_user_path")]
    pub location: PathBuf,
    pub action: SkillAction,
    pub sha256: String,
    #[serde(serialize_with = "crate::transaction::serialize_user_plan")]
    pub plan: FilePlan,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillIssue {
    pub name: String,
    pub code: String,
    pub message: String,
}
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileReport {
    pub registered: Vec<String>,
    pub disabled: Vec<String>,
    pub removed: Vec<String>,
    pub issues: Vec<SkillIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Content {
    markdown: String,
    sha256: String,
}
impl Content {
    fn new(name: &str, markdown: &str) -> AppResult<Self> {
        plugins::parse_skill(markdown, name)?;
        Ok(Self {
            markdown: markdown.into(),
            sha256: hex::encode(Sha256::digest(markdown.as_bytes())),
        })
    }
    fn validate(&self, name: &str) -> AppResult<plugins::SkillMetadata> {
        if self.markdown.len() > SKILL_LIMIT
            || hex::encode(Sha256::digest(self.markdown.as_bytes())) != self.sha256
        {
            return Err(invalid("Skill 记录大小或 SHA-256 无效"));
        }
        plugins::parse_skill(&self.markdown, name)
    }
    fn matches(&self, fingerprint: Option<&Fingerprint>) -> bool {
        fingerprint.is_some_and(|f| {
            f.kind == "file" && f.size == self.markdown.len() as u64 && f.sha256 == self.sha256
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Previous {
    deployment_id: String,
    content: Content,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrashBinding {
    id: String,
    manifest_sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeploymentRecord {
    schema_version: u32,
    operation_id: String,
    action: SkillAction,
    content: Content,
    previous: Option<Previous>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    trash: Option<TrashBinding>,
}
struct DeploymentRow {
    transaction_id: String,
    name: String,
    install_path: PathBuf,
    desired_enabled: bool,
    status: String,
}

fn checked_path(engine: &FileEngine, path: &Path) -> AppResult<PathBuf> {
    let root = &engine.paths.root;
    engine.paths.stable_directory(root)?;
    if !path.is_absolute()
        || !path.starts_with(root)
        || path.components().any(|c| matches!(c, Component::ParentDir))
    {
        return Err(AppError::PathEscape(crate::paths::display_path(path)));
    }
    let mut current = root.clone();
    for component in path
        .strip_prefix(root)
        .map_err(|e| invalid(e.to_string()))?
        .components()
    {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(AppError::Denied("拒绝 Skill 路径中的符号链接".into()));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(e.into()),
        }
    }
    let resolved = paths::ensure_within(path, std::slice::from_ref(root))?;
    if resolved != path {
        return Err(AppError::Denied("Skill 路径已被重定向".into()));
    }
    Ok(resolved)
}
fn location(engine: &FileEngine, name: &str) -> AppResult<PathBuf> {
    plugins::validate_slug(name)?;
    checked_path(engine, &engine.paths.skills().join(name).join("SKILL.md"))
}
fn scratch_path(engine: &FileEngine, id: &str, name: &str) -> AppResult<PathBuf> {
    if !id.strip_prefix("skill_").is_some_and(|s| {
        s.len() == 32
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) {
        return Err(invalid("Skill 操作 ID 无效"));
    }
    plugins::validate_slug(name)?;
    checked_path(
        engine,
        &engine
            .paths
            .staging()
            .join("skill-scratch")
            .join(id)
            .join(name)
            .join("SKILL.md"),
    )
}
fn read_text(engine: &FileEngine, path: &Path, limit: usize) -> AppResult<String> {
    checked_path(engine, path)?;
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.len() > limit as u64 {
        return Err(invalid("Skill/适配文件须为大小受限的普通文件"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 != 0 {
            return Err(AppError::Denied("不接受带执行权限的 Skill 资产".into()));
        }
    }
    let file = fs::File::open(path)?;
    checked_path(engine, path)?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid("Skill 读取超过上限"));
    }
    String::from_utf8(bytes).map_err(|_| invalid("Skill 必须是 UTF-8 文本"))
}
fn exact_entries(engine: &FileEngine, directory: &Path, names: &[&str]) -> AppResult<()> {
    checked_path(engine, directory)?;
    if !fs::symlink_metadata(directory)?.is_dir() {
        return Err(invalid("Skill 所在位置不是普通目录"));
    }
    let entries = fs::read_dir(directory)?
        .take(names.len() + 1)
        .collect::<Result<Vec<_>, _>>()?;
    if entries.len() != names.len()
        || entries
            .iter()
            .any(|e| !names.iter().any(|n| e.file_name() == *n))
    {
        return Err(AppError::Denied(
            "Skill 目录含未声明资产；仅接受规定的声明文件".into(),
        ));
    }
    Ok(())
}
fn read_standalone(engine: &FileEngine, name: &str) -> AppResult<Content> {
    let target = location(engine, name)?;
    exact_entries(engine, target.parent().unwrap(), &["SKILL.md"])?;
    Content::new(name, &read_text(engine, &target, SKILL_LIMIT)?)
}
fn absent(engine: &FileEngine, target: &Path) -> AppResult<bool> {
    checked_path(engine, target)?;
    match fs::symlink_metadata(target.parent().unwrap()) {
        Ok(meta) if meta.is_dir() => Ok(fs::read_dir(target.parent().unwrap())?.next().is_none()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Ok(_) => Err(invalid("Skill 目录被普通文件替换")),
        Err(e) => Err(e.into()),
    }
}

fn registry_row(engine: &FileEngine, name: &str) -> AppResult<Option<SkillInfo>> {
    Ok(engine
        .db
        .conn()
        .query_row(
            "SELECT name,description,location,source,enabled,created_at FROM skills WHERE name=?1",
            [name],
            |r| {
                Ok(SkillInfo {
                    name: r.get(0)?,
                    description: r.get(1)?,
                    location: PathBuf::from(r.get::<_, String>(2)?),
                    source: r.get(3)?,
                    enabled: r.get(4)?,
                    created_at: r.get(5)?,
                    kind: SkillKind::Unknown,
                    valid: false,
                    issues: vec![],
                })
            },
        )
        .optional()?)
}
/// Source and exact location both matter: source='external' alone is not an
/// ownership grant. In particular, game-package Skills are never standalone.
fn standalone_owner(engine: &FileEngine, name: &str) -> AppResult<Option<SkillInfo>> {
    plugins::validate_slug(name)?;
    if is_system_skill(name) {
        return Err(AppError::Conflict("不能接管系统 Skill 名称".into()));
    }
    let plugin: bool = engine.db.conn().query_row(
        "SELECT EXISTS(SELECT 1 FROM plugins WHERE slug=?1)",
        [name],
        |r| r.get(0),
    )?;
    let pending: bool = engine.db.conn().query_row(
        "SELECT EXISTS(SELECT 1 FROM plugin_deployments p JOIN file_transactions f ON f.id=p.transaction_id WHERE p.slug=?1 AND f.status NOT IN ('rolled_back','failed'))",
        [name], |r|r.get(0))?;
    if plugin || pending {
        return Err(AppError::Conflict("名称属于游戏插件或其待处理部署".into()));
    }
    let row = registry_row(engine, name)?;
    let expected = engine.paths.skills().join(name).join("SKILL.md");
    if row
        .as_ref()
        .is_some_and(|s| s.source != SOURCE || s.location != expected)
    {
        return Err(AppError::Conflict(
            "Skill 来源/位置不属于独立 Skill，拒绝接管".into(),
        ));
    }
    Ok(row)
}
fn deployment_rows(engine: &FileEngine) -> AppResult<Vec<DeploymentRow>> {
    let conn = engine.db.conn();
    let mut stmt = conn.prepare("SELECT d.transaction_id,d.name,d.install_path,d.desired_enabled,coalesce(f.status,'missing') FROM skills_deployments d LEFT JOIN file_transactions f ON f.id=d.transaction_id ORDER BY d.rowid DESC")?;
    let rows = stmt.query_map([], |r| {
        Ok(DeploymentRow {
            transaction_id: r.get(0)?,
            name: r.get(1)?,
            install_path: PathBuf::from(r.get::<_, String>(2)?),
            desired_enabled: r.get(3)?,
            status: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Bind the standard trash manifest/DB row to exactly this owned document and
/// transaction. No paths supplied by the manifest are used for filesystem reads.
fn trash_binding(
    engine: &FileEngine,
    transaction_id: &str,
    target: &Path,
    content: &Content,
) -> AppResult<(TrashBinding, PathBuf)> {
    let items = {
        let conn = engine.db.conn();
        let mut stmt = conn.prepare("SELECT id,original_path,trash_path,transaction_id,size,is_dir,game_id,label,reason,created_at FROM trash_items WHERE transaction_id=?1 LIMIT 2")?;
        let rows = stmt.query_map([transaction_id], |r| {
            Ok(trash::TrashItem {
                id: r.get(0)?,
                original_path: r.get(1)?,
                trash_path: r.get(2)?,
                transaction_id: r.get(3)?,
                size: r.get::<_, i64>(4)? as u64,
                is_dir: r.get(5)?,
                game_id: r.get(6)?,
                label: r.get(7)?,
                reason: r.get(8)?,
                created_at: r.get(9)?,
                restored_at: None,
                exists: false,
                restore_requires_rollback: false,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    if items.len() != 1 {
        return Err(invalid("Skill 卸载必须绑定单个标准回收站项目"));
    }
    let item = &items[0];
    if !item.id.strip_prefix("trash_").is_some_and(|s| {
        s.len() == 32
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) {
        return Err(invalid("Skill 回收站 ID 无效"));
    }
    let directory = engine.paths.trash_entry(&item.id);
    let payload = checked_path(engine, &directory.join("payload"))?;
    if Path::new(&item.original_path) != target
        || Path::new(&item.trash_path) != payload
        || item.size != content.markdown.len() as u64
        || item.is_dir
        || item.game_id.is_some()
    {
        return Err(AppError::Conflict(
            "Skill 回收站项目不匹配原文件/目标/大小".into(),
        ));
    }
    let manifest = read_text(engine, &directory.join("manifest.json"), MANIFEST_LIMIT)?;
    let decoded: trash::TrashItem = serde_json::from_str(&manifest)?;
    if serde_json::to_value(&decoded)? != serde_json::to_value(item)? {
        return Err(AppError::Conflict(
            "Skill 标准回收站 manifest 与持久记录不一致".into(),
        ));
    }
    Ok((
        TrashBinding {
            id: item.id.clone(),
            manifest_sha256: hex::encode(Sha256::digest(manifest.as_bytes())),
        },
        payload,
    ))
}

fn trash_payload(
    engine: &FileEngine,
    row: &DeploymentRow,
    record: &DeploymentRecord,
) -> AppResult<PathBuf> {
    let expected = record
        .trash
        .as_ref()
        .ok_or_else(|| invalid("Skill 卸载缺少标准回收站凭据"))?;
    let (actual, payload) = trash_binding(
        engine,
        &row.transaction_id,
        &row.install_path,
        &record.content,
    )?;
    if expected.id != actual.id || expected.manifest_sha256 != actual.manifest_sha256 {
        return Err(AppError::Conflict(
            "Skill 回收站凭据或 manifest SHA-256 已改变".into(),
        ));
    }
    Ok(payload)
}

fn verify_plan(
    engine: &FileEngine,
    row: &DeploymentRow,
    record: &DeploymentRecord,
    plan: &FilePlan,
) -> AppResult<()> {
    if record.schema_version != 1
        || plan.id != row.transaction_id
        || !plan.requires_review
        || plan.entries.len() != 1
    {
        return Err(invalid("Skill 部署必须是要求审查的单文件事务"));
    }
    record.content.validate(&row.name)?;
    let target = location(engine, &row.name)?;
    if row.install_path != target {
        return Err(AppError::Denied("Skill 部署记录位置不匹配".into()));
    }
    let entry = &plan.entries[0];
    if !record.content.matches(entry.source.as_ref()) {
        return Err(AppError::Conflict(
            "Skill 原文不匹配批准计划的源指纹".into(),
        ));
    }
    match record.action {
        SkillAction::Install => {
            if record.trash.is_some() {
                return Err(invalid("Skill 安装不能携带卸载回收站凭据"));
            }
            let scratch = scratch_path(engine, &record.operation_id, &row.name)?;
            if !matches!(&entry.operation, FileOp::Copy { src,dst } if *src==scratch && *dst==target)
            {
                return Err(invalid("Skill 安装不是指定 scratch 到固定位置的复制"));
            }
            match &record.previous {
                Some(previous) => {
                    previous.content.validate(&row.name)?;
                    if !previous.content.matches(entry.before.as_ref()) {
                        return Err(AppError::Conflict("Skill 覆盖前快照不匹配".into()));
                    }
                }
                None if entry.before.is_some() => return Err(invalid("禁止覆盖无归属 Skill 文件")),
                None => {}
            }
        }
        SkillAction::Uninstall => {
            let parked = trash_payload(engine, row, record)?;
            if !matches!(&entry.operation, FileOp::Move { src,dst } if *src==target && *dst==parked)
                || entry.before.is_some()
                || record.previous.is_none()
            {
                return Err(invalid("Skill 卸载不是到标准回收站的无覆盖移动"));
            }
        }
    }
    if let Some(previous) = &record.previous {
        previous.content.validate(&row.name)?;
        let previous_json: String = engine.db.conn().query_row(
            "SELECT CASE WHEN length(CAST(record_json AS BLOB))<=?3 THEN record_json ELSE '' END FROM skills_deployments WHERE transaction_id=?1 AND name=?2",
            params![previous.deployment_id,row.name,RECORD_LIMIT], |r|r.get(0))?;
        let prior: DeploymentRecord = serde_json::from_str(&previous_json)?;
        if prior.action != SkillAction::Install
            || prior.content.sha256 != previous.content.sha256
            || (record.action == SkillAction::Uninstall
                && previous.content.sha256 != record.content.sha256)
        {
            return Err(invalid("Skill 旧版本来源记录不匹配"));
        }
    }
    Ok(())
}
fn record_for(engine: &FileEngine, row: &DeploymentRow) -> AppResult<DeploymentRecord> {
    let json: String = engine.db.conn().query_row(
        "SELECT CASE WHEN length(CAST(record_json AS BLOB))<=?2 THEN record_json ELSE '' END FROM skills_deployments WHERE transaction_id=?1",
        params![row.transaction_id,RECORD_LIMIT], |r|r.get(0))?;
    let record: DeploymentRecord = serde_json::from_str(&json)?;
    verify_plan(engine, row, &record, &engine.get_plan(&row.transaction_id)?)?;
    Ok(record)
}
fn committed_content(engine: &FileEngine, name: &str, disk: &Content) -> AppResult<Option<String>> {
    for row in deployment_rows(engine)?
        .iter()
        .filter(|r| r.name == name && r.status == "committed")
    {
        if let Ok(record) = record_for(engine, row) {
            if record.action == SkillAction::Install && record.content.sha256 == disk.sha256 {
                return Ok(Some(row.transaction_id.clone()));
            }
        }
    }
    Ok(None)
}
fn disable(engine: &FileEngine, name: &str) -> AppResult<bool> {
    let expected = engine.paths.skills().join(name).join("SKILL.md");
    Ok(engine.db.conn().execute("UPDATE skills SET enabled=0 WHERE name=?1 AND source='external' AND location=?2 AND enabled<>0", params![name,expected.to_string_lossy()])? != 0)
}
fn reconcile_locked(engine: &FileEngine) -> AppResult<ReconcileReport> {
    let mut groups: BTreeMap<String, Vec<DeploymentRow>> = BTreeMap::new();
    for row in deployment_rows(engine)? {
        groups.entry(row.name.clone()).or_default().push(row);
    }
    let mut report = ReconcileReport::default();
    for (name, rows) in groups {
        let outcome = (|| -> AppResult<bool> {
            standalone_owner(engine, &name)?;
            if rows.iter().any(|r| {
                matches!(
                    r.status.as_str(),
                    "applying" | "rolling_back" | "recovery_failed"
                )
            }) {
                return Err(AppError::Conflict(
                    "Skill 文件事务待恢复，拒绝使用中间状态".into(),
                ));
            }
            let target = location(engine, &name)?;
            if absent(engine, &target)? {
                for row in rows.iter().filter(|r| r.status == "committed") {
                    let record = match record_for(engine, row) {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    if record.action == SkillAction::Uninstall {
                        let parked = trash_payload(engine, row, &record)?;
                        let text = read_text(engine, &parked, SKILL_LIMIT)?;
                        if Content::new(&name, &text)?.sha256 == record.content.sha256 {
                            return Ok(false);
                        }
                    }
                }
                return Err(AppError::NotFound(
                    "Skill 文件不存在且没有匹配的已提交卸载".into(),
                ));
            }
            let disk = read_standalone(engine, &name)?;
            if committed_content(engine, &name, &disk)?.is_none() {
                return Err(AppError::Conflict(
                    "磁盘 Skill 不匹配任何已提交、完整验证的部署".into(),
                ));
            }
            let metadata = disk.validate(&name)?;
            // Recheck ownership under the same DB lock used by the upsert.
            let mut conn = engine.db.conn();
            let tx = conn.transaction()?;
            let changed = tx.execute("INSERT INTO skills(name,description,location,source,enabled,created_at) SELECT ?1,?2,?3,'external',?4,?5 WHERE NOT EXISTS(SELECT 1 FROM plugins WHERE slug=?1) AND NOT EXISTS(SELECT 1 FROM plugin_deployments p JOIN file_transactions f ON f.id=p.transaction_id WHERE p.slug=?1 AND f.status NOT IN ('rolled_back','failed')) ON CONFLICT(name) DO UPDATE SET description=excluded.description,enabled=excluded.enabled WHERE skills.source='external' AND skills.location=excluded.location",params![name,metadata.description,target.to_string_lossy(),rows[0].desired_enabled,util::now()])?;
            if changed != 1 {
                return Err(AppError::Conflict("Skill 注册发生来源冲突".into()));
            }
            tx.commit()?;
            Ok(true)
        })();
        match outcome {
            Ok(true) => report.registered.push(name),
            Ok(false) => {
                if disable(engine, &name)? {
                    report.disabled.push(name.clone());
                }
                report.removed.push(name);
            }
            Err(error) => {
                // Never change a colliding plugin or unknown-source row.
                if standalone_owner(engine, &name).is_ok() && disable(engine, &name)? {
                    report.disabled.push(name.clone());
                }
                report.issues.push(SkillIssue {
                    name,
                    code: "unverified_skill".into(),
                    message: error.to_string(),
                });
            }
        }
    }
    Ok(report)
}
/// Files/history are never changed. Run after startup recovery, commit/rollback,
/// and before exposing Skills. Unknown/tampered states fail closed.
pub fn reconcile_deployments(engine: &FileEngine) -> AppResult<ReconcileReport> {
    let _lock = lock();
    let _files = crate::transaction::write_lock();
    reconcile_locked(engine)
}

fn persist_plan(
    engine: &FileEngine,
    name: &str,
    mut record: DeploymentRecord,
    desired_enabled: bool,
) -> AppResult<SkillPreview> {
    let target = location(engine, name)?;
    let metadata = record.content.validate(name)?;
    let plan = match record.action {
        SkillAction::Install => engine.plan_file_ops(
            vec![FileOp::Copy {
                src: scratch_path(engine, &record.operation_id, name)?,
                dst: target.clone(),
            }],
            format!("安装声明式 Skill {name}"),
        )?,
        SkillAction::Uninstall => {
            checked_path(engine, &engine.paths.trash())?;
            engine.paths.stable_directory(&engine.paths.trash())?;
            let plan = trash::plan_trash(
                engine,
                &trash::TrashRequest {
                    paths: vec![target.to_string_lossy().into_owned()],
                    game_id: None,
                    reason: format!("卸载独立 Skill {name}"),
                },
            )?;
            let (binding, _) = trash_binding(engine, &plan.id, &target, &record.content)?;
            record.trash = Some(binding);
            plan
        }
    };
    let json = serde_json::to_string(&record)?;
    if json.len() > RECORD_LIMIT {
        return Err(invalid("Skill 部署记录超过上限"));
    }
    let plan = engine.require_review(&plan.id)?;
    let row = DeploymentRow {
        transaction_id: plan.id.clone(),
        name: name.into(),
        install_path: target.clone(),
        desired_enabled,
        status: "planned".into(),
    };
    verify_plan(engine, &row, &record, &plan)?;
    standalone_owner(engine, name)?;
    engine.db.conn().execute("INSERT INTO skills_deployments(transaction_id,name,record_json,install_path,desired_enabled,created_at) VALUES (?1,?2,?3,?4,?5,?6)",params![plan.id,name,json,target.to_string_lossy(),desired_enabled,util::now()])?;
    Ok(SkillPreview {
        name: name.into(),
        description: metadata.description,
        location: target,
        action: record.action,
        sha256: record.content.sha256.clone(),
        plan,
        warnings: vec![
            "只校验声明结构与文件完整性，不证明指令可信或兼容；需人工审查，绝不自动执行正文。"
                .into(),
        ],
    })
}

/// No directory/archive input. Only a new inert scratch file is written here;
/// the destination is changed only by the returned reviewed FileEngine plan.
pub fn plan_install_skill(
    engine: &FileEngine,
    name: &str,
    markdown: &str,
) -> AppResult<SkillPreview> {
    let _lock = lock();
    let content = Content::new(name, markdown)?;
    let id = util::new_id("skill");
    let (previous, desired_enabled) = {
        let _files = crate::transaction::write_lock();
        standalone_owner(engine, name)?;
        reconcile_locked(engine)?;
        let target = location(engine, name)?;
        let rows = deployment_rows(engine)?;
        let desired = rows
            .iter()
            .find(|r| r.name == name)
            .is_none_or(|r| r.desired_enabled);
        let previous = if absent(engine, &target)? {
            None
        } else {
            let disk = read_standalone(engine, name)?;
            let provenance = committed_content(engine, name, &disk)?
                .ok_or_else(|| AppError::Conflict("不接管未通过此工作流登记的既有文件".into()))?;
            Some(Previous {
                deployment_id: provenance,
                content: disk,
            })
        };
        let scratch = scratch_path(engine, &id, name)?;
        let directory = scratch.parent().unwrap();
        let parent = directory.parent().unwrap();
        checked_path(engine, parent)?;
        fs::create_dir_all(parent)?;
        checked_path(engine, parent)?;
        fs::create_dir(directory)?;
        checked_path(engine, &scratch)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&scratch)?;
        file.write_all(markdown.as_bytes())?;
        file.sync_all()?;
        (previous, desired)
    };
    persist_plan(
        engine,
        name,
        DeploymentRecord {
            schema_version: 1,
            operation_id: id,
            action: SkillAction::Install,
            content,
            previous,
            trash: None,
        },
        desired_enabled,
    )
}

/// Removal uses the standard manager trash API/manifest, never staging/deletion.
/// Its reviewed transaction can be rolled back, or restored by the trash API.
pub fn plan_uninstall_skill(engine: &FileEngine, name: &str) -> AppResult<SkillPreview> {
    let _lock = lock();
    let (content, previous, desired) = {
        let _files = crate::transaction::write_lock();
        standalone_owner(engine, name)?.ok_or_else(|| AppError::NotFound(name.into()))?;
        reconcile_locked(engine)?;
        let content = read_standalone(engine, name)?;
        let provenance = committed_content(engine, name, &content)?
            .ok_or_else(|| AppError::Conflict("不能卸载未验证归属的 Skill".into()))?;
        let desired = deployment_rows(engine)?
            .iter()
            .find(|r| r.name == name)
            .is_none_or(|r| r.desired_enabled);
        let previous = Previous {
            deployment_id: provenance,
            content: content.clone(),
        };
        (content, previous, desired)
    };
    persist_plan(
        engine,
        name,
        DeploymentRecord {
            schema_version: 1,
            operation_id: util::new_id("skill"),
            action: SkillAction::Uninstall,
            content,
            previous: Some(previous),
            trash: None,
        },
        desired,
    )
}

fn inspect(engine: &FileEngine, mut info: SkillInfo) -> (SkillInfo, Option<String>) {
    let result = (|| -> AppResult<String> {
        plugins::validate_slug(&info.name)?;
        let plugin: Option<(String,PathBuf,String,bool,bool)> = engine.db.conn().query_row(
            "SELECT source,install_path,adapter_json,validated,enabled FROM plugins WHERE slug=?1", [&info.name],
            |r|Ok((r.get(0)?,PathBuf::from(r.get::<_,String>(1)?),r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        if let Some((source, root, adapter_json, validated, plugin_enabled)) = plugin {
            // Effective availability must not overwrite the independent Skill preference.
            info.enabled &= plugin_enabled;
            let expected = engine.paths.plugins().join("games").join(&info.name);
            if root != expected
                || info.location != expected.join("SKILL.md")
                || info.source != source
            {
                return Err(AppError::Conflict(
                    "插件 Skill 的位置/来源/校验状态不匹配".into(),
                ));
            }
            if source != SOURCE {
                return Err(AppError::Denied("未知插件 Skill 来源".into()));
            }
            info.kind = SkillKind::Plugin;
            if !validated {
                return Err(AppError::Denied("关联插件未通过完整性校验".into()));
            }
            plugin_workflow::validate_package_directory(engine, &expected)?;
            let markdown = read_text(engine, &info.location, SKILL_LIMIT)?;
            let adapter = read_text(engine, &expected.join("adapter.json"), ADAPTER_LIMIT)?;
            let package = plugins::parse_package(&info.name, &adapter, &markdown)?;
            if !plugins::matches_registered_adapter(&adapter_json, &package.adapter)? {
                return Err(AppError::Conflict(
                    "插件 Skill 对应适配包与登记内容不一致".into(),
                ));
            }
            info.description = package.skill.description;
            Ok(markdown)
        } else {
            standalone_owner(engine, &info.name)?;
            info.kind = SkillKind::Standalone;
            let disk = read_standalone(engine, &info.name)?;
            if committed_content(engine, &info.name, &disk)?.is_none() {
                return Err(AppError::Conflict("独立 Skill 缺少匹配的已提交部署".into()));
            }
            if deployment_rows(engine)?.iter().any(|r| {
                r.name == info.name
                    && matches!(
                        r.status.as_str(),
                        "applying" | "rolling_back" | "recovery_failed"
                    )
            }) {
                return Err(AppError::Conflict("Skill 文件事务待恢复".into()));
            }
            info.description = disk.validate(&info.name)?.description;
            Ok(disk.markdown)
        }
    })();
    match result {
        Ok(markdown) => {
            info.valid = true;
            (info, Some(markdown))
        }
        Err(error) => {
            info.issues.push(error.to_string());
            (info, None)
        }
    }
}

/// Enumerates the embedded catalog and registered metadata. Invalid/unowned
/// records cannot supply Skill content.
pub fn list_skills(engine: &FileEngine) -> AppResult<Vec<SkillInfo>> {
    let _lock = lock();
    // The package workflow owns plugin Skill hashes and rollback provenance.
    // Never read a mutable external plugin Skill using schema validation alone.
    crate::plugin_workflow::reconcile_deployments(engine)?;
    let _files = crate::transaction::write_lock();
    reconcile_locked(engine)?;
    let names: Vec<String> = {
        let conn = engine.db.conn();
        let mut stmt = conn.prepare("SELECT name FROM skills ORDER BY name")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut catalog = Vec::with_capacity(SYSTEM_SKILLS.len() + names.len());
    for system in SYSTEM_SKILLS {
        catalog.push(system.info(system.enabled(&engine.db)?)?);
    }
    // Reserved names always resolve to embedded documents. Legacy registry
    // rows remain in the database with their original ownership and settings.
    for name in names.into_iter().filter(|name| !is_system_skill(name)) {
        let row = registry_row(engine, &name)?.ok_or(AppError::NotFound(name))?;
        catalog.push(inspect(engine, row).0);
    }
    Ok(catalog)
}
pub fn get_skill(engine: &FileEngine, name: &str) -> AppResult<SkillDetail> {
    let _lock = lock();
    plugins::validate_slug(name)?;
    if let Some(system) = system_skill(name) {
        return Ok(SkillDetail {
            skill: system.info(system.enabled(&engine.db)?)?,
            markdown: system.markdown.into(),
        });
    }
    crate::plugin_workflow::reconcile_deployments(engine)?;
    let _files = crate::transaction::write_lock();
    reconcile_locked(engine)?;
    let row = registry_row(engine, name)?.ok_or_else(|| AppError::NotFound(name.into()))?;
    let (skill, markdown) = inspect(engine, row);
    let markdown = markdown.ok_or_else(|| AppError::Denied(skill.issues.join("; ")))?;
    Ok(SkillDetail { skill, markdown })
}

/// Toggles an embedded system Skill or a registered Skill with known ownership.
/// Enabling requires a currently verified document; disabling cannot load code.
pub fn set_enabled(engine: &FileEngine, name: &str, enabled: bool) -> AppResult<SkillInfo> {
    let _lock = lock();
    plugins::validate_slug(name)?;
    if let Some(system) = system_skill(name) {
        let info = system.info(enabled)?;
        engine.db.conn().execute(
            "INSERT INTO system_skill_preferences(name,enabled) VALUES (?1,?2)
             ON CONFLICT(name) DO UPDATE SET enabled=excluded.enabled",
            params![name, enabled],
        )?;
        return Ok(info);
    }
    crate::plugin_workflow::reconcile_deployments(engine)?;
    let _files = crate::transaction::write_lock();
    reconcile_locked(engine)?;
    let row = registry_row(engine, name)?.ok_or_else(|| AppError::NotFound(name.into()))?;
    let (info, _) = inspect(engine, row);
    if info.kind == SkillKind::Unknown || (enabled && !info.valid) {
        return Err(AppError::Denied(
            "不能切换来源未知的 Skill 或启用未验证文档".into(),
        ));
    }
    if info.kind != SkillKind::Standalone {
        let ready: bool = engine.db.conn().query_row(
            "SELECT enabled<>0 AND validated<>0 FROM plugins WHERE slug=?1",
            [name],
            |r| r.get(0),
        )?;
        if enabled && !ready {
            return Err(AppError::Denied(
                "插件未启用或未验证，不能启用关联 Skill".into(),
            ));
        }
    }
    let mut conn = engine.db.conn();
    let tx = conn.transaction()?;
    if info.kind == SkillKind::Standalone {
        let collision: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM plugins WHERE slug=?1) OR EXISTS(SELECT 1 FROM plugin_deployments p JOIN file_transactions f ON f.id=p.transaction_id WHERE p.slug=?1 AND f.status NOT IN ('rolled_back','failed'))",[name],|r|r.get(0))?;
        if collision {
            return Err(AppError::Conflict("Skill 名称被插件部署占用".into()));
        }
        let count = tx.execute(
            "UPDATE skills_deployments SET desired_enabled=?2 WHERE name=?1",
            params![name, enabled],
        )?;
        if count == 0 {
            return Err(AppError::Denied("独立 Skill 缺少部署所有权".into()));
        }
    }
    let count = tx.execute(
        "UPDATE skills SET enabled=?4 WHERE name=?1 AND source=?2 AND location=?3",
        params![name, info.source, info.location.to_string_lossy(), enabled],
    )?;
    if count != 1 {
        return Err(AppError::Conflict("Skill 来源在切换时已改变".into()));
    }
    tx.commit()?;
    Ok(SkillInfo { enabled, ..info })
}

#[cfg(test)]
#[path = "skills_tests.rs"]
mod tests;
