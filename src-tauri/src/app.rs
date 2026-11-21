use crate::{db::Db, error::AppResult, plugins, storage::AppPaths, transaction::FileEngine};
use std::path::PathBuf;
use tracing_subscriber::prelude::*;

fn log_filter() -> tracing_subscriber::EnvFilter {
    // Remote libraries may log payloads before our credential redactor runs.
    tracing_subscriber::EnvFilter::new("info,rmcp=off,reqwest=off,reqwest_mcp=off,hyper=off,h2=off")
}

pub struct AppServices {
    pub engine: FileEngine,
    pub recovery_error: Option<String>,
    pub recovered: Vec<String>,
    pub(crate) runtime: crate::mcp::McpRuntime,
    pub mcp_connections: crate::mcp_connections::McpConnections,
    pub downloads: crate::download_tasks::DownloadTasks,
    _log_guard: Option<tracing_appender::non_blocking::WorkerGuard>,
}

impl AppServices {
    pub fn initialize(root: PathBuf) -> AppResult<Self> {
        Self::initialize_paths(AppPaths::new(root)?, Default::default())
    }

    pub(crate) fn initialize_paths(
        paths: AppPaths,
        runtime: crate::mcp::McpRuntime,
    ) -> AppResult<Self> {
        let file = tracing_appender::rolling::daily(paths.logs(), "manager.log");
        let (writer, guard) = tracing_appender::non_blocking(file);
        let logging = tracing_subscriber::registry()
            .with(log_filter())
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(writer),
            )
            .try_init()
            .is_ok();
        let db = Db::open(&paths.db_file())?;
        let engine = FileEngine::new(db, paths);
        crate::plugin_publish::recover_pending_publications(&engine)?;
        crate::manager_publication::recover_pending_publications(&engine)?;
        let (recovered, recovery_error) = match engine.recover_interrupted() {
            Ok(ids) => (ids, None),
            Err(error) => {
                tracing::error!(%error,"transaction recovery requires attention");
                (Vec::new(), Some(error.to_string()))
            }
        };
        plugins::retire_builtin_adapters(&engine.db, &engine.paths)?;
        let plugin_report = crate::plugin_workflow::reconcile_deployments(&engine)?;
        for issue in plugin_report.issues {
            tracing::warn!(
                code = issue.code,
                message = issue.message,
                "plugin reconciliation"
            );
        }
        let skill_report = crate::skills::reconcile_deployments(&engine)?;
        for issue in skill_report.issues {
            tracing::warn!(
                name = issue.name,
                message = issue.message,
                "skill reconciliation"
            );
        }
        crate::install::reconcile(&engine.db)?;
        crate::mcp::McpServers::new(engine.db.clone()).seed_presets()?;
        Ok(Self {
            engine,
            recovery_error,
            recovered,
            runtime: runtime.clone(),
            mcp_connections: crate::mcp_connections::McpConnections::with_runtime(runtime),
            downloads: Default::default(),
            _log_guard: if logging { Some(guard) } else { None },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_library_payload_logging_is_disabled() {
        let subscriber = tracing_subscriber::registry().with(log_filter());
        tracing::subscriber::with_default(subscriber, || {
            assert!(!tracing::enabled!(target: "rmcp::transport", tracing::Level::ERROR));
            assert!(!tracing::enabled!(target: "reqwest", tracing::Level::WARN));
            assert!(!tracing::enabled!(target: "hyper::client", tracing::Level::INFO));
            assert!(tracing::enabled!(target: "agent_mod_manager_lib", tracing::Level::INFO));
        });
    }
    #[test]
    fn new_and_reopened_library_have_no_adapters() {
        let dir = tempfile::tempdir().unwrap();
        let app = AppServices::initialize(dir.path().join("data")).unwrap();
        assert!(app.recovery_error.is_none());
        assert!(plugins::list_plugins(&app.engine.db).unwrap().is_empty());
        assert!(!app.engine.paths.plugins().join("games").exists());
        drop(app);
        let app = AppServices::initialize(dir.path().join("data")).unwrap();
        assert!(plugins::list_plugins(&app.engine.db).unwrap().is_empty());
        assert!(!app.engine.paths.plugins().join("games").exists());
    }

    #[test]
    fn upgrade_retires_only_builtin_adapters_and_allows_reinstall_under_the_same_slug() {
        use crate::{games, plugin_workflow, plugins::test_fixtures};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("data");
        let paths = AppPaths::new(root.clone()).unwrap();
        let db = Db::open(&paths.db_file()).unwrap();
        test_fixtures::install(&db, &paths).unwrap();
        // Model an old profile with two bundled adapters (one missing on disk)
        // and a separately installed adapter bound to another game.
        db.conn().execute_batch("
            UPDATE plugins SET source='builtin' WHERE slug IN ('minecraft-java','fallout4');
            UPDATE skills SET source='builtin' WHERE name IN ('minecraft-java','fallout4');
            UPDATE plugins SET enabled=0 WHERE slug='minecraft-java';
            INSERT INTO games(id,name,slug,root_path,adapter_slug,adapter_version,created_at,updated_at)
                VALUES ('old','My game','minecraft-java','old-game-root','minecraft-java','1','then','then'),
                       ('keep','External','skyrim-se','other-game-root','skyrim-se','2','then','then');
            INSERT INTO installs(id,game_id,name,installed_at,updated_at)
                VALUES ('saved-mod','old','My Mod','then','then');
            INSERT INTO mcp_servers(id,name,transport,created_at,plugin_slug)
                VALUES ('old-server','Old','http','then','minecraft-java');
        ").unwrap();
        let packages = paths.plugins().join("games");
        let old_dir = packages.join("minecraft-java");
        std::fs::write(old_dir.join("local-notes.txt"), b"keep me").unwrap();
        std::fs::rename(packages.join("fallout4"), temp.path().join("already-moved")).unwrap();
        drop(db);

        let app = AppServices::initialize(root.clone()).unwrap();
        let db = &app.engine.db;
        let game = games::get_game(db, "old").unwrap();
        assert!(game.adapter_slug.is_none() && game.adapter_version.is_none());
        assert_eq!(game.installed_mod_count, 1);
        assert_eq!(
            games::get_game(db, "keep").unwrap().adapter_slug.as_deref(),
            Some("skyrim-se")
        );
        assert!(plugins::list_plugins(db)
            .unwrap()
            .iter()
            .all(|p| p.source == "external"));
        assert!(plugins::adapter_for(db, "skyrim-se").is_ok());
        let registrations_gone: bool = db.conn().query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM skills WHERE name IN ('minecraft-java','fallout4'))
                AND NOT EXISTS(SELECT 1 FROM mcp_servers WHERE id='old-server')",
            [], |r| r.get(0),
        ).unwrap();
        assert!(registrations_gone);
        assert!(!old_dir.exists());
        let backups = paths.backups().join("retired-adapters");
        let saved: Vec<_> = backups
            .read_dir()
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(saved.len(), 1);
        assert_eq!(
            std::fs::read(saved[0].join("local-notes.txt")).unwrap(),
            b"keep me"
        );

        let (slug, json, markdown) = test_fixtures::PACKAGES[0];
        let draft = plugin_workflow::plan_write_draft(&app.engine, slug, json, markdown).unwrap();
        app.engine.approve_plan(&draft.plan.id).unwrap();
        app.engine.apply_file_ops(&draft.plan.id).unwrap();
        let deploy = plugin_workflow::plan_install_plugin(&app.engine, &draft.draft.id).unwrap();
        app.engine.approve_plan(&deploy.plan.id).unwrap();
        app.engine.apply_file_ops(&deploy.plan.id).unwrap();
        drop(app);
        let app = AppServices::initialize(root).unwrap();
        assert_eq!(
            plugin_workflow::read_plugin_package(&app.engine, slug)
                .unwrap()
                .slug,
            slug
        );
        assert_eq!(backups.read_dir().unwrap().count(), 1);
    }
}
