//! Offline tests for automatic review, send preflight, and reconciliation.
use super::*;
use crate::{db::Db, manager_publication as publication, plugin_workflow, storage::AppPaths};
use serde_json::{json, Value};
use std::{collections::VecDeque, sync::Mutex};

const TITLE: &str = "Automatic Listing";
const SLUG: &str = "automatic-manager-publication";
const GAME_NAME: &str = "Minecraft Java Edition";
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
        Self::with_game_name(GAME_NAME)
    }

    fn with_game_name(game_name: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("manager")).unwrap();
        let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
        let settings =
            Settings::with_secret_store(engine.db.clone(), Arc::new(TestSecrets::default()));
        settings.write_secret(SECRET_REF, KEY).unwrap();
        let mut adapter: Value =
            serde_json::from_str(plugins::test_fixtures::PACKAGES[0].1).unwrap();
        adapter["slug"] = json!(SLUG);
        adapter["name"] = json!(game_name);
        adapter["version"] = json!("1.2.3");
        adapter["coverUrl"] = Value::Null;
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
        let listing = publication_copy::listing_from_reply(
            &package,
            r#"{
            "title":"Automatic Listing",
            "summary":"A reviewed summary",
            "description":"Read the whole review before publishing.",
            "tags":["reviewed","fixture"]
        }"#,
        )
        .unwrap();
        assert_eq!(listing.tags, self.expected_tags());
        assert_eq!(listing.cover_url, None);
        assert!(listing.cover_png.is_empty());
        listing
    }

    fn expected_tags(&self) -> Vec<String> {
        vec![
            plugins::parse_package(SLUG, &self.adapter, &self.skill)
                .unwrap()
                .adapter
                .name,
            "reviewed".into(),
            "fixture".into(),
        ]
    }

    async fn prepare(&self, remote: &dyn Remote) -> AppResult<PublicationRecord> {
        self.prepare_source(&self.draft.draft.id, remote).await
    }

    async fn prepare_source(
        &self,
        draft_id: &str,
        remote: &dyn Remote,
    ) -> AppResult<PublicationRecord> {
        publication::prepare_automatic_with(
            &self.engine,
            &self.settings,
            remote,
            Some(draft_id.to_owned()),
            None,
            self.listing(),
            &self.cancel,
        )
        .await
    }

    fn install(&self) {
        let install =
            plugin_workflow::plan_install_plugin(&self.engine, &self.draft.draft.id).unwrap();
        self.engine.approve_plan(&install.plan.id).unwrap();
        self.engine.apply_file_ops(&install.plan.id).unwrap();
        assert_eq!(
            plugin_workflow::reconcile_deployments(&self.engine)
                .unwrap()
                .registered,
            vec![SLUG.to_owned()]
        );
    }

    async fn prepare_installed(&self, remote: &dyn Remote) -> AppResult<PublicationRecord> {
        publication::prepare_automatic_with(
            &self.engine,
            &self.settings,
            remote,
            None,
            Some(SLUG.to_owned()),
            self.listing(),
            &self.cancel,
        )
        .await
    }

    fn draft_at_version(&self, version: &str) -> plugin_workflow::DraftPreview {
        let mut adapter: Value = serde_json::from_str(&self.adapter).unwrap();
        adapter["version"] = json!(version);
        let adapter = serde_json::to_string_pretty(&adapter).unwrap();
        let draft =
            plugin_workflow::plan_write_draft(&self.engine, SLUG, &adapter, &self.skill).unwrap();
        self.engine.approve_plan(&draft.plan.id).unwrap();
        self.engine.apply_file_ops(&draft.plan.id).unwrap();
        draft
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
    json!({"id":id,"releaseId":release_id,"slug":slug,"version":version,"name":TITLE,"tags":[GAME_NAME,"reviewed","fixture"],"hidden":false})
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
    assert_eq!(record.preview.tags, f.expected_tags());
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
async fn installed_source_update_previews_and_sends_the_same_game_name_tag() {
    let f = Fixture::new();
    f.install();
    let owner = row(22, 47, SLUG, "1.2.3");
    let preparation = Script::new([page(vec![owner.clone()])]);
    let record = f.prepare_installed(&preparation).await.unwrap();
    let expected_tags = f.expected_tags();
    assert!(matches!(
        &record.preview.source,
        publication::PublicationSource::Installed { slug, .. } if slug == SLUG
    ));
    assert_eq!(record.preview.tags, expected_tags);
    assert_eq!(
        record.preview.listing.as_ref().unwrap().tags,
        record.preview.tags
    );
    assert_eq!(
        record.preview.target,
        PublicationTarget::Release { plugin_id: 22 }
    );
    assert_eq!(record.preview.version, "1.2.4");
    f.approve(&record);

    let sending = Script::new([
        page(vec![owner]),
        Ok(json!({"plugin":row(22, 48, SLUG, "1.2.4")})),
    ]);
    let completed = f.publish(&sending, &record).await.unwrap();
    assert_eq!(completed.state, PublicationState::Completed);
    let calls = sending.take_calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].0, "publish_manager_plugin_release");
    assert_eq!(calls[1].1["tags"], json!(record.preview.tags));
    assert_eq!(preparation.take_calls().len(), 1);
}

