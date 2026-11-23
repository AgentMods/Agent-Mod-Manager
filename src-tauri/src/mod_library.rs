//! Durable, inert archives kept independently of the download and import caches.
use crate::{
    artifacts::{self, ArtifactRecord, DownloadOrigin},
    db::Db,
    download,
    error::{AppError, AppResult},
    games, paths, safe_fs,
    transaction::{self, FileEngine},
    util,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use std::{
    fs::{self, File},
    io::{ErrorKind, Read, Seek, Write},
    path::{Component, Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

const COLUMNS: &str =
    "id,game_name,game_id,name,filename,archive_relative_path,payload_relative_path,\
 size,sha256,source_id,project_id,version_id,version,created_at,integrity_verified,project_page,author_hint";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryEntry {
    #[serde(rename = "artifactId")]
    pub id: String,
    pub game_name: String,
    pub game_id: Option<String>,
    pub name: String,
    pub filename: String,
    pub archive_relative_path: String,
    pub payload_relative_path: String,
    pub size: u64,
    pub sha256: String,
    pub source_id: String,
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub version: String,
    pub created_at: String,
    /// The upstream published a strong hash; local SHA-256 is checked even when false.
    pub integrity_verified: bool,
    pub project_page: Option<String>,
    /// Public display hint from the request; not a provider-authenticated identity.
    pub author_hint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VerifiedLibraryEntry {
    pub entry: LibraryEntry,
    pub archive_path: PathBuf,
    pub payload_path: PathBuf,
    pub origin: Option<DownloadOrigin>,
}

/// A trusted in-process caller establishes approval and the binding of these
/// values to a committed desktop import. Never accept this struct from IPC.
#[derive(Clone, Copy)]
pub struct ApprovedArtifact<'a> {
    pub artifact_id: &'a str,
    pub path: &'a Path,
    pub filename: &'a str,
    pub size: u64,
    pub sha256: &'a str,
}

struct Receipt<'a> {
    file: ApprovedArtifact<'a>,
    sha512: Option<&'a str>,
    name: &'a str,
    source_id: &'a str,
    project_id: Option<&'a str>,
    version_id: Option<&'a str>,
    version: &'a str,
    project_page: Option<&'a str>,
    author_hint: Option<&'a str>,
    integrity_verified: bool,
    relocate: Option<&'a ArtifactRecord>,
    received: Option<ReceivedSource<'a>>,
}

/// Optional public display metadata, independent of transport and source identity.
#[derive(Clone, Copy, Default)]
pub struct DownloadMetadata<'a> {
    pub project_page: Option<&'a str>,
    pub author_hint: Option<&'a str>,
}

impl DownloadMetadata<'_> {
    /// Validate hints before starting a network transfer.
    pub(crate) fn validate(self) -> AppResult<()> {
        validate_download_metadata(self).map(|_| ())
    }
}

/// Frozen native download classification stored atomically with the exact
/// completion receipt. Catalog records without this marker cannot be retried
/// through the native recovery workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NativeDownloadIntent {
    pub game_name: String,
    pub game_id: Option<String>,
    pub project_page: Option<String>,
    pub author_hint: Option<String>,
}

impl NativeDownloadIntent {
    pub fn new(
        game_name: &str,
        game_id: Option<&str>,
        metadata: DownloadMetadata<'_>,
    ) -> AppResult<Self> {
        let game_name = validated_game_name(game_name)?.0;
        if game_id.is_some_and(|id| {
            id.trim().is_empty() || id.len() > 512 || id.chars().any(char::is_control)
        }) {
            return Err(AppError::Invalid("原始下载归类的游戏标识无效".into()));
        }
        let (project_page, author_hint) = validate_download_metadata(metadata)?;
        Ok(Self {
            game_name: game_name.into(),
            game_id: game_id.map(str::to_owned),
            project_page,
            author_hint,
        })
    }

    pub fn without_transport_page(&self, transport_url: &str) -> Self {
        let mut intent = self.clone();
        if intent
            .project_page
            .as_deref()
            .is_some_and(|page| artifacts::same_public_endpoint(page, transport_url))
        {
            intent.project_page = None;
        }
        intent
    }

    pub(crate) fn validated(&self) -> AppResult<Self> {
        Self::new(
            &self.game_name,
            self.game_id.as_deref(),
            DownloadMetadata {
                project_page: self.project_page.as_deref(),
                author_hint: self.author_hint.as_deref(),
            },
        )
    }

    pub(crate) fn check_target(&self, game_name: &str, game_id: Option<&str>) -> AppResult<()> {
        if self.game_id.as_deref() != game_id
            || (game_id.is_none() && game_key(game_name) != game_key(&self.game_name))
        {
            return Err(AppError::Conflict("重试必须使用原下载归类的游戏".into()));
        }
        Ok(())
    }
}

fn validate_download_metadata(
    metadata: DownloadMetadata<'_>,
) -> AppResult<(Option<String>, Option<String>)> {
    let page = metadata
        .project_page
        .map(artifacts::explicit_project_page)
        .transpose()?;
    let author = metadata
        .author_hint
        .map(|author| {
            let author = author.trim();
            if author.is_empty()
                || author.len() > 256
                || author.chars().any(char::is_control)
                || author.contains("://")
            {
                return Err(AppError::Invalid(
                    "作者提示必须是公开作者名，不能是链接".into(),
                ));
            }
            Ok(author.to_owned())
        })
        .transpose()?;
    Ok((page, author))
}

