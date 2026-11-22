use super::*;
use crate::{
    agent::{tests::install_adapter_package, tools::AgentTool},
    error::{AppError, AppResult},
    manager_publication::{self, PublishedPlugin, Remote},
    plugins,
    publication_copy::PublicationListing,
    settings::SecretStore,
    transaction::{tests::setup, FileEngine},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio_util::sync::CancellationToken;

const TEST_MANAGER_KEY: &str = "offline-manager-publication-key";

#[derive(Default)]
struct FixtureSecrets(AtomicUsize);
impl SecretStore for FixtureSecrets {
    fn read(&self, key: &str) -> AppResult<Option<String>> {
        assert_eq!(key, "mcp.glossmod_http.api_key");
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Some(TEST_MANAGER_KEY.into()))
    }

    fn write(&self, key: &str, value: &str) -> AppResult<()> {
        assert_eq!(key, "mcp.glossmod_http.api_key");
        assert_eq!(value, TEST_MANAGER_KEY);
        Ok(())
    }
}

#[derive(Default)]
struct FixtureRemote {
    owned: Vec<PublishedPlugin>,
    calls: Mutex<Vec<(String, Value)>>,
}

#[async_trait::async_trait]
impl Remote for FixtureRemote {
    async fn call(&self, name: &str, args: Value, _: &CancellationToken) -> AppResult<Value> {
        self.calls.lock().unwrap().push((name.to_owned(), args));
        if name != "list_my_manager_plugins" {
            return Err(AppError::Denied(
                "fixture only permits owner listing".into(),
            ));
        }
        Ok(json!({"items":self.owned}))
    }
}

fn generated_listing() -> PublicationListing {
    let (slug, adapter, skill) = plugins::test_fixtures::PACKAGES[0];
    let package = plugins::parse_package(slug, adapter, skill).unwrap();
    crate::publication_copy::listing_from_reply(
        &package,
        r#"{"title":"适配包发布测试","summary":"自动生成的测试摘要","description":"用于验证自动发布预览。","tags":["适配测试"],"design":{"palette":"grove","icon":"sprout"}}"#,
    )
    .unwrap()
}

type Fixture = (
    tempfile::TempDir,
    ToolContext,
    Arc<dyn AgentTool>,
    Value,
    Arc<FixtureRemote>,
    Arc<FixtureSecrets>,
    std::path::PathBuf,
);

fn fixture_with_listing(owner: Option<PublishedPlugin>, listing: PublicationListing) -> Fixture {
    let (temp, engine, _) = setup();
    let draft = install_adapter_package(&engine, plugins::test_fixtures::PACKAGES[0]);
    let secrets = Arc::new(FixtureSecrets::default());
    let settings = Settings::with_secret_store(engine.db.clone(), secrets.clone());
    settings
        .write_secret("mcp.glossmod_http.api_key", TEST_MANAGER_KEY)
        .unwrap();
    let remote = Arc::new(FixtureRemote {
        owned: owner.into_iter().collect(),
        ..Default::default()
    });
    let tool = test_tool(settings, remote.clone(), listing);
    let args = json!({"draft_id":draft.id});
    (
        temp,
        ToolContext {
            engine,
            cancel: CancellationToken::new(),
            progress: None,
            resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
        },
        tool,
        args,
        remote,
        secrets,
        draft.path,
    )
}

fn fixture(owner: Option<PublishedPlugin>) -> Fixture {
    fixture_with_listing(owner, generated_listing())
}

/// Reuse the same offline preparation path in the Agent-loop rejection test.
pub(crate) fn no_owner_test_tool(engine: &FileEngine) -> Arc<dyn AgentTool> {
    let settings =
        Settings::with_secret_store(engine.db.clone(), Arc::new(FixtureSecrets::default()));
    settings
        .write_secret("mcp.glossmod_http.api_key", TEST_MANAGER_KEY)
        .unwrap();
    test_tool(
        settings,
        Arc::new(FixtureRemote::default()),
        generated_listing(),
    )
}

