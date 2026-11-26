use crate::{
    artifacts,
    db::Db,
    error::{AppError, AppResult},
    game_roots, games, install, mod_library, mod_management,
    transaction::{self, FileEngine},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, io::ErrorKind, path::PathBuf};

#[derive(Debug, Deserialize)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum Target {
    Archive(String),
    Install(String),
}

impl Target {
    fn parts(&self) -> (&'static str, &str) {
        match self {
            Self::Archive(id) => ("archive", id),
            Self::Install(id) => ("install", id),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Patch {
    name: Option<String>,
    tag_ids: Option<Vec<String>>,
}

fn display_text(text: &str, max: usize) -> Option<&str> {
    let text = text.trim();
    (!text.is_empty() && text.chars().count() <= max && !text.chars().any(char::is_control))
        .then_some(text)
}

pub(crate) fn display_version(text: &str) -> Option<&str> {
    display_text(text, 256)
}

// Thunderstore's selected version is the release number itself. Opaque resource IDs
// from other providers must never be displayed as a version.
fn provider_version(source: &str, id: &str) -> Option<String> {
    if source != "thunderstore" || id.len() > 64 || id.split('.').count() != 3 {
        return None;
    }
    id.split('.')
        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        .then(|| id.to_owned())
}

fn historical_receipt(
    conn: &Connection,
    origin: &artifacts::DownloadOrigin,
    hash: Option<&str>,
) -> AppResult<Option<artifacts::ArtifactRecord>> {
    let receipt = match artifacts::by_id_from(conn, &origin.artifact_id) {
        // A malformed historical receipt is not evidence of its metadata.
        Err(AppError::Db(rusqlite::Error::FromSqlConversionFailure(..))) => None,
        result => result?,
    };
    Ok(receipt.filter(|r| {
        r.origin().as_ref() == Some(origin) && hash.is_none_or(|h| h == r.artifact.sha256)
    }))
}

pub(crate) fn recover_archive_metadata(
    conn: &Connection,
    entry: &mut mod_library::LibraryEntry,
) -> AppResult<()> {
    let mut version = display_version(&entry.version).map(str::to_owned);
    if let (Some(project), Some(file)) = (&entry.project_id, &entry.version_id) {
        if version.is_none() || entry.project_page.is_none() {
            let origin = artifacts::DownloadOrigin {
                artifact_id: entry.id.clone(),
                source_id: entry.source_id.clone(),
                project_id: project.clone(),
                version_id: file.clone(),
            };
            if let Some(receipt) = historical_receipt(conn, &origin, Some(&entry.sha256))? {
                version = version.or_else(|| display_version(&receipt.version).map(str::to_owned));
                if entry.project_page.is_none() {
                    entry.project_page = receipt.project_page;
                }
            }
        }
        version = version.or_else(|| provider_version(&entry.source_id, file));
    }
    if entry.source_id == "local"
        && entry.id.starts_with("received-")
        && (version.is_none() || entry.project_page.is_none())
    {
        let (label, page) = crate::browse::install::stock_archive_metadata(conn, entry)?;
        version = version.or(label);
        if entry.project_page.is_none() {
            entry.project_page = page;
        }
    }
    if entry.project_page.is_none() {
        entry.project_page = entry
            .project_id
            .as_deref()
            .and_then(|id| crate::sources::known_project_page(&entry.source_id, id));
    }
    entry.version = version
        .or_else(|| filename_version(&entry.filename))
        .unwrap_or_default();
    Ok(())
}

pub(crate) fn filename_version(filename: &str) -> Option<String> {
    static EXPLICIT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)(?:^|[-_ .])v([0-9]+(?:\.[0-9]+){1,5})\.(?:zip|7z|rar|jar|mrpack|tar\.gz|tgz)$",
        )
        .expect("valid explicit archive version pattern")
    });
    if let Some(captures) = EXPLICIT.captures(filename) {
        return display_version(&captures[1]).map(str::to_owned);
    }

    static PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)^.+?-([0-9]+)-([0-9]+(?:[-.][0-9]+){1,5})-([0-9]{10})\.(?:zip|7z|rar)$",
        )
        .expect("valid Nexus filename pattern")
    });
    let captures = PATTERN.captures(filename)?;
    let timestamp = captures[3].parse::<u64>().ok()?;
    if captures[1].parse::<u64>().ok()? == 0 || !(1_000_000_000..3_000_000_000).contains(&timestamp)
    {
        return None;
    }
    let version = captures[2].replace('-', ".");
    display_version(&version)?;
    Some(version)
}