enum CatalogMode<'a> {
    Retain,
    Move(DownloadMetadata<'a>),
    Retry,
}

/// Keep a completed download after verifying the catalog and the exact bytes.
/// Repeating an identical request resumes an interrupted publication.
pub fn ingest_download(
    engine: &FileEngine,
    catalog: &ArtifactRecord,
    game_name: &str,
    game_id: Option<&str>,
) -> AppResult<LibraryEntry> {
    ingest_catalog(engine, catalog, game_name, game_id, CatalogMode::Retain)
}

/// Transfer newly completed native bytes into the library on the same volume.
/// The pending row is committed before the rename; recording the new catalog
/// path and making the library entry visible are one SQLite transaction.
pub fn ingest_new_download(
    engine: &FileEngine,
    catalog: &ArtifactRecord,
    game_name: &str,
    game_id: Option<&str>,
) -> AppResult<LibraryEntry> {
    ingest_new_download_with_metadata(
        engine,
        catalog,
        game_name,
        game_id,
        DownloadMetadata::default(),
    )
}

/// Ingest a newly transferred catalog entry with optional display metadata.
/// Prefer a native source homepage over any separately supplied project page.
pub fn ingest_new_download_with_metadata(
    engine: &FileEngine,
    catalog: &ArtifactRecord,
    game_name: &str,
    game_id: Option<&str>,
    metadata: DownloadMetadata<'_>,
) -> AppResult<LibraryEntry> {
    ingest_catalog(
        engine,
        catalog,
        game_name,
        game_id,
        CatalogMode::Move(metadata),
    )
}

pub(crate) fn retry_native_download(
    engine: &FileEngine,
    catalog: &ArtifactRecord,
    game_name: &str,
    game_id: Option<&str>,
) -> AppResult<LibraryEntry> {
    ingest_catalog(engine, catalog, game_name, game_id, CatalogMode::Retry)
}

fn ingest_catalog(
    engine: &FileEngine,
    catalog: &ArtifactRecord,
    game_name: &str,
    game_id: Option<&str>,
    mode: CatalogMode<'_>,
) -> AppResult<LibraryEntry> {
    let _files = transaction::write_lock();
    let catalog = artifacts::verify_catalog_entry(&engine.db, catalog)?;
    let intent = catalog.native_library_intent();
    if let Some(intent) = intent {
        intent.check_target(game_name, game_id)?;
    } else if matches!(mode, CatalogMode::Retry) {
        return Err(AppError::Denied(
            "该完成下载没有可重试的本机归类记录".into(),
        ));
    }
    let (fallback_page, author_hint) = if let Some(intent) = intent {
        (intent.project_page.clone(), intent.author_hint.clone())
    } else if let CatalogMode::Move(metadata) = mode {
        validate_download_metadata(metadata)?
    } else {
        (None, None)
    };
    let relocate = !matches!(mode, CatalogMode::Retain);
    let game_name = intent.map_or(game_name, |intent| &intent.game_name);
    let artifact = &catalog.artifact;
    let source = catalog.transport_path();
    let relative = source
        .strip_prefix(engine.paths.downloads())
        .map_err(|_| invalid_source())?;
    let expected_basename = format!("{}-{}", artifact.id, artifact.filename);
    let mut parts = relative.components();
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Component::Normal(bucket)), Some(Component::Normal(filename)), None)
            if bucket
                .to_str()
                .is_some_and(|key| valid_hash(key, 32) || valid_hash(key, 64))
                && filename == expected_basename.as_str() => {}
        // Older completed catalog fixtures and receipts may be stored directly
        // under downloads; the exact catalog and UUID-prefixed basename still bind them.
        (Some(Component::Normal(filename)), None, None)
            if filename == expected_basename.as_str() => {}
        _ => return Err(invalid_source()),
    }
    let origin = catalog.origin();
    let source_id = if origin.is_some() || catalog.source_id == artifacts::WEB_SOURCE {
        catalog.source_id.as_str()
    } else {
        "local"
    };
    ingest_locked(
        engine,
        Receipt {
            file: ApprovedArtifact {
                artifact_id: &artifact.id,
                path: source,
                filename: &artifact.filename,
                size: artifact.size,
                sha256: &artifact.sha256,
            },
            sha512: Some(&artifact.sha512),
            name: &catalog.name,
            source_id,
            project_id: origin.as_ref().map(|o| o.project_id.as_str()),
            version_id: origin.as_ref().map(|o| o.version_id.as_str()),
            version: &catalog.version,
            project_page: catalog.project_page.as_deref().or(fallback_page.as_deref()),
            author_hint: author_hint.as_deref(),
            integrity_verified: artifact.integrity_verified,
            relocate: relocate.then_some(&catalog),
            received: None,
        },
        game_name,
        game_id,
    )
}

