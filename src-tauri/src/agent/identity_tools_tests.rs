use super::*;
use crate::{agent::policy::PermissionMode, transaction::tests::setup};
use tokio_util::sync::CancellationToken;

fn fixture() -> (tempfile::TempDir, ToolContext, IdentityTool, Value) {
    let (temp, engine, game) = setup();
    std::fs::write(game.join("FixtureGame.exe"), b"inert fixture").unwrap();
    let args = json!({"directory":game,"public_query":"FixtureGame game executable"});
    (
        temp,
        ToolContext {
            engine,
            cancel: CancellationToken::new(),
            progress: None,
            resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
        },
        IdentityTool {
            runtime: McpRuntime::default(),
            search: true,
        },
        args,
    )
}

#[tokio::test]
async fn search_requires_review_and_does_not_register_the_root() {
    let (_temp, context, tool, args) = fixture();
    let registry = ToolRegistry::default();
    register(&registry, tool.runtime).unwrap();
    let tool = registry.get("search_game_identity").unwrap();
    assert!(registry.get("confirm_game_identity").is_err());
    assert_eq!(
        registry
            .get("preview_game_identity")
            .unwrap()
            .definition()
            .effect,
        Effect::Read
    );
    for mode in [PermissionMode::RequestApproval, PermissionMode::AutoApprove] {
        assert!(mode.needs_approval(tool.definition().effect));
    }
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert_eq!(prepared.effect, Effect::Publish);
    assert!(prepared.plan.is_none());
    assert_eq!(prepared.preview["provider"], "bing");
    assert_eq!(
        prepared.preview["destination"],
        "https://www.bing.com/search"
    );
    assert_eq!(prepared.preview["arguments"], args);
    assert_eq!(prepared.preview["untrustedResults"], true);
    assert!(prepared.preview["fingerprint"]
        .as_str()
        .is_some_and(|value| value.len() == 64));
    let result = tool.execute(&context, args, prepared).await.unwrap();
    assert_eq!(result["status"], "needs_configuration");
    assert_eq!(result["provider"], "bing");
    assert_eq!(result["candidates"], json!([]));
    assert_eq!(
        context
            .engine
            .db
            .conn()
            .query_row("SELECT COUNT(*) FROM games", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn invalid_fields_private_queries_and_cancellation_never_store_evidence() {
    let (_temp, context, tool, args) = fixture();
    for change in [
        json!({"unknown":true}),
        json!({"public_query":"/Users/alice/private.txt"}),
        json!({"public_query":"x".repeat(513)}),
        json!({"confirmation":"SEARCH_GAME_IDENTITY"}),
    ] {
        let mut invalid = args.clone();
        invalid
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        assert!(tool.prepare(&context, &invalid).await.is_err());
    }
    let prepared = tool.prepare(&context, &args).await.unwrap();
    context.cancel.cancel();
    assert!(matches!(
        tool.execute(&context, args.clone(), prepared).await,
        Err(AppError::Cancelled)
    ));
    assert!(matches!(
        tool.prepare(&context, &args).await,
        Err(AppError::Cancelled)
    ));
    assert_eq!(
        context
            .engine
            .db
            .conn()
            .query_row("SELECT COUNT(*) FROM game_identity_searches", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn changed_directory_arguments_or_approval_metadata_cannot_reuse_search_approval() {
    let (_temp, context, tool, args) = fixture();
    let prepared = tool.prepare(&context, &args).await.unwrap();
    let mut changed = args.clone();
    changed["public_query"] = json!("Different public game");
    assert!(matches!(
        tool.execute(&context, changed, prepared).await,
        Err(AppError::Denied(_))
    ));
    let mut prepared = tool.prepare(&context, &args).await.unwrap();
    prepared.effect = Effect::Read;
    assert!(matches!(
        tool.execute(&context, args.clone(), prepared).await,
        Err(AppError::Denied(_))
    ));
    for (field, value) in [
        ("provider", json!("brave")),
        (
            "destination",
            json!("https://api.search.brave.com/res/v1/web/search"),
        ),
        ("untrustedResults", json!(false)),
    ] {
        let mut prepared = tool.prepare(&context, &args).await.unwrap();
        prepared.preview[field] = value;
        assert!(matches!(
            tool.execute(&context, args.clone(), prepared).await,
            Err(AppError::Denied(_))
        ));
    }
    let prepared = tool.prepare(&context, &args).await.unwrap();
    std::fs::write(
        PathBuf::from(args["directory"].as_str().unwrap()).join("new-file.txt"),
        b"changed",
    )
    .unwrap();
    assert!(tool.execute(&context, args, prepared).await.is_err());
}

#[tokio::test]
async fn local_preview_never_searches_or_grants_identity_and_rejects_extra_fields() {
    let (_temp, context, mut tool, args) = fixture();
    tool.search = false;
    assert!(tool.prepare(&context, &args).await.is_err());
    let args = json!({"directory":args["directory"]});
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert_eq!(prepared.effect, Effect::Read);
    let result = tool.execute(&context, args, prepared).await.unwrap();
    assert_eq!(result["localCandidate"]["requiresConfirmation"], true);
}
