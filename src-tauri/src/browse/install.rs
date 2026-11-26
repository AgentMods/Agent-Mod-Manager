//! Durable, card-scoped Agent installs. The webview never supplies provider IDs,
//! install arguments or URLs; a browse result capability is resolved natively.
use super::{matching::GameScope, resolve_work, BrowseState, TrustedWork};
use crate::{
    agent::runtime::{AgentRuntime, TurnRequest},
    app::AppServices,
    db::Db,
    error::{AppError, AppResult},
    history::History,
    mod_library, util,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::{borrow::Cow, collections::BTreeSet, sync::Mutex};
use tauri::{Runtime, State, WebviewWindow};

mod scope;
mod stock;
mod three_dm;
pub(crate) use scope::InstallScope;

// The application has one SQLite connection and one desktop Agent. Keep create +
// launch exclusive, but never hold a DB guard while entering AgentRuntime.
static START: Mutex<()> = Mutex::new(());

pub(crate) fn init(db: &Db) -> AppResult<()> {
    let conn = db.conn();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS browse_install_tasks (
           id TEXT PRIMARY KEY,
           conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
           game_id TEXT NOT NULL REFERENCES games(id) ON DELETE CASCADE,
           source_id TEXT NOT NULL,
           project_id TEXT NOT NULL,
           provider_game_id TEXT NOT NULL,
           install_id TEXT,
           transaction_id TEXT,
           artifact_id TEXT,
           UNIQUE(conversation_id)
         );
         CREATE INDEX IF NOT EXISTS idx_browse_install_work
           ON browse_install_tasks(game_id,source_id,project_id,provider_game_id);
         CREATE TABLE IF NOT EXISTS browse_install_downloads (
           task_id TEXT NOT NULL REFERENCES browse_install_tasks(id) ON DELETE CASCADE,
           artifact_id TEXT NOT NULL,
           file_id TEXT NOT NULL,
           sha256 TEXT NOT NULL,
           version TEXT,
           PRIMARY KEY(task_id,artifact_id)
         );
         CREATE INDEX IF NOT EXISTS idx_browse_install_download_archive
           ON browse_install_downloads(artifact_id,sha256);
         CREATE TABLE IF NOT EXISTS browse_install_titles (
           task_id TEXT PRIMARY KEY REFERENCES browse_install_tasks(id) ON DELETE CASCADE,
           title TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS browse_install_project_pages (
           task_id TEXT PRIMARY KEY REFERENCES browse_install_tasks(id) ON DELETE CASCADE,
           project_page TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS browse_install_handles (
           window_label TEXT NOT NULL,
           search_id TEXT NOT NULL,
           item_id TEXT NOT NULL,
           game_id TEXT NOT NULL REFERENCES games(id) ON DELETE CASCADE,
           source_id TEXT NOT NULL,
           project_id TEXT NOT NULL,
           provider_game_id TEXT NOT NULL,
           PRIMARY KEY(window_label,search_id,item_id,game_id)
         );",
    )?;
    let has_version = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('browse_install_downloads') WHERE name='version')",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !has_version {
        conn.execute(
            "ALTER TABLE browse_install_downloads ADD COLUMN version TEXT",
            [],
        )?;
    }
    Ok(())
}

/// Recover the binding created from the original native catalog result for a chat follow-up.
pub(crate) fn conversation_scope(
    conn: &Connection,
    conversation: &str,
) -> AppResult<Option<InstallScope>> {
    let task = conn
        .query_row(
            "SELECT b.id,b.game_id,b.source_id,b.project_id,b.provider_game_id,p.project_page
             FROM browse_install_tasks b LEFT JOIN browse_install_project_pages p ON p.task_id=b.id
             WHERE b.conversation_id=?1",
            [conversation],
            |row| {
                Ok((
                    row.get(0)?,
                    WorkKey {
                        game_id: row.get(1)?,
                        source_id: row.get(2)?,
                        project_id: row.get(3)?,
                        provider_game_id: row.get(4)?,
                        project_page: row.get(5)?,
                    },
                ))
            },
        )
        .optional()?;
    task.map(|(id, key)| {
        let game = crate::games::get_game_from(conn, &key.game_id)?;
        Ok(InstallScope::new(id, key, GameScope::new(game)))
    })
    .transpose()
}

