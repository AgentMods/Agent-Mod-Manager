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
        let (recovered, recovery_error) = match engine.recover_interrupted() {
            Ok(ids) => (ids, None),
            Err(error) => {
                tracing::error!(%error,"transaction recovery requires attention");
                (Vec::new(), Some(error.to_string()))
            }
        };
        plugins::seed_builtins(&engine.db, &engine.paths)?;
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
    fn initialization_seeds_plugins_and_can_reopen_database() {
        let dir = tempfile::tempdir().unwrap();
        let app = AppServices::initialize(dir.path().join("data")).unwrap();
        assert!(app.recovery_error.is_none());
        assert_eq!(plugins::list_plugins(&app.engine.db).unwrap().len(), 6);
        drop(app);
        let app = AppServices::initialize(dir.path().join("data")).unwrap();
        assert_eq!(plugins::list_plugins(&app.engine.db).unwrap().len(), 6);
    }
}
