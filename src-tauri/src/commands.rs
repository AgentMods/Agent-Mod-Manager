use crate::sources::{
    resolutions::{resolution_id as parse_resolution_id, ResolutionResult, ResolutionScope},
    DependencyGraph, DependencyGraphRequest, ResolveRequest, SearchQuery, SearchResult, SourceKeys,
    Sources, VersionListRequest, VersionListResult,
};
use crate::{
    app::AppServices,
    error::{AppError, AppResult},
    plugins, snapshot,
    transaction::{FileOp, FilePlan},
    trash,
};
use crate::{games, paths::user_path};
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub async fn read_dir(
    state: State<'_, AppServices>,
    path: std::path::PathBuf,
) -> AppResult<Vec<crate::util::DirEntryInfo>> {
    blocking(state.engine.clone(), move |e| {
        crate::filesystem::read_dir(&e, &path)
    })
    .await
}
#[tauri::command]
pub async fn read_file(
    state: State<'_, AppServices>,
    path: std::path::PathBuf,
) -> AppResult<crate::filesystem::TextFile> {
    blocking(state.engine.clone(), move |e| {
        crate::filesystem::read_file(&e, &path)
    })
    .await
}

/// This endpoint is not registered as an Agent or external MCP tool.
#[tauri::command]
pub async fn purge_trash(
    state: State<'_, AppServices>,
    ids: Vec<String>,
    confirmation: String,
) -> AppResult<u64> {
    blocking(state.engine.clone(), move |e| {
        crate::trash::purge_trash(&e.db, &e.paths, &ids, &confirmation)
    })
    .await
}

#[tauri::command]
pub async fn download_mod(
    state: State<'_, AppServices>,
    agent: State<'_, AgentRuntime>,
    app: tauri::AppHandle,
    task_id: String,
    resolution_id: Option<String>,
    artifact_id: Option<String>,
    classification: Option<crate::download_tasks::ClassificationInput>,
) -> AppResult<crate::download_tasks::LibraryDownloadReceipt> {
    use tauri::Emitter;
    let classification = classification.unwrap_or_default();
    let resolution_id = match (resolution_id, artifact_id) {
        (Some(id), None) => parse_resolution_id(&id)?,
        (None, Some(id)) => {
            return state
                .downloads
                .retry(&state.engine, &id, classification)
                .await
                .map(crate::download_tasks::LibraryDownloadReceipt::into_user_paths);
        }
        _ => {
            return Err(AppError::Invalid(
                "请提供下载解析 ID 或已保存的下载回执 ID，不能同时提供".into(),
            ))
        }
    };
    let event_id = task_id.clone();
    state
        .downloads
        .run(
            &state.engine,
            &agent.resolutions,
            task_id,
            resolution_id,
            classification,
            std::sync::Arc::new(move |progress| {
                let _ = app.emit(
                    "download://progress",
                    serde_json::json!({"taskId":event_id,"progress":progress}),
                );
            }),
        )
        .await
        .map(crate::download_tasks::LibraryDownloadReceipt::into_user_paths)
}
#[tauri::command]
pub fn cancel_download(state: State<'_, AppServices>, task_id: String) -> AppResult<()> {
    state.downloads.cancel(&task_id)
}
#[tauri::command]
pub async fn list_downloads(
    state: State<'_, AppServices>,
) -> AppResult<Vec<crate::artifacts::ArtifactRecord>> {
    blocking(state.engine.clone(), |e| {
        let mut records = crate::artifacts::list(&e.db)?;
        for record in &mut records {
            record.artifact.path = user_path(&record.artifact.path).into_owned();
        }
        Ok(records)
    })
    .await
}

#[tauri::command]
pub async fn mcp_list(state: State<'_, AppServices>) -> AppResult<Vec<crate::mcp::ServerConfig>> {
    blocking(state.engine.clone(), move |e| {
        crate::mcp::McpServers::new(e.db).list()
    })
    .await
}
/// Desktop-only: return presence for this server's saved references, never their values.
#[tauri::command]
pub async fn mcp_credential_status(
    state: State<'_, AppServices>,
    agent: State<'_, AgentRuntime>,
    id: String,
) -> AppResult<std::collections::BTreeMap<String, bool>> {
    crate::mcp_connections::require_external(&id)?;
    let servers = crate::mcp::McpServers::new(state.engine.db.clone());
    let settings = agent.settings.clone();
    blocking(servers, move |servers| {
        servers.credential_status(&settings, &id)
    })
    .await
}

#[tauri::command]
pub async fn mcp_create(
    state: State<'_, AppServices>,
    config: crate::mcp::ServerConfig,
) -> AppResult<()> {
    crate::mcp_connections::require_external(&config.id)?;
    blocking(state.engine.clone(), move |e| {
        crate::mcp::McpServers::new(e.db).create(&config)
    })
    .await
}
#[tauri::command]
pub async fn mcp_update(
    state: State<'_, AppServices>,
    agent: State<'_, AgentRuntime>,
    config: crate::mcp::ServerConfig,
) -> AppResult<()> {
    crate::mcp_connections::require_external(&config.id)?;
    if config.plugin_slug.is_some() {
        return Err(AppError::Denied("不能手动修改插件工具".into()));
    }
    let servers = crate::mcp::McpServers::new(state.engine.db.clone());
    state
        .mcp_connections
        .revoke_if_needed(&agent.registry, &config.id, || servers.update(&config))
        .await
}
#[tauri::command]
pub async fn mcp_remove(
    state: State<'_, AppServices>,
    agent: State<'_, AgentRuntime>,
    id: String,
) -> AppResult<()> {
    crate::mcp_connections::require_external(&id)?;
    let servers = crate::mcp::McpServers::new(state.engine.db.clone());
    state
        .mcp_connections
        .revoke(&agent.registry, &id, || servers.remove(&id))
        .await
}
#[tauri::command]
pub async fn mcp_connect(
    state: State<'_, AppServices>,
    agent: State<'_, AgentRuntime>,
    expected_config: crate::mcp::ServerConfig,
    accepts_unsandboxed_stdio: bool,
) -> AppResult<crate::mcp_connections::ConnectionInfo> {
    if expected_config.plugin_slug.is_some() {
        // Reconcile the deployed files before accepting launch confirmation. A
        // missing/changed package can revoke ownership or disable its MCP here.
        blocking(state.engine.clone(), |e| {
            crate::plugin_workflow::reconcile_deployments(&e)?;
            Ok(())
        })
        .await?;
    }
    state
        .mcp_connections
        .connect(
            agent.settings.clone(),
            &agent.registry,
            expected_config,
            accepts_unsandboxed_stdio,
        )
        .await
}
#[tauri::command]
pub async fn mcp_pending_game_tools(
    state: State<'_, AppServices>,
) -> AppResult<Vec<crate::mcp::ServerConfig>> {
    blocking(state.engine.clone(), |e| {
        crate::mcp::McpServers::new(e.db).pending_game_tools()
    })
    .await
}

