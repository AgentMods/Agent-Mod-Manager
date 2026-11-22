//! Bounded filesystem observations, separate from past loader selections.
use super::{fabric::FabricEvidence, fabric::FabricStatus, fabric_manifest, Context};
use crate::{
    error::{AppError, AppResult},
    install::INSTALL_STATE,
    plugins::validate_relative_path,
    safe_fs,
    transaction::FileEngine,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[cfg(test)]
use std::{
    ffi::OsString,
    time::{Duration, Instant},
};

#[cfg(test)]
const MAX_ENTRIES: usize = 512;
#[cfg(test)]
const MAX_JARS: usize = 64;
#[cfg(test)]
const MAX_RECORDS: usize = 128;
const MAX_RECORD_BYTES: usize = 64 * 1024;
#[cfg(test)]
const MAX_MANIFEST_PATHS: usize = 4096;
#[cfg(test)]
const SOFT_TIME_LIMIT: Duration = Duration::from_secs(3);
const BUILTIN_IDS: &[&str] = &["minecraft", "java", "fabricloader"];

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ScanStatus {
    Scanned,
    Missing,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MetadataStatus {
    Present,
    Absent,
    Invalid,
    Limited,
    Unreadable,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Ownership {
    Recorded,
    Unrecorded,
    Ambiguous,
    Unverified,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RelationStatus {
    Exact,
    VersionDiffers,
    Ambiguous,
    NotListed,
    Unverified,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LogRelation {
    status: RelationStatus,
    line: Option<usize>,
    version: Option<String>,
}

impl LogRelation {
    fn without_line(status: RelationStatus) -> Self {
        Self {
            status,
            line: None,
            version: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JarEvidence {
    file: String,
    metadata_status: MetadataStatus,
    id: Option<String>,
    version: Option<String>,
    ownership: Ownership,
    log_relation: LogRelation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FabricDiskEvidence {
    directory: String,
    status: ScanStatus,
    jars: Vec<JarEvidence>,
}

impl FabricDiskEvidence {
    fn empty(status: ScanStatus) -> Self {
        Self {
            directory: "mods/".into(),
            status,
            jars: vec![],
        }
    }
}

#[cfg(test)]
pub(super) fn inspect(
    engine: &FileEngine,
    ctx: &Context,
    log: Option<&FabricEvidence>,
) -> Option<FabricDiskEvidence> {
    if ctx.adapter.slug != "minecraft-java"
        || !ctx.adapter.install_roots.iter().any(|r| {
            r.id == "mods"
                && r.kind == crate::plugins::RootKind::Game
                && r.relative_path == "mods"
                && r.platforms.contains(&ctx.platform)
        })
    {
        return None;
    }
    let result = match ctx.root("mods") {
        Ok(dir) if dir == ctx.root.join("mods") => scan(engine, ctx, &dir, log),
        Err(e) => Err(e),
        _ => Err(AppError::Denied("无法确认实例的 mods 目录".into())),
    };
    Some(match result {
        Ok(report) => report,
        Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            FabricDiskEvidence::empty(ScanStatus::Missing)
        }
        Err(_) => FabricDiskEvidence::empty(ScanStatus::Unavailable),
    })
}

fn directory_state(ctx: &Context, dir: &Path) -> AppResult<(safe_fs::FileIdentity, SystemTime)> {
    ctx.guard(dir)?;
    super::stable_directory(dir)?;
    let modified = fs::symlink_metadata(dir)?.modified()?;
    let id =
        safe_fs::identity(dir)?.ok_or_else(|| AppError::Denied("无法核对 mods 目录".into()))?;
    Ok((id, modified))
}

fn confirm_directory(
    ctx: &Context,
    dir: &Path,
    before: &(safe_fs::FileIdentity, SystemTime),
) -> AppResult<()> {
    if directory_state(ctx, dir).is_ok_and(|after| &after == before) {
        return Ok(());
    }
    Err(AppError::Conflict("诊断期间 mods 目录有变化".into()))
}

#[cfg(test)]
fn scan(
    engine: &FileEngine,
    ctx: &Context,
    dir: &Path,
    log: Option<&FabricEvidence>,
) -> AppResult<FabricDiskEvidence> {
    let before = directory_state(ctx, dir)?;
    let deadline = Instant::now() + SOFT_TIME_LIMIT;
    let mut candidates = Vec::new();
    let mut partial = false;
    for (index, entry) in safe_fs::directory_entries(dir, &before.0)?.enumerate() {
        if index == MAX_ENTRIES || Instant::now() >= deadline {
            partial = true;
            break;
        }
        let Ok((name, is_dir)) = entry else {
            partial = true;
            continue;
        };
        let Some(name) = name.to_str() else {
            partial = true;
            continue;
        };
        if name.starts_with('.') || !name.ends_with(".jar") {
            continue;
        }
        if name.len() > 255 || validate_relative_path(name, false).is_err() {
            partial = true;
            continue;
        }
        if !is_dir {
            candidates.push(name.to_owned());
        }
    }
    confirm_directory(ctx, dir, &before)?;
    candidates.sort_unstable();
    let owners = OwnerEvidence::read(engine, &ctx.game.id, dir, deadline).unwrap_or_default();
    let mut jars = Vec::new();
    partial |= candidates.len() > MAX_JARS;
    for file in candidates.into_iter().take(MAX_JARS) {
        if Instant::now() >= deadline {
            partial = true;
            break;
        }
        let path = dir.join(&file);
        let (metadata_status, id, version) = match inspect_jar(ctx, &path) {
            Ok(fabric_manifest::Manifest::Present { id, version }) => {
                (MetadataStatus::Present, Some(id), Some(version))
            }
            Ok(fabric_manifest::Manifest::Absent) => (MetadataStatus::Absent, None, None),
            Ok(fabric_manifest::Manifest::Invalid) => (MetadataStatus::Invalid, None, None),
            Ok(fabric_manifest::Manifest::Limited) => (MetadataStatus::Limited, None, None),
            Err(_) => (MetadataStatus::Unreadable, None, None),
        };
        partial |= !matches!(
            metadata_status,
            MetadataStatus::Present | MetadataStatus::Absent
        );
        jars.push(JarEvidence {
            file: super::clean_text(&file),
            metadata_status,
            id,
            version,
            ownership: owners.classify(&path),
            log_relation: LogRelation::without_line(RelationStatus::Unverified),
        });
    }
    confirm_directory(ctx, dir, &before)?;
    compare_log(&mut jars, log);
    Ok(FabricDiskEvidence {
        directory: "mods/".into(),
        status: if partial {
            ScanStatus::Partial
        } else {
            ScanStatus::Scanned
        },
        jars,
    })
}

fn inspect_jar(ctx: &Context, path: &Path) -> AppResult<fabric_manifest::Manifest> {
    ctx.guard(path)?;
    let file = safe_fs::open_regular(path)?;
    ctx.guard(path)?;
    safe_fs::verify_regular_at(&file, path)?;
    let before = file.metadata()?;
    let modified = before.modified()?;
    let result = fabric_manifest::inspect(&file, before.len());
    ctx.guard(path)?;
    safe_fs::verify_regular_at(&file, path)?;
    let after = file.metadata()?;
    if before.len() != after.len() || modified != after.modified()? {
        return Err(AppError::Conflict("读取期间 JAR 有变化".into()));
    }
    Ok(result)
}

fn compare_log(jars: &mut [JarEvidence], startup: Option<&FabricEvidence>) {
    let mut count = HashMap::new();
    for id in jars.iter().filter_map(|j| j.id.as_ref()) {
        *count.entry(id.clone()).or_insert(0usize) += 1;
    }
    let selection = startup.filter(|log| log.status == FabricStatus::SelectionReported);
    for jar in jars {
        let Some((id, version)) = jar.id.as_deref().zip(jar.version.as_deref()) else {
            continue;
        };
        if count[id] > 1 {
            jar.log_relation = LogRelation::without_line(RelationStatus::Ambiguous);
            continue;
        }
        let Some(log) = selection.filter(|_| !BUILTIN_IDS.contains(&id)) else {
            continue;
        };
        jar.log_relation = match log.mods.iter().find(|row| row.id == id) {
            Some(row) if !row.nested => LogRelation {
                status: if row.version == version {
                    RelationStatus::Exact
                } else {
                    RelationStatus::VersionDiffers
                },
                line: Some(row.line),
                version: Some(row.version.clone()),
            },
            None if log.omitted_count == 0 => LogRelation::without_line(RelationStatus::NotListed),
            _ => LogRelation::without_line(RelationStatus::Unverified),
        };
    }
}

#[cfg(test)]
#[derive(Default)]
struct OwnerEvidence {
    matches: HashMap<OsString, usize>,
    complete: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RecordedPath {
    Legacy(PathBuf),
    File { path: PathBuf },
}

#[cfg(test)]
impl OwnerEvidence {
    fn read(engine: &FileEngine, game_id: &str, dir: &Path, deadline: Instant) -> AppResult<Self> {
        let mut evidence = Self {
            complete: true,
            ..Self::default()
        };
        let conn = engine.db.conn();
        let mut query = conn.prepare(&format!(
            "SELECT CASE WHEN length(CAST(files_json AS BLOB))<=?2 THEN files_json END
             FROM installs WHERE game_id=?1 AND
             CASE WHEN EXISTS (SELECT 1 FROM install_transactions WHERE install_id=installs.id)
               THEN {INSTALL_STATE} ELSE status END='enabled' ORDER BY id LIMIT ?3"
        ))?;
        let rows = query.query_map(
            rusqlite::params![game_id, MAX_RECORD_BYTES, MAX_RECORDS + 1],
            |row| row.get::<_, Option<String>>(0),
        )?;
        let mut remaining = MAX_MANIFEST_PATHS;
        for (index, row) in rows.enumerate() {
            if index == MAX_RECORDS || Instant::now() >= deadline {
                evidence.complete = false;
                break;
            }
            let Some(files): Option<Vec<RecordedPath>> =
                row?.and_then(|s| serde_json::from_str(&s).ok())
            else {
                evidence.complete = false;
                continue;
            };
            let Some(left) = remaining.checked_sub(files.len()) else {
                evidence.complete = false;
                break;
            };
            remaining = left;
            let mut paths = std::collections::HashSet::new();
            for RecordedPath::Legacy(path) | RecordedPath::File { path } in files {
                if !path.is_absolute() {
                    evidence.complete = false;
                } else if path.parent() == Some(dir) {
                    paths.insert(path.into_os_string());
                }
            }
            for path in paths {
                *evidence.matches.entry(path).or_default() += 1;
            }
        }
        Ok(evidence)
    }

    fn classify(&self, path: &Path) -> Ownership {
        match self
            .matches
            .get(path.as_os_str())
            .copied()
            .unwrap_or_default()
        {
            2.. => Ownership::Ambiguous,
            _ if !self.complete => Ownership::Unverified,
            1 => Ownership::Recorded,
            _ => Ownership::Unrecorded,
        }
    }
}

/// Inventory every eligible JAR. The private spool preserves directory-wide
/// duplicate-ID checks and name ordering across output pages without a RAM list.
pub(super) fn stream(
    engine: &FileEngine,
    ctx: &Context,
    log: Option<&FabricEvidence>,
    cancel: &tokio_util::sync::CancellationToken,
    mut emit: impl FnMut(FabricDiskEvidence) -> AppResult<()>,
) -> AppResult<Option<FabricDiskEvidence>> {
    if ctx.adapter.slug != "minecraft-java"
        || !ctx.adapter.install_roots.iter().any(|r| {
            r.id == "mods"
                && r.kind == crate::plugins::RootKind::Game
                && r.relative_path == "mods"
                && r.platforms.contains(&ctx.platform)
        })
    {
        return Ok(None);
    }
    let check = || {
        if cancel.is_cancelled() {
            Err(AppError::Cancelled)
        } else {
            Ok(())
        }
    };
    let result = (|| -> AppResult<FabricDiskEvidence> {
        let dir = ctx.root("mods")?;
        if dir != ctx.root.join("mods") {
            return Err(AppError::Denied("无法确认实例的 mods 目录".into()));
        }
        let before = directory_state(ctx, &dir)?;
        let temp = tempfile::tempdir()?;
        let spool = rusqlite::Connection::open(temp.path().join("fabric.sqlite"))?;
        spool.execute_batch(
            "PRAGMA journal_mode=OFF; PRAGMA cache_size=-1024;
            CREATE TABLE jars(name TEXT PRIMARY KEY,id TEXT,body TEXT NOT NULL);
            CREATE INDEX jars_id ON jars(id);
            CREATE TABLE owners(path TEXT PRIMARY KEY, count INTEGER NOT NULL);
            BEGIN;",
        )?;
        let ownership_complete = read_owners(engine, &ctx.game.id, &dir, &spool, cancel)?;
        let mut partial = false;
        for entry in safe_fs::directory_entries(&dir, &before.0)? {
            check()?;
            let Ok((name, is_dir)) = entry else {
                partial = true;
                continue;
            };
            let Some(name) = name.to_str() else {
                partial = true;
                continue;
            };
            if is_dir || name.starts_with('.') || !name.ends_with(".jar") {
                continue;
            }
            if name.len() > 255 || validate_relative_path(name, false).is_err() {
                partial = true;
                continue;
            }
            let path = dir.join(name);
            let (metadata_status, id, version) = match inspect_jar(ctx, &path) {
                Ok(fabric_manifest::Manifest::Present { id, version }) => {
                    (MetadataStatus::Present, Some(id), Some(version))
                }
                Ok(fabric_manifest::Manifest::Absent) => (MetadataStatus::Absent, None, None),
                Ok(fabric_manifest::Manifest::Invalid) => (MetadataStatus::Invalid, None, None),
                Ok(fabric_manifest::Manifest::Limited) => (MetadataStatus::Limited, None, None),
                Err(_) => (MetadataStatus::Unreadable, None, None),
            };
            partial |= !matches!(
                metadata_status,
                MetadataStatus::Present | MetadataStatus::Absent
            );
            let ownership = match spool.query_row(
                "SELECT COALESCE((SELECT count FROM owners WHERE path=?1),0)",
                [path.to_str()],
                |row| row.get::<_, usize>(0),
            )? {
                2.. => Ownership::Ambiguous,
                _ if !ownership_complete => Ownership::Unverified,
                1 => Ownership::Recorded,
                _ => Ownership::Unrecorded,
            };
            let row = JarEvidence {
                file: super::clean_text(name),
                metadata_status,
                id,
                version,
                ownership,
                log_relation: LogRelation::without_line(RelationStatus::Unverified),
            };
            spool.execute(
                "INSERT INTO jars VALUES(?1,?2,?3)",
                rusqlite::params![name, row.id, serde_json::to_string(&row)?],
            )?;
        }
        confirm_directory(ctx, &dir, &before)?;
        let mut report = FabricDiskEvidence::empty(if partial {
            ScanStatus::Partial
        } else {
            ScanStatus::Scanned
        });
        let mut query = spool.prepare("SELECT body, id IS NOT NULL AND EXISTS(SELECT 1 FROM jars AS other WHERE other.id=jars.id AND other.name<>jars.name) FROM jars ORDER BY name")?;
        let mut rows = query.query([])?;
        while let Some(row) = rows.next()? {
            check()?;
            if report.jars.len() == 64 {
                confirm_directory(ctx, &dir, &before)?;
                emit(FabricDiskEvidence {
                    jars: std::mem::take(&mut report.jars),
                    ..report.clone()
                })?;
            }
            let mut jar: JarEvidence = serde_json::from_str(&row.get::<_, String>(0)?)?;
            if row.get::<_, bool>(1)? {
                jar.log_relation = LogRelation::without_line(RelationStatus::Ambiguous);
            } else {
                compare_log(std::slice::from_mut(&mut jar), log);
            }
            report.jars.push(jar);
        }
        confirm_directory(ctx, &dir, &before)?;
        Ok(report)
    })();
    match result {
        Ok(report) => Ok(Some(report)),
        Err(AppError::Cancelled) => Err(AppError::Cancelled),
        Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(Some(FabricDiskEvidence::empty(ScanStatus::Missing)))
        }
        Err(_) => Ok(Some(FabricDiskEvidence::empty(ScanStatus::Unavailable))),
    }
}

fn read_owners(
    engine: &FileEngine,
    game_id: &str,
    dir: &Path,
    spool: &rusqlite::Connection,
    cancel: &tokio_util::sync::CancellationToken,
) -> AppResult<bool> {
    let mut complete = true;
    let mut after: Option<String> = None;
    loop {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let rows = {
            let conn = engine.db.conn();
            let mut query = conn.prepare(&format!(
                "SELECT id,CASE WHEN length(CAST(files_json AS BLOB))<=?2 THEN files_json END
                 FROM installs WHERE game_id=?1 AND (?3 IS NULL OR id>?3) AND
                 CASE WHEN EXISTS (SELECT 1 FROM install_transactions WHERE install_id=installs.id)
                   THEN {INSTALL_STATE} ELSE status END='enabled' ORDER BY id LIMIT 32"
            ))?;
            let values = query
                .query_map(rusqlite::params![game_id, MAX_RECORD_BYTES, after], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })?;
            values.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if rows.is_empty() {
            break;
        }
        for (id, files) in rows {
            after = Some(id);
            let Some(files): Option<Vec<RecordedPath>> =
                files.and_then(|s| serde_json::from_str(&s).ok())
            else {
                complete = false;
                continue;
            };
            let mut paths = std::collections::HashSet::new();
            for RecordedPath::Legacy(path) | RecordedPath::File { path } in files {
                if !path.is_absolute() {
                    complete = false;
                } else if path.parent() == Some(dir) {
                    paths.insert(path);
                }
            }
            for path in paths {
                spool.execute("INSERT INTO owners(path,count) VALUES(?1,1) ON CONFLICT(path) DO UPDATE SET count=count+1", [path.to_str()])?;
            }
        }
    }
    Ok(complete)
}