/// Publish from a trusted desktop import receipt, never from an external path.
/// `path` is the applied copy under staging/imports, not the user's selection.
pub fn ingest_import(
    engine: &FileEngine,
    artifact: &ApprovedArtifact<'_>,
    game_name: &str,
    game_id: Option<&str>,
) -> AppResult<LibraryEntry> {
    let _files = transaction::write_lock();
    let imports = engine.paths.staging().join("imports");
    let relative = artifact
        .path
        .strip_prefix(imports)
        .map_err(|_| invalid_source())?;
    let mut parts = relative.components();
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Component::Normal(bucket)), Some(Component::Normal(filename)), None)
            if bucket.to_str().is_some_and(|bucket| {
                uuid::Uuid::parse_str(bucket).is_ok_and(|id| id.hyphenated().to_string() == bucket)
            }) && filename == artifact.filename => {}
        _ => return Err(invalid_source()),
    }
    ingest_locked(
        engine,
        Receipt {
            file: *artifact,
            sha512: None,
            name: artifact.filename,
            source_id: "local",
            project_id: None,
            version_id: None,
            version: "",
            project_page: None,
            author_hint: None,
            integrity_verified: false,
            relocate: None,
            received: None,
        },
        game_name,
        game_id,
    )
}

pub(crate) struct ReceivedChecks<'a> {
    pub source: &'a mut File,
    pub expected_identity: &'a safe_fs::FileIdentity,
    pub mod_name: &'a str,
    pub project_page: Option<&'a str>,
    pub cancel: &'a CancellationToken,
}

struct ReceivedSource<'a> {
    file: &'a mut File,
    identity: &'a safe_fs::FileIdentity,
    cancel: &'a CancellationToken,
}

/// Trusted receipt adapter for a completed file in one of the configured
/// inboxes. This is not a command and must never accept arbitrary user paths.
/// Callers with a held preview-verified handle use `ingest_received_checked`.
pub fn ingest_received(
    engine: &FileEngine,
    artifact: &ApprovedArtifact<'_>,
    game_name: &str,
    game_id: Option<&str>,
    mod_name: &str,
    project_page: Option<&str>,
) -> AppResult<LibraryEntry> {
    received_path(engine, artifact)?;
    let mut source = safe_fs::open_regular(artifact.path)?;
    let identity = safe_fs::identity(artifact.path)?.ok_or_else(changed)?;
    ingest_received_checked(
        engine,
        artifact,
        game_name,
        game_id,
        ReceivedChecks {
            source: &mut source,
            expected_identity: &identity,
            mod_name,
            project_page,
            cancel: &CancellationToken::new(),
        },
    )
}

fn same_receipt_path(left: &Path, right: &Path) -> bool {
    // The stock config can use ordinary Windows spelling while AppPaths is canonical.
    let left = paths::user_path(left);
    let right = paths::user_path(right);
    paths::contains_path(&left, &right) && paths::contains_path(&right, &left)
}

/// Only advertise or ingest from the exact managed directory of the stock MCP.
pub(crate) fn playwright_receipt_dir(engine: &FileEngine) -> AppResult<Option<PathBuf>> {
    let expected = crate::storage::playwright_downloads(&engine.paths.root);
    let configured = crate::mcp::playwright_receipt_dir(&engine.db)?;
    Ok(configured
        .filter(|root| same_receipt_path(&expected, root))
        .filter(|_| {
            engine.paths.stable_directory(&expected).is_ok()
                && engine
                    .paths
                    .guard(&expected, &[])
                    .is_ok_and(|resolved| resolved == expected)
        })
        .map(|_| expected))
}

fn received_path(engine: &FileEngine, receipt: &ApprovedArtifact<'_>) -> AppResult<()> {
    download::safe_filename(receipt.filename)?;
    let path = receipt.path;
    if !path.is_absolute()
        || path.file_name().and_then(|name| name.to_str()) != Some(receipt.filename)
    {
        return Err(invalid_source());
    }
    let parent = path.parent().ok_or_else(invalid_source)?;
    let incoming = crate::storage::incoming_downloads(&engine.paths.root);
    let permitted = if same_receipt_path(&incoming, parent) {
        incoming
    } else {
        playwright_receipt_dir(engine)?
            .filter(|root| same_receipt_path(root, parent))
            .ok_or_else(invalid_source)?
    };
    let resolved = engine.paths.guard(path, &[])?;
    // Compare the actual parent as well as the supplied spelling. The held
    // no-follow handle checked below still rejects links and reparse points.
    if !resolved
        .parent()
        .is_some_and(|dir| same_receipt_path(dir, &permitted))
        || !same_receipt_path(&resolved, path)
    {
        return Err(invalid_source());
    }
    Ok(())
}

pub(crate) fn ingest_received_checked(
    engine: &FileEngine,
    artifact: &ApprovedArtifact<'_>,
    game_name: &str,
    game_id: Option<&str>,
    checks: ReceivedChecks<'_>,
) -> AppResult<LibraryEntry> {
    let _files = transaction::write_lock();
    received_path(engine, artifact)?;
    let project_page = checks.project_page.and_then(artifacts::public_project_page);
    let received = ReceivedSource {
        file: checks.source,
        identity: checks.expected_identity,
        cancel: checks.cancel,
    };
    received.verify_at(artifact.path)?;
    ingest_locked(
        engine,
        Receipt {
            file: *artifact,
            sha512: None,
            name: checks.mod_name,
            source_id: "local",
            project_id: None,
            version_id: None,
            version: "",
            project_page: project_page.as_deref(),
            author_hint: None,
            integrity_verified: false,
            relocate: None,
            received: Some(received),
        },
        game_name,
        game_id,
    )
}

