//! Offline tests for automatic review, send preflight, and reconciliation.
use super::*;
use crate::{db::Db, manager_publication as publication, plugin_workflow, storage::AppPaths};
use serde_json::{json, Value};
use std::{collections::VecDeque, sync::Mutex};

const TITLE: &str = "Automatic Listing";
const SLUG: &str = "automatic-manager-publication";
const KEY: &str = "offline-automatic-publication-key-1";
const OTHER_KEY: &str = "offline-automatic-publication-key-2";

#[derive(Default)]
struct TestSecrets(Mutex<Option<String>>);

impl SecretStore for TestSecrets {
    fn read(&self, key: &str) -> AppResult<Option<String>> {
        assert_eq!(key, SECRET_REF);
        Ok(self.0.lock().unwrap().clone())
    }

    fn write(&self, key: &str, value: &str) -> AppResult<()> {
        assert_eq!(key, SECRET_REF);
        *self.0.lock().unwrap() = Some(value.into());
        Ok(())
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    engine: FileEngine,
    settings: Settings,
    draft: plugin_workflow::DraftPreview,
    adapter: String,
    skill: String,
    cancel: CancellationToken,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("manager")).unwrap();
        let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
        let settings =
            Settings::with_secret_store(engine.db.clone(), Arc::new(TestSecrets::default()));
        settings.write_secret(SECRET_REF, KEY).unwrap();
        let mut adapter: Value =
            serde_json::from_str(plugins::test_fixtures::PACKAGES[0].1).unwrap();
        adapter["slug"] = json!(SLUG);
        adapter["version"] = json!("1.2.3");
        let adapter = serde_json::to_string_pretty(&adapter).unwrap();
        let skill = format!("---\r\nname: {SLUG}\r\ndescription: Original text\r\n---\r\n\r\nKeep the original behavior.\r\n");
        let draft = plugin_workflow::plan_write_draft(&engine, SLUG, &adapter, &skill).unwrap();
        engine.approve_plan(&draft.plan.id).unwrap();
        engine.apply_file_ops(&draft.plan.id).unwrap();
        Self {
            _temp: temp,
            engine,
            settings,
            draft,
            adapter,
            skill,
            cancel: CancellationToken::new(),
        }
    }

    fn listing(&self) -> PublicationListing {
        let package = plugins::parse_package(SLUG, &self.adapter, &self.skill).unwrap();
        publication_copy::listing_from_reply(
            &package,
            r#"{
            "title":"Automatic Listing",
            "summary":"A reviewed summary",
            "description":"Read the whole review before publishing.",
            "tags":["reviewed","fixture"],
            "design":{"palette":"grove","icon":"gear"}
        }"#,
        )
        .unwrap()
    }

    async fn prepare(&self, remote: &dyn Remote) -> AppResult<PublicationRecord> {
        publication::prepare_automatic_with(
            &self.engine,
            &self.settings,
            remote,
            Some(self.draft.draft.id.clone()),
            None,
            self.listing(),
            &self.cancel,
        )
        .await
    }

    fn approve(&self, record: &PublicationRecord) {
        publication::approve_publication(&self.engine, &record.id, &record.preview.digest).unwrap();
    }

    async fn publish(
        &self,
        remote: &dyn Remote,
        record: &PublicationRecord,
    ) -> AppResult<PublicationRecord> {
        publication::publish_automatic_with(
            &self.engine,
            &self.settings,
            remote,
            &record.id,
            &record.preview.digest,
            &self.cancel,
        )
        .await
    }

    fn load(&self, record: &PublicationRecord) -> PublicationRecord {
        publication::get_publication(&self.engine, &record.id).unwrap()
    }

    fn edit_source(&self) {
        std::fs::write(
            self.draft.draft.path.join("SKILL.md"),
            format!("{}Changed", self.skill),
        )
        .unwrap();
    }
}

#[derive(Default)]
struct Script {
    steps: Mutex<VecDeque<AppResult<Value>>>,
    calls: Mutex<Vec<(String, Value)>>,
}