/// A card retry and a chat follow-up can refer to different conversations for the same work.
pub(crate) fn sibling_install_running(conn: &Connection, conversation: &str) -> AppResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM browse_install_tasks b
           JOIN browse_install_tasks other USING (game_id,source_id,project_id,provider_game_id)
           JOIN agent_turns t ON t.conversation_id=other.conversation_id AND t.status='running'
           WHERE b.conversation_id=?1 AND other.id<>b.id)",
        [conversation],
        |row| row.get(0),
    )?)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowseInstallTask {
    id: String,
    conversation_id: String,
    turn_id: Option<String>,
    status: TaskStatus,
    install_id: Option<String>,
    transaction_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum TaskStatus {
    Running,
    Installed,
    Unverified,
    Rejected,
    Cancelled,
    Interrupted,
    Failed,
}

fn has_table(conn: &Connection, name: &str) -> AppResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='table' AND name=?1)",
        [name],
        |row| row.get(0),
    )?)
}

/// Recover display metadata only from a stock archive association. Inbox
/// imports stay local (this is not a DownloadOrigin or update capability).
pub(crate) fn stock_archive_metadata(
    conn: &Connection,
    entry: &mod_library::LibraryEntry,
) -> AppResult<(Option<String>, Option<String>)> {
    let Some(game) = entry.game_id.as_deref() else {
        return Ok((None, None));
    };
    let is_digest =
        |value: &str| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
    if entry.source_id != "local"
        || !entry.id.strip_prefix("received-").is_some_and(is_digest)
        || !is_digest(&entry.sha256)
        || !has_table(conn, "browse_install_downloads")?
        || !has_table(conn, "browse_install_tasks")?
    {
        return Ok((None, None));
    }

    // Browse tables are initialized by AgentRuntime, not the core migrations.
    // Read legacy associations even if the column and page table do not exist.
    let version_column: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('browse_install_downloads','main') WHERE name='version')",
        [],
        |row| row.get(0),
    )?;
    let (page, join) = if has_table(conn, "browse_install_project_pages")? {
        (
            "p.project_page",
            "LEFT JOIN main.browse_install_project_pages p ON p.task_id=b.id",
        )
    } else {
        ("NULL", "")
    };
    let version = if version_column { "d.version" } else { "NULL" };
    let mut stmt = conn.prepare(&format!(
        "SELECT b.source_id,b.project_id,b.provider_game_id,d.file_id,b.conversation_id,{version},{page}
         FROM main.browse_install_downloads d JOIN main.browse_install_tasks b ON b.id=d.task_id {join}
         WHERE b.game_id=?1 AND d.artifact_id=?2 AND d.sha256=?3",
    ))?;
    let mut rows = stmt.query(params![game, entry.id, entry.sha256])?;
    let mut identity = None;
    let mut work = None;
    let mut conversations = BTreeSet::new();
    let mut versions = BTreeSet::new();
    let mut pages = BTreeSet::new();
    while let Some(row) = rows.next()? {
        let key = WorkKey {
            game_id: game.to_owned(),
            source_id: row.get(0)?,
            project_id: row.get(1)?,
            provider_game_id: row.get(2)?,
            project_page: row.get(6)?,
        };
        let file: String = row.get(3)?;
        if !key.valid_stock_identity(&file) {
            return Ok((None, None));
        }
        let candidate = (
            key.source_id.clone(),
            key.origin_project().into_owned(),
            key.provider_game_id.clone(),
            file,
        );
        if identity.as_ref().is_some_and(|id| id != &candidate) {
            return Ok((None, None));
        }
        identity = Some(candidate);
        if let Some(page) = key.project_page() {
            pages.insert(page);
        }
        if let Some(label) = row
            .get::<_, Option<String>>(5)?
            .as_deref()
            .and_then(stock::clean_version)
        {
            versions.insert(label.to_owned());
        }
        conversations.insert(row.get::<_, String>(4)?);
        work = Some(key);
    }
    let (Some(key), Some((_, _, _, file))) = (work, identity) else {
        return Ok((None, None));
    };
    let website = if pages.is_empty() {
        key.legacy_stock_project_page()
    } else {
        unique_value(pages)
    };
    let version = if key.source_id == "thunderstore" {
        Some(file)
    } else if stock::version_list_tool(&key.source_id).is_none() {
        None
    } else {
        if versions.is_empty() && has_table(conn, "tool_calls")? {
            for conversation in conversations {
                match stock::history_file_version(conn, &key, &conversation, &file, None)? {
                    stock::FileVersionEvidence::Exact(label) => {
                        versions.insert(label);
                    }
                    stock::FileVersionEvidence::Unknown => {}
                    stock::FileVersionEvidence::Ambiguous => return Ok((None, website)),
                }
            }
        }
        unique_value(versions)
    };
    Ok((version, website))
}

