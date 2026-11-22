//! Completed-download catalog, without signed URLs or request credentials.
use crate::{
    db::Db,
    download::DownloadArtifact,
    error::{AppError, AppResult},
    mod_library::NativeDownloadIntent,
    sources::ResolvedMod,
    transaction::FileEngine,
    util,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub(crate) const WEB_SOURCE: &str = "web";

/// Catalog claims about a download, not authentication of a remote provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadOrigin {
    pub artifact_id: String,
    pub source_id: String,
    pub project_id: String,
    pub version_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRecord {
    pub artifact: DownloadArtifact,
    pub source_id: String,
    pub project_id: String,
    pub version_id: String,
    pub name: String,
    pub version: String,
    pub created_at: String,
    pub project_page: Option<String>,
    #[serde(skip)]
    transport_path: Option<PathBuf>,
    #[serde(skip)]
    native_library_intent: Option<NativeDownloadIntent>,
}

impl ArtifactRecord {
    /// Claims from a structurally bound catalog row. A deployment must also
    /// check the canonical payload path, filename and hash before recording origin.
    pub(crate) fn transport_path(&self) -> &Path {
        self.transport_path
            .as_deref()
            .unwrap_or(&self.artifact.path)
    }

    pub(crate) fn native_library_intent(&self) -> Option<&NativeDownloadIntent> {
        self.native_library_intent.as_ref()
    }

    pub fn origin(&self) -> Option<DownloadOrigin> {
        if self.source_id == WEB_SOURCE {
            return None;
        }
        let artifact_id = &self.artifact.id;
        // Legacy fingerprint-keyed rows could have their payload rebound
        // without changing provider IDs. They cannot establish exact origin.
        let uuid = uuid::Uuid::parse_str(artifact_id).ok()?;
        if uuid.hyphenated().to_string() != *artifact_id
            || self.transport_path().file_name()?.to_str()?
                != format!("{artifact_id}-{}", self.artifact.filename)
        {
            return None;
        }
        [&self.source_id, &self.project_id, &self.version_id]
            .into_iter()
            .all(|id| valid_origin_id(id))
            .then(|| DownloadOrigin {
                artifact_id: artifact_id.clone(),
                source_id: self.source_id.clone(),
                project_id: self.project_id.clone(),
                version_id: self.version_id.clone(),
            })
    }
}

const MAX_ORIGIN_ID_BYTES: usize = 512;

fn valid_origin_id(id: &str) -> bool {
    // Keep provider-native tokens such as Thunderstore's namespace/name, but
    // not URL schemes, authorities, query strings or control characters.
    id.len() <= MAX_ORIGIN_ID_BYTES
        && id.split('/').all(|part| {
            !matches!(part, "" | "." | "..")
                && part.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'~' | b'+')
                })
        })
}

pub fn record(
    engine: &FileEngine,
    artifact: &DownloadArtifact,
    source: &ResolvedMod,
) -> AppResult<()> {
    record_resolved(engine, artifact, source, None)
}

pub(crate) fn record_with_library_intent(
    engine: &FileEngine,
    artifact: &DownloadArtifact,
    source: &ResolvedMod,
    intent: &NativeDownloadIntent,
) -> AppResult<()> {
    record_resolved(engine, artifact, source, Some(intent))
}

fn record_resolved(
    engine: &FileEngine,
    artifact: &DownloadArtifact,
    source: &ResolvedMod,
    intent: Option<&NativeDownloadIntent>,
) -> AppResult<()> {
    // Never copy the transfer URL (which can include credentials), even if a
    // provider accidentally labels the equivalent endpoint as its homepage.
    let project_page = source
        .homepage_url
        .as_deref()
        .filter(|page| !same_public_endpoint(page, &source.download_url));
    let intent = intent.map(|intent| intent.without_transport_page(&source.download_url));
    record_with_metadata(
        engine,
        artifact,
        CatalogMetadata {
            source_id: &source.source_id,
            project_id: &source.project_id,
            version_id: &source.version_id,
            name: &source.name,
            version: &source.version,
            project_page,
            native_library_intent: intent.as_ref(),
        },
    )
}

/// These IDs come from the approved call to the official Mod Inn MCP endpoint.
pub(crate) fn record_glossmod_file_with_library_intent(
    engine: &FileEngine,
    artifact: &DownloadArtifact,
    work_id: u64,
    file_id: u64,
    intent: &NativeDownloadIntent,
) -> AppResult<()> {
    record_with_metadata(
        engine,
        artifact,
        CatalogMetadata {
            source_id: "glossmod_http",
            project_id: &work_id.to_string(),
            version_id: &file_id.to_string(),
            name: &artifact.filename,
            version: "",
            project_page: None,
            native_library_intent: Some(intent),
        },
    )
}

/// Legacy/unclassified test receipt; not eligible for native retry.
#[cfg(test)]
pub(crate) fn record_web(engine: &FileEngine, artifact: &DownloadArtifact) -> AppResult<()> {
    record_web_result(engine, artifact, None)
}

