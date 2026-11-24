use crate::{
    db::Db,
    error::{AppError, AppResult},
    mod_organization::{clean, insert_automatic_tags, local_tags, OrganizationSuggestion},
    mod_organization_fallback::display_name as fallback_display_name,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path};

#[derive(Debug)]
pub(crate) struct Job {
    pub install_id: String,
    pub game_id: String,
    pub locale: String,
    pub evidence: Value,
    attempts: i64,
}

fn eligible() -> String {
    format!(
        "({}) IN ('enabled','disabled','installed','partially_restored')
         AND NOT EXISTS(SELECT 1 FROM install_missing_removals m WHERE m.install_id=installs.id)
         AND NOT EXISTS(SELECT 1 FROM removed_mod_items m WHERE m.game_id=installs.game_id
                        AND m.kind='install' AND m.item_id=installs.id)",
        crate::install::effective_install_state()
            .replace("ELSE status END", "ELSE installs.status END")
    )
}

pub(crate) fn initialize(db: &Db) -> AppResult<()> {
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    let locale = crate::app_locale::effective_conn(&tx);
    let ids = {
        let mut stmt = tx.prepare(&format!("SELECT installs.id FROM installs JOIN games g ON g.id=installs.game_id WHERE {} AND NOT EXISTS(SELECT 1 FROM mod_organization_jobs j WHERE j.install_id=installs.id) ORDER BY installs.rowid", eligible()))?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for id in ids {
        let row: Option<(String,String,String,Option<String>,bool,String)> = tx.query_row("SELECT i.name,i.metadata_json,i.files_json,o.display_name,coalesce(o.tags_edited,0),(SELECT json_group_array(t.name) FROM install_mod_tags l JOIN mod_tags t ON t.id=l.tag_id WHERE l.install_id=i.id AND t.game_id=i.game_id) FROM installs i LEFT JOIN install_organizations o ON o.install_id=i.id WHERE i.id=?1", [&id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
        let Some((original, metadata, files, display, edited, tags)) = row else {
            continue;
        };
        let metadata = parse(&metadata);
        let files = parse(&files);
        let tags: Vec<String> = serde_json::from_str(&tags).unwrap_or_default();
        let previous_locale = metadata["organization"]["locale"]
            .as_str()
            .unwrap_or(&locale);
        let display_is_trusted = display.as_deref().is_some_and(|name| {
            meaningful_title(name)
                && (Some(name) != fallback_display_name(&original, previous_locale).as_deref()
                    || (name != original
                        && metadata["organization"]["displayName"].as_str() == Some(name)))
        });
        let fallback = local_tags(
            previous_locale,
            display.as_deref().unwrap_or(&original),
            metadata["modType"].as_str().unwrap_or(""),
            array(&files).filter_map(|f| f["path"].as_str()),
        );
        if !(display_is_trusted
            && (edited || tags.iter().any(|tag| !fallback.contains(&tag.as_str()))))
        {
            tx.execute(
                "INSERT INTO mod_organization_jobs(install_id,locale) VALUES (?1,?2)",
                params![id, locale],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn parse(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or(Value::Null)
}

fn array(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

// Only project bounded values from known columns. Paths are handled separately.
fn safe_text(value: &str, max: usize) -> Option<&str> {
    let value = clean(value, max)?;
    let lower = value.to_ascii_lowercase();
    (!value.contains(['/', '\\', '@', '?', '='])
        && !value.contains("://")
        && ![
            "bearer ",
            "password:",
            "token:",
            "api key:",
            "sk-",
            "ghp_",
            "github_pat_",
            "akia",
            "xoxb-",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        && !lower.ends_with(':'))
    .then_some(value)
}

fn basename(value: &str) -> Option<&str> {
    if value.contains("://") || value.ends_with(['/', '\\']) {
        return None;
    }
    safe_text(value.rsplit(['/', '\\']).next()?, 200)
}

fn relative_paths(files: &Value, root: Option<&str>) -> Vec<String> {
    let root = root.map(|value| value.replace('\\', "/"));
    let mut paths = BTreeSet::new();
    for file in array(files).take(256) {
        let Some(raw) = file["path"].as_str() else {
            continue;
        };
        let raw = raw.replace('\\', "/");
        let path = Path::new(&raw);
        let relative = root.as_ref().and_then(|root| path.strip_prefix(root).ok());
        let path = relative.unwrap_or(path);
        let safe_relative = !path.as_os_str().is_empty()
            && path.components().all(|c| {
                if let std::path::Component::Normal(name) = c {
                    name.to_str().is_some_and(|name| {
                        safe_text(name, 160).is_some()
                            && !name.contains(':')
                            && !matches!(
                                name.to_ascii_lowercase().as_str(),
                                "private"
                                    | ".ssh"
                                    | ".aws"
                                    | "credentials"
                                    | "credentials.json"
                                    | "secrets.json"
                            )
                            && !name.to_ascii_lowercase().starts_with(".env")
                    })
                } else {
                    false
                }
            });
        if safe_relative && !raw.split('/').any(|p| matches!(p, "." | "..")) {
            if let Some(path) = path.to_str().filter(|path| path.len() <= 240) {
                paths.insert(path.replace('\\', "/"));
            }
        }
        if paths.len() == 24 {
            break;
        }
    }
    paths.into_iter().collect()
}

pub(crate) fn meaningful_title(value: &str) -> bool {
    let Some(value) = safe_text(value, 200) else {
        return false;
    };
    let lower = value.to_lowercase();
    if [
        ".zip", ".rar", ".7z", ".tar", ".gz", ".bz2", ".xz", ".pak", ".jar", ".mcpack", ".mod",
        ".001",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
    {
        return false;
    }
    let parts: Vec<_> = lower
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect();
    // Downloads often combine generic packaging words with IDs, digests or versions.
    parts.iter().any(|part| {
        ![
            "mod", "mods", "archive", "package", "pack", "file", "download", "unnamed", "unknown",
            "untitled", "id", "v", "version", "zip", "rar", "7z",
        ]
        .contains(part)
            && !(part.len() >= 6 && part.bytes().all(|byte| byte.is_ascii_hexdigit()))
            && !(part.len() >= 24
                && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
                && part.bytes().any(|byte| byte.is_ascii_digit()))
            && part
                .trim_start_matches('v')
                .chars()
                .any(char::is_alphabetic)
            && !["mod", "download", "file", "pack", "id", "version", "v"]
                .iter()
                .any(|prefix| {
                    part.strip_prefix(prefix).is_some_and(|tail| {
                        !tail.is_empty() && tail.bytes().all(|byte| byte.is_ascii_digit())
                    })
                })
    }) && uuid::Uuid::parse_str(value).is_err()
}

pub(crate) fn next(db: &Db) -> AppResult<Option<Job>> {
    let conn = db.conn();
    let base = conn.query_row(&format!(
        "SELECT installs.id,installs.game_id,j.locale,j.attempts,g.name,installs.name,installs.source,
         installs.install_root,installs.files_json,installs.metadata_json,o.display_name
         FROM mod_organization_jobs j JOIN installs ON installs.id=j.install_id
         JOIN games g ON g.id=installs.game_id LEFT JOIN install_organizations o ON o.install_id=installs.id
         WHERE j.status='pending' AND j.ready_at<=unixepoch() AND {}
         ORDER BY j.ready_at,j.rowid LIMIT 1", eligible()), [], |r| {
            Ok((Job { install_id:r.get(0)?,game_id:r.get(1)?,locale:r.get(2)?,attempts:r.get(3)?,evidence:Value::Null },
               r.get::<_, String>(4)?,r.get::<_, String>(5)?,r.get::<_, String>(6)?,r.get::<_, Option<String>>(7)?,
               r.get::<_, String>(8)?,r.get::<_, String>(9)?,r.get::<_, Option<String>>(10)?))
         }).optional()?;
    let Some((mut job, game, original, source, root, files, metadata, display)) = base else {
        return Ok(None);
    };
    let metadata = parse(&metadata);
    let files = relative_paths(&parse(&files), root.as_deref());
    let origin = &metadata["downloadOrigin"];
    let digest = metadata["artifactSha256"].as_str();
    let library: Option<(String, String, String)> = conn.query_row(
        "SELECT name,filename,source_id FROM mod_library_entries WHERE id=?1 AND sha256=?2 AND state='ready'",
        params![metadata["libraryEntryId"].as_str(),digest], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let receipt: Option<(String, String, String)> = conn.query_row(
        "SELECT name,filename,source_id FROM download_artifacts
         WHERE ((id=?1 AND source_id=?2 AND project_id=?3 AND version_id=?4)
           OR (source_id='web' AND path=?5))
           AND json_extract(CASE WHEN json_valid(artifact_json) THEN artifact_json END,'$.sha256')=?6
         ORDER BY id LIMIT 1",
        params![origin["artifactId"].as_str(),origin["sourceId"].as_str(),origin["projectId"].as_str(),origin["versionId"].as_str(),metadata["artifactPath"].as_str(),digest],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let mut tags = conn.prepare("SELECT t.name FROM install_mod_tags l JOIN mod_tags t ON t.id=l.tag_id WHERE l.install_id=?1 AND t.game_id=?2 ORDER BY t.name_key LIMIT 16")?;
    let tags = tags
        .query_map(params![job.install_id, job.game_id], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut existing =
        conn.prepare("SELECT name FROM mod_tags WHERE game_id=?1 ORDER BY name_key LIMIT 40")?;
    let existing = existing
        .query_map([&job.game_id], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let project = |row: Option<&(String, String, String)>| {
        row.map(|(title,filename,provider)| json!({
        "title":safe_text(title,200).filter(|title| meaningful_title(title)), "fileName":basename(filename), "provider":safe_text(provider,48)
    }))
    };
    let staged = &metadata["organization"];
    let suggestion = staged["displayName"]
        .as_str()
        .filter(|_| staged["locale"].as_str() == Some(&job.locale))
        .and_then(|name| safe_text(name, 200))
        .filter(|name| meaningful_title(name));
    job.evidence = json!({
        "gameName":safe_text(&game,200), "originalTitle":safe_text(&original,200),
        "currentDisplayName":display.as_deref().and_then(|s| safe_text(s,200)).filter(|s| meaningful_title(s)),
        "suggestedDisplayName":suggestion,
        "fileName":library.as_ref().and_then(|row| basename(&row.1)).or_else(|| receipt.as_ref().and_then(|row| basename(&row.1))).or_else(|| metadata["artifactPath"].as_str().and_then(basename)),
        "relativeFilePaths":files, "assignedTags":tags.iter().filter_map(|tag| safe_text(tag,48)).collect::<Vec<_>>(), "existingTags":existing.iter().filter_map(|tag| safe_text(tag,48)).collect::<Vec<_>>(),
        "source":safe_text(&source,48), "modType":metadata["modType"].as_str().and_then(|s| safe_text(s,48)),
        "libraryEntry":project(library.as_ref()), "downloadReceipt":project(receipt.as_ref())
    });
    Ok(Some(job))
}

pub(crate) fn enabled(conn: &Connection) -> AppResult<bool> {
    let has_preferences: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='system_skill_preferences' AND type='table')", [], |r| r.get(0))?;
    if !has_preferences {
        return Ok(true);
    }
    Ok(conn
        .query_row(
            "SELECT enabled FROM system_skill_preferences WHERE name=?1",
            [crate::skills::MOD_ORGANIZER_SKILL_NAME],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(true))
}

fn can_settle(conn: &Connection, job: &Job) -> AppResult<bool> {
    if !enabled(conn)? {
        return Ok(false);
    }
    Ok(conn.query_row(&format!("SELECT EXISTS(SELECT 1 FROM installs JOIN games g ON g.id=installs.game_id
         JOIN mod_organization_jobs j ON j.install_id=installs.id WHERE installs.id=?1 AND installs.game_id=?2
         AND j.status='pending' AND j.locale=?3 AND j.attempts=?4 AND j.ready_at<=unixepoch() AND {})",eligible()),
        params![job.install_id,job.game_id,job.locale,job.attempts], |r| r.get(0))?)
}

pub(crate) fn semantic_name(value: &str) -> bool {
    let lower = value.to_lowercase();
    let descriptive = lower
        .split(['_', '-', ' ', '.'])
        .filter(|part| {
            !matches!(
                *part,
                "zzz"
                    | "zz"
                    | "z"
                    | "p"
                    | "data"
                    | "config"
                    | "configs"
                    | "manifest"
                    | "main"
                    | "index"
                    | "init"
                    | "core"
                    | "default"
                    | "settings"
                    | "readme"
                    | "license"
                    | "content"
                    | "contents"
                    | "asset"
                    | "assets"
                    | "binary"
                    | "bin"
                    | "mods"
                    | "mod"
                    | "plugins"
                    | "plugin"
                    | "scripts"
                    | "script"
                    | "textures"
                    | "sounds"
                    | "audio"
                    | "localization"
                    | "resources"
                    | "resource"
                    | "pak"
                    | "paks"
                    | "ue4ss"
                    | "win64"
                    | "binaries"
                    | "common"
                    | "game"
                    | "logicmods"
                    | "enabled"
                    | "disabled"
                    | "info"
                    | "metadata"
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    meaningful_title(&descriptive)
}

fn semantic_filename(path: &str) -> bool {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(semantic_name)
}

fn grounded_title(job: &Job) -> bool {
    let evidence = &job.evidence;
    let is_title_evidence = |value: &Value| value.as_str().is_some_and(semantic_name);
    // A meaningful artifact or deployed component can support a translation.
    // Generic packaging, deployment directories, and IDs carry no title evidence.
    [
        &evidence["libraryEntry"]["title"],
        &evidence["downloadReceipt"]["title"],
        &evidence["suggestedDisplayName"],
        &evidence["originalTitle"],
        &evidence["currentDisplayName"],
    ]
    .into_iter()
    .any(is_title_evidence)
        || std::iter::once(&evidence["fileName"])
            .chain(array(&evidence["relativeFilePaths"]))
            .filter_map(Value::as_str)
            .any(|path| {
                let mut components = path.rsplit('/');
                components.next().is_some_and(semantic_filename) || components.any(semantic_name)
            })
}

pub(crate) fn complete(db: &Db, job: &Job, suggestion: &OrganizationSuggestion) -> AppResult<bool> {
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    if !can_settle(&tx, job)? {
        return Ok(false);
    }
    let title = suggestion.display_name.trim();
    if !meaningful_title(title)
        || !semantic_name(title)
        || !grounded_title(job)
        || suggestion.tags.is_empty()
        || suggestion.tags.len() > 3
    {
        return Err(AppError::Invalid("整理结果缺少可信名称或有效标签".into()));
    }
    let mut tags = Vec::new();
    let mut tag_keys = BTreeSet::new();
    for tag in &suggestion.tags {
        let tag =
            safe_text(tag, 48).ok_or_else(|| AppError::Invalid("整理结果包含无效标签".into()))?;
        if tag_keys.insert(tag.to_lowercase()) {
            tags.push(tag);
        }
    }
    if tags.is_empty() {
        return Err(AppError::Invalid("整理结果标签数量须为 1–3 个".into()));
    }
    let other = crate::mod_organization_fallback::tag_name("Other", &job.locale).to_lowercase();
    if tags.len() == 1 && tags[0].to_lowercase() == other {
        return Err(AppError::Invalid("仅有其他分类时缺少有效整理信息".into()));
    }
    tx.execute(
        "INSERT INTO install_organizations(install_id,display_name) VALUES (?1,?2)
         ON CONFLICT(install_id) DO UPDATE SET display_name=excluded.display_name",
        params![job.install_id, title],
    )?;
    let edited: bool = tx.query_row(
        "SELECT tags_edited FROM install_organizations WHERE install_id=?1",
        [&job.install_id],
        |r| r.get(0),
    )?;
    if !edited {
        tx.execute(
            "DELETE FROM install_mod_tags WHERE install_id=?1",
            [&job.install_id],
        )?;
        insert_automatic_tags(&tx, &job.install_id, &job.game_id, tags)?;
    }
    tx.execute(
        "UPDATE mod_organization_jobs SET status='done' WHERE install_id=?1",
        [&job.install_id],
    )?;
    tx.commit()?;
    Ok(true)
}

pub(crate) fn insufficient(db: &Db, job: &Job) -> AppResult<()> {
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    if can_settle(&tx, job)? {
        tx.execute(
            "UPDATE mod_organization_jobs SET status='unavailable' WHERE install_id=?1",
            [&job.install_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn retry(db: &Db, job: &Job) -> AppResult<()> {
    let delay = (15i64 << job.attempts.clamp(0, 8)).min(3600);
    db.conn().execute(
        "UPDATE mod_organization_jobs SET attempts=min(attempts+1,9),ready_at=unixepoch()+?1
        WHERE install_id=?2 AND status='pending' AND attempts=?3 AND ready_at<=unixepoch()",
        params![delay, job.install_id, job.attempts],
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "mod_organization_jobs_tests.rs"]
mod tests;