fn display_name(
    conn: &Connection,
    game_id: &str,
    kind: &str,
    id: &str,
) -> AppResult<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT name FROM mod_display_names WHERE game_id=?1 AND kind=?2 AND item_id=?3",
            params![game_id, kind, id],
            |row| row.get(0),
        )
        .optional()?)
}

fn archive_tags(conn: &Connection, game_id: &str, id: &str) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT tag_id FROM archive_mod_tags WHERE game_id=?1 AND archive_id=?2 ORDER BY tag_id",
    )?;
    let ids = stmt.query_map(params![game_id, id], |row| row.get(0))?;
    Ok(ids.collect::<Result<_, _>>()?)
}

fn install_row(conn: &Connection, game_id: &str, id: &str) -> AppResult<(Option<String>, Value)> {
    let state = install::effective_install_state();
    let (root, raw, _, _): (Option<String>, String, String, bool) = conn
        .query_row(
            &format!(
                "SELECT install_root,metadata_json,({state}),EXISTS(
          SELECT 1 FROM removed_mod_items WHERE game_id=?1 AND kind='install' AND item_id=?2
        ) FROM installs WHERE game_id=?1 AND id=?2"
            ),
            params![game_id, id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get::<_, String>(2)?,
                    row.get(3)?,
                ))
            },
        )
        .optional()?
        .filter(|(_, _, status, removed)| mod_management::install_is_visible(status, *removed))
        .ok_or_else(|| AppError::NotFound(id.into()))?;
    Ok((root, serde_json::from_str(&raw)?))
}

pub fn update(engine: &FileEngine, game_id: &str, target: &Target, patch: Patch) -> AppResult<()> {
    let name = patch
        .name
        .as_deref()
        .map(|name| {
            display_text(name, 200)
                .ok_or_else(|| AppError::Invalid("模组名称须为 1–200 个字符".into()))
        })
        .transpose()?;
    let _guard = transaction::write_lock();
    let mut conn = engine.db.conn();
    let tx = conn.transaction()?;
    games::get_game_from(&tx, game_id)?;
    match target {
        Target::Archive(id) => {
            mod_library::visible_entry(&tx, game_id, id)?;
        }
        Target::Install(id) => {
            install_row(&tx, game_id, id)?;
        }
    }
    let (kind, id) = target.parts();
    if let Some(name) = name {
        tx.execute(
            "INSERT INTO mod_display_names(game_id,kind,item_id,name) VALUES (?1,?2,?3,?4)
             ON CONFLICT(game_id,kind,item_id) DO UPDATE SET name=excluded.name,archive_seeded=0",
            params![game_id, kind, id, name],
        )?;
    }
    if let Some(tag_ids) = patch.tag_ids {
        match target {
            Target::Install(_) => {
                crate::mod_organization::set_install_tags(&tx, game_id, id, &tag_ids)?
            }
            Target::Archive(_) => {
                tx.execute(
                    "INSERT OR IGNORE INTO archive_mod_tag_sets(game_id,archive_id) VALUES (?1,?2)",
                    params![game_id, id],
                )?;
                tx.execute(
                    "DELETE FROM archive_mod_tags WHERE game_id=?1 AND archive_id=?2",
                    params![game_id, id],
                )?;
                for tag in tag_ids.iter().collect::<BTreeSet<_>>() {
                    if tx.execute("INSERT INTO archive_mod_tags(game_id,archive_id,tag_id) SELECT ?1,?2,id FROM mod_tags WHERE game_id=?1 AND id=?3",
                        params![game_id,id,tag])? == 0 {
                        return Err(AppError::NotFound(tag.clone()));
                    }
                }
            }
        }
    }
    Ok(tx.commit()?)
}