pub(crate) fn record_web_with_library_intent(
    engine: &FileEngine,
    artifact: &DownloadArtifact,
    intent: &NativeDownloadIntent,
) -> AppResult<()> {
    record_web_result(engine, artifact, Some(intent))
}

fn record_web_result(
    engine: &FileEngine,
    artifact: &DownloadArtifact,
    intent: Option<&NativeDownloadIntent>,
) -> AppResult<()> {
    record_with_metadata(
        engine,
        artifact,
        CatalogMetadata {
            source_id: WEB_SOURCE,
            project_id: "",
            version_id: "",
            name: &artifact.filename,
            version: "",
            project_page: None,
            native_library_intent: intent,
        },
    )
}

struct CatalogMetadata<'a> {
    source_id: &'a str,
    project_id: &'a str,
    version_id: &'a str,
    name: &'a str,
    version: &'a str,
    project_page: Option<&'a str>,
    native_library_intent: Option<&'a NativeDownloadIntent>,
}

// Additional public metadata lives in the existing JSON object; the artifact
// wire format and older catalog rows remain compatible.
#[derive(Serialize, Deserialize)]
struct CatalogArtifact {
    #[serde(flatten)]
    artifact: DownloadArtifact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_page: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transport_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_library_intent: Option<NativeDownloadIntent>,
}

pub(crate) fn same_public_endpoint(page: &str, transport: &str) -> bool {
    let stripped = |value: &str| {
        let mut url = url::Url::parse(value).ok()?;
        url.set_query(None);
        url.set_fragment(None);
        Some(url)
    };
    stripped(page)
        .zip(stripped(transport))
        .is_some_and(|(left, right)| left == right)
}

fn project_page_url(raw: &str) -> Option<url::Url> {
    let url = crate::sources::validate_download_url(raw).ok()?;
    if raw != raw.trim()
        || raw.len() > 2048
        || raw.contains('\\')
        || raw.chars().any(char::is_control)
        || url.scheme() != "https"
        || url.port().is_some()
    {
        return None;
    }
    let host = url.host_str()?.trim_end_matches('.');
    let cdn_host = host
        .split('.')
        .next()
        .is_some_and(|label| label == "cdn" || label.starts_with("cdn-"))
        || [
            "forgecdn.net",
            "nexus-cdn.com",
            "cloudfront.net",
            "akamaized.net",
            "r2.dev",
        ]
        .iter()
        .any(|suffix| {
            host == *suffix
                || host
                    .strip_suffix(*suffix)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        })
        || matches!(
            host,
            "objects.githubusercontent.com"
                | "release-assets.githubusercontent.com"
                | "codeload.github.com"
        );
    let path = url
        .path()
        .to_ascii_lowercase()
        .replace("%2e", ".")
        .replace("%2f", "/");
    let last = path.trim_end_matches('/').rsplit('/').next()?;
    let artifact_link = [
        ".zip", ".jar", ".mrpack", ".7z", ".rar", ".tar", ".gz", ".tgz", ".pak", ".dll", ".exe",
        ".asi", ".esp", ".esm", ".esl", ".bsa", ".ba2", ".bin",
    ]
    .iter()
    .any(|ext| last.ends_with(*ext))
        || matches!(last, "download" | "downloadfile")
        || path.contains("/releases/download/");
    (!cdn_host && !artifact_link).then_some(url)
}

pub(crate) fn public_project_page(raw: &str) -> Option<String> {
    let mut url = project_page_url(raw)?;
    url.set_query(None);
    (url.as_str().len() <= 2048).then(|| url.into())
}

pub(crate) fn explicit_project_page(raw: &str) -> AppResult<String> {
    project_page_url(raw)
        .filter(|url| url.query().is_none() && url.as_str().len() <= 2048)
        .map(Into::into)
        .ok_or_else(|| {
            AppError::Invalid(
                "项目页必须是无凭据、查询参数或片段的公开 HTTPS 页面，不能使用下载链接".into(),
            )
        })
}

