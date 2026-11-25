//! Durable, card-scoped Agent installs. The webview never supplies provider IDs,
//! install arguments or URLs; a browse result capability is resolved natively.
use super::{matching::GameScope, resolve_work, BrowseState, TrustedWork};
use crate::{
    agent::{
        policy::PermissionMode,
        runtime::{AgentRuntime, TurnRequest},
    },
    app::AppServices,
    db::Db,
    error::{AppError, AppResult},
    history::History,
    util,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::{borrow::Cow, sync::Mutex};
use tauri::{Runtime, State, WebviewWindow};

mod scope;
mod stock;
mod three_dm;
pub(crate) use scope::InstallScope;

// The application has one SQLite connection and one desktop Agent. Keep create +
// launch exclusive, but never hold a DB guard while entering AgentRuntime.
static START: Mutex<()> = Mutex::new(());

pub(crate) fn init(db: &Db) -> AppResult<()> {
    db.conn().execute_batch(
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
           PRIMARY KEY(task_id,artifact_id)
         );
         CREATE TABLE IF NOT EXISTS browse_install_titles (
           task_id TEXT PRIMARY KEY REFERENCES browse_install_tasks(id) ON DELETE CASCADE,
           title TEXT NOT NULL
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
    Ok(())
}

pub(crate) fn is_task_conversation(conn: &Connection, id: &str) -> AppResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM browse_install_tasks WHERE conversation_id=?1)",
        [id],
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

#[derive(Clone)]
struct WorkKey {
    game_id: String,
    source_id: String,
    project_id: String,
    provider_game_id: String,
}

impl From<&TrustedWork> for WorkKey {
    fn from(work: &TrustedWork) -> Self {
        Self {
            game_id: work.game_id.clone(),
            source_id: work.source_id.clone(),
            project_id: work.project_id.clone(),
            provider_game_id: work.provider_game_id.clone(),
        }
    }
}

impl WorkKey {
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
           EXISTS (SELECT 1 FROM approvals a WHERE a.conversation_id=b.conversation_id
             AND a.status='rejected' AND a.title<>'ask_user')
         FROM browse_install_tasks b
         LEFT JOIN agent_turns t ON t.conversation_id=b.conversation_id
         WHERE b.game_id=?1 AND b.source_id=?2 AND b.project_id=?3 AND b.provider_game_id=?4
         ORDER BY b.rowid DESC LIMIT 1",
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
    let scope = InstallScope::new(id, key.clone(), game);
    let request = TurnRequest {
        conversation_id: conversation.id.clone(),
        content: scope.instruction(),
        attachments: None,
        model: None,
        permission_mode: Some(PermissionMode::RequestApproval),
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