pub(crate) fn inherit_archive_edit(
    conn: &Connection,
    game_id: &str,
    archive_id: &str,
    install_id: &str,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO mod_display_names(game_id,kind,item_id,name,archive_seeded)
         SELECT game_id,'install',?3,name,1 FROM mod_display_names WHERE game_id=?1 AND kind='archive' AND item_id=?2",
        params![game_id, archive_id, install_id],
    )?;
    let has_tag_edit: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM archive_mod_tag_sets WHERE game_id=?1 AND archive_id=?2)",
        params![game_id, archive_id],
        |row| row.get(0),
    )?;
    if has_tag_edit {
        crate::mod_organization::set_install_tags(
            conn,
            game_id,
            install_id,
            &archive_tags(conn, game_id, archive_id)?,
        )?;
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveDisplay {
    #[serde(flatten)]
    entry: mod_library::LibraryEntry,
    tag_ids: Vec<String>,
    website: Option<String>,
}

pub(crate) fn archive_display(
    db: &Db,
    game_id: &str,
    entries: Vec<mod_library::LibraryEntry>,
) -> AppResult<Vec<ArchiveDisplay>> {
    let conn = db.conn();
    entries
        .into_iter()
        .map(|mut entry| {
            if let Some(name) = display_name(&conn, game_id, "archive", &entry.id)? {
                entry.name = name;
            }
            let tag_ids = archive_tags(&conn, game_id, &entry.id)?;
            let website = entry.project_page.clone();
            Ok(ArchiveDisplay {
                entry,
                tag_ids,
                website,
            })
        })
        .collect()
}

fn archived_artifact_id<'a>(
    metadata: &'a Value,
    origin: Option<&'a artifacts::DownloadOrigin>,
) -> Option<&'a str> {
    metadata["libraryEntryId"]
        .as_str()
        .or_else(|| origin.map(|origin| origin.artifact_id.as_str()))
}