impl Script {
    fn new(steps: impl IntoIterator<Item = AppResult<Value>>) -> Self {
        Self {
            steps: Mutex::new(steps.into_iter().collect()),
            ..Self::default()
        }
    }

    fn take_calls(&self) -> Vec<(String, Value)> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
}

#[async_trait::async_trait]
impl Remote for Script {
    async fn call(&self, name: &str, arguments: Value, _: &CancellationToken) -> AppResult<Value> {
        self.calls.lock().unwrap().push((name.into(), arguments));
        self.steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected remote call")
    }
}

fn row(id: i64, release_id: i64, slug: &str, version: &str) -> Value {
    json!({"id":id,"releaseId":release_id,"slug":slug,"version":version,"name":TITLE,"tags":["fixture","reviewed"],"hidden":false})
}

fn page(items: Vec<Value>) -> AppResult<Value> {
    Ok(json!({"items": items}))
}

fn own_page_call(offset: u32) -> (String, Value) {
    (
        "list_my_manager_plugins".to_owned(),
        json!({"limit":100,"offset":offset}),
    )
}

fn filled_page(start: u32) -> Vec<Value> {
    (start..start + PAGE_LIMIT)
        .map(|i| {
            row(
                i64::from(i) + 1000,
                i64::from(i) + 5000,
                &format!("{SLUG}-{i}"),
                "9.0.0",
            )
        })
        .collect()
}

#[tokio::test]
async fn searches_all_pages_and_snapshots_approved_derived_release() {
    let f = Fixture::new();
    let owner = row(22, 47, SLUG, "1.2.3+previous");
    let mut second = filled_page(100);
    second[99] = owner.clone();
    let preparation = Script::new([page(filled_page(0)), page(second), page(vec![])]);
    let record = f.prepare(&preparation).await.unwrap();
    assert_eq!(
        preparation.take_calls(),
        [own_page_call(0), own_page_call(100), own_page_call(200)]
    );
    assert_eq!(
        record.preview.target,
        PublicationTarget::Release { plugin_id: 22 }
    );
    assert_eq!(record.preview.version, "1.2.4");
    assert_eq!(record.preview.original_version.as_deref(), Some("1.2.3"));
    assert_eq!(
        record.preview.remote_version.as_deref(),
        Some("1.2.3+previous")
    );
    assert_eq!(record.preview.listing, Some(f.listing()));
    assert_eq!(record, f.load(&record));
    assert_ne!(record.preview.files[0].text, f.adapter);
    assert_ne!(record.preview.files[1].text, f.skill);
    let raw = serde_json::to_value(&record).unwrap();
    assert!(!raw.to_string().contains(KEY));
    assert_eq!(
        raw["preview"]["snapshot"]["sourceSha256"],
        json!([
            hex::encode(Sha256::digest(f.adapter.as_bytes())),
            hex::encode(Sha256::digest(f.skill.as_bytes())),
        ])
    );
    let sending = Script::new([
        page(vec![owner]),
        Ok(json!({"plugin":row(22, 48, SLUG, "1.2.4")})),
    ]);
    assert!(
        f.publish(&sending, &record).await.is_err(),
        "preview alone does not authorize sending"
    );
    f.approve(&record);
    let completed = f.publish(&sending, &record).await.unwrap();
    assert_eq!(completed.state, PublicationState::Completed);
    assert_eq!(completed.remote.unwrap().release_id, 48);
    assert_eq!(
        std::fs::read(f.draft.draft.path.join("adapter.json")).unwrap(),
        f.adapter.as_bytes()
    );
    assert_eq!(
        std::fs::read(f.draft.draft.path.join("SKILL.md")).unwrap(),
        f.skill.as_bytes()
    );
    assert_eq!(
        sending.take_calls(),
        [
            own_page_call(0),
            (
                "publish_manager_plugin_release".into(),
                json!({
                    "plugin_id":22,"adapter":record.preview.files[0].text,"skill":record.preview.files[1].text,"tags":record.preview.tags,
                })
            )
        ]
    );
}