impl ReceivedSource<'_> {
    fn verify_at(&self, path: &Path) -> AppResult<()> {
        if self.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        safe_fs::verify_regular_at(self.file, path)?;
        if safe_fs::identity(path)?.as_ref() != Some(self.identity) {
            return Err(changed());
        }
        Ok(())
    }
}

fn invalid_source() -> AppError {
    AppError::Denied("模组库只接收明确验证的管理器归档来源".into())
}

fn changed() -> AppError {
    AppError::Conflict("归档大小、摘要或文件身份与记录不符".into())
}

fn valid_hash(hash: &str, len: usize) -> bool {
    hash.len() == len && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
}

fn validated_game_name(name: &str) -> AppResult<(&str, String)> {
    let name = name.trim();
    let key = game_key(name);
    if key.is_empty() || name.chars().any(char::is_control) {
        return Err(AppError::Invalid("游戏名称为空或含控制字符".into()));
    }
    Ok((name, key))
}

pub(crate) fn game_key(name: &str) -> String {
    name.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn safe_label(label: &str, default: &str) -> String {
    let mut result = String::new();
    for ch in label.trim().chars() {
        if result.len() + ch.len_utf8() > 80 {
            break;
        }
        result.push(if ch.is_control() || "/\\<>:\"|?*".contains(ch) {
            '-'
        } else if ch.is_whitespace() {
            ' '
        } else {
            ch
        });
    }
    let result = result
        .trim_start_matches(['.', ' ', '~'])
        .trim_end_matches(['.', ' ']);
    if result.is_empty() {
        default.into()
    } else if paths::safe_join(Path::new("candidate"), result).is_err() {
        format!("{default}-{result}")
    } else {
        result.into()
    }
}

fn category_directory(
    conn: &Connection,
    root: &Path,
    key: &str,
    game_name: &str,
) -> AppResult<String> {
    if let Some(directory) = conn
        .query_row(
            "SELECT directory FROM mod_library_categories WHERE key=?1",
            [key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(directory);
    }
    let mut statement = conn.prepare("SELECT directory FROM mod_library_categories")?;
    let mut used = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .map(|row| row.map(|name| name.to_lowercase()))
        .collect::<Result<std::collections::HashSet<_>, _>>()?;
    let root_identity = safe_fs::identity(root)?.ok_or_else(changed)?;
    for child in safe_fs::directory_entries(root, &root_identity)? {
        let (name, _) = child?;
        if let Some(name) = name.to_str() {
            used.insert(name.to_lowercase());
        }
    }
    let label = safe_label(game_name, "game");
    let digest = hex::encode(Sha256::digest(key));
    for width in std::iter::once(0).chain((8..=64).step_by(4)) {
        let candidate = if width == 0 {
            label.clone()
        } else {
            format!("{label}--{}", &digest[..width])
        };
        if !used.contains(&candidate.to_lowercase()) && !exists(&root.join(&candidate))? {
            conn.execute(
                "INSERT INTO mod_library_categories(key,directory) VALUES (?1,?2)",
                params![key, candidate],
            )?;
            return Ok(candidate);
        }
    }
    Err(AppError::Conflict("无法创建不冲突的模组库游戏目录".into()))
}

fn read_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<LibraryEntry> {
    Ok(LibraryEntry {
        id: row.get(0)?,
        game_name: row.get(1)?,
        game_id: row.get(2)?,
        name: row.get(3)?,
        filename: row.get(4)?,
        archive_relative_path: row.get(5)?,
        payload_relative_path: row.get(6)?,
        size: row.get(7)?,
        sha256: row.get(8)?,
        source_id: row.get(9)?,
        project_id: row.get(10)?,
        version_id: row.get(11)?,
        version: row.get(12)?,
        created_at: row.get(13)?,
        integrity_verified: row.get(14)?,
        project_page: row.get(15)?,
        author_hint: row.get(16)?,
    })
}

/// Reserve the identity and spelling under the write lock before creating files.
/// A pending row is never listed or accepted by the install APIs.
fn ingest_locked(
    engine: &FileEngine,
    mut receipt: Receipt<'_>,
    game_name: &str,
    game_id: Option<&str>,
) -> AppResult<LibraryEntry> {
    let file = &receipt.file;
    download::safe_filename(file.filename)?;
    if !valid_id(file.artifact_id)
        || !valid_hash(file.sha256, 64)
        || receipt.sha512.is_some_and(|hash| !valid_hash(hash, 128))
        || file.size > i64::MAX as u64
    {
        return Err(AppError::Invalid("归档标识、大小或摘要无效".into()));
    }
    let sha256 = file.sha256.to_ascii_lowercase();
    let sha512 = receipt.sha512.map(str::to_ascii_lowercase);
    let game = game_id
        .map(|id| games::get_game(&engine.db, id))
        .transpose()?;
    let (game_name, name_key) =
        validated_game_name(game.as_ref().map_or(game_name, |game| game.name.as_str()))?;
    let category_key =
        game_id.map_or_else(|| format!("name:{name_key}"), |id| format!("game:{id}"));
    let name = if receipt.name.trim().is_empty()
        || receipt.name.len() > 512
        || receipt.name.chars().any(char::is_control)
    {
        file.filename
    } else {
        receipt.name.trim()
    };
    engine.paths.stable_directory(&engine.paths.mod_library())?;

    let (entry, fresh) = {
        let mut conn = engine.db.conn();
        let tx = conn.transaction()?;
        let existing = tx
            .query_row(
                &format!(
                    "SELECT {COLUMNS},game_name_key,sha512 FROM mod_library_entries WHERE id=?1"
                ),
                [file.artifact_id],
                |row| {
                    Ok((
                        read_entry(row)?,
                        row.get::<_, String>(17)?,
                        row.get::<_, Option<String>>(18)?,
                    ))
                },
            )
            .optional()?;
        if let Some((entry, existing_name_key, existing_sha512)) = existing {
            match (entry.game_id.as_deref(), game_id) {
                (Some(bound), Some(wanted)) if bound == wanted => {}
                (None, None) if existing_name_key == name_key => {}
                (None, Some(_)) if existing_name_key == name_key => {
                    return Err(AppError::Conflict("请明确绑定未登记的模组归档".into()));
                }
                _ => return Err(AppError::Conflict("归档已归属其他游戏".into())),
            }
            if entry.filename != file.filename
                || entry.size != file.size
                || entry.sha256 != sha256
                || existing_sha512 != sha512
                || entry.source_id != receipt.source_id
                || entry.project_id.as_deref() != receipt.project_id
                || entry.version_id.as_deref() != receipt.version_id
            {
                return Err(AppError::Conflict("归档标识已有不同的来源或内容".into()));
            }
            (entry, false)
        } else {
            let dir =
                category_directory(&tx, &engine.paths.mod_library(), &category_key, game_name)?;
            let base = format!(
                "Mods/{dir}/{}--{}",
                safe_label(name, "mod"),
                file.artifact_id
            );
            let entry = LibraryEntry {
                id: file.artifact_id.into(),
                game_name: game_name.into(),
                game_id: game_id.map(str::to_owned),
                name: name.into(),
                filename: file.filename.into(),
                archive_relative_path: format!("{base}/archive/{}", file.filename),
                payload_relative_path: format!("{base}/payload"),
                size: file.size,
                sha256,
                source_id: receipt.source_id.into(),
                project_id: receipt.project_id.map(str::to_owned),
                version_id: receipt.version_id.map(str::to_owned),
                version: receipt.version.into(),
                created_at: util::now(),
                integrity_verified: receipt.integrity_verified,
                project_page: receipt.project_page.map(str::to_owned),
                author_hint: receipt.author_hint.map(str::to_owned),
            };
            let (_, payload) = locations(engine, &entry)?;
            if exists(payload.parent().expect("validated library layout"))? {
                return Err(AppError::Conflict(
                    "模组库目标已存在，未接管现有文件".into(),
                ));
            }
            tx.execute(
                "INSERT INTO mod_library_entries (id,category_key,game_name,game_name_key,game_id,name,filename,
                 archive_relative_path,payload_relative_path,size,sha256,sha512,source_id,project_id,version_id,
                 version,created_at,integrity_verified,project_page,author_hint,state)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,'pending')",
                params![entry.id, category_key, entry.game_name, name_key, entry.game_id, entry.name,
                    entry.filename, entry.archive_relative_path, entry.payload_relative_path, entry.size,
                    entry.sha256, sha512, entry.source_id, entry.project_id, entry.version_id, entry.version,
                    entry.created_at, entry.integrity_verified, entry.project_page, entry.author_hint],
            )?;
            tx.commit()?;
            (entry, true)
        }
    };

    let (archive, payload) = locations(engine, &entry)?;
    if fresh {
        // Create the newly reserved mod directory exclusively. Even a concurrent
        // claimant cannot cause the first attempt to adopt a pre-existing file.
        safe_fs::create_dir(payload.parent().expect("validated library layout"))?;
    }
    publish(
        engine,
        file.path,
        &archive,
        &payload,
        &entry,
        sha512.as_deref(),
        Publication {
            relocate: receipt.relocate.is_some(),
            received: receipt.received.take(),
        },
    )?;
    let mut conn = engine.db.conn();
    let tx = conn.transaction()?;
    if let Some(catalog) = receipt.relocate {
        artifacts::relocate_catalog_entry(&tx, catalog, &archive)?;
    }
    tx.execute(
        "UPDATE mod_library_entries SET state='ready' WHERE id=?1",
        [&entry.id],
    )?;
    tx.commit()?;
    Ok(entry)
}