// Recover display data from the exact historical receipt, without matching by project or name alone.
pub(crate) fn recorded_fields(
    conn: &Connection,
    game_id: &str,
    metadata: &Value,
    origin: Option<&artifacts::DownloadOrigin>,
    stored_version: Option<String>,
    original_name: &str,
) -> AppResult<(Option<String>, Option<String>)> {
    let mut version = stored_version
        .as_deref()
        .and_then(display_version)
        .map(str::to_owned);
    let mut website = metadata["projectPage"]
        .as_str()
        .and_then(artifacts::public_project_page);
    let hash = metadata["artifactSha256"].as_str();
    let mut archive_filename = None;
    if version.is_none() || website.is_none() {
        if let Some(id) = archived_artifact_id(metadata, origin) {
            let game_key = match games::get_game_from(conn, game_id) {
                Ok(game) => Some(mod_library::game_key(&game.name)),
                Err(AppError::NotFound(_)) => None,
                Err(error) => return Err(error),
            };
            let archive = conn.query_row(
                &format!("SELECT {} FROM mod_library_entries WHERE id=?1 AND state='ready'
                 AND (game_id=?2 OR (game_id IS NULL AND game_name_key=?7)) AND (?3 IS NULL OR sha256=?3)
                 AND (?4 IS NULL OR (source_id=?4 AND project_id=?5 AND version_id=?6))", mod_library::COLUMNS),
                params![id, game_id, hash, origin.map(|o| &o.source_id), origin.map(|o| &o.project_id), origin.map(|o| &o.version_id), game_key],
                mod_library::read_entry,
            ).optional()?;
            if let Some(mut archive) = archive {
                recover_archive_metadata(conn, &mut archive)?;
                archive_filename = Some(archive.filename);
                version = version.or_else(|| display_version(&archive.version).map(str::to_owned));
                website = website.or(archive.project_page);
            }
        }
    }
    if version.is_none() || website.is_none() {
        if let Some(origin) = origin {
            if let Some(receipt) = historical_receipt(conn, origin, hash)? {
                version = version.or_else(|| display_version(&receipt.version).map(str::to_owned));
                website = website.or(receipt.project_page);
                archive_filename = archive_filename.or(Some(receipt.artifact.filename));
            }
            version = version.or_else(|| provider_version(&origin.source_id, &origin.version_id));
            website = website.or_else(|| {
                crate::sources::known_project_page(&origin.source_id, &origin.project_id)
            });
        }
    }
    if version.is_none() {
        version = [
            metadata["archiveFilename"].as_str(),
            archive_filename.as_deref(),
            metadata["filename"].as_str(),
            metadata["artifactPath"]
                .as_str()
                .and_then(|path| path.rsplit(['/', '\\']).next()),
            Some(original_name),
        ]
        .into_iter()
        .flatten()
        .find_map(filename_version);
    }
    Ok((version, website))
}

// Saving an update to the library also creates a new archive row. Only an
// exact provider/project match can carry a name forward to that archive.
pub(crate) fn inherit_archive_name(
    conn: &Connection,
    entry: &mod_library::LibraryEntry,
    game_name_key: &str,
) -> AppResult<()> {
    let (Some(game_id), Some(project_id), Some(_)) =
        (&entry.game_id, &entry.project_id, &entry.version_id)
    else {
        return Ok(());
    };
    conn.execute("INSERT OR IGNORE INTO mod_display_names(game_id,kind,item_id,name)
      SELECT ?1,'archive',?2,n.name FROM mod_library_entries p
      JOIN mod_display_names n ON n.game_id=?1 AND n.kind='archive' AND n.item_id=p.id
      WHERE p.id<>?2 AND p.state='ready' AND p.source_id=?3 AND p.project_id=?4
        AND (p.game_id=?1 OR (p.game_id IS NULL AND p.game_name_key=?5))
        AND NOT EXISTS (SELECT 1 FROM removed_mod_items h WHERE h.game_id=?1 AND h.kind='archive' AND h.item_id=p.id)
      ORDER BY p.created_at DESC,p.rowid DESC LIMIT 1",
      params![game_id,entry.id,entry.source_id,project_id,game_name_key])?;
    Ok(())
}

// A name written directly to the pending install wins. Otherwise prefer the
// reviewed predecessor, then an incoming archive name, then exact identity.
pub(crate) fn inherit_install_name(
    conn: &Connection,
    game_id: &str,
    install_id: &str,
    transaction_id: &str,
    metadata: &Value,
) -> AppResult<()> {
    let explicit: Option<String> = conn
        .query_row(
            "SELECT n.name FROM install_replacements r JOIN mod_display_names n ON n.game_id=?1
         AND n.kind='install' AND n.item_id=r.replaced_install_id
         WHERE r.transaction_id=?2 AND r.replacement_install_id=?3",
            params![game_id, transaction_id, install_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(name) = explicit {
        conn.execute(
            "INSERT INTO mod_display_names(game_id,kind,item_id,name,archive_seeded) VALUES (?1,'install',?2,?3,0)
             ON CONFLICT(game_id,kind,item_id) DO UPDATE SET name=excluded.name,archive_seeded=0
             WHERE mod_display_names.archive_seeded=1",
            params![game_id,install_id,name],
        )?;
        return Ok(());
    }
    let origin = install::recorded_download_origin(metadata);
    let project = origin
        .as_ref()
        .filter(|origin| !origin.source_id.is_empty() && !origin.project_id.is_empty());
    conn.execute("INSERT OR IGNORE INTO mod_display_names(game_id,kind,item_id,name)
      SELECT ?1,'install',?2,n.name FROM installs p
      JOIN mod_display_names n ON n.game_id=p.game_id AND n.kind='install' AND n.item_id=p.id
      LEFT JOIN install_transactions a ON a.install_id=p.id AND a.action='install'
      LEFT JOIN file_transactions f ON f.id=a.transaction_id
      WHERE p.game_id=?1 AND p.id<>?2
        AND (f.status='committed' OR (a.install_id IS NULL AND p.status NOT IN ('planned','rolled_back')))
        AND (json_extract(CASE WHEN json_valid(p.metadata_json) THEN p.metadata_json END,'$.libraryEntryId')=?3
         OR (json_extract(CASE WHEN json_valid(p.metadata_json) THEN p.metadata_json END,'$.downloadOrigin.sourceId')=?4
         AND json_extract(CASE WHEN json_valid(p.metadata_json) THEN p.metadata_json END,'$.downloadOrigin.projectId')=?5))
      ORDER BY coalesce(f.finished_at,p.installed_at) DESC,p.rowid DESC LIMIT 1",
      params![game_id,install_id,metadata["libraryEntryId"].as_str(),project.map(|o| &o.source_id),project.map(|o| &o.project_id)])?;
    Ok(())
}

fn archived_directory(
    engine: &FileEngine,
    entry: &mod_library::LibraryEntry,
) -> AppResult<PathBuf> {
    let directory = mod_library::storage_directory(engine, entry)?;
    engine.paths.stable_directory(&directory)?;
    Ok(directory)
}

pub(crate) fn resolve_storage_directory(
    engine: &FileEngine,
    game_id: &str,
    target: &Target,
) -> AppResult<PathBuf> {
    let (root, metadata, archive) = {
        let conn = engine.db.conn();
        games::get_game_from(&conn, game_id)?;
        match target {
            Target::Archive(id) => {
                return archived_directory(engine, &mod_library::visible_entry(&conn, game_id, id)?)
            }
            Target::Install(id) => {
                let (root, metadata) = install_row(&conn, game_id, id)?;
                let origin = install::recorded_download_origin(&metadata);
                let archive = archived_artifact_id(&metadata, origin.as_ref())
                    .map(|id| match mod_library::visible_entry(&conn, game_id, id) {
                        Ok(entry) => Ok(Some(entry)),
                        Err(AppError::NotFound(_)) => Ok(None),
                        Err(error) => Err(error),
                    })
                    .transpose()?
                    .flatten();
                (root, metadata, archive)
            }
        }
    };
    if let Some(archive) = archive {
        match archived_directory(engine, &archive) {
            Ok(directory) => return Ok(directory),
            Err(AppError::Io(error)) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    if let Some(directory) = legacy_archive_directory(engine, &metadata)? {
        return Ok(directory);
    }
    let root = root
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| AppError::NotFound("模组安装目录".into()))?;
    let game = games::get_game(&engine.db, game_id)?;
    let mut scopes: Vec<_> = game_roots::registered_user_roots(&engine.db, game_id)?
        .into_values()
        .collect();
    scopes.push(PathBuf::from(game.root_path));
    if engine.paths.overlaps_storage(&root)
        || !scopes.iter().any(|scope| root.starts_with(scope))
        || engine.paths.guard(&root, &scopes)? != root
    {
        return Err(AppError::Denied("安装目录不在该游戏的登记范围内".into()));
    }
    engine.paths.stable_directory(&root)?;
    Ok(root)
}

fn legacy_archive_directory(engine: &FileEngine, metadata: &Value) -> AppResult<Option<PathBuf>> {
    let Some(recorded) = metadata["artifactPath"].as_str().map(std::path::Path::new) else {
        return Ok(None);
    };
    if ![engine.paths.downloads(), engine.paths.staging()]
        .iter()
        .any(|base| crate::paths::user_path(recorded).starts_with(crate::paths::user_path(base)))
    {
        return Ok(None);
    }
    let path = crate::paths::canonicalize_lenient(recorded)?;
    // The stored spelling must remain at the same managed location.
    if crate::paths::user_path(recorded) != crate::paths::user_path(&path)
        || engine.paths.guard(&path, &[])? != path
    {
        return Err(AppError::Denied("原包路径已被重定向".into()));
    }
    let Some(directory) = path.parent() else {
        return Ok(None);
    };
    match std::fs::symlink_metadata(&path) {
        Ok(file) if file.file_type().is_file() => {
            engine.paths.stable_directory(directory)?;
            Ok(Some(directory.to_owned()))
        }
        Ok(_) => Err(AppError::Denied("原包不是普通文件".into())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
#[path = "mod_metadata_tests.rs"]
mod tests;