#[tokio::test]
async fn preparation_returns_generated_files_and_listing_without_publication_consent() {
    let (_temp, context, tool, args, remote, secrets, _) = fixture(None);
    let prepared = tool.prepare(&context, &args).await.unwrap();
    assert_eq!(prepared.effect, Effect::Publish);
    assert!(prepared.plan.is_none());

    let record: manager_publication::PublicationRecord =
        serde_json::from_value(prepared.preview.clone()).unwrap();
    let listing = generated_listing();
    assert_eq!(record.preview.listing.as_ref(), Some(&listing));
    assert_eq!(
        record.preview.target,
        manager_publication::PublicationTarget::First
    );
    assert!(record.preview.remote_version.is_none());
    assert!(record.preview.original_version.is_some());
    assert_eq!(record.preview.files.len(), 2);
    for file in &record.preview.files {
        assert_eq!(file.size as usize, file.text.len());
        assert_eq!(
            file.sha256,
            hex::encode(Sha256::digest(file.text.as_bytes()))
        );
    }
    assert!(record.preview.files[0].text.contains(&listing.title));
    assert!(record.preview.files[1]
        .text
        .contains("amm-publication-display:v1"));
    assert!(record.preview.files[1].text.contains(&listing.summary));
    assert_eq!(record.preview.digest.len(), 64);
    assert!(record.approved_digest.is_none());
    assert!(record.remote.is_none());
    assert_eq!(
        record.state,
        manager_publication::PublicationState::Intended
    );

    let calls = remote.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "list_my_manager_plugins");
    assert_eq!(calls[0].1, json!({"limit":100,"offset":0}));
    assert!(secrets.0.load(Ordering::SeqCst) >= 2);
}

#[tokio::test]
async fn installed_source_automatically_matches_the_owned_plugin() {
    let slug = plugins::test_fixtures::PACKAGES[0].0;
    let owner = PublishedPlugin {
        id: 42,
        release_id: 55,
        slug: slug.into(),
        version: "1.2.3".into(),
        tags: vec!["existing".into()],
        hidden: false,
        name: "Fixture owner plugin".into(),
        reason: String::new(),
    };
    let (_temp, context, tool, _, remote, _, _) = fixture(Some(owner.clone()));
    let prepared = tool.prepare(&context, &json!({"slug":slug})).await.unwrap();
    assert_eq!(
        prepared.preview["preview"]["target"],
        json!({"kind":"release","pluginId":owner.id})
    );
    assert_eq!(prepared.preview["preview"]["remoteVersion"], owner.version);
    assert_eq!(prepared.preview["preview"]["slug"], slug);
    assert_eq!(
        prepared.preview["preview"]["listing"]["tags"],
        json!(["适配测试"])
    );
    assert_eq!(remote.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cancellation_before_execute_never_grants_publication_consent() {
    let (_temp, context, tool, args, remote, _, _) = fixture(None);
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
    assert_eq!(remote.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn changed_source_between_preview_and_approval_is_rejected_before_write() {
    let (_temp, context, tool, args, remote, _, path) = fixture(None);
    let prepared = tool.prepare(&context, &args).await.unwrap();
    let id = prepared.preview["id"].as_str().unwrap().to_owned();
    std::fs::write(
        std::path::Path::new(&path).join("SKILL.md"),
        "changed content",
    )
    .unwrap();
    assert!(tool.execute(&context, args, prepared).await.is_err());
    let record = manager_publication::get_publication(&context.engine, &id).unwrap();
    assert!(record.approved_digest.is_none());
    assert_eq!(
        record.state,
        manager_publication::PublicationState::Intended
    );
    assert_eq!(remote.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn invalid_arguments_and_prepared_effect_cannot_bypass_publish_review() {
    let (_temp, context, tool, args, remote, secrets, _) = fixture(None);
    let slug = plugins::test_fixtures::PACKAGES[0].0;
    for invalid in [
        json!({}),
        json!({"draft_id":"   "}),
        json!({"draft_id":args["draft_id"],"slug":slug}),
        json!({"draft_id":args["draft_id"],"plugin_id":42}),
        json!({"draft_id":args["draft_id"],"tags":["fixture"]}),
        json!({"draft_id":args["draft_id"],"upload_url":"https://elsewhere.invalid"}),
        json!({"draft_id":12}),
    ] {
        assert!(tool.prepare(&context, &invalid).await.is_err());
    }
    assert_eq!(
        tool.definition().parameters["properties"]
            .as_object()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        tool.definition().parameters["oneOf"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(remote.calls.lock().unwrap().is_empty());
    assert_eq!(secrets.0.load(Ordering::SeqCst), 0);

    let mut prepared = tool.prepare(&context, &args).await.unwrap();
    prepared.effect = Effect::Read;
    assert!(matches!(
        tool.execute(&context, args, prepared).await,
        Err(AppError::Denied(_))
    ));
}

#[tokio::test]
async fn invalid_generated_listing_leaves_no_publication_record() {
    let mut listing = generated_listing();
    listing.title.clear();
    let (_temp, context, tool, args, remote, _, _) = fixture_with_listing(None, listing);
    assert!(tool.prepare(&context, &args).await.is_err());
    assert!(
        manager_publication::list_publications(&context.engine, None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(remote.calls.lock().unwrap().len(), 1);
    assert_eq!(remote.calls.lock().unwrap()[0].0, "list_my_manager_plugins");
}