fn exists(path: &Path) -> AppResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn locations(engine: &FileEngine, entry: &LibraryEntry) -> AppResult<(PathBuf, PathBuf)> {
    download::safe_filename(&entry.filename)?;
    let parts: Vec<_> = entry.archive_relative_path.split('/').collect();
    match parts.as_slice() {
        ["Mods", category, module, "archive", filename]
            if !category.contains('\\')
                && !module.contains('\\')
                && *filename == entry.filename
                && valid_id(&entry.id)
                && module.ends_with(&format!("--{}", entry.id))
                && entry.payload_relative_path == format!("Mods/{category}/{module}/payload") => {}
        _ => return Err(AppError::Conflict("模组库相对路径无效".into())),
    }
    let archive = paths::safe_join(&engine.paths.root, &entry.archive_relative_path)?;
    let payload = paths::safe_join(&engine.paths.root, &entry.payload_relative_path)?;
    if !archive.starts_with(engine.paths.mod_library())
        || !payload.starts_with(engine.paths.mod_library())
    {
        return Err(AppError::Conflict("模组库相对路径越界".into()));
    }
    Ok((archive, payload))
}

struct Publication<'a> {
    relocate: bool,
    received: Option<ReceivedSource<'a>>,
}

fn try_relocate_download(
    engine: &FileEngine,
    source: &Path,
    archive: &Path,
    entry: &LibraryEntry,
    sha512: Option<&str>,
) -> AppResult<bool> {
    if !exists(source)? {
        return Ok(false);
    }
    if engine.paths.guard(source, &[])? != source {
        return Err(invalid_source());
    }
    let identity = safe_fs::identity(source)?.ok_or_else(changed)?;
    verify_file(source, entry, sha512)?;
    match safe_fs::rename_checked(source, archive, &identity) {
        Ok(()) => {
            // Persist the destination before committing removal of the old name.
            #[cfg(unix)]
            {
                sync_directory(engine, archive.parent().expect("validated library layout"))?;
                sync_directory(engine, source.parent().expect("validated transport path"))?;
            }
            Ok(true)
        }
        // An unusual mount beneath the active root is supported by retaining
        // the source and using the verified copy protocol instead.
        Err(AppError::Io(error)) if error.kind() == ErrorKind::CrossesDevices => Ok(false),
        Err(error) => Err(error),
    }
}

