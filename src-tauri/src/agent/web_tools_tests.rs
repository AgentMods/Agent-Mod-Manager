use super::*;
use crate::{db::Db, storage::AppPaths, transaction::FileEngine};
use tokio_util::sync::CancellationToken;

fn fixture() -> (tempfile::TempDir, ToolContext) {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(dir.path().join("data")).unwrap();
    let db = Db::open(&paths.db_file()).unwrap();
    let context = ToolContext {
        engine: FileEngine::new(db, paths),
        cancel: CancellationToken::new(),
        progress: None,
        resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
    };
    (dir, context)
}

#[tokio::test]
async fn registers_read_only_public_query_tool_and_reports_missing_configuration() {
    let (_dir, context) = fixture();
    let registry = ToolRegistry::default();
    register(&registry, McpRuntime::default()).unwrap();
    let tool = registry.get("web_search").unwrap();
    let definition = tool.definition();
    assert_eq!(definition.effect, Effect::Read);
    assert_eq!(definition.parameters["additionalProperties"], false);
    assert_eq!(definition.parameters["properties"]["query"]["minLength"], 1);
    assert!(definition.parameters["properties"]["query"]
        .get("maxLength")
        .is_none());
    assert_eq!(definition.parameters["properties"]["count"]["minimum"], 1);
    assert!(definition.parameters["properties"]["count"]
        .get("maximum")
        .is_none());
    assert_eq!(
        definition.parameters["properties"]["cursor"]["format"],
        "uuid"
    );
    assert!(definition.description.contains("untrusted"));
    assert!(definition.description.contains("never upload local paths"));

    let args = json!({"query":"public mod info", "count":3});
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert_eq!(prepared.effect, Effect::Read);
    assert_eq!(prepared.preview["provider"], "bing");
    assert_eq!(prepared.preview["count"], 3);
    assert_eq!(prepared.preview["untrustedResults"], true);
    assert!(prepared.preview.get("query").is_none());

    let result = tool.execute(&context, args, prepared).await.unwrap();
    assert_eq!(result["provider"], "bing");
    assert_eq!(result["status"], "needs_configuration");
    assert_eq!(result["results"], json!([]));
}

#[tokio::test]
async fn rejects_unknown_or_invalid_query_fields_and_obeys_cancellation() {
    let (_dir, context) = fixture();
    let tool = WebSearchTool {
        runtime: McpRuntime::default(),
        sessions: web_search::SearchSessions::default(),
    };

    for args in [
        json!({"query":"public terms", "extra":"must reject"}),
        json!({"query":"public terms", "count":0}),
        json!({"query":"/Users/alice/private.txt"}),
        json!({"query":"public terms", "cursor":"https://www.bing.com/search"}),
    ] {
        assert!(matches!(
            tool.prepare(&context, &args).await,
            Err(AppError::Invalid(_) | AppError::Denied(_))
        ));
    }

    let args = json!({"query":"public description ".repeat(100), "count":1000});
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert_eq!(prepared.preview["count"], 1000);
    assert_eq!(prepared.preview["continuation"], false);
    let bogus = json!({"query":"public terms", "cursor":Uuid::new_v4()});
    let prepared = tool.prepare(&context, &bogus).await.unwrap();
    assert_eq!(prepared.preview["continuation"], true);
    assert!(matches!(
        tool.execute(&context, bogus, prepared).await,
        Err(AppError::NotFound(_))
    ));

    let cancelled = context;
    cancelled.cancel.cancel();
    assert!(matches!(
        tool.prepare(&cancelled, &json!({"query":"public terms"}))
            .await,
        Err(AppError::Cancelled)
    ));
}