#[tokio::test]
async fn long_safe_game_name_is_preserved_as_one_tag_in_preview_and_first_send() {
    let game_name = format!("{} Edition", "Long Game ".repeat(8));
    assert!(game_name.chars().count() > 32);
    let f = Fixture::with_game_name(&game_name);
    let record = f.prepare(&Script::new([page(vec![])])).await.unwrap();
    assert_eq!(record.preview.tags[0], game_name);
    assert_eq!(
        record.preview.tags,
        record.preview.listing.as_ref().unwrap().tags
    );
    f.approve(&record);

    let sending = Script::new([
        page(vec![]),
        Ok(json!({"plugin":row(31, 59, SLUG, "1.2.3")})),
    ]);
    assert_eq!(
        f.publish(&sending, &record).await.unwrap().state,
        PublicationState::Completed
    );
    let calls = sending.take_calls();
    assert_eq!(calls[1].1["tags"], json!(record.preview.tags));
    assert_eq!(calls[1].1["tags"][0], game_name);
}

#[tokio::test]
async fn legacy_preview_without_game_name_is_rejected_before_send_but_history_remains_readable() {
    let f = Fixture::new();
    let current = f.prepare(&Script::new([page(vec![])])).await.unwrap();
    let game_name = f.expected_tags().into_iter().next().unwrap();
    let mut preview = current.preview.clone();
    assert_eq!(preview.tags.remove(0), game_name);
    assert_eq!(preview.listing.as_mut().unwrap().tags.remove(0), game_name);
    preview.digest = preview.checked_digest().unwrap();
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE manager_publications
             SET preview_json=?2,digest=?3,approved_digest=?3,state='intended',remote_json=NULL
             WHERE id=?1",
            rusqlite::params![
                current.id,
                serde_json::to_string(&preview).unwrap(),
                preview.digest
            ],
        )
        .unwrap();
    let legacy = f.load(&current);

    let remote = Script::default();
    assert!(matches!(
        f.publish(&remote, &legacy).await,
        Err(AppError::Conflict(message))
            if message.contains("游戏名称标签") && message.contains("重新预览")
    ));
    assert!(remote.take_calls().is_empty());

    let receipt = publication::PublishedPlugin {
        id: 31,
        release_id: 59,
        slug: SLUG.to_owned(),
        version: legacy.preview.version.clone(),
        tags: legacy.preview.tags.clone(),
        hidden: false,
        name: legacy.preview.listing.as_ref().unwrap().title.clone(),
        reason: String::new(),
    };
    f.engine
        .db
        .conn()
        .execute(
            "UPDATE manager_publications SET state='completed',remote_json=?2 WHERE id=?1",
            rusqlite::params![legacy.id, serde_json::to_string(&receipt).unwrap()],
        )
        .unwrap();
    let completed_history = f.load(&legacy);
    assert_eq!(completed_history.state, PublicationState::Completed);
    assert_eq!(completed_history.preview.tags, legacy.preview.tags);
    assert_eq!(
        f.load(&legacy)
            .remote
            .as_ref()
            .map(|plugin| plugin.release_id),
        Some(59)
    );
}

