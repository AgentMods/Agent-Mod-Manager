pub mod agent;
mod agent_attachment;
mod agent_notifications;
mod analytics;
pub mod app;
pub mod app_locale;
mod app_update;
pub mod archive;
pub mod artifacts;
pub(crate) mod browse;
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
pub mod manager_catalog;
#[cfg(test)]
mod manager_plugin_roundtrip_tests;
pub mod manager_publication;
pub mod mcp;
pub mod mcp_connections;
pub mod mcp_server;
pub mod mod_language;
pub mod mod_library;
mod mod_management;
pub mod mod_organization;
mod mod_organization_fallback;
mod mod_organization_jobs;
mod mod_organizer;
pub mod paths;
pub mod plugin_publish;
pub mod plugin_publish_remote;
pub mod plugin_workflow;
pub mod plugins;
pub(crate) mod publication_copy;
mod safe_fs;
pub mod settings;
mod seven_zip;
#[cfg(test)]
mod seven_zip_roundtrip_tests;
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
    let mut normal_exit = true;
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .manage(agent_attachment::AttachmentConversions::default())
        .manage(browse::BrowseState::default())
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                if let Some(browse) = window.try_state::<browse::BrowseState>() {
                    browse.cancel_window(window.label());
                }
                if let Some(conversions) =
                    window.try_state::<agent_attachment::AttachmentConversions>()
                {
                    conversions.cancel_window(window.label());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_bootstrap_state,
            browse::browse_list_sources,
            browse::browse_authorize,
            browse::browse_start_search,
            browse::browse_get_page,
            browse::browse_select_game_match,
            browse::browse_open_project,
            browse::browse_cancel_search,
            browse::install::browse_install_task,
            browse::install::start_browse_install,
            browse::install::list_browse_install_tasks,
            commands::get_app_locale,
            commands::set_app_locale,
            app_update::get_app_update_status,
            app_update::get_app_update_route,
            app_update::set_app_update_route,
            app_update::check_app_update,
            commands::migrate_storage,
            commands::start_onboarding,
            commands::set_onboarding_completed,
            commands::list_plugins,
            commands::get_game_adapter,
            commands::plan_import_archive,
            commands::register_imported_mod,
            commands::list_mod_library,
            commands::remove_managed_mod,
            commands::bind_mod_library_entry,
            commands::preview_fomod,
            commands::set_plugin_enabled,
            commands::remove_installed_plugin,
            commands::write_plugin_draft,
            commands::list_plugin_drafts,
            commands::delete_plugin_draft,
            commands::validate_plugin,
            commands::install_plugin,
            commands::list_manager_plugin_games,
            commands::preview_manager_plugin,
            commands::get_manager_plugin_display,
            commands::prepare_manager_plugin_import,
            commands::install_manager_plugin,
            commands::list_manager_publications,
            commands::get_manager_publication,
            commands::prepare_auto_manager_publication,
            commands::publish_manager_publication,
            commands::reconcile_manager_publication,
            commands::list_my_manager_plugins,
            commands::list_plugin_publications,
            commands::get_plugin_publication,
            commands::reconcile_plugin_publication,
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
            commands::list_game_launch_options,
            commands::set_game_mod_launcher,
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
            commands::list_mod_tags,
            commands::save_mod_tag,
            commands::delete_mod_tag,
            commands::set_mod_tags,
            commands::inspect_install,
            commands::check_modrinth_dependencies,
            commands::review_modrinth_dependency_graph,
            commands::plan_uninstall,
            commands::rollback_transaction,
            agent_attachment::begin_agent_attachment,
            agent_attachment::convert_agent_attachment,
            agent_attachment::cancel_agent_attachment,
            commands::start_turn,
            commands::restart_turn,
            commands::cancel_turn,
            commands::respond_approval,
            commands::replay_agent_events,
            commands::create_conversation,
            commands::list_conversations,
            commands::get_conversation,
            commands::set_conversation_workspace,
            commands::archive_conversation,
            commands::rename_conversation,
            commands::set_conversation_pinned,
            commands::delete_conversation,
            mod_language::get_mod_organization_locale,
            mod_language::set_mod_organization_locale,
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
            commands::cancel_modrinth_dependency_graph,
            commands::read_logs,
            commands::diagnose_game,
            commands::mcp_list,
            commands::mcp_credential_status,
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
            seven_zip::set_resource_dir(app.path().resource_dir()?);
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
            let agent = agent::runtime::AgentRuntime::with_runtime(
                services.engine.clone(),
                settings::Settings::with_secret_store(services.engine.db.clone(), credentials),
                services.runtime.clone(),
                std::sync::Arc::new(move |event| {
                    use tauri::Emitter;
                    agent_notifications::notify(&handle, event);
                    let _ = handle.emit(&format!("agent://{}", event.kind), event);
                }),
            )?;
            let local = mcp_server::LocalMcpServer::new(agent.clone());
            let connections = services.mcp_connections.clone();
            {
                use tauri::Emitter;
                let connections = connections.clone();
                let registry = agent.registry.clone();
                let servers = mcp::McpServers::new(services.engine.db.clone());
                let handle = app.handle().clone();
                services.engine.set_plugin_mcp_change_handler(move |slug| {
                    connections.invalidate_plugin(&registry, &servers, slug);
                    // Settings uses the same refetch path for restoration and revocation.
                    let _ = handle.emit("mcp://restore-progress", ());
                });
            }
            app.manage(local.clone());
            app.manage(agent.clone());
            if services.recovery_error.is_none() {
                let engine = services.engine.clone();
                let events = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    use tauri::Emitter;
                    if let Err(error) =
                        manager_catalog::update_installed_at_startup(&engine, |slug| {
                            let _ = events.emit("plugin://catalog-updated", slug);
                        })
                        .await
                    {
                        tracing::warn!(%error, "could not check for catalog adapter updates");
                    }
                });
                let ui = app.handle().clone();
                let organizer = mod_organizer::start(
                    agent.settings.clone(),
                    services.engine.organization_wake.clone(),
                    move |game, install| {
                        use tauri::Emitter;
                        let _ = ui.emit(
                            "mod://organized",
                            serde_json::json!({"gameId": game, "installId": install}),
                        );
                    },
                );
                app.manage(organizer);
            }
            let update_db = services.engine.db.clone();
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
            analytics::start(app.package_info().version.to_string());
            app_update::setup(app.handle(), update_db);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build Agent Mod Manager")
        .run(move |app, event| match event {
            tauri::RunEvent::ExitRequested { code, .. } => {
                // In particular, storage migration requests a Tauri restart.
                normal_exit = code.is_none_or(|code| code == 0);
            }
            tauri::RunEvent::Exit => {
                let staged = app_update::take_at_exit(app, normal_exit);
                analytics::finish();
                if let Some(staged) = staged {
                    staged.install_at_exit();
                }
            }
            _ => {}
        });
}
