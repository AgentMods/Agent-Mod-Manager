pub mod agent;
mod agent_attachment;
pub mod app;
pub mod archive;
pub mod artifacts;
pub mod commands;
pub mod db;
pub mod diagnostics;
pub mod download;
pub mod download_tasks;
pub mod encrypted_store;
pub mod error;
pub mod filesystem;
pub mod fomod;
mod game_actions;
pub mod game_identity;
pub mod game_roots;
pub mod games;
pub mod history;
pub mod imports;
pub mod install;
pub mod install_dependencies;
mod install_file_evidence;
pub mod mcp;
pub mod mcp_connections;
pub mod mcp_server;
pub mod paths;
pub mod plugin_publish;
pub mod plugin_publish_remote;
pub mod plugin_workflow;
pub mod plugins;
mod safe_fs;
pub mod settings;
pub mod skills;
pub mod snapshot;
pub mod sources;
pub mod storage;
mod storage_location;
pub mod transaction;
pub mod trash;
pub mod util;

#[cfg(not(test))]
pub fn run() {
    use tauri::Manager;
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .invoke_handler(tauri::generate_handler![
            commands::get_bootstrap_state,
            commands::migrate_storage,
            commands::start_onboarding,
            commands::set_onboarding_completed,
            commands::list_plugins,
            commands::get_game_adapter,
            commands::plan_import_archive,
            commands::preview_fomod,
            commands::set_plugin_enabled,
            commands::write_plugin_draft,
            commands::list_plugin_drafts,
            commands::delete_plugin_draft,
            commands::validate_plugin,
            commands::install_plugin,
            commands::prepare_plugin_publication,
            commands::list_plugin_publications,
            commands::reconcile_plugin_publication,
            commands::publish_plugin,
            commands::list_skills,
            commands::get_skill,
            commands::plan_install_skill,
            commands::plan_uninstall_skill,
            commands::set_skill_enabled,
            commands::plan_file_ops,
            commands::get_file_plan,
            commands::apply_plan,
            commands::trash_paths,
            commands::list_trash,
            commands::restore_trash,
            commands::snapshot_create,
            commands::list_snapshots,
            commands::restore_snapshot,
            commands::list_games,
            commands::get_game,
            commands::update_game,
            commands::set_game_adapter,
            commands::remove_game_adapter,
            commands::list_game_conflicts,
            commands::resolve_game_conflict,
            commands::remove_game,
            commands::open_game_location,
            commands::launch_game,
            commands::identify_game,
            commands::preview_game_identity,
            commands::search_game_identity,
            commands::confirm_game_identity,
            commands::scan_games,
            commands::register_game,
            commands::register_user_root,
            commands::list_user_roots,
            commands::plan_install,
            commands::list_installs,
            commands::inspect_install,
            commands::check_modrinth_dependencies,
            commands::review_modrinth_dependency_graph,
            commands::plan_uninstall,
            commands::rollback_transaction,
            agent_attachment::convert_agent_attachment,
            commands::start_turn,
            commands::restart_turn,
            commands::cancel_turn,
            commands::respond_approval,
            commands::replay_agent_events,
            commands::create_conversation,
            commands::list_conversations,
            commands::get_conversation,
            commands::archive_conversation,
            commands::rename_conversation,
            commands::set_conversation_pinned,
            commands::delete_conversation,
            commands::get_settings,
            commands::list_agent_models,
            commands::get_agent_model_metadata,
            commands::set_settings,
            commands::get_composer_preferences,
            commands::set_composer_preferences,
            commands::set_secret,
            commands::set_conversation_permission,
            commands::search_mods,
            commands::resolve_mod,
            commands::list_mod_versions,
            commands::resolve_modrinth_dependency_graph,
            commands::read_logs,
            commands::diagnose_game,
            commands::mcp_list,
            commands::mcp_create,
            commands::mcp_update,
            commands::mcp_remove,
            commands::mcp_connect,
            commands::mcp_disconnect,
            commands::mcp_connections,
            commands::mcp_startup_requests,
            commands::mcp_server_status,
            commands::mcp_server_start,
            commands::mcp_server_stop,
            commands::download_mod,
            commands::cancel_download,
            commands::list_downloads,
            commands::purge_trash,
            commands::read_dir,
            commands::read_file
        ])
        .setup(|app| {
            let location = storage_location::StorageLocation::new(app.path().app_data_dir()?)?;
            let credentials = encrypted_store::open(app.handle(), location.default_root())?;
            let root = location.startup_root()?;
            let paths = storage::AppPaths::new(root)?.with_control_root(location.default_root());
            // Keep executables outside relocatable game data and Agent-accessible roots.
            let runtime = mcp::McpRuntime::bundled(
                app.path().resource_dir()?.join("mcp-runtime/runtime.zip"),
                location.default_root().join("private/mcp-runtime"),
            );
            let services = app::AppServices::initialize_paths(paths, runtime)?;
            app.manage(location);
            let handle = app.handle().clone();
            let agent = agent::runtime::AgentRuntime::new(
                services.engine.clone(),
                settings::Settings::with_secret_store(services.engine.db.clone(), credentials),
                std::sync::Arc::new(move |event| {
                    use tauri::Emitter;
                    let _ = handle.emit(&format!("agent://{}", event.kind), event);
                }),
            )?;
            let local = mcp_server::LocalMcpServer::new(agent.clone());
            let connections = services.mcp_connections.clone();
            app.manage(local.clone());
            app.manage(agent.clone());
            app.manage(services);
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use tauri::Emitter;
                let progress = || {
                    let _ = handle.emit("mcp://restore-progress", ());
                };
                if let Err(error) = local.restore_if_requested(&connections).await {
                    tracing::warn!(%error, "could not restore local MCP server");
                }
                progress();
                connections
                    .restore_requested(&agent.settings, &agent.registry, progress)
                    .await;
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run Agent Mod Manager");
}