/// The `pending` DB row is the publication intent. A fully written scratch can
/// be resumed after a crash even without its source. The final path is never
/// overwritten, and interrupted partial bytes are kept out of the archive dir.
fn publish(
    engine: &FileEngine,
    source: &Path,
    archive: &Path,
    payload: &Path,
    entry: &LibraryEntry,
    sha512: Option<&str>,
    mut publication: Publication<'_>,
) -> AppResult<()> {
    let archive_dir = archive.parent().expect("validated library layout");
    let module = payload.parent().expect("validated library layout");
    safe_fs::create_dir_all(archive_dir)?;
    safe_fs::create_dir_all(payload)?;
    engine.paths.stable_directory(payload)?;
    let archive_exists = exists(archive)?;
    // Ensure the destination's directory chain exists durably before a move
    // whose source is the only copy.
    let moved = if !archive_exists && publication.relocate {
        #[cfg(unix)]
        sync_library_directories(engine, archive_dir, payload, module)?;
        try_relocate_download(engine, source, archive, entry, sha512)?
    } else {
        false
    };
    if !archive_exists && !moved {
        // Retain the extension for format-based structural validation of
        // received archives. The incomplete file is never in archive/.
        let scratch = module.join(format!(".archive-incoming-{}", entry.filename));
        let keep_scratch = if exists(&scratch)? {
            match verify_file(&scratch, entry, sha512) {
                Ok(()) => true,
                Err(AppError::Conflict(_)) => false,
                Err(error) => return Err(error),
            }
        } else {
            false
        };
        if !keep_scratch {
            if let Some(received) = &mut publication.received {
                received.verify_at(source)?;
                received.file.rewind()?;
                copy_verified(
                    received.file,
                    source,
                    &scratch,
                    entry,
                    sha512,
                    Some(received.cancel),
                )?;
                received.verify_at(source)?;
            } else {
                // An undone import or cleared cache may be absent. If no
                // complete copy survives, fail without publishing anything.
                if engine.paths.guard(source, &[])? != source {
                    return Err(invalid_source());
                }
                let mut input = safe_fs::open_regular(source)?;
                copy_verified(&mut input, source, &scratch, entry, sha512, None)?;
            }
        }
        let identity = safe_fs::identity(&scratch)?.ok_or_else(changed)?;
        if let Some(received) = &publication.received {
            crate::archive::list_verified_archive(&scratch, &entry.sha256, received.cancel)?;
            received.verify_at(source)?;
        }
        verify_file(&scratch, entry, sha512)?;
        // This also covers a crash after the full scratch write, before flush.
        #[cfg(unix)]
        safe_fs::open_regular(&scratch)?.sync_all()?;
        safe_fs::rename_checked(&scratch, archive, &identity)?;
    }
    verify_file(archive, entry, sha512)?;
    // Match storage relocation's directory durability contract on Unix. On
    // Windows retries also recover if a ready row outlives a missing rename.
    #[cfg(unix)]
    sync_library_directories(engine, archive_dir, payload, module)?;
    Ok(())
}