fn record_with_metadata(
    engine: &FileEngine,
    artifact: &DownloadArtifact,
    metadata: CatalogMetadata<'_>,
) -> AppResult<()> {
    engine.paths.stable_directory(&engine.paths.root)?;
    engine.paths.stable_directory(&engine.paths.downloads())?;
    let path = crate::paths::ensure_within(&artifact.path, &[engine.paths.downloads()])?;
    if path != artifact.path || !std::fs::symlink_metadata(&path)?.file_type().is_file() {
        return Err(AppError::Denied("完成下载路径不再是规范普通文件".into()));
    }
    let native_library_intent = metadata
        .native_library_intent
        .map(NativeDownloadIntent::validated)
        .transpose()?;
    let path = path.to_string_lossy();
    let conn = engine.db.conn();
    let inserted = conn.execute(
        "INSERT INTO download_artifacts (
            id, path, filename, artifact_json, source_id, project_id, version_id, name, version, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT DO NOTHING",
        params![
            artifact.id,
            path,
            artifact.filename,
            serde_json::to_string(&CatalogArtifact {
                artifact: artifact.clone(),
                project_page: metadata.project_page.and_then(public_project_page),
                transport_path: None,
                native_library_intent: native_library_intent.clone(),
            })?,
            metadata.source_id,
            metadata.project_id,
            metadata.version_id,
            metadata.name,
            metadata.version,
            util::now(),
        ],
    )?;
    if inserted == 1 {
        return Ok(());
    }

    let existing = conn
        .query_row(
            "SELECT artifact_json FROM download_artifacts
             WHERE id=?1 AND path=?2 AND filename=?3 AND source_id=?4 AND project_id=?5
               AND version_id=?6 AND name=?7 AND version=?8",
            params![
                artifact.id,
                path,
                artifact.filename,
                metadata.source_id,
                metadata.project_id,
                metadata.version_id,
                metadata.name,
                metadata.version,
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if existing.is_some_and(|json| {
        serde_json::from_str::<CatalogArtifact>(&json).is_ok_and(|stored| {
            &stored.artifact == artifact && stored.native_library_intent == native_library_intent
        })
    }) {
        return Ok(());
    }
    Err(AppError::Conflict("已存在不同的完成下载记录".into()))
}

fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactRecord> {
    let value: String = row.get(0)?;
    let CatalogArtifact {
        artifact,
        project_page,
        transport_path,
        native_library_intent,
    } = serde_json::from_str(&value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(ArtifactRecord {
        artifact,
        source_id: row.get(1)?,
        project_id: row.get(2)?,
        version_id: row.get(3)?,
        name: row.get(4)?,
        version: row.get(5)?,
        created_at: row.get(6)?,
        project_page: project_page.as_deref().and_then(public_project_page),
        transport_path,
        native_library_intent,
    })
}
pub fn list(db: &Db) -> AppResult<Vec<ArtifactRecord>> {
    let conn = db.conn();
    let mut stmt = conn.prepare("SELECT artifact_json,source_id,project_id,version_id,name,version,created_at FROM download_artifacts ORDER BY created_at DESC,rowid DESC")?;
    let rows = stmt.query_map([], from_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
pub fn for_path(db: &Db, path: &Path) -> AppResult<Option<ArtifactRecord>> {
    Ok(db.conn().query_row("SELECT artifact_json,source_id,project_id,version_id,name,version,created_at FROM download_artifacts WHERE path=?1",[path.to_string_lossy()],from_row).optional()?)
}

pub(crate) fn for_id(db: &Db, id: &str) -> AppResult<Option<ArtifactRecord>> {
    Ok(db.conn().query_row("SELECT artifact_json,source_id,project_id,version_id,name,version,created_at FROM download_artifacts WHERE id=?1", [id], from_row).optional()?)
}

/// Bind a supplied catalog entry back to the current database by both keys.
/// Ingestion validates the underlying file separately before keeping a copy.
pub(crate) fn verify_catalog_entry(db: &Db, entry: &ArtifactRecord) -> AppResult<ArtifactRecord> {
    let invalid = || AppError::Conflict("完成下载的目录记录不匹配".into());
    let current = for_id(db, &entry.artifact.id)?.ok_or_else(invalid)?;
    if for_path(db, &current.artifact.path)?.as_ref() != Some(&current) {
        return Err(invalid());
    }
    if &current != entry {
        // A caller can repeat an exact receipt obtained before a successful
        // relocation. Only the path may differ, and only to its old location.
        let mut prior = current.clone();
        prior.artifact.path = prior.transport_path.take().ok_or_else(invalid)?;
        if &prior != entry {
            return Err(invalid());
        }
    }
    Ok(current)
}

/// Rebase a completed catalog entry without changing its source-binding token.
/// The same transaction publishes the verified library row.
pub(crate) fn relocate_catalog_entry(
    conn: &Connection,
    record: &ArtifactRecord,
    target: &Path,
) -> AppResult<()> {
    if record.artifact.path == target {
        return Ok(());
    }
    if record.transport_path.is_some() {
        return Err(AppError::Conflict("完成下载已归档到其他位置".into()));
    }
    let mut artifact = record.artifact.clone();
    let original = std::mem::replace(&mut artifact.path, target.into());
    let target = target
        .to_str()
        .ok_or_else(|| AppError::Invalid("归档路径编码无效".into()))?;
    let count = conn.execute(
        "UPDATE download_artifacts SET path=?1,artifact_json=?2 WHERE id=?3 AND path=?4",
        params![
            target,
            serde_json::to_string(&CatalogArtifact {
                artifact,
                project_page: record.project_page.clone(),
                transport_path: Some(original.clone()),
                native_library_intent: record.native_library_intent.clone(),
            })?,
            record.artifact.id,
            original.to_str()
        ],
    )?;
    if count != 1 {
        return Err(AppError::Conflict("完成下载记录在归档期间已变化".into()));
    }
    Ok(())
}

#[cfg(test)]
#[path = "artifacts_tests.rs"]
mod tests;
