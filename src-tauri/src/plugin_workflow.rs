//! Declarative package authoring and deployment. Scratch writes are inert;
//! draft/deployment files are changed only by approved FileEngine transactions.
//! Registration is replayable and never executes package content or publishes it.
use crate::{
    error::{AppError, AppResult},
    paths,
    plugins::{self, GamePackage},
    transaction::{FileEngine, FileOp, FilePlan},
    util,
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

const ADAPTER_LIMIT: usize = 256 * 1024;
const SKILL_LIMIT: usize = 128 * 1024;
const RECORD_LIMIT: usize = 1024 * 1024;
const SOURCE: &str = "external";
static WORKFLOW_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    WORKFLOW_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
fn invalid(message: impl Into<String>) -> AppError {
    AppError::Invalid(message.into())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftInfo {
    pub id: String,
    pub slug: String,
    pub path: PathBuf,
    pub write_transaction_id: String,
    pub created_at: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftPreview {
    pub draft: DraftInfo,
    pub plan: FilePlan,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageHashes {
    pub adapter_sha256: String,
    pub skill_sha256: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowIssue {
    pub code: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftValidation {
    pub draft: DraftInfo,
    pub transaction_status: String,
    pub valid: bool,
    pub package: Option<GamePackage>,
    pub hashes: Option<PackageHashes>,
    pub issues: Vec<WorkflowIssue>,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentPreview {
    pub draft_id: String,
    pub slug: String,
    pub version: String,
    pub install_path: PathBuf,
    pub hashes: PackageHashes,
    pub plan: FilePlan,
}
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileReport {
    pub registered: Vec<String>,
    pub disabled: Vec<String>,
    pub issues: Vec<WorkflowIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Content {
    adapter_json: String,
    skill_markdown: String,
    hashes: PackageHashes,
}
impl Content {
    fn new(adapter_json: String, skill_markdown: String) -> Self {
        let hashes = PackageHashes {
            adapter_sha256: hex::encode(Sha256::digest(adapter_json.as_bytes())),
            skill_sha256: hex::encode(Sha256::digest(skill_markdown.as_bytes())),
        };
        Self {
            adapter_json,
            skill_markdown,
            hashes,
        }
    }
    fn package(&self, slug: &str) -> AppResult<GamePackage> {
        if self.adapter_json.len() > ADAPTER_LIMIT || self.skill_markdown.len() > SKILL_LIMIT {
            return Err(invalid("包内容超出大小限制"));
        }
        if Self::new(self.adapter_json.clone(), self.skill_markdown.clone()).hashes != self.hashes {
            return Err(AppError::Conflict(
                "持久包记录的 SHA-256 与原文不一致".into(),
            ));
        }
        plugins::parse_package(slug, &self.adapter_json, &self.skill_markdown)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviousPackage {
    content: Content,
    /// None only for a pre-existing, registered external package baseline.
    deployment_id: Option<String>,
    plugin_enabled: bool,
    skill_enabled: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeploymentRecord {
    schema_version: u32,
    content: Content,
    previous: Option<PreviousPackage>,
    plugin_enabled: bool,
    skill_enabled: bool,
}
struct DeploymentRow {
    transaction_id: String,
    slug: String,
    install_path: PathBuf,
    status: String,
}

fn checked_path(engine: &FileEngine, path: &Path) -> AppResult<PathBuf> {
    let root = &engine.paths.root;
    engine.paths.stable_directory(root)?;
    if !path.is_absolute()
        || !path.starts_with(root)
        || path.components().any(|c| matches!(c, Component::ParentDir))
    {
        return Err(AppError::PathEscape(path.display().to_string()));
    }
    let mut current = root.clone();
    for part in path
        .strip_prefix(root)
        .map_err(|e| invalid(e.to_string()))?
        .components()
    {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(AppError::Denied(format!(
                    "拒绝包路径符号链接: {}",
                    current.display()
                )))
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(e.into()),
        }
    }
    let canonical = paths::ensure_within(path, std::slice::from_ref(root))?;
    if canonical != path {
        return Err(AppError::Denied("包路径已被重定向".into()));
    }
    Ok(canonical)
}

fn external_slug(engine: &FileEngine, slug: &str) -> AppResult<()> {
    plugins::validate_slug(slug)?;
    if plugins::BUILTIN_PACKAGES
        .iter()
        .any(|(name, _, _)| *name == slug)
    {
        return Err(AppError::Denied(
            "禁止覆盖内置适配包 slug，请使用新 slug".into(),
        ));
    }
    let conn = engine.db.conn();
    for (table, key) in [("plugins", "slug"), ("skills", "name")] {
        let source: Option<String> = conn
            .query_row(
                &format!("SELECT source FROM {table} WHERE {key}=?1"),
                [slug],
                |r| r.get(0),
            )
            .optional()?;
        if source.as_deref().is_some_and(|s| s != SOURCE) {
            return Err(AppError::Conflict(format!(
                "保留已有 {table} 来源，不接管: {slug}"
            )));
        }
    }
    Ok(())
}
fn install_path(engine: &FileEngine, slug: &str) -> AppResult<PathBuf> {
    plugins::validate_slug(slug)?;
    checked_path(engine, &engine.paths.plugins().join("games").join(slug))
}
fn draft_path(engine: &FileEngine, id: &str, slug: &str) -> AppResult<PathBuf> {
    if !id.strip_prefix("draft_").is_some_and(|s| {
        s.len() == 32
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) {
        return Err(invalid("草稿 ID 格式无效"));
    }
    plugins::validate_slug(slug)?;
    checked_path(
        engine,
        &engine
            .paths
            .staging()
            .join("plugin-drafts")
            .join(id)
            .join(slug),
    )
}
fn scratch_path(engine: &FileEngine, id: &str) -> AppResult<PathBuf> {
    checked_path(
        engine,
        &engine.paths.staging().join("plugin-scratch").join(id),
    )
}
fn read_text(engine: &FileEngine, path: &Path, limit: usize) -> AppResult<String> {
    checked_path(engine, path)?;
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.len() > limit as u64 {
        return Err(invalid("包文件须为大小受限的普通文件"));
    }
    let file = fs::File::open(path)?;
    checked_path(engine, path)?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid("读取包文件超过上限"));
    }
    String::from_utf8(bytes).map_err(|_| invalid("包文件必须是 UTF-8"))
}
fn read_content(engine: &FileEngine, directory: &Path) -> AppResult<Content> {
    checked_path(engine, directory)?;
    if !fs::symlink_metadata(directory)?.is_dir() {
        return Err(invalid("包路径不是普通目录"));
    }
    let mut entries = fs::read_dir(directory)?;
    let mut count = 0;
    for entry in entries.by_ref().take(3) {
        let entry = entry?;
        if !["adapter.json", "SKILL.md"]
            .iter()
            .any(|n| entry.file_name() == *n)
        {
            return Err(AppError::Denied(
                "声明包只能包含 adapter.json 与 SKILL.md，不接管其他文件".into(),
            ));
        }
        count += 1;
    }
    if count != 2 {
        return Err(invalid("声明包必须恰好包含两个文件"));
    }
    Ok(Content::new(
        read_text(engine, &directory.join("adapter.json"), ADAPTER_LIMIT)?,
        read_text(engine, &directory.join("SKILL.md"), SKILL_LIMIT)?,
    ))
}

/// Invalid content may be saved for iteration, but can never be installed.
/// Only new inert scratch files are written here; target writes require approval.
pub fn plan_write_draft(
    engine: &FileEngine,
    slug: &str,
    adapter_json: &str,
    skill_markdown: &str,
) -> AppResult<DraftPreview> {
    let _lock = lock();
    external_slug(engine, slug)?;
    if adapter_json.len() > ADAPTER_LIMIT || skill_markdown.len() > SKILL_LIMIT {
        return Err(invalid("草稿文件超过适配包/Skill 大小上限"));
    }
    let id = util::new_id("draft");
    let target = draft_path(engine, &id, slug)?;
    if fs::symlink_metadata(&target).is_ok() {
        return Err(AppError::Conflict("草稿目标已存在".into()));
    }
    let scratch = scratch_path(engine, &id)?;
    let parent = scratch
        .parent()
        .ok_or_else(|| invalid("scratch 没有父目录"))?;
    {
        let _files = crate::transaction::write_lock();
        checked_path(engine, parent)?;
        fs::create_dir_all(parent)?;
        checked_path(engine, parent)?;
        fs::create_dir(&scratch)?;
        for (name, content) in [("adapter.json", adapter_json), ("SKILL.md", skill_markdown)] {
            let path = scratch.join(name);
            checked_path(engine, &path)?;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
        }
    }
    let plan = engine.plan_file_ops(
        ["adapter.json", "SKILL.md"]
            .iter()
            .map(|name| FileOp::Copy {
                src: scratch.join(name),
                dst: target.join(name),
            })
            .collect(),
        format!("写入声明式插件草稿 {slug}"),
    )?;
    let plan = engine.require_review(&plan.id)?;
    let draft = DraftInfo {
        id,
        slug: slug.into(),
        path: target,
        write_transaction_id: plan.id.clone(),
        created_at: util::now(),
    };
    engine.db.conn().execute("INSERT INTO plugin_drafts(id,slug,path,write_transaction_id,created_at) VALUES (?1,?2,?3,?4,?5)",
        params![draft.id,draft.slug,draft.path.to_string_lossy(),draft.write_transaction_id,draft.created_at])?;
    Ok(DraftPreview { draft, plan })
}

fn get_draft(engine: &FileEngine, id: &str) -> AppResult<DraftInfo> {
    engine
        .db
        .conn()
        .query_row(
            "SELECT id,slug,path,write_transaction_id,created_at FROM plugin_drafts WHERE id=?1",
            [id],
            |r| {
                Ok(DraftInfo {
                    id: r.get(0)?,
                    slug: r.get(1)?,
                    path: PathBuf::from(r.get::<_, String>(2)?),
                    write_transaction_id: r.get(3)?,
                    created_at: r.get(4)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("插件草稿: {id}")))
}
fn transaction_status(engine: &FileEngine, id: &str) -> AppResult<String> {
    engine
        .db
        .conn()
        .query_row(
            "SELECT status FROM file_transactions WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("文件事务: {id}")))
}

/// A committed draft remains discoverable after process restart. Validation is
/// separate, so an invalid saved draft can still be found and explained.
pub fn list_drafts(engine: &FileEngine) -> AppResult<Vec<DraftInfo>> {
    let _lock = lock();
    let conn = engine.db.conn();
    let mut stmt = conn.prepare("SELECT d.id,d.slug,d.path,d.write_transaction_id,d.created_at FROM plugin_drafts d JOIN file_transactions f ON f.id=d.write_transaction_id WHERE f.status='committed' ORDER BY d.created_at DESC,d.rowid DESC")?;
    let rows = stmt.query_map([], |r| {
        Ok(DraftInfo {
            id: r.get(0)?,
            slug: r.get(1)?,
            path: PathBuf::from(r.get::<_, String>(2)?),
            write_transaction_id: r.get(3)?,
            created_at: r.get(4)?,
        })
    })?;
    let mut drafts = Vec::new();
    for row in rows {
        let draft = row?;
        if draft_path(engine, &draft.id, &draft.slug)? != draft.path {
            return Err(AppError::Denied("草稿记录路径与 ID 不一致".into()));
        }
        drafts.push(draft);
    }
    Ok(drafts)
}

fn verify_copy_plan(
    plan: &FilePlan,
    id: &str,
    source: &Path,
    destination: &Path,
    content: &Content,
    new_only: bool,
) -> AppResult<()> {
    if plan.id != id || !plan.requires_review || plan.entries.len() != 2 {
        return Err(invalid("持久计划不是已要求审查的双文件包事务"));
    }
    for (name, hash, size) in [
        (
            "adapter.json",
            &content.hashes.adapter_sha256,
            content.adapter_json.len(),
        ),
        (
            "SKILL.md",
            &content.hashes.skill_sha256,
            content.skill_markdown.len(),
        ),
    ] {
        let entry = plan.entries.iter().find(|e| matches!(&e.operation, FileOp::Copy { src, dst } if *src == source.join(name) && *dst == destination.join(name)))
            .ok_or_else(|| invalid("持久计划包含不匹配的源或目标"))?;
        if new_only && entry.before.is_some() {
            return Err(invalid("新草稿不能覆盖既存文件"));
        }
        if !entry
            .source
            .as_ref()
            .is_some_and(|s| s.kind == "file" && s.sha256 == *hash && s.size == size as u64)
        {
            return Err(AppError::Conflict("包哈希与批准计划的源快照不一致".into()));
        }
    }
    Ok(())
}

fn validate_inner(engine: &FileEngine, id: &str) -> AppResult<DraftValidation> {
    let draft = get_draft(engine, id)?;
    let status = transaction_status(engine, &draft.write_transaction_id)?;
    let mut report = DraftValidation { draft, transaction_status:status, valid:false, package:None, hashes:None, issues:vec![],
        warnings:vec!["仅验证声明结构与事务完整性，不证明游戏路径资料、兼容性或 Skill 指令可信；必须人工审查。".into()] };
    let result = (|| -> AppResult<(Content, GamePackage)> {
        external_slug(engine, &report.draft.slug)?;
        let expected = draft_path(engine, id, &report.draft.slug)?;
        if expected != report.draft.path {
            return Err(AppError::Denied("草稿路径与登记 ID/slug 不一致".into()));
        }
        if report.transaction_status != "committed" {
            return Err(AppError::Conflict("草稿写入事务尚未提交或已回滚".into()));
        }
        let content = read_content(engine, &expected)?;
        verify_copy_plan(
            &engine.get_plan(&report.draft.write_transaction_id)?,
            &report.draft.write_transaction_id,
            &scratch_path(engine, id)?,
            &expected,
            &content,
            true,
        )?;
        let package = content.package(&report.draft.slug)?;
        Ok((content, package))
    })();
    match result {
        Ok((content, package)) => {
            report.valid = true;
            report.hashes = Some(content.hashes);
            report.package = Some(package);
        }
        Err(error) => report.issues.push(WorkflowIssue {
            code: "invalid_draft".into(),
            message: error.to_string(),
        }),
    }
    Ok(report)
}
pub fn validate_draft(engine: &FileEngine, draft_id: &str) -> AppResult<DraftValidation> {
    let _lock = lock();
    let _files = crate::transaction::write_lock();
    validate_inner(engine, draft_id)
}

fn deployment_rows(engine: &FileEngine) -> AppResult<Vec<DeploymentRow>> {
    let conn = engine.db.conn();
    let mut stmt = conn.prepare("SELECT p.transaction_id,p.slug,p.install_path,coalesce(f.status,'missing') FROM plugin_deployments p LEFT JOIN file_transactions f ON f.id=p.transaction_id ORDER BY p.rowid DESC")?;
    let rows = stmt.query_map([], |r| {
        Ok(DeploymentRow {
            transaction_id: r.get(0)?,
            slug: r.get(1)?,
            install_path: PathBuf::from(r.get::<_, String>(2)?),
            status: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
fn deployment_record(engine: &FileEngine, row: &DeploymentRow) -> AppResult<DeploymentRecord> {
    let json: String = engine.db.conn().query_row("SELECT CASE WHEN length(CAST(package_json AS BLOB))<=?2 THEN package_json ELSE '' END FROM plugin_deployments WHERE transaction_id=?1", params![row.transaction_id,RECORD_LIMIT], |r| r.get(0))?;
    let record: DeploymentRecord = serde_json::from_str(&json)?;
    if record.schema_version != 1 {
        return Err(invalid("部署记录版本不受支持"));
    }
    record.content.package(&row.slug)?;
    if let Some(previous) = &record.previous {
        previous.content.package(&row.slug)?;
    }
    let expected = install_path(engine, &row.slug)?;
    if row.install_path != expected {
        return Err(AppError::Denied("部署路径不属于声明包目标".into()));
    }
    let plan = engine.get_plan(&row.transaction_id)?;
    // The draft may have been parked by a later rollback; only its original
    // spelling and committed source fingerprint are needed for replay.
    let source = plan
        .entries
        .first()
        .and_then(|e| match &e.operation {
            FileOp::Copy { src, .. } => src.parent(),
            _ => None,
        })
        .ok_or_else(|| invalid("部署不是复制事务"))?;
    let slug = source
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| invalid("部署草稿路径无效"))?;
    let id = source
        .parent()
        .and_then(Path::file_name)
        .and_then(|s| s.to_str())
        .ok_or_else(|| invalid("部署草稿 ID 缺失"))?;
    if slug != row.slug || draft_path(engine, id, slug)? != source {
        return Err(invalid("部署源不是登记草稿目录"));
    }
    let draft = get_draft(engine, id)?;
    if draft.slug != slug || draft.path != source {
        return Err(invalid("部署源不匹配持久草稿记录"));
    }
    verify_copy_plan(
        &plan,
        &row.transaction_id,
        source,
        &expected,
        &record.content,
        false,
    )?;
    verify_previous(&plan, &expected, record.previous.as_ref())?;
    Ok(record)
}

fn verify_previous(
    plan: &FilePlan,
    target: &Path,
    previous: Option<&PreviousPackage>,
) -> AppResult<()> {
    if let Some(previous) = previous {
        for (name, hash, size) in [
            (
                "adapter.json",
                &previous.content.hashes.adapter_sha256,
                previous.content.adapter_json.len(),
            ),
            (
                "SKILL.md",
                &previous.content.hashes.skill_sha256,
                previous.content.skill_markdown.len(),
            ),
        ] {
            if !plan.entries.iter().any(|e| {
                e.operation.dst() == target.join(name)
                    && e.before.as_ref().is_some_and(|s| {
                        s.kind == "file" && s.sha256 == *hash && s.size == size as u64
                    })
            }) {
                return Err(AppError::Conflict("旧包快照与事务覆盖前哈希不一致".into()));
            }
        }
    }
    Ok(())
}

#[derive(Clone)]
struct RegistryState {
    plugin_enabled: bool,
    skill_enabled: bool,
    validated: bool,
    adapter_json: String,
}
fn registry_state(
    engine: &FileEngine,
    slug: &str,
    target: &Path,
) -> AppResult<Option<RegistryState>> {
    let conn = engine.db.conn();
    let plugin: Option<(String, String, bool, bool, String)> = conn
        .query_row(
            "SELECT source,install_path,enabled,validated,adapter_json FROM plugins WHERE slug=?1",
            [slug],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let skill: Option<(String, String, bool)> = conn
        .query_row(
            "SELECT source,location,enabled FROM skills WHERE name=?1",
            [slug],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if plugin
        .as_ref()
        .is_some_and(|(s, p, ..)| s != SOURCE || Path::new(p) != target)
        || skill
            .as_ref()
            .is_some_and(|(s, p, _)| s != SOURCE || Path::new(p) != target.join("SKILL.md"))
    {
        return Err(AppError::Conflict(
            "已有插件或 Skill 路径/来源不属于此部署，拒绝接管".into(),
        ));
    }
    Ok(
        plugin.map(|(_, _, enabled, validated, adapter_json)| RegistryState {
            plugin_enabled: enabled,
            skill_enabled: skill.is_none_or(|(_, _, enabled)| enabled),
            validated,
            adapter_json,
        }),
    )
}

/// Produces exactly two reviewed copies. FileEngine retains any overwritten
/// external version in its normal journal/backup and owns rollback approval.
pub fn plan_install_plugin(engine: &FileEngine, draft_id: &str) -> AppResult<DeploymentPreview> {
    let _lock = lock();
    reconcile_inner(engine)?;
    let validation = validate_inner(engine, draft_id)?;
    if !validation.valid {
        return Err(invalid(
            validation
                .issues
                .iter()
                .map(|i| i.message.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    let draft = validation.draft;
    let content = read_content(engine, &draft.path)?;
    if validation.hashes.as_ref() != Some(&content.hashes) {
        return Err(AppError::Conflict("验证之后草稿已改变".into()));
    }
    let package = content.package(&draft.slug)?;
    external_slug(engine, &draft.slug)?;
    let target = install_path(engine, &draft.slug)?;
    let state = registry_state(engine, &draft.slug, &target)?;
    let mut previous = None;
    if fs::symlink_metadata(&target).is_ok() && fs::read_dir(&target)?.next().is_some() {
        if state.is_none() {
            return Err(AppError::Conflict(
                "目标包含未登记的包，不覆盖未知文件".into(),
            ));
        }
        let old = read_content(engine, &target)?;
        let old_package = old.package(&draft.slug)?;
        let state = state.as_ref().unwrap();
        if state.validated
            && serde_json::from_str::<serde_json::Value>(&state.adapter_json)?
                == serde_json::to_value(&old_package.adapter)?
        {
            let mut provenance = None;
            for row in deployment_rows(engine)?
                .iter()
                .filter(|r| r.slug == draft.slug && r.status == "committed")
            {
                if let Ok(record) = deployment_record(engine, row) {
                    if record.content.hashes == old.hashes {
                        provenance = Some(row.transaction_id.clone());
                        break;
                    }
                }
            }
            previous = Some(PreviousPackage {
                content: old,
                deployment_id: provenance,
                plugin_enabled: state.plugin_enabled,
                skill_enabled: state.skill_enabled,
            });
        }
    }
    let record = DeploymentRecord {
        schema_version: 1,
        content: content.clone(),
        previous,
        plugin_enabled: state.as_ref().is_none_or(|s| s.plugin_enabled),
        skill_enabled: state.as_ref().is_none_or(|s| s.skill_enabled),
    };
    let json = serde_json::to_string(&record)?;
    if json.len() > RECORD_LIMIT {
        return Err(invalid("部署回放记录超过上限"));
    }
    let plan = engine.plan_file_ops(
        ["adapter.json", "SKILL.md"]
            .iter()
            .map(|name| FileOp::Copy {
                src: draft.path.join(name),
                dst: target.join(name),
            })
            .collect(),
        format!(
            "安装声明式适配包 {} {}",
            draft.slug, package.adapter.version
        ),
    )?;
    let plan = engine.require_review(&plan.id)?;
    verify_copy_plan(&plan, &plan.id, &draft.path, &target, &content, false)?;
    verify_previous(&plan, &target, record.previous.as_ref())?;
    engine.db.conn().execute("INSERT INTO plugin_deployments(transaction_id,slug,package_json,install_path,created_at) VALUES (?1,?2,?3,?4,?5)",params![plan.id,draft.slug,json,target.to_string_lossy(),util::now()])?;
    Ok(DeploymentPreview {
        draft_id: draft.id,
        slug: draft.slug,
        version: package.adapter.version,
        install_path: target,
        hashes: content.hashes,
        plan,
    })
}

fn register(
    engine: &FileEngine,
    slug: &str,
    target: &Path,
    content: &Content,
    plugin_enabled: bool,
    skill_enabled: bool,
) -> AppResult<()> {
    let package = content.package(slug)?;
    external_slug(engine, slug)?;
    let state = registry_state(engine, slug, target)?;
    let preserve = state.as_ref().is_some_and(|s| s.validated);
    let plugin_enabled = if preserve {
        state.as_ref().unwrap().plugin_enabled
    } else {
        plugin_enabled
    };
    let skill_enabled = if preserve {
        state.as_ref().unwrap().skill_enabled
    } else {
        skill_enabled
    };
    let mut conn = engine.db.conn();
    let tx = conn.transaction()?;
    let now = util::now();
    tx.execute("INSERT INTO plugins(slug,name,game_name,version,description,source,enabled,validated,install_path,adapter_json,created_at,updated_at) VALUES (?1,?2,?2,?3,?4,'external',?5,1,?6,?7,?8,?8) ON CONFLICT(slug) DO UPDATE SET name=excluded.name,game_name=excluded.game_name,version=excluded.version,description=excluded.description,enabled=excluded.enabled,validated=1,install_path=excluded.install_path,adapter_json=excluded.adapter_json,updated_at=excluded.updated_at",params![slug,package.adapter.name,package.adapter.version,package.skill.description,plugin_enabled,target.to_string_lossy(),content.adapter_json,now])?;
    tx.execute("INSERT INTO skills(name,description,location,source,enabled,created_at) VALUES (?1,?2,?3,'external',?4,?5) ON CONFLICT(name) DO UPDATE SET description=excluded.description,location=excluded.location,enabled=excluded.enabled",params![slug,package.skill.description,target.join("SKILL.md").to_string_lossy(),skill_enabled,now])?;
    tx.commit()?;
    Ok(())
}
fn disable(engine: &FileEngine, slug: &str, target: &Path) -> AppResult<bool> {
    // Never disable an unrelated Skill collision or delete any history/file.
    registry_state(engine, slug, target)?;
    let mut conn = engine.db.conn();
    let tx = conn.transaction()?;
    let count = tx.execute("UPDATE plugins SET enabled=0,validated=0,updated_at=?3 WHERE slug=?1 AND source='external' AND install_path=?2",params![slug,target.to_string_lossy(),util::now()])?;
    tx.execute(
        "UPDATE skills SET enabled=0 WHERE name=?1 AND source='external' AND location=?2",
        params![slug, target.join("SKILL.md").to_string_lossy()],
    )?;
    tx.commit()?;
    Ok(count != 0)
}

fn reconcile_inner(engine: &FileEngine) -> AppResult<ReconcileReport> {
    let _files = crate::transaction::write_lock();
    let mut groups: BTreeMap<String, Vec<DeploymentRow>> = BTreeMap::new();
    for row in deployment_rows(engine)? {
        groups.entry(row.slug.clone()).or_default().push(row);
    }
    let mut report = ReconcileReport::default();
    for (slug, rows) in groups {
        let resolved = (|| -> AppResult<PathBuf> {
            external_slug(engine, &slug)?;
            // Derive the owned spelling even if the disk path is now a link:
            // unsafe disk state must disable, not preserve a trusted DB flag.
            let target = engine.paths.plugins().join("games").join(&slug);
            registry_state(engine, &slug, &target)?;
            Ok(target)
        })();
        let target = match resolved {
            Ok(path) => path,
            Err(e) => {
                report.issues.push(WorkflowIssue {
                    code: "ownership".into(),
                    message: format!("{slug}: {e}"),
                });
                continue;
            }
        };
        if rows.iter().any(|r| {
            matches!(
                r.status.as_str(),
                "applying" | "rolling_back" | "recovery_failed"
            )
        }) {
            if disable(engine, &slug, &target)? {
                report.disabled.push(slug.clone());
            }
            report.issues.push(WorkflowIssue {
                code: "recovery_required".into(),
                message: format!("{slug}: 文件事务尚未恢复；不读取中间包状态"),
            });
            continue;
        }
        let disk = read_content(engine, &target).and_then(|c| {
            c.package(&slug)?;
            Ok(c)
        });
        let mut selected = None;
        if let Ok(disk) = &disk {
            for row in rows.iter().filter(|r| r.status == "committed") {
                match deployment_record(engine, row) {
                    Ok(record) if record.content.hashes == disk.hashes => {
                        selected =
                            Some((record.content, record.plugin_enabled, record.skill_enabled));
                        break;
                    }
                    Ok(_) => {}
                    Err(e) => report.issues.push(WorkflowIssue {
                        code: "invalid_record".into(),
                        message: format!("{}: {e}", row.transaction_id),
                    }),
                }
            }
            if selected.is_none() {
                // Only a verified pre-workflow baseline can be recovered without
                // its own deployment commit; rolled-back deployment snapshots
                // cannot resurrect another rolled-back version.
                for row in rows.iter().filter(|r| {
                    matches!(
                        r.status.as_str(),
                        "planned" | "approved" | "rolled_back" | "failed"
                    )
                }) {
                    if let Ok(record) = deployment_record(engine, row) {
                        if let Some(previous) = record.previous.filter(|p| {
                            p.deployment_id.is_none() && p.content.hashes == disk.hashes
                        }) {
                            selected = Some((
                                previous.content,
                                previous.plugin_enabled,
                                previous.skill_enabled,
                            ));
                            break;
                        }
                    }
                }
            }
        }
        if let Some((content, plugin_enabled, skill_enabled)) = selected {
            register(
                engine,
                &slug,
                &target,
                &content,
                plugin_enabled,
                skill_enabled,
            )?;
            report.registered.push(slug);
        } else {
            if disable(engine, &slug, &target)? {
                report.disabled.push(slug.clone());
            }
            report.issues.push(WorkflowIssue {
                code: "unregistered_disk_state".into(),
                message: format!(
                    "{slug}: {}",
                    disk.err().map_or_else(
                        || "磁盘包与已提交部署/已恢复基线不匹配；未注册".into(),
                        |e| e.to_string()
                    )
                ),
            });
        }
    }
    Ok(report)
}
/// Run after startup recovery, apply, rollback and before exposing packages.
/// Registry writes for each package/Skill pair are atomic; files are never changed.
pub fn reconcile_deployments(engine: &FileEngine) -> AppResult<ReconcileReport> {
    let _lock = lock();
    reconcile_inner(engine)
}

/// Desktop metadata control, not a package install or a filesystem mutation.
/// Keep the Skill's independent user preference; readers compute its effective
/// state from both flags. Enabling never trusts a registry boolean alone.
pub fn set_enabled(
    engine: &FileEngine,
    slug: &str,
    enabled: bool,
) -> AppResult<plugins::PluginInfo> {
    let _lock = lock();
    plugins::validate_slug(slug)?;
    reconcile_inner(engine)?;
    let _files = crate::transaction::write_lock();
    let info = plugins::list_plugins(&engine.db)?
        .into_iter()
        .find(|p| p.slug == slug)
        .ok_or_else(|| AppError::NotFound(format!("适配包: {slug}")))?;
    let target = engine.paths.plugins().join("games").join(slug);
    if !matches!(info.source.as_str(), "builtin" | SOURCE)
        || Path::new(&info.install_path) != target
    {
        return Err(AppError::Denied(
            "不能切换来源或安装位置未知的适配包".into(),
        ));
    }
    if enabled {
        let content = read_content(engine, &target)?;
        let package = content.package(slug)?;
        let registered: String = engine.db.conn().query_row(
            "SELECT CASE WHEN length(CAST(adapter_json AS BLOB))<=?2 THEN adapter_json ELSE '' END FROM plugins WHERE slug=?1",
            params![slug, ADAPTER_LIMIT],
            |r| r.get(0),
        )?;
        if !info.validated
            || package.adapter.version != info.version
            || serde_json::from_str::<serde_json::Value>(&registered)?
                != serde_json::to_value(&package.adapter)?
        {
            return Err(AppError::Denied("适配包内容与已校验登记不一致".into()));
        }
        let proven = if info.source == "builtin" {
            plugins::BUILTIN_PACKAGES
                .iter()
                .any(|(name, adapter, skill)| {
                    *name == slug
                        && *adapter == content.adapter_json
                        && *skill == content.skill_markdown
                })
        } else {
            let mut matched = false;
            for row in deployment_rows(engine)?
                .iter()
                .filter(|row| row.slug == slug && row.status == "committed")
            {
                if deployment_record(engine, row)?.content.hashes == content.hashes {
                    matched = true;
                    break;
                }
            }
            matched
        };
        if !proven {
            return Err(AppError::Denied(
                "适配包缺少匹配的内置版本或已提交部署".into(),
            ));
        }
    }
    let count = engine.db.conn().execute(
        "UPDATE plugins SET enabled=?2,updated_at=?3 WHERE slug=?1 AND source=?4 AND install_path=?5",
        params![slug, enabled, util::now(), info.source, info.install_path],
    )?;
    if count != 1 {
        return Err(AppError::Conflict("适配包归属在切换时已改变".into()));
    }
    Ok(plugins::PluginInfo { enabled, ..info })
}

#[cfg(test)]
#[path = "plugin_workflow_tests.rs"]
mod tests;