#[cfg(unix)]
fn sync_directory(engine: &FileEngine, dir: &Path) -> AppResult<()> {
    engine.paths.stable_directory(dir)?;
    File::open(dir)?.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn sync_library_directories(
    engine: &FileEngine,
    archive: &Path,
    payload: &Path,
    module: &Path,
) -> AppResult<()> {
    for dir in [
        archive,
        payload,
        module,
        module.parent().unwrap(),
        &engine.paths.mod_library(),
        &engine.paths.root,
    ] {
        sync_directory(engine, dir)?;
    }
    Ok(())
}

fn copy_verified(
    input: &mut File,
    source: &Path,
    scratch: &Path,
    entry: &LibraryEntry,
    sha512: Option<&str>,
    cancel: Option<&CancellationToken>,
) -> AppResult<()> {
    safe_fs::verify_regular_at(input, source)?;
    if input.metadata()?.len() != entry.size {
        return Err(changed());
    }
    if exists(scratch)? {
        safe_fs::rename_exclusive(
            scratch,
            &scratch.with_file_name(util::new_id(".archive-incomplete")),
        )?;
    }
    let mut output = safe_fs::create_new(scratch)?;
    hash_file(input, source, entry, sha512, Some(&mut output), cancel)?;
    safe_fs::verify_regular_at(&output, scratch)?;
    output.sync_all()?;
    Ok(())
}

fn verify_file(path: &Path, entry: &LibraryEntry, sha512: Option<&str>) -> AppResult<()> {
    let mut file = safe_fs::open_regular(path)?;
    hash_file(&mut file, path, entry, sha512, None, None)
}

fn hash_file(
    file: &mut File,
    path: &Path,
    entry: &LibraryEntry,
    sha512: Option<&str>,
    mut output: Option<&mut File>,
    cancel: Option<&CancellationToken>,
) -> AppResult<()> {
    safe_fs::verify_regular_at(file, path)?;
    let before = file.metadata()?;
    if before.len() != entry.size {
        return Err(changed());
    }
    let mut digest = Sha256::new();
    let mut strong = sha512.map(|_| Sha512::new());
    let mut size = 0u64;
    let mut buffer = vec![0; 128 * 1024];
    let mut reader = (&mut *file).take(entry.size + 1);
    loop {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(AppError::Cancelled);
        }
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > entry.size {
            return Err(changed());
        }
        let bytes = &buffer[..n];
        if let Some(writer) = &mut output {
            writer.write_all(bytes)?;
        }
        digest.update(bytes);
        if let Some(strong) = &mut strong {
            strong.update(bytes);
        }
    }
    safe_fs::verify_regular_at(file, path)?;
    let after = file.metadata()?;
    if size != entry.size
        || after.len() != entry.size
        || before.modified()? != after.modified()?
        || hex::encode(digest.finalize()) != entry.sha256
        || strong
            .zip(sha512)
            .is_some_and(|(actual, expected)| hex::encode(actual.finalize()) != expected)
    {
        return Err(changed());
    }
    Ok(())
}

/// Bound records belong only to their exact registration. Unbound candidates
/// are listed by their normalized human game name, not by a lossy folder slug.
pub fn list_for_game(db: &Db, game_id: &str) -> AppResult<Vec<LibraryEntry>> {
    let conn = db.conn();
    let game = games::get_game_from(&conn, game_id)?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM mod_library_entries WHERE state='ready' AND NOT EXISTS (SELECT 1 FROM removed_mod_items h WHERE h.game_id=?1 AND h.kind='archive' AND h.item_id=mod_library_entries.id) \
         AND (game_id=?1 OR (game_id IS NULL AND game_name_key=?2)) ORDER BY created_at DESC,id"
    ))?;
    let rows = stmt.query_map(params![game_id, game_key(&game.name)], read_entry)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn require_scope(
    conn: &Connection,
    game_id: &str,
    entry: &LibraryEntry,
    name_key: &str,
) -> AppResult<()> {
    let game = games::get_game_from(conn, game_id)?;
    crate::mod_management::require_archive_visible(conn, game_id, &entry.id)?;
    if let Some(bound) = &entry.game_id {
        if bound == game_id {
            return Ok(());
        }
        return Err(AppError::Denied("归档属于另一已登记游戏".into()));
    }
    if game_key(&game.name) != name_key {
        return Err(AppError::Denied("归档与此游戏名称不匹配".into()));
    }
    let mut stmt = conn.prepare("SELECT name FROM games WHERE id != ?1")?;
    for name in stmt.query_map([game_id], |row| row.get::<_, String>(0))? {
        if game_key(&name?) == name_key {
            return Err(AppError::Conflict(
                "存在同名的游戏登记，请先明确绑定模组归档".into(),
            ));
        }
    }
    Ok(())
}

fn read_ready(
    conn: &Connection,
    artifact_id: &str,
) -> AppResult<(LibraryEntry, String, Option<String>)> {
    conn.query_row(
        &format!("SELECT {COLUMNS},game_name_key,sha512 FROM mod_library_entries WHERE state='ready' AND id=?1"),
        [artifact_id],
        |row| Ok((read_entry(row)?, row.get(17)?, row.get(18)?)),
    ).optional()?.ok_or_else(|| AppError::NotFound(format!("模组库归档: {artifact_id}")))
}

fn verified_locations(
    engine: &FileEngine,
    entry: &LibraryEntry,
    sha512: Option<&str>,
) -> AppResult<(PathBuf, PathBuf)> {
    let (archive, payload) = locations(engine, entry)?;
    engine.paths.stable_directory(&engine.paths.mod_library())?;
    if exists(&payload)? {
        engine.paths.stable_directory(&payload)?;
        if safe_fs::identity(&payload)?.is_none() {
            return Err(changed());
        }
    }
    verify_file(&archive, entry, sha512)?;
    Ok((archive, payload))
}

