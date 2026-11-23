use super::mod_organization_fallback::{display_name as fallback_display_name, tag_name};
use crate::{
    db::Db,
    error::{AppError, AppResult},
    util,
};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

const MAX_TAGS: usize = 16;
const TAG_COLORS: [&str; 8] = [
    "#6366F1", "#15803D", "#B45309", "#BE185D", "#0369A1", "#7E22CE", "#B91C1C", "#0F766E",
];

fn auto_color(key: &str) -> &'static str {
    let hash = key.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u32::from(byte))
    });
    TAG_COLORS[hash as usize % TAG_COLORS.len()]
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationSuggestion {
    pub display_name: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModTag {
    pub id: String,
    pub name: String,
    pub color: String,
}

pub(crate) fn clean(text: &str, max: usize) -> Option<&str> {
    let text = text.trim();
    (!text.is_empty() && text.chars().count() <= max && !text.chars().any(char::is_control))
        .then_some(text)
}

fn require_game(conn: &rusqlite::Connection, game_id: &str) -> AppResult<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM games WHERE id=?1)",
        [game_id],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(AppError::NotFound(game_id.into()));
    }
    Ok(())
}

pub fn list_mod_tags(db: &Db, game_id: &str) -> AppResult<Vec<ModTag>> {
    let conn = db.conn();
    require_game(&conn, game_id)?;
    let mut stmt =
        conn.prepare("SELECT id,name,color FROM mod_tags WHERE game_id=?1 ORDER BY name_key,id")?;
    let rows = stmt.query_map([game_id], |r| {
        Ok(ModTag {
            id: r.get(0)?,
            name: r.get(1)?,
            color: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn save_mod_tag(
    db: &Db,
    game_id: &str,
    id: Option<&str>,
    name: &str,
    color: &str,
) -> AppResult<ModTag> {
    let name =
        clean(name, 48).ok_or_else(|| AppError::Invalid("标签名称须为 1–48 个字符".into()))?;
    let key = name.to_lowercase();
    if color.len() != 7
        || !color.starts_with('#')
        || !color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
    {
        return Err(AppError::Invalid("标签颜色须为 #RRGGBB".into()));
    }
    let color = color.to_ascii_uppercase();
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    require_game(&tx, game_id)?;
    let existing: Option<String> = tx
        .query_row(
            "SELECT id FROM mod_tags WHERE game_id=?1 AND name_key=?2",
            params![game_id, key],
            |r| r.get(0),
        )
        .optional()?;
    if existing
        .as_deref()
        .is_some_and(|existing| Some(existing) != id)
    {
        return Err(AppError::Conflict("该游戏已有同名标签".into()));
    }
    let id = if let Some(id) = id {
        if tx.execute(
            "UPDATE mod_tags SET name=?3,name_key=?4,color=?5 WHERE game_id=?1 AND id=?2",
            params![game_id, id, name, key, color],
        )? == 0
        {
            return Err(AppError::NotFound(id.into()));
        }
        tx.execute(
            "UPDATE install_organizations SET tags_edited=1 WHERE install_id IN
           (SELECT install_id FROM install_mod_tags WHERE tag_id=?1)",
            [id],
        )?;
        id.to_owned()
    } else {
        let id = util::new_id("tag");
        tx.execute(
            "INSERT INTO mod_tags(id,game_id,name,name_key,color) VALUES (?1,?2,?3,?4,?5)",
            params![id, game_id, name, key, color],
        )?;
        id
    };
    tx.commit()?;
    Ok(ModTag {
        id,
        name: name.into(),
        color,
    })
}

pub fn delete_mod_tag(db: &Db, game_id: &str, id: &str) -> AppResult<()> {
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    tx.execute("UPDATE install_organizations SET tags_edited=1 WHERE install_id IN
      (SELECT l.install_id FROM install_mod_tags l JOIN mod_tags t ON t.id=l.tag_id WHERE t.game_id=?1 AND t.id=?2)", params![game_id,id])?;
    if tx.execute(
        "DELETE FROM mod_tags WHERE game_id=?1 AND id=?2",
        params![game_id, id],
    )? == 0
    {
        return Err(AppError::NotFound(id.into()));
    }
    tx.commit()?;
    Ok(())
}

fn require_install(tx: &Transaction<'_>, game_id: &str, install_id: &str) -> AppResult<()> {
    require_game(tx, game_id)?;
    let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM installs WHERE game_id=?1 AND id=?2
      AND (EXISTS(SELECT 1 FROM install_transactions a JOIN file_transactions f ON f.id=a.transaction_id
                  WHERE a.install_id=installs.id AND a.action='install' AND f.status='committed')
           OR (status NOT IN ('planned','rolled_back') AND NOT EXISTS(SELECT 1 FROM install_transactions a WHERE a.install_id=installs.id AND a.action='install'))))",
      params![game_id,install_id], |r| r.get(0))?;
    if !exists {
        return Err(AppError::NotFound(install_id.into()));
    }
    Ok(())
}

pub fn set_mod_tags(db: &Db, game_id: &str, install_id: &str, tag_ids: &[String]) -> AppResult<()> {
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    require_install(&tx, game_id, install_id)?;
    tx.execute(
        "INSERT INTO install_organizations(install_id,tags_edited) VALUES (?1,1)
        ON CONFLICT(install_id) DO UPDATE SET tags_edited=1",
        [install_id],
    )?;
    tx.execute(
        "DELETE FROM install_mod_tags WHERE install_id=?1",
        [install_id],
    )?;
    for id in tag_ids.iter().collect::<BTreeSet<_>>() {
        if tx.execute("INSERT INTO install_mod_tags(install_id,tag_id) SELECT ?1,id FROM mod_tags WHERE game_id=?2 AND id=?3",
            params![install_id,game_id,id])? == 0 {
            return Err(AppError::NotFound(id.clone()));
        }
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn stage(db: &Db, metadata: &mut Value, suggestion: Option<OrganizationSuggestion>) {
    let locale = crate::settings::mod_organization_locale(db);
    let display_name = suggestion
        .as_ref()
        .and_then(|value| clean(&value.display_name, 200));
    let tags: Vec<_> = suggestion
        .iter()
        .flat_map(|value| value.tags.iter())
        .filter_map(|tag| clean(tag, 48))
        .take(MAX_TAGS)
        .collect();
    metadata["organization"] =
        serde_json::json!({"locale":locale,"displayName":display_name.unwrap_or(""),"tags":tags});
}

pub(crate) fn local_tags<'a>(
    locale: &str,
    name: &str,
    mod_type: &str,
    files: impl Iterator<Item = &'a str>,
) -> Vec<&'static str> {
    let description = format!("{name} {mod_type}").to_lowercase();
    let mut tags = Vec::new();
    for (terms, category) in [
        (&["texture", "visual", "材质", "视觉"][..], "Visuals"),
        (
            &["translation", "localization", "翻译", "汉化"][..],
            "Localization",
        ),
        (&["gameplay", "balance", "玩法", "平衡"][..], "Gameplay"),
        (&["audio", "sound", "音乐", "音效"][..], "Audio"),
        (
            &["framework", "dependency", "前置", "框架"][..],
            "Framework",
        ),
    ] {
        if terms.iter().any(|term| description.contains(term)) {
            tags.push(tag_name(category, locale));
        }
    }
    for path in files {
        let tag = match path
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "dds" | "tga" => Some(tag_name("Visuals", locale)),
            "wav" | "ogg" | "wem" => Some(tag_name("Audio", locale)),
            _ => None,
        };
        if let Some(tag) = tag.filter(|tag| !tags.contains(tag)) {
            tags.push(tag);
        }
    }
    if tags.is_empty() {
        tags.push(tag_name("Other", locale));
    }
    tags
}

/// Run within the same SQLite commit that marks the file journal committed.
/// The durable job is the once-only marker, including after replay.
pub fn finalize(tx: &Transaction<'_>, transaction_id: &str) -> AppResult<bool> {
    let install: Option<(String,String,String,String,String,String)> = tx.query_row(
        "SELECT i.id,i.game_id,i.name,i.source,i.metadata_json,i.files_json FROM installs i JOIN install_transactions a ON a.install_id=i.id
         WHERE a.transaction_id=?1 AND a.action='install'
         AND EXISTS(SELECT 1 FROM file_transactions WHERE id=a.transaction_id AND status='committed')
         AND EXISTS(SELECT 1 FROM games WHERE id=i.game_id)
         AND NOT EXISTS(SELECT 1 FROM mod_organization_jobs WHERE install_id=i.id)", [transaction_id],
         |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
    let Some((id, game_id, name, source, metadata, files)) = install else {
        return Ok(false);
    };
    let metadata: Value = serde_json::from_str(&metadata).unwrap_or(Value::Null);
    let stable = |field: &str| metadata[field].as_str().filter(|value| !value.is_empty());
    // An explicit replacement wins; otherwise use catalog/library identity or an
    // exact local artifact digest. Names and paths are never identity keys.
    let prior: Option<(String,Option<String>,bool)> = tx.query_row("SELECT p.id,json_extract(CASE WHEN json_valid(p.metadata_json) THEN p.metadata_json END,'$.organization.locale'),coalesce(o.tags_edited,0)
       FROM installs p LEFT JOIN install_organizations o ON o.install_id=p.id
       LEFT JOIN install_transactions a ON a.install_id=p.id AND a.action='install'
       LEFT JOIN file_transactions f ON f.id=a.transaction_id
       WHERE p.game_id=?1 AND p.id<>?2 AND (f.status='committed' OR (f.status='rolled_back' AND o.install_id IS NOT NULL) OR (a.install_id IS NULL AND p.status NOT IN ('planned','rolled_back')))
         AND (p.id=(SELECT replaced_install_id FROM install_replacements WHERE transaction_id=?3)
           OR json_extract(CASE WHEN json_valid(p.metadata_json) THEN p.metadata_json END,'$.libraryEntryId')=?4
           OR (p.source=?5 AND json_extract(CASE WHEN json_valid(p.metadata_json) THEN p.metadata_json END,'$.sourceModId')=?6)
           OR (p.source=?5 AND (?4 IS NULL OR json_extract(CASE WHEN json_valid(p.metadata_json) THEN p.metadata_json END,'$.libraryEntryId') IS NULL) AND ?6 IS NULL AND json_extract(CASE WHEN json_valid(p.metadata_json) THEN p.metadata_json END,'$.sourceModId') IS NULL AND json_extract(CASE WHEN json_valid(p.metadata_json) THEN p.metadata_json END,'$.artifactSha256')=?7))
       ORDER BY coalesce(p.id=(SELECT replaced_install_id FROM install_replacements WHERE transaction_id=?3),0) DESC,
         coalesce(f.finished_at,p.installed_at) DESC,p.rowid DESC LIMIT 1",
       params![game_id,id,transaction_id,stable("libraryEntryId"),source,stable("sourceModId"),stable("artifactSha256")],
       |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let suggestion = &metadata["organization"];
    let legacy_locale;
    let locale = match suggestion["locale"].as_str() {
        Some(staged) => staged,
        None => {
            legacy_locale = crate::settings::effective_mod_locale(tx);
            &legacy_locale
        }
    };
    tx.execute(
        "INSERT INTO mod_organization_jobs(install_id,locale) VALUES (?1,?2)",
        params![id, locale],
    )?;
    let has_organization: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM install_organizations WHERE install_id=?1)",
        [&id],
        |r| r.get(0),
    )?;
    if has_organization {
        return Ok(true);
    }
    let enabled = crate::mod_organization_jobs::enabled(tx)?;
    let tags_edited = prior.as_ref().is_some_and(|p| p.2);
    let display_name = enabled
        .then(|| {
            suggestion["displayName"]
                .as_str()
                .and_then(|value| clean(value, 200))
                .map(str::to_owned)
                .filter(|value| {
                    crate::mod_organization_jobs::meaningful_title(value)
                        && crate::mod_organization_jobs::semantic_name(value)
                })
                .or_else(|| fallback_display_name(&name, locale))
                .filter(|value| {
                    crate::mod_organization_jobs::meaningful_title(value)
                        && crate::mod_organization_jobs::semantic_name(value)
                })
        })
        .flatten();
    tx.execute(
        "INSERT INTO install_organizations(install_id,display_name,tags_edited) VALUES (?1,?2,?3)",
        params![id, display_name, tags_edited],
    )?;
    let mut tags: Vec<_> = enabled
        .then_some(&suggestion["tags"])
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tag| clean(tag.as_str()?, 48))
        .take(MAX_TAGS)
        .collect();
    if let Some(prior) = prior
        .as_ref()
        .filter(|p| tags_edited || (enabled && tags.is_empty() && p.1.as_deref() == Some(locale)))
    {
        let copied = tx.execute(
            "INSERT INTO install_mod_tags(install_id,tag_id)
            SELECT ?1,l.tag_id FROM install_mod_tags l JOIN mod_tags t ON t.id=l.tag_id
            WHERE l.install_id=?2 AND (?3 OR t.name_key<>?4)",
            params![
                id,
                prior.0,
                tags_edited,
                tag_name("Other", locale).to_lowercase()
            ],
        )?;
        if tags_edited || copied > 0 {
            return Ok(true);
        }
    }
    if !enabled {
        return Ok(true);
    }
    if tags.is_empty() {
        let files: Value = serde_json::from_str(&files).unwrap_or(Value::Null);
        tags = local_tags(
            locale,
            display_name.as_deref().unwrap_or(&name),
            metadata["modType"].as_str().unwrap_or(""),
            files
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|file| file["path"].as_str()),
        );
        tags.retain(|tag| *tag != tag_name("Other", locale));
    }
    insert_automatic_tags(tx, &id, &game_id, tags)?;
    Ok(true)
}

pub(crate) fn insert_automatic_tags<'a>(
    tx: &Transaction<'_>,
    id: &str,
    game_id: &str,
    tags: impl IntoIterator<Item = &'a str>,
) -> AppResult<()> {
    for tag in tags {
        let key = tag.to_lowercase();
        tx.execute("INSERT INTO mod_tags(id,game_id,name,name_key,color) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(game_id,name_key) DO NOTHING",
            params![util::new_id("tag"),game_id,tag,key,auto_color(&key)])?;
        tx.execute("INSERT INTO install_mod_tags(install_id,tag_id) SELECT ?1,id FROM mod_tags WHERE game_id=?2 AND name_key=?3
            ON CONFLICT(install_id,tag_id) DO NOTHING", params![id,game_id,key])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_install_can_be_tagged_without_cross_game_changes() {
        let (_temp, engine, _) = crate::transaction::tests::setup();
        let db = &engine.db;
        db.conn().execute_batch("INSERT INTO games(id,name,slug,root_path,created_at,updated_at) VALUES ('other','Other','other','other','now','now');
            INSERT INTO installs(id,game_id,name,installed_at,updated_at) VALUES ('legacy','g','Legacy','now','now');").unwrap();
        let a = save_mod_tag(db, "g", None, "Same", "#abcdef").unwrap();
        let b = save_mod_tag(db, "other", None, "Same", "#000000").unwrap();
        assert_eq!(a.color, "#ABCDEF");
        assert!(save_mod_tag(db, "g", None, " SAME ", "#001122").is_err());
        assert!(save_mod_tag(db, "g", None, "Invalid", "#00112Z").is_err());
        assert!(save_mod_tag(db, "g", Some(&b.id), "Wrong", "#000000").is_err());
        set_mod_tags(db, "g", "legacy", &[a.id.clone(), a.id.clone()]).unwrap();
        assert!(set_mod_tags(db, "g", "legacy", std::slice::from_ref(&b.id)).is_err());
        let count = || {
            db.conn()
                .query_row(
                    "SELECT count(*) FROM install_mod_tags WHERE install_id='legacy'",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap()
        };
        assert_eq!(count(), 1);
        assert!(delete_mod_tag(db, "g", &b.id).is_err());
        delete_mod_tag(db, "g", &a.id).unwrap();
        assert_eq!(count(), 0);
        assert_eq!(list_mod_tags(db, "other").unwrap()[0].id, b.id);
    }
}