fn unique_value(values: BTreeSet<String>) -> Option<String> {
    (values.len() == 1)
        .then(|| values.into_iter().next())
        .flatten()
}

#[derive(Clone)]
struct WorkKey {
    game_id: String,
    source_id: String,
    project_id: String,
    provider_game_id: String,
    project_page: Option<String>,
}

impl From<&TrustedWork> for WorkKey {
    fn from(work: &TrustedWork) -> Self {
        Self {
            game_id: work.game_id.clone(),
            source_id: work.source_id.clone(),
            project_id: work.project_id.clone(),
            provider_game_id: work.provider_game_id.clone(),
            project_page: work.project_page.clone(),
        }
    }
}

impl WorkKey {
    fn project_page(&self) -> Option<String> {
        super::links::project(
            &self.source_id,
            &self.provider_game_id,
            self.origin_project().as_ref(),
            self.project_page.as_deref()?,
        )
    }

    fn valid_stock_identity(&self, file: &str) -> bool {
        if self.source_id == "thunderstore" {
            return self.project_id.split_once('/').is_some_and(|(team, name)| {
                [team, name, &self.provider_game_id]
                    .into_iter()
                    .all(|part| {
                        !part.contains(['/', '~', ':']) && super::links::valid_provider_id(part)
                    })
            }) && stock::valid_version(file);
        }
        if stock::download_tool(&self.source_id).is_none() || stock::number(file).is_err() {
            return false;
        }
        let project = self.origin_project();
        if self.source_id == "nexusmods" {
            return project.split_once('/').is_some_and(|(game, id)| {
                game == self.provider_game_id
                    && !game.contains(['/', '~', ':'])
                    && super::links::valid_provider_id(game)
                    && stock::number(id).is_ok()
            });
        }
        stock::number(&project).is_ok() && stock::number(&self.provider_game_id).is_ok()
    }

    fn legacy_stock_project_page(&self) -> Option<String> {
        let project = self.origin_project();
        let page = match self.source_id.as_str() {
            "gamebanana" => format!(
                "https://gamebanana.com/mods/{}",
                stock::number(&project).ok()?
            ),
            "nexusmods" => {
                let (game, id) = project.split_once('/')?;
                format!(
                    "https://www.nexusmods.com/{game}/mods/{}",
                    stock::number(id).ok()?
                )
            }
            "thunderstore" => {
                let (team, name) = project.split_once('/')?;
                format!(
                    "https://thunderstore.io/c/{}/p/{team}/{name}",
                    self.provider_game_id
                )
            }
            "curseforge" => return crate::sources::known_project_page("curseforge", &project),
            // A numeric mod.io ID does not encode its work or game slug.
            _ => return None,
        };
        super::links::project(&self.source_id, &self.provider_game_id, &project, &page)
    }