async fn decide_game_tool(
    state: State<'_, AppServices>,
    expected_config: crate::mcp::ServerConfig,
    allowed: bool,
) -> AppResult<()> {
    blocking(state.engine.clone(), move |e| {
        crate::plugin_workflow::reconcile_deployments(&e)?;
        let _guard = crate::transaction::write_lock();
        crate::mcp::McpServers::new(e.db.clone())
            .set_game_tool_consent(&expected_config, allowed)?;
        e.notify_plugin_mcp_changed();
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn mcp_approve_game_tool(
    state: State<'_, AppServices>,
    expected_config: crate::mcp::ServerConfig,
) -> AppResult<()> {
    decide_game_tool(state, expected_config, true).await
}

#[tauri::command]
pub async fn mcp_decline_game_tool(
    state: State<'_, AppServices>,
    expected_config: crate::mcp::ServerConfig,
) -> AppResult<()> {
    decide_game_tool(state, expected_config, false).await
}

#[tauri::command]
pub async fn mcp_disconnect(
    state: State<'_, AppServices>,
    agent: State<'_, AgentRuntime>,
    id: String,
) -> AppResult<()> {
    let servers = crate::mcp::McpServers::new(state.engine.db.clone());
    state
        .mcp_connections
        .disconnect(&agent.registry, &servers, &id)
        .await
}
#[tauri::command]
pub async fn mcp_startup_requests(state: State<'_, AppServices>) -> AppResult<Vec<String>> {
    blocking(state.engine.clone(), |e| {
        crate::mcp::McpServers::new(e.db).startup_requests()
    })
    .await
}
#[tauri::command]
pub async fn mcp_connections(
    state: State<'_, AppServices>,
) -> AppResult<Vec<crate::mcp_connections::ConnectionInfo>> {
    Ok(state.mcp_connections.list().await)
}

#[tauri::command]
pub async fn mcp_server_status(
    server: State<'_, crate::mcp_server::LocalMcpServer>,
) -> AppResult<crate::mcp_server::McpServerStatus> {
    Ok(server.status().await)
}

/// These controls are desktop-only and never appear in the tool registry.
#[tauri::command]
pub async fn mcp_server_start(
    server: State<'_, crate::mcp_server::LocalMcpServer>,
    state: State<'_, AppServices>,
    confirmation: String,
) -> AppResult<crate::mcp_server::McpServerStatus> {
    if confirmation != "START_LOCAL_MCP" {
        return Err(AppError::Denied("需在桌面确认开启本地工具".into()));
    }
    server.start_desktop(&state.mcp_connections).await
}

#[tauri::command]
pub async fn mcp_server_stop(
    server: State<'_, crate::mcp_server::LocalMcpServer>,
    state: State<'_, AppServices>,
) -> AppResult<()> {
    server.stop_desktop(&state.mcp_connections).await
}

#[tauri::command]
pub async fn read_logs(
    state: State<'_, AppServices>,
    game_id: String,
    options: Option<crate::diagnostics::ReadOptions>,
) -> AppResult<crate::diagnostics::LogReport> {
    blocking(state.engine.clone(), move |e| {
        crate::diagnostics::read_logs_page(
            &e,
            game_id,
            options.unwrap_or_default(),
            "desktop",
            &tokio_util::sync::CancellationToken::new(),
        )
    })
    .await
}

#[tauri::command]
pub async fn diagnose_game(
    state: State<'_, AppServices>,
    game_id: String,
    options: Option<crate::diagnostics::ReadOptions>,
) -> AppResult<crate::diagnostics::Diagnosis> {
    blocking(state.engine.clone(), move |e| {
        crate::diagnostics::diagnose_game_page(
            &e,
            game_id,
            options.unwrap_or_default(),
            "desktop",
            &tokio_util::sync::CancellationToken::new(),
        )
    })
    .await
}

#[tauri::command]
pub async fn search_mods(
    agent: State<'_, AgentRuntime>,
    query: SearchQuery,
) -> AppResult<SearchResult> {
    crate::agent::source_tools::sources(&agent.settings)?
        .search(&query)
        .await
}
#[tauri::command]
pub async fn resolve_mod(
    agent: State<'_, AgentRuntime>,
    request: ResolveRequest,
) -> AppResult<ResolutionResult> {
    let result = crate::agent::source_tools::sources(&agent.settings)?
        .resolve(&request)
        .await?;
    agent.resolutions.publish(ResolutionScope::Desktop, result)
}
#[tauri::command]
pub async fn list_mod_versions(request: VersionListRequest) -> AppResult<VersionListResult> {
    Sources::new(SourceKeys::default())?
        .list_versions(&request)
        .await
}
#[tauri::command]
pub async fn resolve_modrinth_dependency_graph(
    state: State<'_, AppServices>,
    request: DependencyGraphRequest,
    operation_id: Option<uuid::Uuid>,
) -> AppResult<DependencyGraph> {
    let operation = state.dependency_operations.start(operation_id)?;
    Sources::new(SourceKeys::default())?
        .modrinth_dependency_graph(&request, &operation.cancel)
        .await
}

#[tauri::command]
pub fn cancel_modrinth_dependency_graph(
    state: State<'_, AppServices>,
    operation_id: uuid::Uuid,
) -> bool {
    state.dependency_operations.cancel(operation_id)
}
use crate::{
    agent::{
        approval::ApprovalResponse,
        events::AgentEvent,
        runtime::{AgentRuntime, PromptOrigin, TurnRequest},
    },
    history::{Conversation, ConversationDetail, History},
    settings::AppSettings,
};

#[tauri::command]
pub async fn start_turn(
    state: State<'_, AgentRuntime>,
    request: TurnRequest,
    origin: Option<PromptOrigin>,
) -> AppResult<String> {
    let turn_id = blocking(state.inner().clone(), move |agent| match origin {
        Some(origin) => agent.start_turn_with_origin(request, origin),
        None => agent.start_turn(request),
    })
    .await?;
    crate::analytics::track("agent_turn_accepted");
    Ok(turn_id)
}
#[tauri::command]
pub async fn restart_turn(
    state: State<'_, AgentRuntime>,
    request: TurnRequest,
    message_id: String,
    edit: bool,
) -> AppResult<String> {
    let conversation_id = blocking(state.inner().clone(), move |agent| {
        agent.restart_turn(request, &message_id, edit)
    })
    .await?;
    crate::analytics::track("agent_turn_restart_accepted");
    Ok(conversation_id)
}
#[tauri::command]
pub async fn cancel_turn(
    state: State<'_, AgentRuntime>,
    conversation_id: String,
    turn_id: String,
) -> AppResult<()> {
    blocking(state.inner().clone(), move |agent| {
        agent.cancel_turn(&conversation_id, &turn_id)
    })
    .await
}
#[tauri::command]
pub async fn respond_approval(
    state: State<'_, AgentRuntime>,
    conversation_id: String,
    approval_id: String,
    response: ApprovalResponse,
) -> AppResult<()> {
    blocking(state.inner().clone(), move |agent| {
        agent.respond_approval(&conversation_id, &approval_id, response)
    })
    .await
}
#[tauri::command]
pub async fn replay_agent_events(
    state: State<'_, AgentRuntime>,
    conversation_id: String,
    after: i64,
    limit: usize,
) -> AppResult<Vec<AgentEvent>> {
    blocking(state.events.clone(), move |events| {
        events.replay(&conversation_id, after, limit)
    })
    .await
}
#[tauri::command]
pub async fn create_conversation(
    state: State<'_, AppServices>,
    title: String,
    game_id: Option<String>,
    category: Option<String>,
    workspace_game_ids: Option<Vec<String>>,
) -> AppResult<Conversation> {
    blocking(state.engine.clone(), move |e| {
        History::new(e.db).create_conversation_with_workspace(
            &title,
            game_id.as_deref(),
            category.as_deref(),
            workspace_game_ids.as_deref(),
        )
    })
    .await
}
#[tauri::command]
pub async fn set_conversation_workspace(
    state: State<'_, AppServices>,
    id: String,
    game_ids: serde_json::Value,
) -> AppResult<Conversation> {
    // Require the IPC key even when it is null, so omitting it cannot
    // silently widen the selection to all games.
    let game_ids: Option<Vec<String>> = serde_json::from_value(game_ids)
        .map_err(|_| AppError::Invalid("工作区游戏必须是游戏 ID 数组或 null".into()))?;
    blocking(state.engine.clone(), move |e| {
        History::new(e.db).set_conversation_workspace(&id, game_ids.as_deref())
    })
    .await
}
#[tauri::command]
pub async fn list_conversations(
    state: State<'_, AppServices>,
    archived: bool,
) -> AppResult<Vec<Conversation>> {
    blocking(state.engine.clone(), move |e| {
        History::new(e.db).list_conversations(archived)
    })
    .await
}
#[tauri::command]
pub async fn get_conversation(
    state: State<'_, AppServices>,
    id: String,
) -> AppResult<ConversationDetail> {
    blocking(state.engine.clone(), move |e| {
        History::new(e.db).get_conversation(&id)
    })
    .await
}
#[tauri::command]
pub async fn archive_conversation(
    state: State<'_, AppServices>,
    id: String,
    archived: bool,
) -> AppResult<()> {
    blocking(state.engine.clone(), move |e| {
        History::new(e.db).archive_conversation(&id, archived)
    })
    .await
}
#[tauri::command]
pub async fn rename_conversation(
    state: State<'_, AppServices>,
    id: String,
    title: String,
) -> AppResult<()> {
    blocking(state.engine.clone(), move |e| {
        History::new(e.db).rename_conversation(&id, &title)
    })
    .await
}
#[tauri::command]
pub async fn set_conversation_pinned(
    state: State<'_, AppServices>,
    id: String,
    pinned: bool,
) -> AppResult<()> {
    blocking(state.engine.clone(), move |e| {
        History::new(e.db).set_conversation_pinned(&id, pinned)
    })
    .await
}
#[tauri::command]
pub async fn delete_conversation(state: State<'_, AgentRuntime>, id: String) -> AppResult<()> {
    blocking(state.inner().clone(), move |agent| {
        agent.delete_conversation(&id)
    })
    .await
}
#[tauri::command]
pub async fn set_conversation_permission(
    state: State<'_, AppServices>,
    id: String,
    mode: Option<String>,
) -> AppResult<()> {
    blocking(state.engine.clone(), move |e| {
        History::new(e.db).set_permission_mode(&id, mode.as_deref())
    })
    .await
}
#[tauri::command]
pub async fn get_settings(agent: State<'_, AgentRuntime>) -> AppResult<AppSettings> {
    agent.settings.load()
}
#[tauri::command]
pub async fn get_composer_preferences(
    agent: State<'_, AgentRuntime>,
) -> AppResult<crate::settings::ComposerPreferences> {
    agent.settings.load_composer_preferences()
}
#[tauri::command]
pub async fn set_composer_preferences(
    agent: State<'_, AgentRuntime>,
    preferences: crate::settings::ComposerPreferences,
) -> AppResult<()> {
    agent.settings.save_composer_preferences(&preferences)
}
#[tauri::command]
pub async fn list_agent_models(
    agent: State<'_, AgentRuntime>,
    base_url: String,
    api_key: Option<String>,
) -> AppResult<Vec<String>> {
    let base = crate::settings::parse_model_base_url(&base_url)?;
    let api_key = match api_key.filter(|key| !key.is_empty()) {
        Some(key) => Some(key),
        None => {
            let saved = agent.settings.load()?;
            let saved_base = crate::settings::parse_model_base_url(&saved.base_url)?;
            if base.as_str().trim_end_matches('/') == saved_base.as_str().trim_end_matches('/') {
                match saved.api_key_ref {
                    Some(reference) => agent.settings.read_secret(&reference)?,
                    None => None,
                }
            } else {
                None
            }
        }
    };
    crate::agent::model_catalog::list(&base, api_key.as_deref()).await
}

#[tauri::command]
pub async fn get_agent_model_metadata(
    base_url: String,
    model_ids: Vec<String>,
) -> AppResult<Vec<crate::agent::model_catalog::ModelMetadata>> {
    crate::agent::model_catalog::reasoning_metadata(&base_url, &model_ids).await
}

#[tauri::command]
pub async fn set_settings(agent: State<'_, AgentRuntime>, settings: AppSettings) -> AppResult<()> {
    agent.settings.save(&settings)?;
    agent.engine.organization_wake.notify_one();
    Ok(())
}
#[tauri::command]
pub async fn set_secret(
    agent: State<'_, AgentRuntime>,
    key: String,
    value: String,
) -> AppResult<()> {
    let settings = agent.settings.clone();
    tauri::async_runtime::spawn_blocking(move || settings.write_secret(&key, &value))
        .await
        .map_err(|_| AppError::Other("凭据保存任务未完成".into()))?
}

#[tauri::command]
pub async fn list_games(state: State<'_, AppServices>) -> AppResult<Vec<games::Game>> {
    blocking(state.engine.clone(), |e| {
        crate::install::reconcile(&e.db)?;
        games::list_games(&e.db)
    })
    .await
}

#[tauri::command]
pub async fn get_dashboard_stats(
    state: State<'_, AppServices>,
) -> AppResult<crate::dashboard::DashboardStats> {
    blocking(state.engine.clone(), |engine| {
        crate::dashboard::read_stats(&engine.db)
    })
    .await
}

#[tauri::command]
pub async fn get_game(state: State<'_, AppServices>, id: String) -> AppResult<games::Game> {
    blocking(state.engine.clone(), move |e| {
        crate::install::reconcile(&e.db)?;
        games::get_game(&e.db, &id)
    })
    .await
}

#[tauri::command]
pub async fn identify_game(
    state: State<'_, AppServices>,
    directory: std::path::PathBuf,
) -> AppResult<games::GameCandidate> {
    blocking(state.engine.clone(), move |e| {
        games::identify_game(&e.db, &directory)
    })
    .await
}

#[tauri::command]
pub async fn preview_game_identity(
    state: State<'_, AppServices>,
    directory: std::path::PathBuf,
) -> AppResult<crate::game_identity::GameIdentityPreview> {
    blocking(state.engine.clone(), move |engine| {
        crate::game_identity::preview(&engine, &directory)
    })
    .await
}

#[tauri::command]
pub async fn search_game_identity(
    state: State<'_, AppServices>,
    directory: std::path::PathBuf,
    expected_fingerprint: String,
    public_query: String,
    confirmation: String,
) -> AppResult<crate::game_identity::GameIdentitySearch> {
    crate::game_identity::search(
        &state.engine,
        &state.runtime,
        &directory,
        &expected_fingerprint,
        &public_query,
        &confirmation,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .map(crate::game_identity::GameIdentitySearch::into_user_paths)
}

// Root registration is desktop-owned, never an Agent/MCP tool.
#[tauri::command]
pub async fn confirm_game_identity(
    state: State<'_, AppServices>,
    search_id: String,
    candidate_index: usize,
    name: String,
    confirmation: String,
) -> AppResult<games::Game> {
    let game = blocking(state.engine.clone(), move |engine| {
        crate::game_identity::confirm(&engine, &search_id, candidate_index, &name, &confirmation)
    })
    .await?;
    crate::analytics::track("game_registered");
    Ok(game)
}

#[tauri::command]
pub async fn scan_games(
    state: State<'_, AppServices>,
    options: games::ScanOptions,
) -> AppResult<games::ScanReport> {
    blocking(state.engine.clone(), move |e| {
        games::scan_games_page(
            &e.db,
            options,
            "desktop",
            &tokio_util::sync::CancellationToken::new(),
        )
    })
    .await
}

#[tauri::command]
pub async fn register_game(
    state: State<'_, AppServices>,
    directory: std::path::PathBuf,
    adapter_slug: Option<String>,
    from_discovery: Option<bool>,
    cloud_adapter: Option<crate::manager_catalog::CatalogReleaseSelection>,
    discovery_name: Option<String>,
) -> AppResult<games::Game> {
    let game = if let Some(release) = cloud_adapter {
        if from_discovery != Some(true) || adapter_slug.is_some() {
            return Err(AppError::Invalid(
                "只有未选本地适配包的扫描结果才能自动安装云端适配包".into(),
            ));
        }
        let registration = blocking(state.engine.clone(), move |e| {
            games::CatalogGameRegistration::new(&e.paths, &directory, discovery_name.as_deref())
        })
        .await?;
        // No filesystem or DB lock spans the anonymous release revalidation/download.
        let preview =
            crate::manager_catalog::preview(&state.engine, &release.plugin_id, &release.release_id)
                .await?;
        blocking(state.engine.clone(), move |e| {
            crate::plugin_workflow::register_catalog_game(&e, preview, registration)
        })
        .await?
    } else {
        blocking(state.engine.clone(), move |e| {
            crate::game_actions::register_local_from_desktop(
                &e,
                &directory,
                adapter_slug.as_deref(),
                from_discovery == Some(true),
            )
        })
        .await?
    };
    crate::analytics::track("game_registered");
    Ok(game)
}

#[tauri::command]
pub async fn update_game(
    state: State<'_, AppServices>,
    id: String,
    name: String,
    launch_path: Option<String>,
) -> AppResult<games::Game> {
    blocking(state.engine.clone(), move |e| {
        crate::game_actions::update_game(&e, &id, &name, launch_path.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn set_game_adapter(
    state: State<'_, AppServices>,
    id: String,
    adapter_slug: String,
) -> AppResult<games::Game> {
    blocking(state.engine.clone(), move |engine| {
        crate::game_actions::set_game_adapter(&engine, &id, &adapter_slug)
    })
    .await
}

#[tauri::command]
pub async fn remove_game_adapter(
    state: State<'_, AppServices>,
    id: String,
) -> AppResult<games::Game> {
    blocking(state.engine.clone(), move |engine| {
        crate::game_actions::remove_game_adapter(&engine, &id)
    })
    .await
}

#[tauri::command]
pub async fn list_game_conflicts(
    state: State<'_, AppServices>,
    game_id: String,
    change: Option<crate::game_actions::GameChange>,
) -> AppResult<Vec<crate::game_actions::GamePlanConflict>> {
    blocking(state.engine.clone(), move |engine| {
        crate::game_actions::list_game_conflicts(&engine, &game_id, change.unwrap_or_default())
    })
    .await
}

#[tauri::command]
pub async fn resolve_game_conflict(
    state: State<'_, AppServices>,
    agent: State<'_, AgentRuntime>,
    game_id: String,
    conflict_id: String,
    change: Option<crate::game_actions::GameChange>,
    confirm_approved: Option<bool>,
) -> AppResult<()> {
    blocking(
        (state.engine.clone(), agent.inner().clone()),
        move |(engine, agent)| {
            crate::game_actions::resolve_game_conflict(
                &engine,
                &game_id,
                &conflict_id,
                change.unwrap_or_default(),
                confirm_approved.unwrap_or(false),
                |conversation_id, approval_id| {
                    agent.respond_approval(
                        conversation_id,
                        approval_id,
                        ApprovalResponse {
                            approved: false,
                            answer: serde_json::Value::Null,
                        },
                    )
                },
            )
        },
    )
    .await
}

#[tauri::command]
pub async fn remove_game(state: State<'_, AppServices>, id: String) -> AppResult<()> {
    blocking(state.engine.clone(), move |e| {
        crate::game_actions::remove_game(&e, &id)
    })
    .await
}

#[tauri::command]
pub async fn open_game_location(
    app: AppHandle,
    state: State<'_, AppServices>,
    id: String,
    target: String,
) -> AppResult<()> {
    blocking(state.engine.clone(), move |e| {
        let _guard = crate::transaction::write_lock();
        let directory = crate::game_actions::resolve_location(&e, &id, &target)?;
        open_directory(&app, &directory)
    })
    .await
}

fn open_directory(app: &AppHandle, directory: &std::path::Path) -> AppResult<()> {
    #[cfg(target_os = "macos")]
    if directory
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
    {
        // Launch Services opens an .app as a program. Reveal it in Finder instead.
        return app.opener().reveal_item_in_dir(directory).map_err(|error| {
            AppError::Presented(crate::error::NativeMessage::new(
                "game.open_folder_failed",
                [("detail", error.to_string())],
            ))
        });
    }
    let directory = user_path(directory)
        .into_owned()
        .into_os_string()
        .into_string()
        .map_err(|_| AppError::Invalid("目录路径包含无法表示的字符".into()))?;
    app.opener()
        .open_path(directory, None::<&str>)
        .map_err(|error| {
            AppError::Presented(crate::error::NativeMessage::new(
                "game.open_folder_failed",
                [("detail", error.to_string())],
            ))
        })
}

#[tauri::command]
pub async fn list_game_launch_options(
    state: State<'_, AppServices>,
    id: String,
) -> AppResult<Vec<crate::game_actions::GameLaunchOption>> {
    blocking(state.engine.clone(), move |engine| {
        crate::game_actions::list_game_launch_options(&engine, &id)
    })
    .await
}

#[tauri::command]
pub async fn set_game_mod_launcher(
    state: State<'_, AppServices>,
    id: String,
    launcher: serde_json::Value,
) -> AppResult<games::Game> {
    blocking(state.engine.clone(), move |engine| {
        crate::game_actions::set_game_mod_launcher(&engine, &id, launcher)
    })
    .await
}

#[tauri::command]
pub async fn launch_game(
    state: State<'_, AppServices>,
    id: String,
    method: Option<String>,
) -> AppResult<()> {
    blocking(state.engine.clone(), move |e| {
        crate::game_actions::launch_from_desktop(&e, &id, method.as_deref())
    })
    .await?;
    crate::analytics::track("game_launch_requested");
    Ok(())
}

async fn blocking<S: Send + 'static, T: Send + 'static>(
    state: S,
    work: impl FnOnce(S) -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(move || work(state))
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
}

#[cfg(not(test))]
#[tauri::command]
pub(crate) async fn get_bootstrap_state(
    state: State<'_, AppServices>,
    agent: State<'_, AgentRuntime>,
    storage: State<'_, crate::storage_location::StorageLocation>,
) -> AppResult<serde_json::Value> {
    let (completed, first_run, resumable) = agent.settings.onboarding_state()?;
    Ok(serde_json::json!({
        "dataRoot": user_path(&state.engine.paths.root),
        "storageMigrationError": storage.failure()?,
        "recoveredTransactions": state.recovered,
        "recoveryError": state.recovery_error,
        "onboardingCompleted": completed,
        "isFirstRun": first_run,
        "resumeOnboarding": resumable,
    }))
}

#[cfg(not(test))]
#[tauri::command]
pub(crate) async fn migrate_storage(
    app: AppHandle,
    state: State<'_, AppServices>,
    storage: State<'_, crate::storage_location::StorageLocation>,
    parent: std::path::PathBuf,
) -> AppResult<()> {
    storage.schedule(&state.engine, &parent)?;
    app.request_restart();
    Ok(())
}

#[tauri::command]
pub fn get_app_locale(state: State<'_, AppServices>) -> AppResult<crate::app_locale::AppLocale> {
    crate::app_locale::get(&state.engine.db)
}

#[tauri::command]
pub fn set_app_locale(
    state: State<'_, AppServices>,
    locale: Option<String>,
) -> AppResult<crate::app_locale::AppLocale> {
    crate::app_locale::set(&state.engine.db, locale)
}

#[tauri::command]
pub fn start_onboarding(agent: State<'_, AgentRuntime>) -> AppResult<()> {
    agent.settings.start_onboarding()
}

#[tauri::command]
pub fn set_onboarding_completed(agent: State<'_, AgentRuntime>, completed: bool) -> AppResult<()> {
    agent
        .settings
        .set("onboarding_completed", serde_json::Value::Bool(completed))
}

#[tauri::command]
pub async fn list_plugins(state: State<'_, AppServices>) -> AppResult<Vec<plugins::PluginInfo>> {
    blocking(state.engine.clone(), |e| {
        crate::plugin_workflow::reconcile_deployments(&e)?;
        plugins::list_plugins(&e.db)
    })
    .await
}

#[tauri::command]
pub async fn get_game_adapter(
    state: State<'_, AppServices>,
    game_id: String,
) -> AppResult<plugins::GameAdapter> {
    blocking(state.engine.clone(), move |engine| {
        crate::plugin_workflow::reconcile_deployments(&engine)?;
        let game = games::get_game(&engine.db, &game_id)?;
        let slug = game
            .adapter_slug
            .ok_or_else(|| AppError::NotFound("游戏尚未绑定适配包".into()))?;
        plugins::adapter_for(&engine.db, &slug)
    })
    .await
}

#[tauri::command]
pub async fn set_plugin_enabled(
    state: State<'_, AppServices>,
    slug: String,
    enabled: bool,
) -> AppResult<plugins::PluginInfo> {
    blocking(state.engine.clone(), move |engine| {
        crate::plugin_workflow::set_enabled(&engine, &slug, enabled)
    })
    .await
}

#[tauri::command]
pub async fn write_plugin_draft(
    state: State<'_, AppServices>,
    slug: String,
    adapter_json: String,
    skill_markdown: String,
) -> AppResult<crate::plugin_workflow::DraftPreview> {
    blocking(state.engine.clone(), move |e| {
        crate::plugin_workflow::plan_write_draft(&e, &slug, &adapter_json, &skill_markdown)
    })
    .await
}

#[tauri::command]
pub async fn list_plugin_drafts(
    state: State<'_, AppServices>,
) -> AppResult<Vec<crate::plugin_workflow::DraftInfo>> {
    blocking(state.engine.clone(), |e| {
        crate::plugin_workflow::list_drafts(&e)
    })
    .await
}

#[tauri::command]
pub async fn delete_plugin_draft(state: State<'_, AppServices>, draft_id: String) -> AppResult<()> {
    // Desktop invokes this only after the user confirms this exact draft ID.
    blocking(state.engine.clone(), move |engine| {
        crate::plugin_workflow::delete_confirmed_draft(&engine, &draft_id)
    })
    .await
}

#[tauri::command]
pub async fn validate_plugin(
    state: State<'_, AppServices>,
    draft_id: String,
) -> AppResult<crate::plugin_workflow::DraftValidation> {
    blocking(state.engine.clone(), move |e| {
        crate::plugin_workflow::validate_draft(&e, &draft_id)
    })
    .await
}

#[tauri::command]
pub async fn install_plugin(
    state: State<'_, AppServices>,
    draft_id: String,
) -> AppResult<crate::plugin_workflow::DeploymentPreview> {
    blocking(state.engine.clone(), move |e| {
        crate::plugin_workflow::plan_install_plugin(&e, &draft_id)
    })
    .await
}

#[tauri::command]
pub async fn list_manager_plugin_games(
    query: Option<String>,
) -> AppResult<crate::manager_catalog::CatalogSearch> {
    crate::manager_catalog::search(query.as_deref().unwrap_or("")).await
}

#[tauri::command]
pub async fn preview_manager_plugin(
    state: State<'_, AppServices>,
    plugin_id: String,
    release_id: String,
) -> AppResult<crate::manager_catalog::CatalogPreview> {
    crate::manager_catalog::preview(&state.engine, &plugin_id, &release_id).await
}

#[tauri::command]
pub async fn prepare_manager_plugin_import(
    state: State<'_, AppServices>,
    plugin_id: String,
    release_id: String,
) -> AppResult<crate::manager_catalog::CatalogImport> {
    crate::manager_catalog::prepare_import(&state.engine, &plugin_id, &release_id).await
}

/// The desktop catalog's Install action authorizes only this selected release.
#[tauri::command]
pub async fn install_manager_plugin(
    state: State<'_, AppServices>,
    plugin_id: String,
    release_id: String,
) -> AppResult<()> {
    crate::manager_catalog::install(&state.engine, &plugin_id, &release_id).await?;
    crate::analytics::track("plugin_installed");
    Ok(())
}

#[tauri::command]
pub async fn remove_installed_plugin(
    state: State<'_, AppServices>,
    slug: String,
) -> AppResult<crate::plugin_workflow::PluginRemovalReceipt> {
    blocking(state.engine.clone(), move |e| {
        crate::plugin_workflow::remove_installed_plugin(&e, &slug)
    })
    .await
}

#[tauri::command]
pub async fn get_manager_plugin_display(
    release_id: String,
) -> AppResult<Option<crate::manager_catalog::ReleaseDisplay>> {
    crate::manager_catalog::release_display(&release_id).await
}

#[tauri::command]
pub async fn prepare_auto_manager_publication(
    state: State<'_, AppServices>,
    agent: State<'_, AgentRuntime>,
    draft_id: Option<String>,
    slug: Option<String>,
) -> AppResult<crate::manager_publication::PublicationRecord> {
    crate::manager_publication::prepare_automatic(
        &state.engine,
        &agent.settings,
        draft_id,
        slug,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
}

#[tauri::command]
pub async fn get_manager_publication(
    state: State<'_, AppServices>,
    id: String,
) -> AppResult<crate::manager_publication::PublicationRecord> {
    blocking(state.engine.clone(), move |engine| {
        crate::manager_publication::get_publication(&engine, &id)
    })
    .await
}

#[tauri::command]
pub async fn list_manager_publications(
    state: State<'_, AppServices>,
    before: Option<String>,
) -> AppResult<Vec<crate::manager_publication::PublicationRecord>> {
    blocking(state.engine.clone(), move |engine| {
        crate::manager_publication::list_publications(&engine, before.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn list_my_manager_plugins(
    agent: State<'_, AgentRuntime>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> AppResult<Vec<crate::manager_publication::PublishedPlugin>> {
    crate::manager_publication::list_own_plugins(
        &agent.settings,
        limit.unwrap_or(20),
        offset.unwrap_or(0),
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
}

#[tauri::command]
pub async fn reconcile_manager_publication(
    state: State<'_, AppServices>,
    agent: State<'_, AgentRuntime>,
    id: String,
) -> AppResult<crate::manager_publication::ReconciliationOutcome> {
    crate::manager_publication::reconcile_publication(
        &state.engine,
        &agent.settings,
        &id,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
}

#[tauri::command]
pub async fn get_plugin_publication(
    state: State<'_, AppServices>,
    id: String,
) -> AppResult<crate::plugin_publish::PublicationRecord> {
    blocking(state.engine.clone(), move |engine| {
        crate::plugin_publish::get_publication(&engine, &id)
    })
    .await
}

#[tauri::command]
pub async fn list_plugin_publications(
    state: State<'_, AppServices>,
    before: Option<String>,
) -> AppResult<Vec<crate::plugin_publish::PublicationRecord>> {
    blocking(state.engine.clone(), move |engine| {
        crate::plugin_publish::list_publications(&engine, before.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn reconcile_plugin_publication(
    state: State<'_, AppServices>,
    agent: State<'_, AgentRuntime>,
    id: String,
) -> AppResult<crate::plugin_publish_remote::ReconciliationOutcome> {
    crate::plugin_publish_remote::reconcile_publication(
        &state.engine,
        &agent.settings,
        &id,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
}

#[tauri::command]
pub async fn publish_manager_publication(
    state: State<'_, AppServices>,
    agent: State<'_, AgentRuntime>,
    id: String,
    digest: String,
    confirmation: String,
) -> AppResult<crate::manager_publication::PublicationRecord> {
    if confirmation != "PUBLISH_PLUGIN" {
        return Err(AppError::Denied(
            "发布管理器插件需要确认目标和文件原文".into(),
        ));
    }
    let engine = state.engine.clone();
    let approve_id = id.clone();
    let approve_digest = digest.clone();
    blocking(engine.clone(), move |engine| {
        crate::manager_publication::approve_publication(&engine, &approve_id, &approve_digest)
    })
    .await?;
    crate::manager_publication::publish_approved(
        &engine,
        &agent.settings,
        &id,
        &digest,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
}

#[tauri::command]
pub async fn list_skills(
    state: State<'_, AppServices>,
    settings_only: Option<bool>,
) -> AppResult<Vec<crate::skills::SkillInfo>> {
    blocking(state.engine.clone(), move |engine| {
        if settings_only == Some(true) {
            crate::skills::list_settings_skills(&engine)
        } else {
            crate::skills::list_skills(&engine)
        }
    })
    .await
}

#[tauri::command]
pub async fn get_skill(
    state: State<'_, AppServices>,
    name: String,
) -> AppResult<crate::skills::SkillDetail> {
    blocking(state.engine.clone(), move |engine| {
        crate::skills::get_skill(&engine, &name)
    })
    .await
}

#[tauri::command]
pub async fn plan_install_skill(
    state: State<'_, AppServices>,
    name: String,
    markdown: String,
) -> AppResult<crate::skills::SkillPreview> {
    blocking(state.engine.clone(), move |engine| {
        crate::skills::plan_install_skill(&engine, &name, &markdown)
    })
    .await
}

#[tauri::command]
pub async fn plan_uninstall_skill(
    state: State<'_, AppServices>,
    name: String,
) -> AppResult<crate::skills::SkillPreview> {
    blocking(state.engine.clone(), move |engine| {
        crate::skills::plan_uninstall_skill(&engine, &name)
    })
    .await
}

#[tauri::command]
pub async fn plan_import_archive(
    state: State<'_, AppServices>,
    path: std::path::PathBuf,
) -> AppResult<crate::imports::ImportPreview> {
    blocking(state.engine.clone(), move |engine| {
        crate::imports::plan_import_archive(&engine, &path)
    })
    .await
}

#[tauri::command]
pub async fn register_imported_mod(
    state: State<'_, AppServices>,
    game_id: String,
    transaction_id: String,
) -> AppResult<crate::mod_library::LibraryEntry> {
    blocking(state.engine.clone(), move |engine| {
        crate::imports::register_imported_mod(&engine, &game_id, &transaction_id)
    })
    .await
}

#[tauri::command]
pub async fn bind_mod_library_entry(
    state: State<'_, AppServices>,
    game_id: String,
    artifact_id: String,
) -> AppResult<crate::mod_library::LibraryEntry> {
    blocking(state.engine.clone(), move |engine| {
        crate::mod_library::bind_entry(&engine, &game_id, &artifact_id)
    })
    .await
}

#[tauri::command]
pub async fn remove_managed_mod(
    state: State<'_, AppServices>,
    game_id: String,
    target: crate::mod_management::Target,
) -> AppResult<()> {
    blocking(state.engine.clone(), move |engine| {
        crate::mod_management::remove(&engine, &game_id, &target)
    })
    .await
}

#[tauri::command]
pub async fn list_mod_library(
    state: State<'_, AppServices>,
    game_id: String,
) -> AppResult<Vec<crate::mod_metadata::ArchiveDisplay>> {
    blocking(state.engine.clone(), move |engine| {
        let entries = crate::mod_library::list_for_game(&engine.db, &game_id)?;
        crate::mod_metadata::archive_display(&engine.db, &game_id, entries)
    })
    .await
}

#[tauri::command]
pub async fn update_mod_metadata(
    state: State<'_, AppServices>,
    game_id: String,
    target: crate::mod_metadata::Target,
    patch: crate::mod_metadata::Patch,
) -> AppResult<()> {
    blocking(state.engine.clone(), move |engine| {
        crate::mod_metadata::update(&engine, &game_id, &target, patch)
    })
    .await
}

#[tauri::command]
pub async fn open_mod_storage_directory(
    app: AppHandle,
    state: State<'_, AppServices>,
    game_id: String,
    target: crate::mod_metadata::Target,
) -> AppResult<()> {
    blocking(state.engine.clone(), move |engine| {
        let _guard = crate::transaction::write_lock();
        let directory = crate::mod_metadata::resolve_storage_directory(&engine, &game_id, &target)?;
        open_directory(&app, &directory)
    })
    .await
}

#[tauri::command]
pub async fn preview_fomod(
    state: State<'_, AppServices>,
    artifact_path: std::path::PathBuf,
) -> AppResult<crate::fomod::FomodPreview> {
    blocking(state.engine.clone(), move |engine| {
        crate::fomod::preview(&engine, &artifact_path)
    })
    .await
}

#[tauri::command]
pub async fn set_skill_enabled(
    state: State<'_, AppServices>,
    name: String,
    enabled: bool,
) -> AppResult<crate::skills::SkillInfo> {
    blocking(state.engine.clone(), move |engine| {
        let skill = crate::skills::set_enabled(&engine, &name, enabled)?;
        if name == crate::skills::MOD_ORGANIZER_SKILL_NAME {
            engine.organization_wake.notify_one();
        }
        Ok(skill)
    })
    .await
}

#[tauri::command]
pub async fn plan_file_ops(
    state: State<'_, AppServices>,
    operations: Vec<FileOp>,
    summary: String,
) -> AppResult<FilePlan> {
    blocking(state.engine.clone(), move |e| {
        e.plan_file_ops(operations, summary)
    })
    .await
    .map(FilePlan::into_user_paths)
}

#[tauri::command]
pub async fn get_file_plan(state: State<'_, AppServices>, plan_id: String) -> AppResult<FilePlan> {
    blocking(state.engine.for_local_agent(), move |e| {
        e.get_plan(&plan_id)
    })
    .await
    .map(FilePlan::into_user_paths)
}

fn desktop_plan_event(
    db: &crate::db::Db,
    plan_id: &str,
    registered_plugins: &[String],
) -> Option<&'static str> {
    // Workflow records identify the exact committed plan; summaries and paths
    // are not provenance. A lookup failure must not fail the already-applied plan.
    let (import, restore, plugin): (bool, bool, Option<String>) = db
        .conn()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM desktop_archive_imports WHERE transaction_id=?1),
                    EXISTS(SELECT 1 FROM trash_restore_links WHERE transaction_id=?1),
                    (SELECT slug FROM active_plugin_deployments WHERE transaction_id=?1)",
            [plan_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok()?;
    match (import, restore, plugin) {
        (true, false, None) => Some("mod_archive_import_committed"),
        (false, true, None) => Some("trash_restored"),
        (false, false, Some(slug)) if registered_plugins.contains(&slug) => {
            Some("plugin_installed")
        }
        _ => None,
    }
}

/// Desktop confirmation endpoint. Agent tools must use the separate approval
/// coordinator and may not invoke this IPC endpoint.
#[tauri::command]
pub async fn apply_plan(state: State<'_, AppServices>, plan_id: String) -> AppResult<FilePlan> {
    let (plan, event) = blocking(state.engine.for_local_agent(), move |e| {
        e.approve_plan(&plan_id)?;
        let result = e.apply_file_ops(&plan_id)?;
        crate::install::reconcile(&e.db)?;
        let plugins = crate::plugin_workflow::reconcile_after_apply(&e, &result)?;
        crate::skills::reconcile_deployments(&e)?;
        let event = crate::analytics::enabled()
            .then(|| desktop_plan_event(&e.db, &result.id, &plugins.registered))
            .flatten();
        Ok((result, event))
    })
    .await?;
    if let Some(event) = event {
        crate::analytics::track(event);
    }
    Ok(plan.into_user_paths())
}

#[tauri::command]
pub async fn plan_install(
    state: State<'_, AppServices>,
    request: crate::install::InstallRequest,
) -> AppResult<crate::install::InstallPreview> {
    blocking(state.engine.clone(), move |e| {
        crate::install::plan_install(&e, &request)
    })
    .await
}

#[tauri::command]
pub async fn list_installs(
    state: State<'_, AppServices>,
    game_id: String,
) -> AppResult<Vec<crate::install::InstalledMod>> {
    blocking(state.engine.clone(), move |e| {
        crate::mod_management::visible_installs(&e, &game_id)
    })
    .await
    .map(|items| {
        items
            .into_iter()
            .map(crate::install::InstalledMod::into_user_paths)
            .collect()
    })
}

#[tauri::command]
pub async fn inspect_install(
    state: State<'_, AppServices>,
    install_id: String,
) -> AppResult<crate::install::InstallInspection> {
    blocking(state.engine.clone(), move |e| {
        crate::install::inspect_install(&e, &install_id)
    })
    .await
}

#[tauri::command]
pub async fn check_modrinth_dependencies(
    state: State<'_, AppServices>,
    request: crate::install_dependencies::CheckRequest,
    operation_id: Option<uuid::Uuid>,
) -> AppResult<crate::install_dependencies::DependencyReport> {
    let operation = state.dependency_operations.start(operation_id)?;
    let cancel = operation.cancel.clone();
    blocking(state.engine.clone(), move |e| {
        crate::install_dependencies::check_with_cancel(&e, &request, &cancel)
    })
    .await
}

#[tauri::command]
pub async fn review_modrinth_dependency_graph(
    state: State<'_, AppServices>,
    request: crate::install_dependencies::GraphReviewRequest,
    operation_id: Option<uuid::Uuid>,
) -> AppResult<crate::install_dependencies::GraphReview> {
    let operation = state.dependency_operations.start(operation_id)?;
    let sources = Sources::new(SourceKeys::default())?;
    crate::install_dependencies::review_graph(
        state.engine.clone(),
        &sources,
        request,
        &operation.cancel,
    )
    .await
}

#[tauri::command]
pub async fn plan_uninstall(
    state: State<'_, AppServices>,
    install_id: String,
) -> AppResult<FilePlan> {
    blocking(state.engine.clone(), move |e| {
        crate::install::plan_uninstall(&e, &install_id)
    })
    .await
    .map(FilePlan::into_user_paths)
}

#[tauri::command]
pub async fn rollback_transaction(
    state: State<'_, AppServices>,
    transaction_id: String,
) -> AppResult<()> {
    blocking(state.engine.for_local_agent(), move |e| {
        e.rollback_desktop_transaction(&transaction_id)?;
        crate::install::reconcile(&e.db)?;
        crate::plugin_workflow::reconcile_deployments(&e)?;
        crate::skills::reconcile_deployments(&e)?;
        Ok(())
    })
    .await?;
    crate::analytics::track("transaction_rolled_back");
    Ok(())
}

#[tauri::command]
pub async fn register_user_root(
    state: State<'_, AppServices>,
    game_id: String,
    root_id: String,
    directory: std::path::PathBuf,
) -> AppResult<std::path::PathBuf> {
    blocking(state.engine.clone(), move |e| {
        crate::game_roots::register_user_root(&e, &game_id, &root_id, &directory)
    })
    .await
    .map(|path| user_path(&path).into_owned())
}

#[tauri::command]
pub async fn list_user_roots(
    state: State<'_, AppServices>,
    game_id: String,
) -> AppResult<std::collections::BTreeMap<String, std::path::PathBuf>> {
    blocking(state.engine.clone(), move |e| {
        crate::game_roots::registered_user_roots(&e.db, &game_id)
    })
    .await
    .map(|roots| {
        roots
            .into_iter()
            .map(|(id, path)| (id, user_path(&path).into_owned()))
            .collect()
    })
}

#[tauri::command]
pub async fn trash_paths(
    state: State<'_, AppServices>,
    request: trash::TrashRequest,
) -> AppResult<FilePlan> {
    blocking(state.engine.clone(), move |e| {
        trash::plan_trash(&e, &request)
    })
    .await
    .map(FilePlan::into_user_paths)
}

#[tauri::command]
pub async fn list_trash(
    state: State<'_, AppServices>,
    include_restored: bool,
) -> AppResult<Vec<trash::TrashItem>> {
    blocking(state.engine.clone(), move |e| {
        trash::list_trash(&e.db, include_restored)
    })
    .await
    .map(|items| {
        items
            .into_iter()
            .map(trash::TrashItem::into_user_paths)
            .collect()
    })
}

#[tauri::command]
pub async fn restore_trash(
    state: State<'_, AppServices>,
    ids: Vec<String>,
    overwrite: bool,
) -> AppResult<FilePlan> {
    blocking(state.engine.clone(), move |e| {
        trash::plan_restore(&e, &ids, overwrite)
    })
    .await
    .map(FilePlan::into_user_paths)
}

#[tauri::command]
pub async fn snapshot_create(
    state: State<'_, AppServices>,
    game_id: String,
    label: String,
) -> AppResult<(snapshot::Snapshot, FilePlan)> {
    blocking(state.engine.clone(), move |e| {
        snapshot::plan_create(&e, &game_id, &label)
    })
    .await
    .map(|(snapshot, plan)| (snapshot.into_user_paths(), plan.into_user_paths()))
}

#[tauri::command]
pub async fn list_snapshots(
    state: State<'_, AppServices>,
    game_id: String,
) -> AppResult<Vec<snapshot::Snapshot>> {
    blocking(state.engine.clone(), move |e| {
        snapshot::list_snapshots(&e, &game_id)
    })
    .await
    .map(|items| {
        items
            .into_iter()
            .map(snapshot::Snapshot::into_user_paths)
            .collect()
    })
}

#[tauri::command]
pub async fn restore_snapshot(
    state: State<'_, AppServices>,
    game_id: String,
    snapshot_id: String,
) -> AppResult<FilePlan> {
    blocking(state.engine.clone(), move |e| {
        snapshot::plan_restore(&e, &game_id, &snapshot_id)
    })
    .await
    .map(FilePlan::into_user_paths)
}

#[tauri::command]
pub async fn list_mod_tags(
    state: State<'_, AppServices>,
    game_id: String,
) -> AppResult<Vec<crate::mod_organization::ModTag>> {
    blocking(state.engine.clone(), move |e| {
        crate::mod_organization::list_mod_tags(&e.db, &game_id)
    })
    .await
}

#[tauri::command]
pub async fn save_mod_tag(
    state: State<'_, AppServices>,
    game_id: String,
    id: Option<String>,
    name: String,
    color: String,
) -> AppResult<crate::mod_organization::ModTag> {
    blocking(state.engine.clone(), move |e| {
        crate::mod_organization::save_mod_tag(&e.db, &game_id, id.as_deref(), &name, &color)
    })
    .await
}

#[tauri::command]
pub async fn delete_mod_tag(
    state: State<'_, AppServices>,
    game_id: String,
    id: String,
) -> AppResult<()> {
    blocking(state.engine.clone(), move |e| {
        crate::mod_organization::delete_mod_tag(&e.db, &game_id, &id)
    })
    .await
}

#[tauri::command]
pub async fn set_mod_tags(
    state: State<'_, AppServices>,
    game_id: String,
    install_id: String,
    tag_ids: Vec<String>,
) -> AppResult<()> {
    blocking(state.engine.clone(), move |e| {
        crate::mod_organization::set_mod_tags(&e.db, &game_id, &install_id, &tag_ids)
    })
    .await?;
    crate::analytics::track("mod_tags_updated");
    Ok(())
}