#[tokio::test]
async fn none_is_first_but_duplicate_in_another_page_and_read_errors_never_are() {
    let f = Fixture::new();
    let mut first = filled_page(0);
    first[0] = row(22, 47, SLUG, "1.2.0");
    let ambiguous = Script::new([page(first), page(vec![row(23, 48, SLUG, "1.0.0")])]);
    assert!(matches!(
        f.prepare(&ambiguous).await,
        Err(AppError::Conflict(_))
    ));
    assert_eq!(
        ambiguous.take_calls(),
        [own_page_call(0), own_page_call(100)]
    );
    let interrupted = Script::new([page(filled_page(0)), Err(unavailable())]);
    assert!(f.prepare(&interrupted).await.is_err());
    let repeated = Script::new([page(filled_page(0)), page(filled_page(0))]);
    assert!(f.prepare(&repeated).await.is_err());
    assert!(publication::list_publications(&f.engine, None)
        .unwrap()
        .is_empty());
    let no_match = Script::new([page(vec![row(22, 47, &format!("{SLUG}-extra"), "9.0.0")])]);
    let record = f.prepare(&no_match).await.unwrap();
    assert_eq!(record.preview.target, PublicationTarget::First);
    assert!(record.preview.remote_version.is_none());
    assert_eq!(record.preview.version, "1.2.3");
    f.approve(&record);
    let first_created_elsewhere = Script::new([page(vec![row(22, 47, SLUG, "0.1.0")])]);
    assert!(matches!(
        f.publish(&first_created_elsewhere, &record).await,
        Err(AppError::Conflict(_))
    ));
    assert_eq!(first_created_elsewhere.take_calls(), [own_page_call(0)]);
    assert_eq!(f.load(&record).state, PublicationState::Intended);
}

#[tokio::test]
async fn send_rejects_changed_versions_owner_matches_credentials_and_source() {
    let f = Fixture::new();
    let owner = row(22, 47, SLUG, "1.2.3");
    let record = f
        .prepare(&Script::new([page(vec![owner.clone()])]))
        .await
        .unwrap();
    f.approve(&record);
    for lookup in [
        page(vec![row(22, 48, SLUG, "1.2.4")]),
        page(vec![row(22, 48, SLUG, "1.2.3")]),
        page(vec![row(23, 47, SLUG, "1.2.3")]),
        page(vec![owner.clone(), row(23, 48, SLUG, "1.2.3")]),
        page(vec![]),
        Err(unavailable()),
    ] {
        let read_only = Script::new([lookup]);
        assert!(f.publish(&read_only, &record).await.is_err());
        assert_eq!(read_only.take_calls(), [own_page_call(0)]);
        assert_eq!(f.load(&record).state, PublicationState::Intended);
    }
    let no_io = Script::default();
    f.settings.write_secret(SECRET_REF, OTHER_KEY).unwrap();
    assert!(matches!(
        f.publish(&no_io, &record).await,
        Err(AppError::Conflict(_))
    ));
    f.settings.write_secret(SECRET_REF, KEY).unwrap();
    f.edit_source();
    assert!(f.publish(&no_io, &record).await.is_err());
    assert!(no_io.take_calls().is_empty());
    assert_eq!(f.load(&record).state, PublicationState::Intended);
}

#[tokio::test]
async fn receipt_with_wrong_title_is_uncertain_and_cannot_be_resent() {
    let f = Fixture::new();
    let record = f.prepare(&Script::new([page(vec![])])).await.unwrap();
    f.approve(&record);
    let mut mismatched = row(22, 48, SLUG, "1.2.3");
    mismatched["name"] = json!("Unreviewed Title");
    let remote = Script::new([page(vec![]), Ok(json!({"plugin":mismatched}))]);
    let unknown = f.publish(&remote, &record).await.unwrap();
    assert_eq!(unknown.state, PublicationState::Unknown);
    assert!(unknown.remote.is_none());
    assert!(f.publish(&remote, &record).await.is_err());
    assert_eq!(remote.take_calls().len(), 2);

    let reconcile = Script::new([page(vec![mismatched])]);
    assert_eq!(
        reconcile_with(&f.engine, &f.settings, &reconcile, &unknown, &f.cancel)
            .await
            .unwrap(),
        ReconciliationOutcome::Unconfirmed
    );
    assert_eq!(reconcile.take_calls(), [own_page_call(0)]);
}