/// Explicitly associate a verified unbound archive with a selected registered
/// copy of the named game. This does not move the archive or read its source.
pub fn bind_entry(
    engine: &FileEngine,
    game_id: &str,
    artifact_id: &str,
) -> AppResult<LibraryEntry> {
    let _files = transaction::write_lock();
    let (mut entry, key, sha512) = {
        let conn = engine.db.conn();
        let game = games::get_game_from(&conn, game_id)?;
        let (entry, key, sha512) = read_ready(&conn, artifact_id)?;
        crate::mod_management::require_archive_visible(&conn, game_id, artifact_id)?;
        if entry
            .game_id
            .as_deref()
            .is_some_and(|bound| bound != game_id)
        {
            return Err(AppError::Denied("归档已绑定到另一已登记游戏".into()));
        }
        if entry.game_id.is_none() && game_key(&game.name) != key {
            return Err(AppError::Denied("归档与此游戏名称不匹配".into()));
        }
        (entry, key, sha512)
    };
    verified_locations(engine, &entry, sha512.as_deref())?;
    if entry.game_id.is_none() {
        let mut conn = engine.db.conn();
        let tx = conn.transaction()?;
        let game = games::get_game_from(&tx, game_id)?;
        if game_key(&game.name) != key || tx.execute(
            "UPDATE mod_library_entries SET game_id=?2 WHERE id=?1 AND state='ready' AND game_name_key=?3 AND (game_id IS NULL OR game_id=?2)",
            params![artifact_id, game_id, key],
        )? != 1 {
            return Err(AppError::Conflict("归档或游戏登记已变化".into()));
        }
        tx.commit()?;
        entry.game_id = Some(game_id.into());
    }
    Ok(entry)
}

/// Revalidate both the registration scope and current archive bytes before use.
pub fn verify_for_install(
    engine: &FileEngine,
    game_id: &str,
    artifact_id: &str,
) -> AppResult<VerifiedLibraryEntry> {
    let (entry, sha512) = {
        let conn = engine.db.conn();
        let (entry, key, sha512) = read_ready(&conn, artifact_id)?;
        require_scope(&conn, game_id, &entry, &key)?;
        (entry, sha512)
    };
    let (archive_path, payload_path) = verified_locations(engine, &entry, sha512.as_deref())?;
    let origin = entry
        .project_id
        .as_ref()
        .zip(entry.version_id.as_ref())
        .map(|(project_id, version_id)| DownloadOrigin {
            artifact_id: entry.id.clone(),
            source_id: entry.source_id.clone(),
            project_id: project_id.clone(),
            version_id: version_id.clone(),
        });
    Ok(VerifiedLibraryEntry {
        entry,
        archive_path,
        payload_path,
        origin,
    })
}

/// Path-based installs must use the same scope check as ID-based installs. An
/// unknown, redirected, or pending path inside Mods must not fall back to local.
pub fn for_install_path(
    engine: &FileEngine,
    game_id: &str,
    path: &Path,
) -> AppResult<Option<VerifiedLibraryEntry>> {
    let root = engine.paths.mod_library();
    let canonical = paths::canonicalize_lenient(path)?;
    let spelled_inside = paths::contains_path(
        paths::user_path(&root).as_ref(),
        paths::user_path(path).as_ref(),
    );
    let resolved_inside = paths::contains_path(&root, &canonical);
    if !spelled_inside && !resolved_inside {
        // A retained completion-cache copy must not bypass a known library
        // binding. An unrelated or never-classified old download is unchanged.
        if paths::contains_path(&engine.paths.downloads(), &canonical) {
            if let Some(catalog) = artifacts::for_path(&engine.db, &canonical)? {
                let conn = engine.db.conn();
                let claim = conn
                    .query_row(
                        &format!(
                            "SELECT {COLUMNS},game_name_key FROM mod_library_entries WHERE id=?1"
                        ),
                        [&catalog.artifact.id],
                        |row| Ok((read_entry(row)?, row.get::<_, String>(17)?)),
                    )
                    .optional()?;
                if let Some((entry, key)) = claim {
                    require_scope(&conn, game_id, &entry, &key)?;
                }
            }
        }
        return Ok(None);
    }
    if !spelled_inside || !resolved_inside || paths::user_path(&canonical) != paths::user_path(path)
    {
        return Err(AppError::Denied(
            "不接受经链接或别名引用的模组库归档".into(),
        ));
    }
    let relative = canonical
        .strip_prefix(&engine.paths.root)
        .map_err(|_| AppError::Denied("模组库路径不在活动存储根中".into()))?;
    let relative = relative
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| AppError::Denied("模组库归档路径编码无效".into()))?
        .join("/");
    let id: String = engine
        .db
        .conn()
        .query_row(
            "SELECT id FROM mod_library_entries WHERE archive_relative_path=?1 AND state='ready'",
            [&relative],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| AppError::Denied("路径不对应已准备好的模组库归档".into()))?;
    verify_for_install(engine, game_id, &id).map(Some)
}

#[cfg(test)]
#[path = "mod_library_tests.rs"]
mod tests;