    fn catalog_id(&self) -> Option<&str> {
        match self.source_id.as_str() {
            "glossmod_3dm" => Some("3dm"),
            "nexusmods" => Some("nexus"),
            "curseforge" | "thunderstore" | "modrinth" => Some(&self.source_id),
            _ => None,
        }
    }

    fn origin_project(&self) -> Cow<'_, str> {
        if self.source_id == "nexusmods" && !self.project_id.contains('/') {
            Cow::Owned(format!("{}/{}", self.provider_game_id, self.project_id))
        } else {
            Cow::Borrowed(&self.project_id)
        }
    }

    fn origin_id(&self) -> Option<&str> {
        if self.source_id == "glossmod_http" {
            Some("glossmod_http")
        } else {
            self.catalog_id()
        }
    }
}

// The candidate is written by the dispatcher immediately before executing its
// approved native plan. A completion event or text is not installation evidence.
fn verified_install(conn: &Connection, task_id: &str) -> AppResult<Option<(String, String)>> {
    Ok(conn.query_row(
        &format!("SELECT i.id, f.id FROM browse_install_tasks b
          JOIN installs i ON i.id=b.install_id AND i.game_id=b.game_id
          JOIN install_transactions it ON it.install_id=i.id AND it.action='install'
          JOIN file_transactions f ON f.id=it.transaction_id AND f.id=b.transaction_id
          WHERE b.id=?1 AND f.status='committed' AND f.kind='library_install' AND f.game_id=b.game_id
            AND ({})='enabled'
            AND ((json_extract(i.metadata_json,'$.downloadOrigin.artifactId')=b.artifact_id
            AND json_extract(i.metadata_json,'$.downloadOrigin.sourceId')=CASE b.source_id
                WHEN 'glossmod_3dm' THEN '3dm' WHEN 'nexusmods' THEN 'nexus' ELSE b.source_id END
            AND json_extract(i.metadata_json,'$.downloadOrigin.projectId')=CASE
                WHEN b.source_id='nexusmods' AND instr(b.project_id,'/')=0 THEN b.provider_game_id || '/' || b.project_id ELSE b.project_id END
            AND EXISTS (SELECT 1 FROM download_artifacts a WHERE a.id=b.artifact_id
                AND a.source_id=json_extract(i.metadata_json,'$.downloadOrigin.sourceId')
                AND a.project_id=json_extract(i.metadata_json,'$.downloadOrigin.projectId') AND a.version_id=json_extract(i.metadata_json,'$.downloadOrigin.versionId')))
             OR (json_extract(i.metadata_json,'$.libraryEntryId')=b.artifact_id
                 AND EXISTS (SELECT 1 FROM browse_install_downloads d
                   JOIN browse_install_tasks original ON original.id=d.task_id
                     AND original.game_id=b.game_id AND original.source_id=b.source_id
                     AND original.project_id=b.project_id AND original.provider_game_id=b.provider_game_id
                   WHERE d.artifact_id=b.artifact_id
                     AND d.sha256=json_extract(i.metadata_json,'$.artifactSha256'))))",
            crate::install::INSTALL_STATE.replace("installs.id", "i.id")),
        [task_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?)
}

fn task_for(conn: &Connection, key: &WorkKey) -> AppResult<Option<BrowseInstallTask>> {
    let row = conn
        .query_row(
            "SELECT b.id,b.conversation_id,t.id,t.status,
           EXISTS (SELECT 1 FROM agent_events e JOIN approvals a ON a.id=json_extract(e.payload_json,'$.approvalId')
             WHERE e.conversation_id=b.conversation_id AND e.turn_id=t.id AND e.kind='approval-request'
               AND a.status='rejected' AND a.title<>'ask_user')
         FROM browse_install_tasks b
         LEFT JOIN agent_turns t ON t.conversation_id=b.conversation_id
         WHERE b.game_id=?1 AND b.source_id=?2 AND b.project_id=?3 AND b.provider_game_id=?4
         ORDER BY (t.status='running') DESC, t.rowid DESC, b.rowid DESC LIMIT 1",
            params![
                key.game_id,
                key.source_id,
                key.project_id,
                key.provider_game_id
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, bool>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((id, conversation_id, turn_id, status, rejected)) = row else {
        return Ok(None);
    };
    let receipt = verified_install(conn, &id)?;
    let status = match (status.as_deref(), receipt.is_some(), rejected) {
        (Some("running"), _, _) => TaskStatus::Running,
        (_, true, _) => TaskStatus::Installed,
        (Some("cancelled"), _, _) => TaskStatus::Cancelled,
        (Some("interrupted") | None, _, _) => TaskStatus::Interrupted,
        (_, _, true) => TaskStatus::Rejected,
        (Some("done"), _, _) => TaskStatus::Unverified,
        _ => TaskStatus::Failed,
    };
    let (install_id, transaction_id) = receipt.map_or((None, None), |(i, t)| (Some(i), Some(t)));
    Ok(Some(BrowseInstallTask {
        id,
        conversation_id,
        turn_id,
        status,
        install_id,
        transaction_id,
    }))
}

fn remember_handle(
    conn: &Connection,
    window: &str,
    search_id: &str,
    item_id: &str,
    key: &WorkKey,
) -> AppResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO browse_install_handles(window_label,search_id,item_id,game_id,source_id,project_id,provider_game_id)
          VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![window, search_id, item_id, key.game_id, key.source_id, key.project_id, key.provider_game_id],
    )?;
    Ok(())
}

fn known_work(
    conn: &Connection,
    window: &str,
    search_id: &str,
    item_id: &str,
    game_id: &str,
) -> AppResult<Option<WorkKey>> {
    Ok(conn
        .query_row(
            "SELECT source_id,project_id,provider_game_id FROM browse_install_handles
         WHERE window_label=?1 AND search_id=?2 AND item_id=?3 AND game_id=?4",
            params![window, search_id, item_id, game_id],
            |row| {
                Ok(WorkKey {
                    game_id: game_id.into(),
                    source_id: row.get(0)?,
                    project_id: row.get(1)?,
                    provider_game_id: row.get(2)?,
                    // The handle identifies an existing task, not its metadata.
                    project_page: None,
                })
            },
        )
        .optional()?)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowseInstallIndexItem {
    search_id: String,
    item_id: String,
    game_id: String,
    title: String,
}

#[tauri::command]
pub(crate) fn list_browse_install_tasks<R: Runtime>(
    window: WebviewWindow<R>,
    services: State<'_, AppServices>,
) -> AppResult<Vec<BrowseInstallIndexItem>> {
    list_tasks(&services.engine.db.conn(), window.label())
}

fn list_tasks(conn: &Connection, window: &str) -> AppResult<Vec<BrowseInstallIndexItem>> {
    let mut statement = conn.prepare(
        "SELECT h.search_id,h.item_id,b.game_id,COALESCE(t.title,b.project_id)
           FROM browse_install_tasks b
           JOIN browse_install_handles h ON h.rowid=(SELECT MAX(h2.rowid) FROM browse_install_handles h2
              WHERE h2.window_label=?1 AND h2.game_id=b.game_id AND h2.source_id=b.source_id
                AND h2.project_id=b.project_id AND h2.provider_game_id=b.provider_game_id)
           LEFT JOIN browse_install_titles t ON t.task_id=b.id
           WHERE b.rowid=(SELECT MAX(b2.rowid) FROM browse_install_tasks b2 WHERE b2.game_id=b.game_id
              AND b2.source_id=b.source_id AND b2.project_id=b.project_id AND b2.provider_game_id=b.provider_game_id)
           ORDER BY b.rowid DESC LIMIT 100",
    )?;
    let rows = statement.query_map([window], |r| {
        Ok(BrowseInstallIndexItem {
            search_id: r.get(0)?,
            item_id: r.get(1)?,
            game_id: r.get(2)?,
            title: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[tauri::command]
pub(crate) async fn browse_install_task<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, BrowseState>,
    services: State<'_, AppServices>,
    search_id: String,
    item_id: String,
    game_id: String,
) -> AppResult<Option<BrowseInstallTask>> {
    let db = &services.engine.db;
    {
        let conn = db.conn();
        if let Some(key) = known_work(&conn, window.label(), &search_id, &item_id, &game_id)? {
            return task_for(&conn, &key);
        }
    }
    let work = resolve_work(
        &state,
        &services,
        window.label(),
        &search_id,
        &item_id,
        &game_id,
    )
    .await?;
    let key = WorkKey::from(&work);
    let conn = db.conn();
    let task = task_for(&conn, &key)?;
    if task.is_some() {
        remember_handle(&conn, window.label(), &search_id, &item_id, &key)?;
    }
    Ok(task)
}

#[tauri::command]
pub(crate) async fn start_browse_install<R: Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, BrowseState>,
    services: State<'_, AppServices>,
    agent: State<'_, AgentRuntime>,
    search_id: String,
    item_id: String,
    game_id: String,
) -> AppResult<BrowseInstallTask> {
    let work = resolve_work(
        &state,
        &services,
        window.label(),
        &search_id,
        &item_id,
        &game_id,
    )
    .await?;
    let window = window.label().to_owned();
    let agent = agent.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        start(&agent, &window, &search_id, &item_id, &work)
    })
    .await
    .map_err(|e| AppError::Other(e.to_string()))?
}

fn start(
    agent: &AgentRuntime,
    window: &str,
    search_id: &str,
    item_id: &str,
    work: &TrustedWork,
) -> AppResult<BrowseInstallTask> {
    let _guard = START.lock().unwrap_or_else(|e| e.into_inner());
    let db = &agent.engine.db;
    let key = WorkKey::from(work);
    let game = work.game_scope.clone();
    if !game.current(db) {
        return Err(AppError::Conflict("安装目标游戏已变化".into()));
    }
    {
        let conn = db.conn();
        remember_handle(&conn, window, search_id, item_id, &key)?;
        if let Some(task) = task_for(&conn, &key)?
            .filter(|task| matches!(task.status, TaskStatus::Running | TaskStatus::Installed))
        {
            return Ok(task);
        }
    }
    let id = util::new_id("browse-install");
    let history = History::new(db.clone());
    let conversation = history.create_conversation_with_category(
        "Agent 安装",
        Some(&key.game_id),
        Some("install"),
    )?;
    db.conn().execute(
        "INSERT INTO browse_install_tasks(id,conversation_id,game_id,source_id,project_id,provider_game_id)
          VALUES(?1,?2,?3,?4,?5,?6)",
        params![id, conversation.id, key.game_id, key.source_id, key.project_id, key.provider_game_id],
    )?;
    db.conn().execute(
        "INSERT INTO browse_install_titles(task_id,title) VALUES(?1,?2)",
        params![id, work.title],
    )?;
    if let Some(page) = key.project_page() {
        db.conn().execute(
            "INSERT INTO browse_install_project_pages(task_id,project_page) VALUES(?1,?2)",
            params![id, page],
        )?;
    }
    let scope = InstallScope::new(id, key.clone(), game);
    let request = TurnRequest {
        conversation_id: conversation.id.clone(),
        content: scope.instruction(),
        attachments: None,
        model: None,
        permission_mode: None,
        reasoning_effort: None,
    };
    if let Err(error) = agent.start_browse_turn(request, scope) {
        let _ = history.delete_conversation(&conversation.id);
        return Err(error);
    }
    task_for(&db.conn(), &key)?.ok_or_else(|| AppError::NotFound("安装任务不存在".into()))
}

#[cfg(test)]
mod tests;