#[tokio::test]
async fn uncertain_write_requires_derived_bytes_and_reconciles_without_local_source() {
    let f = Fixture::new();
    let record = f.prepare(&Script::new([page(vec![])])).await.unwrap();
    f.approve(&record);
    let sending = Script::new([page(vec![]), Err(unavailable())]);
    let unknown = f.publish(&sending, &record).await.unwrap();
    assert_eq!(unknown.state, PublicationState::Unknown);
    assert_eq!(
        sending.take_calls()[1],
        (
            "publish_manager_plugin".into(),
            json!({
                "adapter":record.preview.files[0].text,"skill":record.preview.files[1].text,"tags":record.preview.tags,
            })
        )
    );
    assert!(f.publish(&Script::default(), &record).await.is_err());
    let competing = publication::prepare_automatic_with(
        &f.engine,
        &f.settings,
        &Script::new([page(vec![])]),
        Some(f.draft.draft.id.clone()),
        None,
        PublicationListing {
            title: "Another Listing".into(),
            ..f.listing()
        },
        &f.cancel,
    )
    .await
    .unwrap();
    f.approve(&competing);
    let read_only = Script::new([page(vec![])]);
    assert!(matches!(
        f.publish(&read_only, &competing).await,
        Err(AppError::Conflict(_))
    ));
    assert_eq!(read_only.take_calls(), [own_page_call(0)]);
    f.edit_source();
    let candidate = row(22, 48, SLUG, "1.2.3");
    let wrong = Script::new([
        page(vec![candidate.clone()]),
        Ok(json!({
            "adapter":f.adapter,"skill":f.skill,"tags":record.preview.tags,
        })),
    ]);
    assert_eq!(
        reconcile_with(&f.engine, &f.settings, &wrong, &unknown, &f.cancel)
            .await
            .unwrap(),
        ReconciliationOutcome::Unconfirmed
    );
    let correct = Script::new([
        page(vec![candidate.clone()]),
        Ok(json!({
            "adapter":record.preview.files[0].text,"skill":record.preview.files[1].text,"tags":record.preview.tags,
        })),
        page(vec![candidate]),
    ]);
    assert_eq!(
        reconcile_with(&f.engine, &f.settings, &correct, &unknown, &f.cancel)
            .await
            .unwrap(),
        ReconciliationOutcome::Verified
    );
    assert_eq!(f.load(&record).state, PublicationState::Completed);
    assert_eq!(
        correct.take_calls(),
        [
            own_page_call(0),
            (
                "get_my_manager_plugin_texts".into(),
                json!({"plugin_id":22})
            ),
            own_page_call(0)
        ]
    );
}

#[tokio::test]
async fn historical_completed_and_other_credentials_do_not_block_first_but_v1_unknown_does() {
    for prior in ["completed", "unknown", "v1-completed", "v1-unknown"] {
        let f = Fixture::new();
        let first = if prior.starts_with("v1") {
            publication::prepare_draft(
                &f.engine,
                &f.draft.draft.id,
                PublicationTarget::First,
                f.listing().tags,
            )
            .unwrap()
        } else {
            f.prepare(&Script::new([page(vec![])])).await.unwrap()
        };
        f.approve(&first);
        let ack = if prior.ends_with("completed") {
            Ok(json!({"plugin":row(22, 48, SLUG, "1.2.3")}))
        } else {
            Err(unavailable())
        };
        if prior.starts_with("v1") {
            publication::publish_with(
                &f.engine,
                &Script::new([ack]),
                &first.id,
                &first.preview.digest,
                &f.cancel,
            )
            .await
            .unwrap();
        } else {
            f.publish(&Script::new([page(vec![]), ack]), &first)
                .await
                .unwrap();
        }
        f.settings.write_secret(SECRET_REF, OTHER_KEY).unwrap();
        let other = f.prepare(&Script::new([page(vec![])])).await.unwrap();
        assert_eq!(other.preview.target, PublicationTarget::First);
        assert_ne!(other.preview.digest, first.preview.digest);
        f.approve(&other);
        if prior == "v1-unknown" {
            assert!(matches!(
                f.publish(&Script::new([page(vec![])]), &other).await,
                Err(AppError::Conflict(_))
            ));
        } else {
            let sending = Script::new([
                page(vec![]),
                Ok(json!({"plugin":row(33, 59, SLUG, "1.2.3")})),
            ]);
            assert_eq!(
                f.publish(&sending, &other)
                    .await
                    .unwrap()
                    .remote
                    .unwrap()
                    .id,
                33
            );
        }
    }
}

