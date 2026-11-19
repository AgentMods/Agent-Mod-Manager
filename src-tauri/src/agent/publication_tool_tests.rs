use super::*;
use crate::{
    agent::policy::PermissionMode, plugin_workflow, plugins, settings::SecretStore,
    transaction::tests::setup,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct NoRemoteSecrets(AtomicUsize);
impl SecretStore for NoRemoteSecrets {
    fn read(&self, _: &str) -> AppResult<Option<String>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("No credential or network access is allowed in these tests");
    }
    fn write(&self, _: &str, _: &str) -> AppResult<()> {
        unreachable!()
    }
}

fn fixture() -> (
    tempfile::TempDir,
    ToolContext,
    PublishPlugin,
    Value,
    Arc<NoRemoteSecrets>,
    std::path::PathBuf,
) {
    let (temp, engine, _) = setup();
    let mut adapter: Value = serde_json::from_str(plugins::BUILTIN_PACKAGES[0].1).unwrap();
    adapter["slug"] = json!("publication-fixture");
    let draft = plugin_workflow::plan_write_draft(&engine, "publication-fixture", &adapter.to_string(), "---\nname: publication-fixture\ndescription: Public fixture\n---\nReview before applying.\n").unwrap();
    engine.approve_plan(&draft.plan.id).unwrap();
    engine.apply_file_ops(&draft.plan.id).unwrap();
    let secrets = Arc::new(NoRemoteSecrets::default());
    let settings = Settings::with_secret_store(engine.db.clone(), secrets.clone());
    let args = json!({"draft_id":draft.draft.id,"metadata":{"title":"Public fixture","summary":"Summary","description":"Description","tags":["adapter"],"gameIds":[]}});
    (
        temp,
        ToolContext {
            engine,
            cancel: CancellationToken::new(),
            progress: None,
            resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
        },
        PublishPlugin { settings },
        args,
        secrets,
        draft.draft.path,
    )
}

#[tokio::test]
async fn preparation_exposes_exact_files_but_never_approves_or_reads_credentials() {
    let (_temp, context, tool, args, secrets, _) = fixture();
    assert!(PermissionMode::RequestApproval.needs_approval(tool.definition().effect));
    assert!(PermissionMode::AutoApprove.needs_approval(tool.definition().effect));
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert_eq!(prepared.effect, Effect::Publish);
    assert!(prepared.plan.is_none());
    let record: plugin_publish::PublicationRecord =
        serde_json::from_value(prepared.preview).unwrap();
    assert_eq!(record.preview.package.files.len(), 2);
    assert!(record
        .preview
        .package
        .files
        .iter()
        .any(|file| file.name == "SKILL.md" && file.text.contains("Review before applying.")));
    assert_eq!(
        record.preview.package.destination,
        plugin_publish::DESTINATION
    );
    assert!(record.approved_digest.is_none());
    assert!(record.file_id.is_none());
    assert_eq!(secrets.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancellation_before_execute_never_grants_publication_consent() {
    let (_temp, context, tool, args, secrets, _) = fixture();
    let prepared = tool.prepare(&context, &args).await.unwrap();
    let id = prepared.preview["id"].as_str().unwrap().to_owned();
    context.cancel.cancel();
    assert!(matches!(
        tool.execute(&context, args.clone(), prepared).await,
        Err(AppError::Cancelled)
    ));
    assert!(matches!(
        tool.prepare(&context, &args).await,
        Err(AppError::Cancelled)
    ));
    assert!(plugin_publish::get_publication(&context.engine, &id)
        .unwrap()
        .approved_digest
        .is_none());
    assert_eq!(secrets.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn changed_source_between_preview_and_approval_is_rejected_before_network() {
    let (_temp, context, tool, args, secrets, path) = fixture();
    let prepared = tool.prepare(&context, &args).await.unwrap();
    let id = prepared.preview["id"].as_str().unwrap().to_owned();
    std::fs::write(
        std::path::Path::new(&path).join("SKILL.md"),
        "changed content",
    )
    .unwrap();
    assert!(tool.execute(&context, args, prepared).await.is_err());
    assert!(plugin_publish::get_publication(&context.engine, &id)
        .unwrap()
        .approved_digest
        .is_none());
    assert_eq!(secrets.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_arguments_and_prepared_effect_cannot_bypass_publish_review() {
    let (_temp, context, tool, args, secrets, _) = fixture();
    for invalid in [
        json!({"draft_id":args["draft_id"],"metadata":args["metadata"],"upload_url":"https://elsewhere.invalid"}),
        json!({"draft_id":12,"metadata":args["metadata"]}),
    ] {
        assert!(tool.prepare(&context, &invalid).await.is_err());
    }
    let mut prepared = tool.prepare(&context, &args).await.unwrap();
    prepared.effect = Effect::Read;
    assert!(matches!(
        tool.execute(&context, args, prepared).await,
        Err(AppError::Denied(_))
    ));
    assert_eq!(secrets.0.load(Ordering::SeqCst), 0);
}
