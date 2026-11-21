use super::*;
use crate::{
    agent::{policy::PermissionMode, tests::install_adapter_package},
    plugins,
    settings::SecretStore,
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
    let draft = install_adapter_package(&engine, plugins::test_fixtures::PACKAGES[0]);
    let secrets = Arc::new(NoRemoteSecrets::default());
    let settings = Settings::with_secret_store(engine.db.clone(), secrets.clone());
    let args = json!({"draft_id":draft.id,"tags":["adapter"]});
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
        draft.path,
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
    let record: manager_publication::PublicationRecord =
        serde_json::from_value(prepared.preview).unwrap();
    assert_eq!(record.preview.files.len(), 2);
    assert!(record
        .preview
        .files
        .iter()
        .any(|file| file.name == "SKILL.md" && file.text == plugins::test_fixtures::PACKAGES[0].2));
    assert_eq!(
        record.preview.destination,
        crate::plugin_publish::DESTINATION
    );
    assert!(record.approved_digest.is_none());
    assert!(record.remote.is_none());
    assert_eq!(secrets.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn installed_package_and_explicit_release_are_bound_to_the_approval() {
    let (_temp, context, tool, draft, secrets, _) = fixture();
    let slug = plugins::test_fixtures::PACKAGES[0].0;
    let args = json!({"slug":slug,"plugin_id":42,"tags":["适配"]});
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert_eq!(
        prepared.preview["preview"]["target"],
        json!({"kind":"release","pluginId":42})
    );
    assert_eq!(prepared.preview["preview"]["slug"], slug);
    let mut ambiguous = args;
    ambiguous["draft_id"] = draft["draft_id"].clone();
    assert!(tool.prepare(&context, &ambiguous).await.is_err());
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
    assert!(manager_publication::get_publication(&context.engine, &id)
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
    assert!(manager_publication::get_publication(&context.engine, &id)
        .unwrap()
        .approved_digest
        .is_none());
    assert_eq!(secrets.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_arguments_and_prepared_effect_cannot_bypass_publish_review() {
    let (_temp, context, tool, args, secrets, _) = fixture();
    for invalid in [
        json!({"draft_id":args["draft_id"],"tags":args["tags"],"upload_url":"https://elsewhere.invalid"}),
        json!({"draft_id":12,"tags":args["tags"]}),
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