#[tokio::test]
async fn credential_rotation_during_preflight_never_sends() {
    struct RotatingOwnerList(Settings);
    #[async_trait::async_trait]
    impl Remote for RotatingOwnerList {
        async fn call(&self, name: &str, args: Value, _: &CancellationToken) -> AppResult<Value> {
            assert_eq!((name.to_owned(), args), own_page_call(0));
            self.0.write_secret(SECRET_REF, OTHER_KEY)?;
            page(vec![])
        }
    }

    let f = Fixture::new();
    let record = f.prepare(&Script::new([page(vec![])])).await.unwrap();
    f.approve(&record);
    assert!(matches!(
        f.publish(&RotatingOwnerList(f.settings.clone()), &record)
            .await,
        Err(AppError::Conflict(_))
    ));
    assert_eq!(f.load(&record).state, PublicationState::Intended);
}

#[tokio::test]
async fn v2_digest_rejects_tampering_in_review_and_snapshot_fields() {
    let f = Fixture::new();
    let record = f
        .prepare(&Script::new([page(vec![row(22, 48, SLUG, "1.2.3")])]))
        .await
        .unwrap();
    f.approve(&record);
    for (path, replacement) in [
        ("$.listing.title", json!("Modified Review")),
        ("$.originalVersion", json!("1.2.2")),
        ("$.remoteVersion", json!("1.2.3+other")),
        ("$.snapshot.remoteReleaseId", json!(49)),
        ("$.snapshot.sourceSha256[0]", json!("a".repeat(64))),
        ("$.snapshot.credentialBinding", json!("b".repeat(64))),
    ] {
        f.engine
            .db
            .conn()
            .execute(
                "UPDATE manager_publications SET preview_json=json_set(?2,?3,json(?4)) WHERE id=?1",
                rusqlite::params![
                    record.id,
                    serde_json::to_string(&record.preview).unwrap(),
                    path,
                    replacement.to_string()
                ],
            )
            .unwrap();
        assert!(
            publication::get_publication(&f.engine, &record.id).is_err(),
            "{path}"
        );
    }
}

#[test]
fn version_order_uses_semver_precedence_without_integer_overflow() {
    for (source, remote, expected) in [
        ("1.2.3", None, "1.2.3"),
        ("1.2.3+new", Some("1.2.3+old"), "1.2.4"),
        ("1.2.3", Some("1.9.9"), "1.9.10"),
        ("1.2.3-rc.10", Some("1.2.3-rc.9"), "1.2.3-rc.10"),
        ("1.2.3-rc.1", Some("1.2.3-rc.1.1"), "1.2.3"),
        ("1.2.3-rc.1", Some("1.2.3-rc.beta"), "1.2.3"),
        (
            "0.0.0",
            Some("0.1.999999999999999999999"),
            "0.1.1000000000000000000000",
        ),
    ] {
        assert_eq!(published_version(source, remote).unwrap(), expected);
    }
    let max_patch = format!("1.2.{}", "9".repeat(124));
    assert_eq!(max_patch.len(), 128);
    assert_eq!(
        published_version("0.0.0", Some(&max_patch)).unwrap(),
        "1.3.0"
    );
    assert!(published_version("0.0.0", Some("1.2.x")).is_err());
}
