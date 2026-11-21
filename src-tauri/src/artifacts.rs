//! Completed-download catalog, without signed URLs or request credentials.
use crate::{
    db::Db,
    download::DownloadArtifact,
    error::{AppError, AppResult},
    sources::ResolvedMod,
    transaction::FileEngine,
    util,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRecord {
    pub artifact: DownloadArtifact,
    pub source_id: String,
    pub project_id: String,
    pub version_id: String,
    pub name: String,
    pub version: String,
    pub created_at: String,
}

impl ArtifactRecord {
    /// Claims from a structurally bound catalog row. A deployment must also
    /// check the canonical payload path, filename and hash before recording origin.
    pub fn origin(&self) -> Option<DownloadOrigin> {
        if self.source_id == WEB_SOURCE {
            return None;
        }
        let artifact_id = &self.artifact.id;
        // Legacy fingerprint-keyed rows could have their payload rebound
        // without changing provider IDs. They cannot establish exact origin.
        let uuid = uuid::Uuid::parse_str(artifact_id).ok()?;
        if uuid.hyphenated().to_string() != *artifact_id
            || self.artifact.path.file_name()?.to_str()?
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
    record_with_metadata(
        engine,
        artifact,
        CatalogMetadata {
            source_id: &source.source_id,
            project_id: &source.project_id,
            version_id: &source.version_id,
            name: &source.name,
            version: &source.version,
        },
    )
}

/// These IDs come from the approved call to the official Mod Inn MCP endpoint.
pub(crate) fn record_glossmod_file(
    engine: &FileEngine,
    artifact: &DownloadArtifact,
    work_id: u64,
    file_id: u64,
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
        },
    )
}

/// Records a completed download from the reviewed direct-web workflow.
pub(crate) fn record_web(engine: &FileEngine, artifact: &DownloadArtifact) -> AppResult<()> {
    record_with_metadata(
        engine,
        artifact,
        CatalogMetadata {
            source_id: WEB_SOURCE,
            project_id: "",
            version_id: "",
            name: &artifact.filename,
            version: "",
        },
    )
}

struct CatalogMetadata<'a> {
    source_id: &'a str,
    project_id: &'a str,
    version_id: &'a str,
    name: &'a str,
    version: &'a str,
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
            serde_json::to_string(artifact)?,
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
        serde_json::from_str::<DownloadArtifact>(&json).is_ok_and(|stored| &stored == artifact)
    }) {
        return Ok(());
    }
    Err(AppError::Conflict("已存在不同的完成下载记录".into()))
}

fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactRecord> {
    let value: String = row.get(0)?;
    let artifact = serde_json::from_str(&value).map_err(|e| {
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
    })
}
pub fn list(db: &Db) -> AppResult<Vec<ArtifactRecord>> {
    let conn = db.conn();
    let mut stmt = conn.prepare("SELECT artifact_json,source_id,project_id,version_id,name,version,created_at FROM download_artifacts ORDER BY created_at DESC LIMIT 1000")?;
    let rows = stmt.query_map([], from_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
pub fn for_path(db: &Db, path: &Path) -> AppResult<Option<ArtifactRecord>> {
    Ok(db.conn().query_row("SELECT artifact_json,source_id,project_id,version_id,name,version,created_at FROM download_artifacts WHERE path=?1",[path.to_string_lossy()],from_row).optional()?)
}

pub(crate) fn for_id(db: &Db, id: &str) -> AppResult<Option<ArtifactRecord>> {
    Ok(db.conn().query_row("SELECT artifact_json,source_id,project_id,version_id,name,version,created_at FROM download_artifacts WHERE id=?1", [id], from_row).optional()?)
}

#[cfg(test)]
#[path = "artifacts_tests.rs"]
mod tests;