#[tokio::test]
async fn coverless_listing_can_be_published_as_a_first_release() {
    let f = Fixture::new();
    let record = f.prepare(&Script::new([page(vec![])])).await.unwrap();
    assert_eq!(record.preview.target, PublicationTarget::First);
    assert_eq!(record.preview.listing.as_ref().unwrap().cover_url, None);
    f.approve(&record);

    let sending = Script::new([
        page(vec![]),
        Ok(json!({"plugin":row(31, 59, SLUG, "1.2.3")})),
    ]);
    let completed = f.publish(&sending, &record).await.unwrap();
    assert_eq!(completed.state, PublicationState::Completed);
    assert_eq!(completed.remote.unwrap().release_id, 59);
    assert_eq!(
        sending.take_calls(),
        [
            own_page_call(0),
            (
                "publish_manager_plugin".into(),
                json!({
                    "adapter":record.preview.files[0].text,
                    "skill":record.preview.files[1].text,
                    "tags":record.preview.tags,
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
    let remote = Script::new([
        page(vec![]),
        Ok(json!({"plugin":mismatched})),
        page(vec![mismatched.clone()]),
    ]);
    let unknown = f.publish(&remote, &record).await.unwrap();
    assert_eq!(unknown.state, PublicationState::Unknown);
    assert!(unknown.remote.is_none());
    assert!(f.publish(&remote, &record).await.is_err());
    assert_eq!(remote.take_calls().len(), 3);

    let reconcile = Script::new([page(vec![mismatched])]);
    assert_eq!(
        reconcile_with(&f.engine, &f.settings, &reconcile, &unknown, &f.cancel)
            .await
            .unwrap(),
        ReconciliationOutcome::Unconfirmed
    );
    assert_eq!(reconcile.take_calls(), [own_page_call(0)]);
}

#[test]
fn readback_tag_keys_match_case_whitespace_and_deduplication_not_arbitrary_aliases() {
    let approved = ["Combat  Overhaul".into(), "combat-overhaul".into()];
    assert!(publication::same_tags(
        &approved,
        &["Combat　Overhaul".into()]
    ));
    assert!(!publication::same_tags(&approved, &["Gameplay".into()]));
}

#[tokio::test]
async fn committed_receipt_accepts_server_canonical_tags_but_unrelated_readback_does_not() {
    for has_receipt in [true, false] {
        let f = Fixture::new();
        let record = f.prepare(&Script::new([page(vec![])])).await.unwrap();
        f.approve(&record);
        let mut candidate = row(22, 48, SLUG, "1.2.3");
        // The site's shared tag table and aliases can deduplicate both
        // submitted tags into one canonical display name.
        candidate["tags"] = json!(["Canonical Tag"]);
        let remote = Script::new(if has_receipt {
            vec![page(vec![]), Ok(json!({"plugin":candidate}))]
        } else {
            vec![page(vec![]), Err(unavailable()), page(vec![candidate])]
        });
        let result = f.publish(&remote, &record).await.unwrap();
        assert_eq!(result, f.load(&record));
        assert_eq!(remote.take_calls().len(), if has_receipt { 2 } else { 3 });
        if has_receipt {
            assert_eq!(result.state, PublicationState::Completed);
            assert_eq!(result.remote.unwrap().tags, ["Canonical Tag"]);
            assert_eq!(result.preview.tags, record.preview.tags);
        } else {
            assert_eq!(result.state, PublicationState::Unknown);
            assert!(matches!(
                f.publish(&Script::default(), &record).await,
                Err(AppError::Conflict(message)) if message.contains("请打开发布记录查看状态")
            ));
        }
    }
}

#[tokio::test]
async fn automatic_readback_requires_exact_text_and_a_stable_release_after_one_write() {
    for (ack_lost, wrong_text, moved_head) in [
        (true, false, false),
        (false, false, false),
        (true, true, false),
        (true, false, true),
    ] {
        let f = Fixture::new();
        let record = f.prepare(&Script::new([page(vec![])])).await.unwrap();
        f.approve(&record);
        let candidate = row(22, 48, SLUG, "1.2.3");
        let mut partial = candidate.clone();
        partial.as_object_mut().unwrap().remove("name");
        let ack = if ack_lost {
            Err(unavailable())
        } else {
            Ok(json!({"plugin":partial}))
        };
        let adapter = if wrong_text {
            &f.adapter
        } else {
            &record.preview.files[0].text
        };
        let mut responses = vec![
            page(vec![]),
            ack,
            page(vec![candidate.clone()]),
            Ok(
                json!({"adapter":adapter,"skill":record.preview.files[1].text,"tags":record.preview.tags}),
            ),
        ];
        if !wrong_text {
            responses.push(page(vec![if moved_head {
                row(22, 49, SLUG, "1.2.4")
            } else {
                candidate
            }]));
        }
        let remote = Script::new(responses);
        let result = f.publish(&remote, &record).await.unwrap();
        assert_eq!(result, f.load(&record));
        assert_eq!(
            result.remote.as_ref().map(|p| p.release_id),
            if wrong_text || moved_head {
                None
            } else {
                Some(48)
            }
        );
        assert_eq!(
            result.state,
            if wrong_text || moved_head {
                PublicationState::Unknown
            } else {
                PublicationState::Completed
            }
        );
        let calls = remote.take_calls();
        let mut expected = vec![
            "list_my_manager_plugins",
            "publish_manager_plugin",
            "list_my_manager_plugins",
            "get_my_manager_plugin_texts",
        ];
        if !wrong_text {
            expected.push("list_my_manager_plugins");
        }
        assert_eq!(
            calls
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[tokio::test]
async fn uncertain_write_requires_derived_bytes_and_reconciles_without_local_source() {
    let f = Fixture::new();
    let record = f.prepare(&Script::new([page(vec![])])).await.unwrap();
    f.approve(&record);
    let sending = Script::new([page(vec![]), Err(unavailable()), Err(unavailable())]);
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
        Err(AppError::Conflict(message)) if message.contains("请打开发布记录查看状态")
    ));
    assert_eq!(read_only.take_calls(), [own_page_call(0)]);
    f.edit_source();
    let mut candidate = row(22, 48, SLUG, "1.2.3");
    candidate["tags"] = json!([record.preview.tags[0], "Fixture", "Reviewed"]);
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
            "adapter":record.preview.files[0].text,"skill":record.preview.files[1].text,"tags":candidate["tags"],
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
            let mut steps = vec![page(vec![]), ack];
            if prior == "unknown" {
                steps.push(page(vec![]));
            }
            f.publish(&Script::new(steps), &first).await.unwrap();
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
async fn verified_owner_progress_allows_a_strictly_newer_release_after_unknown() {
    // The remote may be at the uncertain release, or may have advanced again.
    // In either case, the old row remains unknown; only the new owner snapshot
    // and strictly newer version authorize the next write.
    for (current_version, current_release_id, next_version) in
        [("1.2.4", 48, "1.2.5"), ("1.2.5", 49, "1.2.6")]
    {
        let f = Fixture::new();
        let before = row(22, 47, SLUG, "1.2.3");
        let uncertain_preview = f
            .prepare(&Script::new([page(vec![before.clone()])]))
            .await
            .unwrap();
        assert_eq!(uncertain_preview.preview.version, "1.2.4");
        f.approve(&uncertain_preview);

        let uncertain_send = Script::new([
            page(vec![before.clone()]),
            Err(unavailable()),
            page(vec![before]),
        ]);
        let uncertain = f
            .publish(&uncertain_send, &uncertain_preview)
            .await
            .unwrap();
        assert_eq!(uncertain.state, PublicationState::Unknown);
        assert_eq!(uncertain_send.take_calls().len(), 3);

        let next_draft = f.draft_at_version(next_version);
        let current = row(22, current_release_id, SLUG, current_version);
        let prepared = f
            .prepare_source(
                &next_draft.draft.id,
                &Script::new([page(vec![current.clone()])]),
            )
            .await
            .unwrap();
        assert_eq!(
            prepared.preview.target,
            PublicationTarget::Release { plugin_id: 22 }
        );
        assert_eq!(
            prepared.preview.remote_version.as_deref(),
            Some(current_version)
        );
        assert_eq!(prepared.preview.version, next_version);
        f.approve(&prepared);

        let send = Script::new([
            page(vec![current]),
            Ok(json!({"plugin":row(22, current_release_id + 1, SLUG, next_version)})),
        ]);
        let completed = f.publish(&send, &prepared).await.unwrap();
        assert_eq!(completed.state, PublicationState::Completed);
        assert_eq!(completed.remote.as_ref().unwrap().id, 22);
        assert_eq!(
            completed.remote.as_ref().unwrap().release_id,
            current_release_id + 1
        );
        assert_eq!(send.take_calls().len(), 2);
        assert_eq!(f.load(&uncertain).state, PublicationState::Unknown);

        // Replaying the same completed plugin/version is idempotent and sends no write.
        let retry = Script::default();
        assert_eq!(f.publish(&retry, &prepared).await.unwrap(), completed);
        assert!(retry.take_calls().is_empty());
    }
}

#[tokio::test]
async fn in_flight_send_still_blocks_a_separately_verified_newer_release() {
    let f = Fixture::new();
    let first = f.prepare(&Script::new([page(vec![])])).await.unwrap();
    f.approve(&first);
    let pending = publication::claim(&f.engine, &f.load(&first)).unwrap();

    let owner = row(22, 48, SLUG, "1.2.3");
    let next = f
        .prepare(&Script::new([page(vec![owner.clone()])]))
        .await
        .unwrap();
    assert_eq!(next.preview.version, "1.2.4");
    f.approve(&next);
    let read_only = Script::new([page(vec![owner])]);
    assert!(matches!(
        f.publish(&read_only, &next).await,
        Err(AppError::Conflict(_))
    ));
    assert_eq!(read_only.take_calls(), [own_page_call(0)]);
    assert_eq!(f.load(&first), pending);
}

#[tokio::test]
async fn foreign_slug_conflict_never_falls_back_to_a_release_write() {
    let f = Fixture::new();
    let first = f.prepare(&Script::new([page(vec![])])).await.unwrap();
    f.approve(&first);
    let foreign = Script::new([page(vec![]), Err(AppError::Conflict("标识已存在".into()))]);
    assert!(matches!(
        f.publish(&foreign, &first).await,
        Err(AppError::Conflict(_))
    ));
    let names: Vec<_> = foreign
        .take_calls()
        .into_iter()
        .map(|call| call.0)
        .collect();
    assert_eq!(names, ["list_my_manager_plugins", "publish_manager_plugin"]);
    assert_eq!(f.load(&first).state, PublicationState::Error);
}

#[tokio::test]
async fn unknown_newer_than_verified_remote_still_blocks_a_new_release_write() {
    let f = Fixture::new();
    let owner = row(22, 47, SLUG, "1.2.3");
    let uncertain_preview = f
        .prepare(&Script::new([page(vec![owner.clone()])]))
        .await
        .unwrap();
    f.approve(&uncertain_preview);
    let uncertain = f
        .publish(
            &Script::new([
                page(vec![owner.clone()]),
                Err(unavailable()),
                page(vec![owner.clone()]),
            ]),
            &uncertain_preview,
        )
        .await
        .unwrap();
    assert_eq!(uncertain.state, PublicationState::Unknown);

    let next_draft = f.draft_at_version("1.2.5");
    let prepared = f
        .prepare_source(
            &next_draft.draft.id,
            &Script::new([page(vec![owner.clone()])]),
        )
        .await
        .unwrap();
    assert_eq!(prepared.preview.version, "1.2.5");
    f.approve(&prepared);

    let no_write = Script::new([page(vec![owner])]);
    assert!(matches!(
        f.publish(&no_write, &prepared).await,
        Err(AppError::Conflict(_))
    ));
    assert_eq!(no_write.take_calls().len(), 1);
    assert_eq!(f.load(&uncertain).state, PublicationState::Unknown);
    assert_eq!(f.load(&prepared).state, PublicationState::Intended);
}

#[tokio::test]
async fn completed_history_for_another_plugin_id_does_not_block_verified_owner() {
    let f = Fixture::new();
    let previous_owner = row(77, 50, SLUG, "1.2.3");
    let previous = f
        .prepare(&Script::new([page(vec![previous_owner.clone()])]))
        .await
        .unwrap();
    assert_eq!(previous.preview.version, "1.2.4");
    f.approve(&previous);
    let previous_done = f
        .publish(
            &Script::new([
                page(vec![previous_owner]),
                Ok(json!({"plugin":row(77, 51, SLUG, "1.2.4")})),
            ]),
            &previous,
        )
        .await
        .unwrap();
    assert_eq!(previous_done.state, PublicationState::Completed);

    // A later owner has the same slug but a different plugin ID. Matching
    // version history for ID 77 must not veto the verified release for ID 22.
    let current_owner = row(22, 60, SLUG, "1.2.3");
    let next_draft = f.draft_at_version("1.2.4");
    let prepared = f
        .prepare_source(
            &next_draft.draft.id,
            &Script::new([page(vec![current_owner.clone()])]),
        )
        .await
        .unwrap();
    assert_eq!(
        prepared.preview.target,
        PublicationTarget::Release { plugin_id: 22 }
    );
    assert_eq!(prepared.preview.version, previous_done.preview.version);
    f.approve(&prepared);

    let send = Script::new([
        page(vec![current_owner]),
        Ok(json!({"plugin":row(22, 61, SLUG, "1.2.4")})),
    ]);
    let completed = f.publish(&send, &prepared).await.unwrap();
    assert_eq!(completed.state, PublicationState::Completed);
    assert_eq!(completed.remote.as_ref().unwrap().id, 22);
    assert_eq!(send.take_calls().len(), 2);
}

#[tokio::test]
async fn same_plugin_history_raises_version_baseline_but_send_checks_remote_head() {
    let f = Fixture::new();
    let remote_head = row(22, 47, SLUG, "1.2.2");
    let previous = f
        .prepare(&Script::new([page(vec![remote_head.clone()])]))
        .await
        .unwrap();
    assert_eq!(previous.preview.version, "1.2.3");
    f.approve(&previous);
    let previous_done = f
        .publish(
            &Script::new([
                page(vec![remote_head.clone()]),
                Ok(json!({"plugin":row(22, 48, SLUG, "1.2.3")})),
            ]),
            &previous,
        )
        .await
        .unwrap();
    assert_eq!(previous_done.state, PublicationState::Completed);
    assert_eq!(previous_done.remote.as_ref().unwrap().id, 22);

    // Local verified history is 1.2.3 while the actual remote head is still 1.2.2.
    let older_source = f.draft_at_version("1.2.2");
    let prepared = f
        .prepare_source(
            &older_source.draft.id,
            &Script::new([page(vec![remote_head.clone()])]),
        )
        .await
        .unwrap();
    assert_eq!(
        prepared.preview.target,
        PublicationTarget::Release { plugin_id: 22 }
    );
    assert_eq!(prepared.preview.remote_version.as_deref(), Some("1.2.2"));
    assert_eq!(prepared.preview.version, "1.2.4");
    f.approve(&prepared);

    // A changed cloud head invalidates this preview before any publication call.
    let advanced_head = row(22, 49, SLUG, "1.2.3");
    let stale_send = Script::new([page(vec![advanced_head.clone()])]);
    assert!(f.publish(&stale_send, &prepared).await.is_err());
    assert_eq!(stale_send.take_calls().len(), 1);
    assert_eq!(f.load(&prepared).state, PublicationState::Intended);

    // Refreshing against the actual 1.2.3 head keeps the next version at 1.2.4.
    let refreshed = f
        .prepare_source(
            &older_source.draft.id,
            &Script::new([page(vec![advanced_head.clone()])]),
        )
        .await
        .unwrap();
    assert_eq!(refreshed.preview.remote_version.as_deref(), Some("1.2.3"));
    assert_eq!(refreshed.preview.version, "1.2.4");
    f.approve(&refreshed);
    let send = Script::new([
        page(vec![advanced_head]),
        Ok(json!({"plugin":row(22, 50, SLUG, "1.2.4")})),
    ]);
    let completed = f.publish(&send, &refreshed).await.unwrap();
    assert_eq!(completed.state, PublicationState::Completed);
    assert_eq!(completed.remote.as_ref().unwrap().id, 22);
    assert_eq!(completed.remote.as_ref().unwrap().version, "1.2.4");
    assert_eq!(send.take_calls().len(), 2);
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
        (
            "$.listing.coverUrl",
            json!("https://www.glossmod.com/manager-plugins/covers/changed.png"),
        ),
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
