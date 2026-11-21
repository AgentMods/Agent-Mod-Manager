//! Local, consent-bound publication preparation. This module never performs I/O
//! over the network and never stores credentials or remote response bodies.
use crate::{
    error::{AppError, AppResult},
    plugin_workflow, plugins,
    transaction::{FileEngine, FileOp, FilePlan},
    util,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::{Cursor, Read, Write},
    path::{Component, Path},
    sync::MutexGuard,
};

pub const DESTINATION: &str = "https://www.glossmod.com/api/v1/mcp";
pub const MAX_ARCHIVE_BYTES: usize = 1024 * 1024;
pub(crate) const ADAPTER_LIMIT: usize = 256 * 1024;
pub(crate) const SKILL_LIMIT: usize = 128 * 1024;
const PREVIEW_LIMIT: usize = 3 * 1024 * 1024;

fn invalid(message: &str) -> AppError {
    AppError::Invalid(message.into())
}
fn conflict(message: &str) -> AppError {
    AppError::Conflict(message.into())
}
fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicationMetadata {
    pub title: String,
    pub summary: String,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub game_ids: Vec<i64>,
}
impl PublicationMetadata {
    pub fn validate(&self) -> AppResult<()> {
        let title_len = self.title.chars().count();
        if !(2..=160).contains(&title_len)
            || self.title.trim().chars().count() < 2
            || self.title.chars().any(char::is_control)
            || self.summary.chars().count() > 500
            || self.description.chars().count() > 50_000
            || self.summary.contains('\0')
            || self.description.contains('\0')
        {
            return Err(invalid(
                "Publication title/summary/description exceeds its bounds",
            ));
        }
        let mut tags = BTreeSet::new();
        if self.tags.len() > 5
            || self.tags.iter().any(|tag| {
                tag.is_empty()
                    || tag.trim() != tag
                    || tag.chars().count() > 20
                    || tag.chars().any(char::is_control)
                    || !tags.insert(tag)
            })
        {
            return Err(invalid(
                "Publication tags must be distinct, nonempty, at most 5 x 20 characters",
            ));
        }
        let mut games = BTreeSet::new();
        if self.game_ids.len() > 10 || self.game_ids.iter().any(|id| *id <= 0 || !games.insert(id))
        {
            return Err(invalid(
                "Publication game IDs must be distinct positive integers, at most 10",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicationFile {
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicPackage {
    pub draft_id: Option<String>,
    pub slug: String,
    pub version: String,
    pub title: String,
    pub summary: String,
    pub description: String,
    pub tags: Vec<String>,
    pub game_ids: Vec<i64>,
    pub files: [PublicationFile; 2],
    pub archive_name: String,
    pub archive_sha256: String,
    pub archive_size: u64,
    pub content_type: String,
    pub work_status: String,
    pub destination: String,
}
impl PublicPackage {
    fn metadata(&self) -> PublicationMetadata {
        PublicationMetadata {
            title: self.title.clone(),
            summary: self.summary.clone(),
            description: self.description.clone(),
            tags: self.tags.clone(),
            game_ids: self.game_ids.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PublicationSource {
    Draft {
        draft_id: String,
        write_transaction_id: String,
    },
    Installed {
        slug: String,
        source: String,
        // Older publication records can lack a deployment ID.
        deployment_transaction_id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicationPreview {
    pub package: PublicPackage,
    pub source: PublicationSource,
    pub digest: String,
}

/// Deliberately neither Debug nor Serialize. The remote layer receives bytes
/// only after approval and fresh provenance/content verification.
pub struct PackageBytes(Vec<u8>);
impl PackageBytes {
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
    pub fn into_vec(self) -> Vec<u8> {
        self.0
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn checked_path(engine: &FileEngine, path: &Path) -> AppResult<()> {
    let root = &engine.paths.root;
    engine.paths.stable_directory(root)?;
    if !path.is_absolute()
        || !path.starts_with(root)
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(AppError::PathEscape(
            "Publication source is not a managed package path".into(),
        ));
    }
    let mut current = root.clone();
    for part in path
        .strip_prefix(root)
        .map_err(|_| invalid("Invalid source path"))?
        .components()
    {
        current.push(part);
        if fs::symlink_metadata(&current)?.file_type().is_symlink() {
            return Err(AppError::Denied(
                "Publication source contains a symlink".into(),
            ));
        }
    }
    if path.canonicalize()? != path {
        return Err(AppError::Denied("Publication source was redirected".into()));
    }
    Ok(())
}

fn read_text(engine: &FileEngine, path: &Path, limit: usize) -> AppResult<String> {
    checked_path(engine, path)?;
    let before = fs::symlink_metadata(path)?;
    if !before.is_file() || before.len() > limit as u64 {
        return Err(invalid(
            "Publication files must be size-limited regular UTF-8 files",
        ));
    }
    let file = fs::File::open(path)?;
    let opened = file.metadata()?;
    checked_path(engine, path)?;
    let after = fs::symlink_metadata(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != opened.dev()
            || before.ino() != opened.ino()
            || after.dev() != opened.dev()
            || after.ino() != opened.ino()
        {
            return Err(conflict("Publication source changed while opening"));
        }
    }
    if !opened.is_file() || opened.len() != before.len() || opened.len() != after.len() {
        return Err(conflict("Publication source changed while opening"));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    checked_path(engine, path)?;
    if bytes.len() > limit {
        return Err(invalid("Publication file exceeds its read limit"));
    }
    String::from_utf8(bytes).map_err(|_| invalid("Publication file is not UTF-8"))
}

fn read_files(engine: &FileEngine, directory: &Path) -> AppResult<[PublicationFile; 2]> {
    checked_path(engine, directory)?;
    if !fs::symlink_metadata(directory)?.is_dir() {
        return Err(invalid("Publication source is not a directory"));
    }
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(directory)?.take(3) {
        names.insert(entry?.file_name());
    }
    if names.len() != 2
        || !names.contains(std::ffi::OsStr::new("adapter.json"))
        || !names.contains(std::ffi::OsStr::new("SKILL.md"))
    {
        return Err(AppError::Denied(
            "Publication package must contain exactly adapter.json and SKILL.md".into(),
        ));
    }
    let file = |name: &str, limit| -> AppResult<PublicationFile> {
        let text = read_text(engine, &directory.join(name), limit)?;
        Ok(PublicationFile {
            name: name.into(),
            size: text.len() as u64,
            sha256: sha256(text.as_bytes()),
            text,
        })
    };
    Ok([
        file("adapter.json", ADAPTER_LIMIT)?,
        file("SKILL.md", SKILL_LIMIT)?,
    ])
}

fn archive(files: &[PublicationFile; 2]) -> AppResult<PackageBytes> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644);
    for (index, name) in ["adapter.json", "SKILL.md"].iter().enumerate() {
        let file = &files[index];
        let limit = if index == 0 {
            ADAPTER_LIMIT
        } else {
            SKILL_LIMIT
        };
        if file.name != *name
            || file.text.len() > limit
            || file.size != file.text.len() as u64
            || file.sha256 != sha256(file.text.as_bytes())
        {
            return Err(invalid("Publication file name, size or digest is invalid"));
        }
        writer.start_file(*name, options)?;
        writer.write_all(file.text.as_bytes())?;
    }
    let bytes = writer.finish()?.into_inner();
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(invalid("Publication ZIP exceeds 1 MiB"));
    }
    Ok(PackageBytes(bytes))
}

fn build_preview(
    source: PublicationSource,
    slug: &str,
    files: [PublicationFile; 2],
    metadata: PublicationMetadata,
) -> AppResult<(PublicationPreview, PackageBytes)> {
    metadata.validate()?;
    let package = plugins::parse_package(slug, &files[0].text, &files[1].text)?;
    let bytes = archive(&files)?;
    let draft_id = match &source {
        PublicationSource::Draft { draft_id, .. } => Some(draft_id.clone()),
        _ => None,
    };
    let package = PublicPackage {
        draft_id,
        slug: slug.into(),
        version: package.adapter.version.clone(),
        title: metadata.title,
        summary: metadata.summary,
        description: metadata.description,
        tags: metadata.tags,
        game_ids: metadata.game_ids,
        files,
        archive_name: format!("{slug}-{}.zip", package.adapter.version),
        archive_sha256: sha256(bytes.as_slice()),
        archive_size: bytes.len() as u64,
        content_type: "application/zip".into(),
        work_status: "published".into(),
        destination: DESTINATION.into(),
    };
    let mut digest = Sha256::new();
    digest.update(b"amm.plugin-publication.v1\0");
    digest.update(serde_json::to_vec(&(&source, &package))?);
    Ok((
        PublicationPreview {
            package,
            source,
            digest: hex::encode(digest.finalize()),
        },
        bytes,
    ))
}

/// Read-only: no scratch file, archive on disk, registry update or SQL migration.
pub fn prepare_package(
    engine: &FileEngine,
    draft_id: &str,
    metadata: PublicationMetadata,
) -> AppResult<(PublicationPreview, PackageBytes)> {
    metadata.validate()?;
    with_draft_source(engine, draft_id, |source, slug, files| {
        build_preview(source, slug, files, metadata)
    })
}

/// Hold the verified draft under the file write lock through a local action.
/// The callback must not acquire that lock or run remote requests.
pub(crate) fn with_draft_source<T>(
    engine: &FileEngine,
    draft_id: &str,
    action: impl FnOnce(PublicationSource, &str, [PublicationFile; 2]) -> AppResult<T>,
) -> AppResult<T> {
    // Workflow locking precedes the file lock; do not invert its lock order.
    let validation = plugin_workflow::validate_draft(engine, draft_id)?;
    if !validation.valid || validation.transaction_status != "committed" {
        return Err(AppError::Denied(
            "Only a valid committed draft can be published".into(),
        ));
    }
    let source = PublicationSource::Draft {
        draft_id: draft_id.into(),
        write_transaction_id: validation.draft.write_transaction_id,
    };
    let _files = lock_live_draft_source(engine, &source)?;
    let files = read_files(engine, &validation.draft.path)?;
    let hashes = validation
        .hashes
        .ok_or_else(|| invalid("Draft provenance has no hashes"))?;
    if files[0].sha256 != hashes.adapter_sha256 || files[1].sha256 != hashes.skill_sha256 {
        return Err(conflict("Draft changed after validation"));
    }
    action(source, &validation.draft.slug, files)
}

// FileEngine deletion and restoration share this lock. Recheck after fresh
// preview verification and hold it through each durable publication transition
// that starts work. A step already durably pending may finish or be reconciled.
fn lock_live_draft_source(
    engine: &FileEngine,
    source: &PublicationSource,
) -> AppResult<Option<MutexGuard<'static, ()>>> {
    let PublicationSource::Draft {
        draft_id,
        write_transaction_id,
    } = source
    else {
        return Ok(None);
    };
    let guard = crate::transaction::write_lock();
    let conn = engine.db.conn();
    plugin_workflow::ensure_draft_not_deleted(&conn, draft_id)?;
    let committed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM plugin_drafts d JOIN file_transactions f
         ON f.id=d.write_transaction_id WHERE d.id=?1 AND f.id=?2 AND f.status='committed')",
        params![draft_id, write_transaction_id],
        |r| r.get(0),
    )?;
    if !committed {
        return Err(conflict("插件草稿写入事务不再处于已提交状态"));
    }
    Ok(Some(guard))
}

fn verify_plan(
    plan: &FilePlan,
    id: &str,
    source: &Path,
    target: &Path,
    files: &[PublicationFile; 2],
) -> AppResult<()> {
    if plan.id != id || !plan.requires_review || plan.entries.len() != 2 {
        return Err(invalid(
            "Installed publication provenance is not a reviewed two-file plan",
        ));
    }
    for file in files {
        let entry = plan.entries.iter().find(|entry| matches!(&entry.operation, FileOp::Copy { src, dst } if *src == source.join(&file.name) && *dst == target.join(&file.name)))
            .ok_or_else(|| invalid("Installed publication plan paths differ"))?;
        if !entry
            .source
            .as_ref()
            .is_some_and(|f| f.kind == "file" && f.size == file.size && f.sha256 == file.sha256)
        {
            return Err(conflict("Installed publication plan hashes differ"));
        }
    }
    Ok(())
}

fn installed_provenance(
    engine: &FileEngine,
    slug: &str,
    target: &Path,
    files: &[PublicationFile; 2],
) -> AppResult<String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Content {
        adapter_json: String,
        skill_markdown: String,
        hashes: plugin_workflow::PackageHashes,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Deployment {
        schema_version: u32,
        content: Content,
    }
    let rows: Vec<(String, String)> = {
        let conn = engine.db.conn();
        let mut stmt = conn.prepare("SELECT p.transaction_id,p.package_json FROM plugin_deployments p JOIN file_transactions f ON f.id=p.transaction_id WHERE p.slug=?1 AND p.install_path=?2 AND f.status='committed' AND length(CAST(p.package_json AS BLOB))<=1048576 ORDER BY p.rowid DESC")?;
        let rows = stmt.query_map(params![slug, target.to_string_lossy()], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (id, json) in rows {
        let record: Deployment = serde_json::from_str(&json)?;
        if record.schema_version != 1
            || record.content.adapter_json != files[0].text
            || record.content.skill_markdown != files[1].text
            || record.content.hashes.adapter_sha256 != files[0].sha256
            || record.content.hashes.skill_sha256 != files[1].sha256
        {
            continue;
        }
        let plan = engine.get_plan(&id)?;
        let source = plan
            .entries
            .first()
            .and_then(|e| match &e.operation {
                FileOp::Copy { src, .. } => src.parent(),
                _ => None,
            })
            .ok_or_else(|| invalid("Deployment has no draft source"))?;
        let draft: Option<(String, String)> = engine
            .db
            .conn()
            .query_row(
                "SELECT id,write_transaction_id FROM plugin_drafts WHERE slug=?1 AND path=?2",
                params![slug, source.to_string_lossy()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (draft_id, write_id) =
            draft.ok_or_else(|| invalid("Deployment draft provenance is missing"))?;
        validate_id(&draft_id, "draft")?;
        let expected = engine
            .paths
            .staging()
            .join("plugin-drafts")
            .join(&draft_id)
            .join(slug);
        if expected != source {
            return Err(invalid("Deployment draft source path differs"));
        }
        verify_plan(&plan, &id, source, target, files)?;
        verify_plan(
            &engine.get_plan(&write_id)?,
            &write_id,
            &engine
                .paths
                .staging()
                .join("plugin-scratch")
                .join(&draft_id),
            source,
            files,
        )?;
        return Ok(id);
    }
    Err(AppError::Denied(
        "Installed external package has no matching committed deployment".into(),
    ))
}

/// Verify the installed files against their reviewed, committed deployment.
pub fn prepare_installed_package(
    engine: &FileEngine,
    slug: &str,
    metadata: PublicationMetadata,
) -> AppResult<(PublicationPreview, PackageBytes)> {
    metadata.validate()?;
    with_installed_source(engine, slug, |source, slug, files| {
        build_preview(source, slug, files, metadata)
    })
}

/// Hold the verified installed package under the file write lock through a
/// local action. The callback must not acquire that lock or run remote requests.
pub(crate) fn with_installed_source<T>(
    engine: &FileEngine,
    slug: &str,
    action: impl FnOnce(PublicationSource, &str, [PublicationFile; 2]) -> AppResult<T>,
) -> AppResult<T> {
    plugins::validate_slug(slug)?;
    let _files = crate::transaction::write_lock();
    let target = engine.paths.plugins().join("games").join(slug);
    let row: Option<(String, bool, String, String, String)> = engine.db.conn().query_row(
        "SELECT source,validated,install_path,CASE WHEN length(CAST(adapter_json AS BLOB))<=262144 THEN adapter_json ELSE '' END,version FROM plugins WHERE slug=?1", [slug],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).optional()?;
    let (source, validated, path, adapter, version) =
        row.ok_or_else(|| AppError::NotFound("Installed package".into()))?;
    if !validated || Path::new(&path) != target || source != "external" {
        return Err(AppError::Denied(
            "Installed package is unverified or has unknown ownership".into(),
        ));
    }
    let files = read_files(engine, &target)?;
    let parsed = plugins::parse_package(slug, &files[0].text, &files[1].text)?;
    if parsed.adapter.version != version
        || !plugins::matches_registered_adapter(&adapter, &parsed.adapter)?
    {
        return Err(conflict(
            "Installed adapter differs from its registry entry",
        ));
    }
    let deployment_transaction_id = installed_provenance(engine, slug, &target, &files)?;
    action(
        PublicationSource::Installed {
            slug: slug.into(),
            source,
            deployment_transaction_id: Some(deployment_transaction_id),
        },
        slug,
        files,
    )
}

fn fresh_preview(
    engine: &FileEngine,
    preview: &PublicationPreview,
) -> AppResult<(PublicationPreview, PackageBytes)> {
    let fresh = match &preview.source {
        PublicationSource::Draft { draft_id, .. } => {
            prepare_package(engine, draft_id, preview.package.metadata())?
        }
        PublicationSource::Installed { slug, .. } => {
            prepare_installed_package(engine, slug, preview.package.metadata())?
        }
    };
    if fresh.0 != *preview {
        return Err(conflict(
            "Publication preview or source is stale; preview and approve again",
        ));
    }
    Ok(fresh)
}

fn verify_preview(preview: &PublicationPreview) -> AppResult<()> {
    let (rebuilt, _) = build_preview(
        preview.source.clone(),
        &preview.package.slug,
        preview.package.files.clone(),
        preview.package.metadata(),
    )?;
    if rebuilt != *preview {
        return Err(conflict(
            "Stored publication preview, destination, archive or digest was altered",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PublicationState {
    Intended,
    Pending,
    Unknown,
    Completed,
    Error,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PublicationPhase {
    Prepared,
    CreateUploadPending,
    UploadCreated,
    StartUploadPending,
    UploadStarted,
    PartPutPending,
    PartUploaded,
    CompleteUploadPending,
    AwaitingScan,
    UploadReady,
    PublishWorkPending,
    Completed,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PublicationStep {
    CreateUpload,
    StartUpload,
    PutPart,
    CompleteUpload,
    PublishWork,
}
impl PublicationStep {
    fn phases(self) -> (PublicationPhase, PublicationPhase, PublicationPhase) {
        use PublicationPhase::*;
        match self {
            Self::CreateUpload => (Prepared, CreateUploadPending, UploadCreated),
            Self::StartUpload => (UploadCreated, StartUploadPending, UploadStarted),
            Self::PutPart => (UploadStarted, PartPutPending, PartUploaded),
            Self::CompleteUpload => (PartUploaded, CompleteUploadPending, UploadReady),
            Self::PublishWork => (UploadReady, PublishWorkPending, Completed),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicationRecord {
    pub id: String,
    pub preview: PublicationPreview,
    pub approved_digest: Option<String>,
    pub state: PublicationState,
    pub phase: PublicationPhase,
    pub last_step: Option<PublicationStep>,
    pub file_id: Option<i64>,
    pub work_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

impl PublicationRecord {
    pub(crate) fn reconciliation_step(&self) -> Option<PublicationStep> {
        self.file_id?;
        match (self.state, self.phase) {
            (PublicationState::Intended, PublicationPhase::AwaitingScan)
            | (PublicationState::Unknown, PublicationPhase::CompleteUploadPending) => {
                Some(PublicationStep::CompleteUpload)
            }
            (PublicationState::Unknown, PublicationPhase::PublishWorkPending) => {
                Some(PublicationStep::PublishWork)
            }
            _ => None,
        }
    }
}

/// Additive and repeatable; call at database startup, not from pure preview.
pub fn migrate(conn: &Connection) -> AppResult<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS plugin_publications (
        id TEXT PRIMARY KEY, preview_json TEXT NOT NULL, digest TEXT NOT NULL UNIQUE,
        approved_digest TEXT, state TEXT NOT NULL, phase TEXT NOT NULL, last_step TEXT,
        file_id INTEGER CHECK(file_id>0), work_id INTEGER CHECK(work_id>0),
        created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
        CHECK(approved_digest IS NULL OR approved_digest=digest)
    );
    CREATE TABLE IF NOT EXISTS plugin_publication_events (
        id INTEGER PRIMARY KEY, publication_id TEXT NOT NULL REFERENCES plugin_publications(id),
        state TEXT NOT NULL, phase TEXT NOT NULL, last_step TEXT, file_id INTEGER, work_id INTEGER,
        created_at TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS plugin_publication_events_publication ON plugin_publication_events(publication_id,id);")?;
    Ok(())
}

fn validate_id(id: &str, prefix: &str) -> AppResult<()> {
    if !id
        .strip_prefix(prefix)
        .and_then(|s| s.strip_prefix('_'))
        .is_some_and(|s| {
            s.len() == 32
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
    {
        return Err(invalid("Publication/provenance ID is invalid"));
    }
    Ok(())
}
fn enum_text(value: impl Serialize) -> AppResult<String> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid("Invalid publication enum"))
}
fn decode<T: serde::de::DeserializeOwned>(s: String) -> AppResult<T> {
    Ok(serde_json::from_value(serde_json::Value::String(s))?)
}
fn load(conn: &Connection, id: &str) -> AppResult<PublicationRecord> {
    validate_id(id, "publish")?;
    let row = conn.query_row("SELECT CASE WHEN length(CAST(preview_json AS BLOB))<=?2 THEN preview_json ELSE '' END,approved_digest,state,phase,last_step,file_id,work_id,created_at,updated_at,digest FROM plugin_publications WHERE id=?1", params![id,PREVIEW_LIMIT], |r| Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,Option<i64>>(5)?,r.get::<_,Option<i64>>(6)?,r.get::<_,String>(7)?,r.get::<_,String>(8)?,r.get::<_,String>(9)?))).optional()?.ok_or_else(|| AppError::NotFound("Publication record".into()))?;
    let preview: PublicationPreview = serde_json::from_str(&row.0)?;
    verify_preview(&preview)?;
    if preview.digest != row.9 || row.1.as_ref().is_some_and(|s| *s != preview.digest) {
        return Err(conflict("Stored publication digest differs"));
    }
    let record = PublicationRecord {
        id: id.into(),
        preview,
        approved_digest: row.1,
        state: decode(row.2)?,
        phase: decode(row.3)?,
        last_step: row.4.map(decode).transpose()?,
        file_id: row.5,
        work_id: row.6,
        created_at: row.7,
        updated_at: row.8,
    };
    verify_record_state(&record)?;
    Ok(record)
}

fn verify_record_state(record: &PublicationRecord) -> AppResult<()> {
    use PublicationPhase::*;
    use PublicationState as State;
    use PublicationStep as Step;
    let valid = match record.phase {
        Prepared => {
            record.state == State::Intended
                && record.last_step.is_none()
                && record.file_id.is_none()
                && record.work_id.is_none()
        }
        Completed => {
            record.state == State::Completed
                && record.last_step == Some(Step::PublishWork)
                && record.file_id.is_some()
                && record.work_id.is_some()
        }
        AwaitingScan => {
            record.state == State::Intended
                && record.last_step == Some(Step::CompleteUpload)
                && record.file_id.is_some()
                && record.work_id.is_none()
        }
        phase => [
            Step::CreateUpload,
            Step::StartUpload,
            Step::PutPart,
            Step::CompleteUpload,
            Step::PublishWork,
        ]
        .iter()
        .any(|step| {
            let (_, pending, after) = step.phases();
            record.last_step == Some(*step)
                && record.work_id.is_none()
                && if phase == pending {
                    matches!(record.state, State::Pending | State::Unknown | State::Error)
                        && if *step == Step::CreateUpload {
                            record.file_id.is_none()
                        } else {
                            record.file_id.is_some()
                        }
                } else {
                    phase == after && record.state == State::Intended && record.file_id.is_some()
                }
        }),
    };
    if !valid
        || record.file_id.is_some_and(|id| id <= 0)
        || record.work_id.is_some_and(|id| id <= 0)
        || record.phase != Prepared && record.approved_digest.is_none()
    {
        return Err(conflict(
            "Stored publication state, phase, consent or remote IDs are inconsistent",
        ));
    }
    Ok(())
}
fn event(conn: &Connection, record: &PublicationRecord) -> AppResult<()> {
    conn.execute("INSERT INTO plugin_publication_events(publication_id,state,phase,last_step,file_id,work_id,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![record.id,enum_text(record.state)?,enum_text(record.phase)?,record.last_step.map(enum_text).transpose()?,record.file_id,record.work_id,record.updated_at])?;
    Ok(())
}
fn save(conn: &Connection, record: &PublicationRecord) -> AppResult<()> {
    conn.execute("UPDATE plugin_publications SET approved_digest=?2,state=?3,phase=?4,last_step=?5,file_id=?6,work_id=?7,updated_at=?8 WHERE id=?1", params![record.id,record.approved_digest,enum_text(record.state)?,enum_text(record.phase)?,record.last_step.map(enum_text).transpose()?,record.file_id,record.work_id,record.updated_at])?;
    event(conn, record)
}
fn update(
    engine: &FileEngine,
    id: &str,
    f: impl FnOnce(&mut PublicationRecord) -> AppResult<bool>,
) -> AppResult<PublicationRecord> {
    let mut conn = engine.db.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut record = load(&tx, id)?;
    if f(&mut record)? {
        record.updated_at = util::now();
        save(&tx, &record)?;
    }
    tx.commit()?;
    Ok(record)
}
fn consent(record: &PublicationRecord, digest: &str) -> AppResult<()> {
    if digest != record.preview.digest || record.approved_digest.as_deref() != Some(digest) {
        return Err(AppError::Denied(
            "Publication requires consent for this exact preview digest".into(),
        ));
    }
    Ok(())
}

/// Exact repeated previews reuse one durable request ID, including after restart.
/// Changed metadata/content yields a different digest and a new unapproved ID.
pub fn create_publication(
    engine: &FileEngine,
    preview: &PublicationPreview,
) -> AppResult<PublicationRecord> {
    fresh_preview(engine, preview)?;
    let json = serde_json::to_string(preview)?;
    if json.len() > PREVIEW_LIMIT {
        return Err(invalid("Publication preview is too large"));
    }
    let _files = lock_live_draft_source(engine, &preview.source)?;
    let mut conn = engine.db.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM plugin_publications WHERE digest=?1",
            [&preview.digest],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        let record = load(&tx, &id)?;
        if record.preview != *preview {
            return Err(conflict("Stored preview differs for this digest"));
        }
        tx.commit()?;
        return Ok(record);
    }
    let now = util::now();
    let record = PublicationRecord {
        id: util::new_id("publish"),
        preview: preview.clone(),
        approved_digest: None,
        state: PublicationState::Intended,
        phase: PublicationPhase::Prepared,
        last_step: None,
        file_id: None,
        work_id: None,
        created_at: now.clone(),
        updated_at: now,
    };
    tx.execute("INSERT INTO plugin_publications(id,preview_json,digest,state,phase,created_at,updated_at) VALUES (?1,?2,?3,'intended','prepared',?4,?4)", params![record.id,json,preview.digest,record.created_at])?;
    event(&tx, &record)?;
    tx.commit()?;
    Ok(record)
}
pub fn get_publication(engine: &FileEngine, id: &str) -> AppResult<PublicationRecord> {
    load(&engine.db.conn(), id)
}
pub fn list_publications(engine: &FileEngine) -> AppResult<Vec<PublicationRecord>> {
    let conn = engine.db.conn();
    let mut stmt = conn.prepare(
        "SELECT id FROM plugin_publications ORDER BY created_at DESC,rowid DESC LIMIT 100",
    )?;
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    ids.iter().map(|id| load(&conn, id)).collect()
}
/// Must be called only by the trusted approval path, never an agent tool that
/// can approve its own proposal. A digest is a binding, not an authentication key.
pub fn approve_publication(
    engine: &FileEngine,
    id: &str,
    approved_digest: &str,
) -> AppResult<PublicationRecord> {
    let current = get_publication(engine, id)?;
    fresh_preview(engine, &current.preview)?;
    let _files = lock_live_draft_source(engine, &current.preview.source)?;
    update(engine, id, |record| {
        if record.preview.digest != approved_digest {
            return Err(conflict("Approval digest differs from the preview"));
        }
        if record.approved_digest.as_deref() == Some(approved_digest) {
            return Ok(false);
        }
        if record.state != PublicationState::Intended || record.phase != PublicationPhase::Prepared
        {
            return Err(conflict("Publication can no longer be approved"));
        }
        record.approved_digest = Some(approved_digest.into());
        Ok(true)
    })
}
pub fn verified_upload_artifact(
    engine: &FileEngine,
    id: &str,
    approved_digest: &str,
) -> AppResult<(PublicationRecord, PackageBytes)> {
    let record = get_publication(engine, id)?;
    consent(&record, approved_digest)?;
    let (_, bytes) = fresh_preview(engine, &record.preview)?;
    Ok((record, bytes))
}
/// Durable before send. A repeated call for a pending/unknown action is an
/// error, not permission to send again, including create_upload's stable ID.
pub fn begin_step(
    engine: &FileEngine,
    id: &str,
    approved_digest: &str,
    step: PublicationStep,
) -> AppResult<PublicationRecord> {
    let (current, _) = verified_upload_artifact(engine, id, approved_digest)?;
    let _files = lock_live_draft_source(engine, &current.preview.source)?;
    update(engine, id, |record| {
        consent(record, approved_digest)?;
        let (before, pending, _) = step.phases();
        if record.state != PublicationState::Intended || record.phase != before {
            return Err(conflict(
                "Publication step is pending, uncertain, terminal or out of order; do not resend",
            ));
        }
        record.state = PublicationState::Pending;
        record.phase = pending;
        record.last_step = Some(step);
        Ok(true)
    })
}

/// The transporter must first verify remote identity, hash, size, readiness and
/// safety. Unknown may be resolved by evidence, never by blindly resending.
pub fn finish_step(
    engine: &FileEngine,
    id: &str,
    step: PublicationStep,
    file_id: Option<i64>,
    work_id: Option<i64>,
) -> AppResult<PublicationRecord> {
    update(engine, id, |record| {
        apply_finish(record, step, file_id, work_id)
    })
}

/// Record independently verified evidence only if the publication did not
/// change while the read-only network requests were in flight.
pub(crate) fn finish_reconciliation(
    engine: &FileEngine,
    observed: &PublicationRecord,
    work_id: Option<i64>,
) -> AppResult<PublicationRecord> {
    let step = observed
        .reconciliation_step()
        .ok_or_else(|| conflict("Publication cannot be reconciled at this phase"))?;
    update(engine, &observed.id, |record| {
        if record != observed {
            return Err(conflict("发布记录已变更，请刷新后重新核对"));
        }
        apply_finish(record, step, observed.file_id, work_id)
    })
}

fn apply_finish(
    record: &mut PublicationRecord,
    step: PublicationStep,
    file_id: Option<i64>,
    work_id: Option<i64>,
) -> AppResult<bool> {
    if file_id.is_some_and(|id| id <= 0) || work_id.is_some_and(|id| id <= 0) {
        return Err(invalid("Remote IDs must be positive integers"));
    }
    let (_, pending, after) = step.phases();
    consent(record, &record.preview.digest)?;
    if record.last_step != Some(step) {
        return Err(conflict("Remote result is for a different step"));
    }
    if file_id.is_some() && record.file_id.is_some() && file_id != record.file_id {
        return Err(conflict("Remote file identity changed"));
    }
    if work_id.is_some() && record.work_id.is_some() && work_id != record.work_id {
        return Err(conflict("Remote work identity changed"));
    }
    if step == PublicationStep::CreateUpload && file_id.is_none()
        || step == PublicationStep::PublishWork && work_id.is_none()
        || step != PublicationStep::PublishWork && work_id.is_some()
        || step != PublicationStep::CreateUpload && record.file_id.is_none()
    {
        return Err(invalid(
            "Remote result IDs do not match the publication step",
        ));
    }
    if record.phase == after
        && matches!(
            record.state,
            PublicationState::Intended | PublicationState::Completed
        )
    {
        return Ok(false);
    }
    let scanning = step == PublicationStep::CompleteUpload
        && record.phase == PublicationPhase::AwaitingScan
        && record.state == PublicationState::Intended;
    if !(record.phase == pending
        && matches!(
            record.state,
            PublicationState::Pending | PublicationState::Unknown
        )
        || scanning)
    {
        return Err(conflict("Remote result cannot transition this publication"));
    }
    record.file_id = file_id.or(record.file_id);
    record.work_id = work_id.or(record.work_id);
    record.phase = after;
    record.state = if step == PublicationStep::PublishWork {
        PublicationState::Completed
    } else {
        PublicationState::Intended
    };
    Ok(true)
}
pub fn await_scan(engine: &FileEngine, id: &str) -> AppResult<PublicationRecord> {
    update(engine, id, |record| {
        if record.last_step != Some(PublicationStep::CompleteUpload) || record.file_id.is_none() {
            return Err(conflict("No completed upload is awaiting a scan"));
        }
        if record.phase == PublicationPhase::AwaitingScan
            && record.state == PublicationState::Intended
        {
            return Ok(false);
        }
        if record.phase != PublicationPhase::CompleteUploadPending
            || !matches!(
                record.state,
                PublicationState::Pending | PublicationState::Unknown
            )
        {
            return Err(conflict("Upload is not awaiting completion"));
        }
        record.phase = PublicationPhase::AwaitingScan;
        record.state = PublicationState::Intended;
        Ok(true)
    })
}
pub fn mark_unknown(
    engine: &FileEngine,
    id: &str,
    step: PublicationStep,
) -> AppResult<PublicationRecord> {
    update(engine, id, |record| {
        if record.last_step != Some(step) || record.phase != step.phases().1 {
            return Err(conflict(
                "Unknown result is for a different publication step",
            ));
        }
        if record.state == PublicationState::Unknown {
            return Ok(false);
        }
        if record.state != PublicationState::Pending {
            return Err(conflict("Only a pending action can become unknown"));
        }
        record.state = PublicationState::Unknown;
        Ok(true)
    })
}
/// Use only for a definitive failure with no uncertain effect. Never persist a
/// remote error body: it can echo authorization headers or signed URLs.
pub fn fail_step(
    engine: &FileEngine,
    id: &str,
    step: PublicationStep,
) -> AppResult<PublicationRecord> {
    update(engine, id, |record| {
        if record.last_step != Some(step) || record.phase != step.phases().1 {
            return Err(conflict("Failure is for a different step"));
        }
        if record.state == PublicationState::Error {
            return Ok(false);
        }
        if record.state != PublicationState::Pending {
            return Err(conflict(
                "An uncertain action requires reconciliation, not failure/retry",
            ));
        }
        record.state = PublicationState::Error;
        Ok(true)
    })
}
/// On restart, every in-flight side effect is uncertain. This never retries or
/// deletes anything; read-only remote reconciliation is the only next action.
pub fn recover_pending_publications(engine: &FileEngine) -> AppResult<Vec<PublicationRecord>> {
    let mut conn = engine.db.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let ids: Vec<String> = {
        let mut stmt = tx.prepare("SELECT id FROM plugin_publications WHERE state='pending'")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut records = Vec::new();
    for id in ids {
        let mut record = load(&tx, &id)?;
        record.state = PublicationState::Unknown;
        record.updated_at = util::now();
        save(&tx, &record)?;
        records.push(record);
    }
    tx.commit()?;
    Ok(records)
}

#[cfg(test)]
#[path = "plugin_publish_tests.rs"]
mod tests;
