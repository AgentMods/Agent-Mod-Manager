use super::*;
use crate::{
    db::Db,
    error::{AppError, AppResult},
    settings::SecretStore,
    storage::AppPaths,
    transaction::FileEngine,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct NoSecrets;

impl SecretStore for NoSecrets {
    fn read(&self, _: &str) -> AppResult<Option<String>> {
        Ok(None)
    }

    fn write(&self, _: &str, _: &str) -> AppResult<()> {
        Ok(())
    }
}

fn fixture() -> (tempfile::TempDir, Settings, ToolContext) {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(dir.path().join("data")).unwrap();
    let db = Db::open(&paths.db_file()).unwrap();
    let settings = Settings::with_secret_store(db.clone(), Arc::new(NoSecrets));
    let context = ToolContext {
        engine: FileEngine::new(db, paths),
        cancel: CancellationToken::new(),
        progress: None,
        resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
    };
    (dir, settings, context)
}

#[tokio::test]
async fn registers_read_only_public_query_tool_and_reports_missing_configuration() {
    let (_dir, settings, context) = fixture();
    let registry = ToolRegistry::default();
    register(&registry, settings).unwrap();
    let tool = registry.get("web_search").unwrap();
    let definition = tool.definition();
    assert_eq!(definition.effect, Effect::Read);
    assert_eq!(definition.parameters["additionalProperties"], false);
    assert_eq!(definition.parameters["properties"]["query"]["minLength"], 1);
    assert_eq!(
        definition.parameters["properties"]["query"]["maxLength"],
        512
    );
    assert_eq!(definition.parameters["properties"]["count"]["minimum"], 1);
    assert_eq!(definition.parameters["properties"]["count"]["maximum"], 10);
    assert!(definition.description.contains("untrusted"));
    assert!(definition.description.contains("never upload local paths"));

    let args = json!({"query":"public mod info", "count":3});
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert_eq!(prepared.effect, Effect::Read);
    assert_eq!(prepared.preview["provider"], "brave");
    assert_eq!(prepared.preview["count"], 3);
    assert_eq!(prepared.preview["untrustedResults"], true);
    assert!(prepared.preview.get("query").is_none());

    let result = tool.execute(&context, args, prepared).await.unwrap();
    assert_eq!(result["provider"], "brave");
    assert_eq!(result["status"], "needs_configuration");
    assert_eq!(result["results"], json!([]));
}

#[tokio::test]
async fn rejects_unknown_or_invalid_query_fields_and_obeys_cancellation() {
    let (_dir, settings, context) = fixture();
    let tool = WebSearchTool { settings };

    for args in [
        json!({"query":"public terms", "extra":"must reject"}),
        json!({"query":"public terms", "count":0}),
        json!({"query":"/Users/alice/private.txt"}),
        json!({"query":"x".repeat(web_search::MAX_QUERY_CHARS + 1)}),
    ] {
        assert!(matches!(
            tool.prepare(&context, &args).await,
            Err(AppError::Invalid(_) | AppError::Denied(_))
        ));
    }

    let cancelled = context;
    cancelled.cancel.cancel();
    assert!(matches!(
        tool.prepare(&cancelled, &json!({"query":"public terms"}))
            .await,
        Err(AppError::Cancelled)
    ));
}
