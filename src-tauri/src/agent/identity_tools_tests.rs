use super::*;
use crate::{agent::policy::PermissionMode, settings::SecretStore, transaction::tests::setup};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct NoSecrets(AtomicUsize);
impl SecretStore for NoSecrets {
    fn read(&self, _: &str) -> AppResult<Option<String>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
    fn write(&self, _: &str, _: &str) -> AppResult<()> {
        Ok(())
    }
}

fn fixture() -> (
    tempfile::TempDir,
    ToolContext,
    IdentityTool,
    Value,
    Arc<NoSecrets>,
) {
    let (temp, engine, game) = setup();
    std::fs::write(game.join("FixtureGame.exe"), b"inert fixture").unwrap();
    let secrets = Arc::new(NoSecrets::default());
    let settings = Settings::with_secret_store(engine.db.clone(), secrets.clone());
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
            settings,
            search: true,
        },
        args,
        secrets,
    )
}

#[tokio::test]
async fn search_preparation_requires_review_without_credentials_or_root_registration() {
    let (_temp, context, tool, args, secrets) = fixture();
    let registry = ToolRegistry::default();
    register(&registry, tool.settings.clone()).unwrap();
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
    assert_eq!(prepared.preview["destination"], web_search::ENDPOINT);
    assert_eq!(prepared.preview["arguments"], args);
    assert_eq!(prepared.preview["untrustedResults"], true);
    assert!(prepared.preview["fingerprint"]
        .as_str()
        .is_some_and(|value| value.len() == 64));
    assert_eq!(secrets.0.load(Ordering::SeqCst), 0);
    let result = tool.execute(&context, args, prepared).await.unwrap();
    assert_eq!(result["status"], "needs_configuration");
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
async fn invalid_fields_private_queries_and_cancellation_never_read_credentials() {
    let (_temp, context, tool, args, secrets) = fixture();
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
    assert_eq!(secrets.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stale_directory_changed_arguments_and_effect_cannot_reuse_search_approval() {
    let (_temp, context, tool, args, secrets) = fixture();
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
    let prepared = tool.prepare(&context, &args).await.unwrap();
    std::fs::write(
        PathBuf::from(args["directory"].as_str().unwrap()).join("new-file.txt"),
        b"changed",
    )
    .unwrap();
    assert!(tool.execute(&context, args, prepared).await.is_err());
    assert_eq!(secrets.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn local_preview_never_searches_or_grants_identity_and_rejects_extra_fields() {
    let (_temp, context, mut tool, args, secrets) = fixture();
    tool.search = false;
    assert!(tool.prepare(&context, &args).await.is_err());
    let args = json!({"directory":args["directory"]});
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert_eq!(prepared.effect, Effect::Read);
    let result = tool.execute(&context, args, prepared).await.unwrap();
    assert_eq!(result["localCandidate"]["requiresConfirmation"], true);
    assert_eq!(secrets.0.load(Ordering::SeqCst), 0);
}
