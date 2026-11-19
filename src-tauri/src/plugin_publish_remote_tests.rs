use super::*;
use crate::{db::Db, plugin_workflow, plugins, storage::AppPaths};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs,
    io::{Cursor, Read},
    sync::{Arc, Mutex},
};

const FILE_ID: i64 = 42;
const WORK_ID: i64 = 91;
const PRIVATE_ERROR: &str = "private-key https://private.invalid/?signature=secret remote error";

struct Fixture {
    _temp: tempfile::TempDir,
    engine: FileEngine,
    record: PublicationRecord,
    bytes: Vec<u8>,
}

impl Fixture {
    fn new(approved: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("manager")).unwrap();
        let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
        publication::migrate(&engine.db.conn()).unwrap();
        let mut adapter: Value = serde_json::from_str(plugins::BUILTIN_PACKAGES[0].1).unwrap();
        adapter["slug"] = json!("remote-publish-test");
        adapter["name"] = json!("Remote Publication Test");
        adapter["version"] = json!("1.2.3");
        let draft = plugin_workflow::plan_write_draft(
            &engine, "remote-publish-test", &serde_json::to_string_pretty(&adapter).unwrap(),
            "---\nname: remote-publish-test\ndescription: Remote test\n---\n\nExact text.\r\n  Keep whitespace.\n",
        ).unwrap();
        engine.approve_plan(&draft.plan.id).unwrap();
        engine.apply_file_ops(&draft.plan.id).unwrap();
        let metadata = publication::PublicationMetadata {
            title: "Remote Publication Test".into(),
            summary: "Exact summary".into(),
            description: "Full description\n  with whitespace.\n".into(),
            tags: vec!["adapter".into(), "fixture".into()],
            game_ids: vec![7, 11],
        };
        let (preview, bytes) =
            publication::prepare_package(&engine, &draft.draft.id, metadata).unwrap();
        let mut record = publication::create_publication(&engine, &preview).unwrap();
        if approved {
            record =
                publication::approve_publication(&engine, &record.id, &preview.digest).unwrap();
        }
        Self {
            _temp: temp,
            engine,
            record,
            bytes: bytes.into_vec(),
        }
    }

    fn reopen(&self) -> FileEngine {
        reopen(&self.engine)
    }

    fn stored(&self) -> PublicationRecord {
        publication::get_publication(&self.reopen(), &self.record.id).unwrap()
    }

    async fn run(&self, remote: &dyn Remote) -> AppResult<PublicationRecord> {
        publish_with(
            &self.engine,
            remote,
            &self.record.id,
            &self.record.preview.digest,
            &CancellationToken::new(),
        )
        .await
    }

    fn pending_at(&self, target: PublicationStep) -> PublicationRecord {
        let id = &self.record.id;
        for step in [
            PublicationStep::CreateUpload,
            PublicationStep::StartUpload,
            PublicationStep::PutPart,
            PublicationStep::CompleteUpload,
            PublicationStep::PublishWork,
        ] {
            let pending =
                publication::begin_step(&self.engine, id, &self.record.preview.digest, step)
                    .unwrap();
            if step == target {
                return pending;
            }
            publication::finish_step(&self.engine, id, step, Some(FILE_ID), None).unwrap();
        }
        unreachable!("every publication step is represented");
    }

    fn recover(&self) -> PublicationRecord {
        let mut recovered = publication::recover_pending_publications(&self.reopen()).unwrap();
        assert_eq!(recovered.len(), 1);
        let record = recovered.pop().unwrap();
        assert_eq!(record.state, PublicationState::Unknown);
        assert_eq!(record, self.stored());
        record
    }

    async fn reconcile(
        &self,
        remote: &MockRemote,
        record: &PublicationRecord,
    ) -> AppResult<ReconciliationOutcome> {
        let result = reconcile_with(
            &self.reopen(),
            remote,
            record.clone(),
            &CancellationToken::new(),
        )
        .await;
        remote.done();
        result
    }

    fn events(&self) -> Vec<(String, String)> {
        let engine = self.reopen();
        let conn = engine.db.conn();
        let mut stmt = conn.prepare("SELECT state,phase FROM plugin_publication_events WHERE publication_id=?1 ORDER BY id").unwrap();
        stmt.query_map([&self.record.id], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }
}

fn reopen(engine: &FileEngine) -> FileEngine {
    FileEngine::new(
        Db::open(&engine.paths.db_file()).unwrap(),
        (*engine.paths).clone(),
    )
}

fn clean() -> Value {
    // tools/list types plus the official site's positive safety predicate.
    json!({"blocked":false,"manually_reviewed":false,"scan_state":"complete","verdict":"clean",
        "legacy":false,"vt_detail":"","policy_version":1,"vt_state":"not_requested","vt_verdict":"unknown"})
}

fn status(package: &PublicPackage, uploaded: bool, ready: bool) -> Value {
    json!({"file":{"id":FILE_ID,"owner_id":10,"original_name":package.archive_name,
        "content_type":"application/zip","size_bytes":package.archive_size,
        "sha256":if ready { package.archive_sha256.as_str() } else { "" },
        "status":if ready { "ready" } else if uploaded { "uploading" } else { "pending" },
        "created_at":"2026-11-18T00:00:00Z","ready_at":null},
        "part_size":5*1024*1024,"uploaded_bytes":if uploaded {package.archive_size} else {0},
        "next_part_number":if uploaded {2} else {1},
        "next_part_bytes":if uploaded {0} else {package.archive_size},
        "next_part_path":if uploaded {None} else {Some(format!("/api/v1/uploads/{FILE_ID}/multipart/1"))},
        "upload_method":"PUT","safety":if ready {clean()} else {Value::Null}})
}

fn work(package: &PublicPackage) -> Value {
    json!({"work":{"id":WORK_ID,"status":"published","title":package.title,
        "mod_version":package.version,"files":[{"file_id":FILE_ID}]}})
}

enum Reply {
    Value(Value),
    Fail,
    Cancel,
    Hang,
}
struct Exchange {
    name: &'static str,
    args: Value,
    phase: PublicationPhase,
    state: PublicationState,
    reply: Reply,
}
impl Exchange {
    fn value(name: &'static str, args: Value, phase: PublicationPhase, value: Value) -> Self {
        let state = if matches!(
            phase,
            PublicationPhase::AwaitingScan | PublicationPhase::UploadReady
        ) {
            PublicationState::Intended
        } else {
            PublicationState::Pending
        };
        Self {
            name,
            args,
            phase,
            state,
            reply: Reply::Value(value),
        }
    }

    fn at_state(mut self, state: PublicationState) -> Self {
        self.state = state;
        self
    }
}

fn script(f: &Fixture) -> Vec<Exchange> {
    use PublicationPhase::*;
    let p = &f.record.preview.package;
    let id = json!({"file_id":FILE_ID});
    vec![
        Exchange::value(
            "create_upload",
            json!({"request_id":f.record.id,"original_name":p.archive_name,"content_type":"application/zip","size_bytes":p.archive_size}),
            CreateUploadPending,
            status(p, false, false),
        ),
        Exchange::value(
            "start_upload",
            id.clone(),
            StartUploadPending,
            status(p, false, false),
        ),
        Exchange::value(
            "get_upload_status",
            id.clone(),
            PartPutPending,
            status(p, false, false),
        ),
        Exchange::value(
            "PUT",
            json!({"path":format!("/api/v1/uploads/{FILE_ID}/multipart/1")}),
            PartPutPending,
            Value::Null,
        ),
        Exchange::value(
            "get_upload_status",
            id.clone(),
            PartPutPending,
            status(p, true, false),
        ),
        Exchange::value(
            "complete_upload",
            id.clone(),
            CompleteUploadPending,
            status(p, true, true),
        ),
        Exchange::value("get_upload_status", id, UploadReady, status(p, true, true)),
        Exchange::value(
            "publish_work",
            json!({"title":p.title,"summary":p.summary,"description":p.description,
            "tags":p.tags,"game_ids":p.game_ids,"mod_version":p.version,"status":"published",
            "files":[{"file_id":FILE_ID,"name":p.archive_name,"visibility":"public","version_type":"latest"}]}),
            PublishWorkPending,
            work(p),
        ),
    ]
}

struct MockRemote {
    engine: FileEngine,
    id: String,
    expected_bytes: Vec<u8>,
    pending: Mutex<VecDeque<Exchange>>,
    seen: Mutex<Vec<String>>,
}
impl MockRemote {
    fn new(f: &Fixture, script: Vec<Exchange>) -> Self {
        Self {
            engine: f.engine.clone(),
            id: f.record.id.clone(),
            expected_bytes: f.bytes.clone(),
            pending: Mutex::new(script.into()),
            seen: Mutex::new(Vec::new()),
        }
    }

    async fn exchange(
        &self,
        name: &str,
        args: Value,
        token: &CancellationToken,
    ) -> AppResult<Value> {
        let expected = self
            .pending
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected network operation");
        assert_eq!(name, expected.name);
        assert_eq!(args, expected.args, "exact wire arguments for {name}");
        // Read through an independent SQLite connection to prove commit-before-send.
        let record = publication::get_publication(&reopen(&self.engine), &self.id).unwrap();
        assert_eq!(record.phase, expected.phase, "durable phase at {name}");
        assert_eq!(record.state, expected.state, "durable state at {name}");
        self.seen.lock().unwrap().push(name.into());
        match expected.reply {
            Reply::Value(value) => Ok(value),
            Reply::Fail => Err(AppError::Other(PRIVATE_ERROR.into())),
            Reply::Cancel => {
                token.cancel();
                Err(AppError::Cancelled)
            }
            Reply::Hang => std::future::pending().await,
        }
    }

    fn done(&self) {
        assert!(
            self.pending.lock().unwrap().is_empty(),
            "mock script not consumed"
        );
    }

    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl Remote for MockRemote {
    async fn call(&self, name: &str, args: Value, token: &CancellationToken) -> AppResult<Value> {
        self.exchange(name, args, token).await
    }

    async fn put(&self, path: &str, bytes: &[u8], token: &CancellationToken) -> AppResult<()> {
        assert_eq!(
            bytes, self.expected_bytes,
            "PUT must contain the exact consented ZIP, not MCP JSON/base64"
        );
        self.exchange("PUT", json!({"path":path}), token)
            .await
            .map(|_| ())
    }
}

#[tokio::test]
async fn exact_zip_metadata_and_every_durable_transition_complete_without_duplicates() {
    let f = Fixture::new(true);
    let p = &f.record.preview.package;
    assert_eq!(p.archive_sha256, hex::encode(Sha256::digest(&f.bytes)));
    assert_eq!(p.archive_size, f.bytes.len() as u64);
    let mut zip = zip::ZipArchive::new(Cursor::new(&f.bytes)).unwrap();
    assert_eq!(zip.len(), 2);
    for (i, expected) in p.files.iter().enumerate() {
        let mut entry = zip.by_index(i).unwrap();
        assert_eq!(entry.name(), expected.name);
        assert_eq!(entry.unix_mode().unwrap() & 0o777, 0o644);
        assert_eq!(entry.compression(), zip::CompressionMethod::Stored);
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, expected.text.as_bytes());
        assert_eq!(hex::encode(Sha256::digest(&bytes)), expected.sha256);
    }
    let remote = MockRemote::new(&f, script(&f));
    let result = f.run(&remote).await.unwrap();
    remote.done();
    assert_eq!(result.state, PublicationState::Completed);
    assert_eq!(result.phase, PublicationPhase::Completed);
    assert_eq!(result.file_id, Some(FILE_ID));
    assert_eq!(result.work_id, Some(WORK_ID));
    assert_eq!(result, f.stored());
    let expected = [
        ("intended", "prepared"),
        ("intended", "prepared"),
        ("pending", "createUploadPending"),
        ("intended", "uploadCreated"),
        ("pending", "startUploadPending"),
        ("intended", "uploadStarted"),
        ("pending", "partPutPending"),
        ("intended", "partUploaded"),
        ("pending", "completeUploadPending"),
        ("intended", "uploadReady"),
        ("pending", "publishWorkPending"),
        ("completed", "completed"),
    ];
    assert_eq!(f.events(), expected.map(|(s, p)| (s.into(), p.into())));
    let calls = remote.count();
    let events = f.events();
    assert_eq!(f.run(&remote).await.unwrap(), result);
    assert_eq!(remote.count(), calls);
    assert_eq!(f.events(), events);
}

struct NoCredentials;
impl crate::settings::SecretStore for NoCredentials {
    fn read(&self, _: &str) -> AppResult<Option<String>> {
        panic!("must not read real credentials")
    }
    fn write(&self, _: &str, _: &str) -> AppResult<()> {
        panic!("must not write credentials")
    }
}

#[tokio::test]
async fn public_entry_point_completed_and_unapproved_never_connect_or_read_credentials() {
    for approved in [false, true] {
        let f = Fixture::new(approved);
        let settings = Settings::with_secret_store(f.engine.db.clone(), Arc::new(NoCredentials));
        if approved {
            let remote = MockRemote::new(&f, script(&f));
            f.run(&remote).await.unwrap();
            remote.done();
        }
        let result = publish_approved(
            &f.engine,
            &settings,
            &f.record.id,
            &f.record.preview.digest,
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(result.is_ok(), approved);
    }
}

#[tokio::test]
async fn public_entry_point_pending_unknown_and_error_never_read_credentials_or_retry() {
    for state in [
        PublicationState::Pending,
        PublicationState::Unknown,
        PublicationState::Error,
    ] {
        let f = Fixture::new(true);
        publication::begin_step(
            &f.engine,
            &f.record.id,
            &f.record.preview.digest,
            PublicationStep::CreateUpload,
        )
        .unwrap();
        match state {
            PublicationState::Unknown => {
                publication::mark_unknown(&f.engine, &f.record.id, PublicationStep::CreateUpload)
                    .unwrap();
            }
            PublicationState::Error => {
                publication::fail_step(&f.engine, &f.record.id, PublicationStep::CreateUpload)
                    .unwrap();
            }
            _ => {}
        }
        let events = f.events();
        let settings = Settings::with_secret_store(f.engine.db.clone(), Arc::new(NoCredentials));
        assert!(publish_approved(
            &f.engine,
            &settings,
            &f.record.id,
            &f.record.preview.digest,
            &CancellationToken::new()
        )
        .await
        .is_err());
        assert_eq!(f.events(), events);
        assert_eq!(f.stored().state, state);
    }
}

#[tokio::test]
async fn missing_consent_wrong_digest_or_modified_source_never_contact_remote() {
    let unapproved = Fixture::new(false);
    let remote = MockRemote::new(&unapproved, vec![]);
    assert!(unapproved.run(&remote).await.is_err());
    assert_eq!(remote.count(), 0);
    let f = Fixture::new(true);
    let remote = MockRemote::new(&f, vec![]);
    assert!(publish_with(
        &f.engine,
        &remote,
        &f.record.id,
        &"0".repeat(64),
        &CancellationToken::new()
    )
    .await
    .is_err());
    let paths: Vec<_> = walkdir::WalkDir::new(&f.engine.paths.root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_name() == "SKILL.md")
        .map(|e| e.path().to_owned())
        .collect();
    assert!(!paths.is_empty());
    for path in paths {
        fs::write(path, "changed after consent\n").unwrap();
    }
    assert!(f.run(&remote).await.is_err());
    assert_eq!(remote.count(), 0);
    assert_eq!(f.stored().phase, PublicationPhase::Prepared);
}

#[tokio::test]
async fn cancellation_before_send_has_no_pending_intent_or_remote_effect() {
    let f = Fixture::new(true);
    let remote = MockRemote::new(&f, vec![]);
    let token = CancellationToken::new();
    token.cancel();
    let before = f.events();
    assert!(matches!(
        publish_with(
            &f.engine,
            &remote,
            &f.record.id,
            &f.record.preview.digest,
            &token
        )
        .await,
        Err(AppError::Cancelled)
    ));
    assert_eq!(remote.count(), 0);
    assert_eq!(f.events(), before);
}

#[tokio::test]
async fn failed_or_cancelled_side_effect_is_unknown_redacted_and_never_retried() {
    for index in [0, 1, 3, 4, 5, 7] {
        for cancelled in [false, true] {
            let f = Fixture::new(true);
            let mut calls = script(&f);
            let phase = calls[index].phase;
            calls.truncate(index + 1);
            calls[index].reply = if cancelled {
                Reply::Cancel
            } else {
                Reply::Fail
            };
            let remote = MockRemote::new(&f, calls);
            let error = f.run(&remote).await.unwrap_err();
            assert_eq!(matches!(error, AppError::Cancelled), cancelled);
            assert!(!error.to_string().contains("private"));
            remote.done();
            let stored = f.stored();
            assert_eq!(stored.state, PublicationState::Unknown, "index {index}");
            assert_eq!(stored.phase, phase);
            let events = f.events();
            assert!(f.run(&remote).await.is_err());
            assert_eq!(remote.count(), index + 1);
            assert_eq!(f.events(), events);
            assert!(publication::recover_pending_publications(&f.reopen())
                .unwrap()
                .is_empty());
            let saved = serde_json::to_string(&stored).unwrap();
            assert!(!saved.contains("private") && !saved.contains("signature"));
            let db = fs::read(f.engine.paths.db_file()).unwrap();
            assert!(!db
                .windows(PRIVATE_ERROR.len())
                .any(|part| part == PRIVATE_ERROR.as_bytes()));
        }
    }
}

#[tokio::test]
async fn dropping_each_inflight_write_recovers_pending_to_unknown_without_replay() {
    for index in [0, 1, 3, 5, 7] {
        let f = Fixture::new(true);
        let mut calls = script(&f);
        let phase = calls[index].phase;
        calls.truncate(index + 1);
        calls[index].reply = Reply::Hang;
        let remote = MockRemote::new(&f, calls);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), f.run(&remote))
                .await
                .is_err()
        );
        remote.done();
        assert_eq!(f.stored().state, PublicationState::Pending);
        assert_eq!(f.stored().phase, phase);
        assert!(f.run(&remote).await.is_err());
        let recovered = publication::recover_pending_publications(&f.reopen()).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].state, PublicationState::Unknown);
        assert!(f.run(&remote).await.is_err());
        assert_eq!(remote.count(), index + 1);
    }
}

#[tokio::test]
async fn mismatched_remote_identity_hashes_parts_and_work_results_fail_closed() {
    let cases = [
        (0, "/file/id", json!(0)),
        (0, "/file/size_bytes", json!(1)),
        (0, "/file/original_name", json!("other.zip")),
        (0, "/file/content_type", json!("text/plain")),
        (0, "/uploaded_bytes", json!(1)),
        (0, "/file/sha256", json!("wrong-hash")),
        (1, "/file/id", json!(43)),
        (2, "/next_part_number", json!(2)),
        (2, "/next_part_bytes", json!(1)),
        (2, "/part_size", json!(1)),
        (2, "/upload_method", json!("POST")),
        (2, "/next_part_path", Value::Null),
        (
            2,
            "/next_part_path",
            json!("/api/v1/uploads/43/multipart/1"),
        ),
        (
            2,
            "/next_part_path",
            json!("/api/v1/uploads/42/multipart/2"),
        ),
        (2, "/next_part_path", json!("/api/v1/admin/settings")),
        (4, "/file/id", json!(43)),
        (4, "/uploaded_bytes", json!(0)),
        (4, "/file/sha256", json!("wrong-hash")),
        (5, "/file/sha256", json!("wrong-hash")),
        (5, "/file/sha256", json!("")),
        (5, "/uploaded_bytes", json!(0)),
        (5, "/file/status", json!("failed")),
        (7, "/work/id", json!(0)),
        (7, "/work/status", json!("draft")),
        (7, "/work/title", json!("Wrong title")),
        (7, "/work/mod_version", json!("99.0")),
        (7, "/work/files", json!([])),
        (7, "/work/files", json!([{"file_id":43}])),
        (7, "/work/files", json!([{"file_id":42},{"file_id":43}])),
    ];
    for (index, pointer, value) in cases {
        let f = Fixture::new(true);
        let mut calls = script(&f);
        if let Reply::Value(response) = &mut calls[index].reply {
            *response.pointer_mut(pointer).unwrap() = value;
        }
        calls.truncate(index + 1);
        let remote = MockRemote::new(&f, calls);
        assert!(
            f.run(&remote).await.is_err(),
            "index {index} pointer {pointer}"
        );
        remote.done();
        assert_eq!(f.stored().state, PublicationState::Unknown);
        assert!(f.run(&remote).await.is_err());
        assert_eq!(remote.count(), index + 1);
    }
}

#[tokio::test]
async fn incomplete_safety_waits_without_publishing_and_blocked_cannot_be_overridden() {
    let mut pending = clean();
    pending["verdict"] = json!("unknown");
    pending["scan_state"] = json!("scanning");
    let mut skipped = clean();
    skipped["scan_state"] = json!("skipped");
    let mut risk = clean();
    risk["verdict"] = json!("suspicious");
    let mut vt_risk = clean();
    vt_risk["vt_verdict"] = json!("malicious");
    for safety in [Value::Null, pending, skipped, risk, vt_risk] {
        let f = Fixture::new(true);
        let mut calls = script(&f);
        if let Reply::Value(response) = &mut calls[5].reply {
            response["safety"] = safety;
        }
        calls.truncate(6);
        let remote = MockRemote::new(&f, calls);
        let record = f.run(&remote).await.unwrap();
        assert_eq!(record.phase, PublicationPhase::AwaitingScan);
        assert_eq!(record.state, PublicationState::Intended);
        assert_eq!(record.work_id, None);
        remote.done();
    }
    let f = Fixture::new(true);
    let mut calls = script(&f);
    if let Reply::Value(response) = &mut calls[5].reply {
        response["safety"]["blocked"] = json!(true);
        response["safety"]["manually_reviewed"] = json!(true);
    }
    calls.truncate(6);
    let remote = MockRemote::new(&f, calls);
    assert!(matches!(f.run(&remote).await, Err(AppError::Denied(_))));
    assert_eq!(f.stored().state, PublicationState::Unknown);
    remote.done();
}

#[test]
fn safety_requires_explicit_evidence_not_guessed_scan_state_or_missing_fields() {
    let f = Fixture::new(true);
    let p = &f.record.preview.package;
    let mut value = status(p, true, true);
    value["safety"]["verdict"] = json!("unknown");
    for state in ["completed", "clean", "complete", "new-state", ""] {
        value["safety"]["scan_state"] = json!(state);
        assert!(!distributable(&serde_json::from_value(value.clone()).unwrap(), p).unwrap());
    }
    value["safety"]["vt_state"] = json!("complete");
    value["safety"]["vt_verdict"] = json!("clean");
    assert!(distributable(&serde_json::from_value(value.clone()).unwrap(), p).unwrap());
    value["safety"]["verdict"] = json!("malicious");
    assert!(!distributable(&serde_json::from_value(value.clone()).unwrap(), p).unwrap());
    value["safety"]["manually_reviewed"] = json!(true);
    assert!(distributable(&serde_json::from_value(value.clone()).unwrap(), p).unwrap());
    for field in [
        "blocked",
        "manually_reviewed",
        "scan_state",
        "verdict",
        "vt_state",
        "vt_verdict",
    ] {
        let mut incomplete = value.clone();
        incomplete["safety"].as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<UploadStatus>(incomplete).is_err(),
            "{field}"
        );
    }
}

#[tokio::test]
async fn acknowledged_completion_waits_for_background_verification_then_resumes_read_only() {
    let f = Fixture::new(true);
    let mut calls = script(&f);
    calls[5].reply = Reply::Value(status(&f.record.preview.package, true, false));
    calls.truncate(6);
    let remote = MockRemote::new(&f, calls);
    assert_eq!(
        f.run(&remote).await.unwrap().phase,
        PublicationPhase::AwaitingScan
    );
    remote.done();
    let p = &f.record.preview.package;
    let mut calls = vec![Exchange::value(
        "get_upload_status",
        json!({"file_id":FILE_ID}),
        PublicationPhase::AwaitingScan,
        status(p, true, true),
    )];
    calls.extend(script(&f).into_iter().skip(6));
    let remote = MockRemote::new(&f, calls);
    assert_eq!(
        f.run(&remote).await.unwrap().state,
        PublicationState::Completed
    );
    remote.done();
    assert_eq!(
        *remote.seen.lock().unwrap(),
        ["get_upload_status", "get_upload_status", "publish_work"]
    );
}

#[tokio::test]
async fn publication_rechecks_ready_file_safety_and_identity_before_send() {
    for (pointer, value) in [
        ("/safety/blocked", json!(true)),
        ("/file/id", json!(43)),
        ("/file/sha256", json!("wrong")),
    ] {
        let f = Fixture::new(true);
        let mut calls = script(&f);
        if let Reply::Value(response) = &mut calls[6].reply {
            *response.pointer_mut(pointer).unwrap() = value;
        }
        calls.truncate(7);
        let remote = MockRemote::new(&f, calls);
        assert!(f.run(&remote).await.is_err());
        assert_eq!(f.stored().phase, PublicationPhase::UploadReady);
        assert_eq!(f.stored().state, PublicationState::Intended);
        remote.done();
    }
}

#[tokio::test]
async fn fresh_status_read_failure_does_not_claim_an_uncertain_publish_effect() {
    let f = Fixture::new(true);
    let mut calls = script(&f);
    calls[6].reply = Reply::Fail;
    calls.truncate(7);
    let remote = MockRemote::new(&f, calls);
    assert!(f.run(&remote).await.is_err());
    remote.done();
    assert_eq!(f.stored().phase, PublicationPhase::UploadReady);
    assert_eq!(f.stored().state, PublicationState::Intended);
    let remote = MockRemote::new(&f, script(&f).into_iter().skip(6).collect());
    assert_eq!(
        f.run(&remote).await.unwrap().state,
        PublicationState::Completed
    );
    remote.done();
}

#[tokio::test]
async fn concurrent_publisher_cannot_replay_an_inflight_create() {
    let f = Fixture::new(true);
    let mut calls = script(&f);
    calls.truncate(1);
    calls[0].reply = Reply::Hang;
    let remote = MockRemote::new(&f, calls);
    let first = f.run(&remote);
    tokio::pin!(first);
    tokio::select! {
        _ = &mut first => panic!("mock write must remain in flight"),
        _ = tokio::time::sleep(Duration::from_millis(30)) => {}
    }
    assert_eq!(f.stored().state, PublicationState::Pending);
    assert!(f.run(&remote).await.is_err());
    assert_eq!(remote.count(), 1);
    remote.done();
}

fn reconciliation_status(record: &PublicationRecord, response: Value) -> Exchange {
    Exchange::value(
        "get_upload_status",
        json!({"file_id": FILE_ID}),
        record.phase,
        response,
    )
    .at_state(record.state)
}

fn reconciliation_list(record: &PublicationRecord, items: Value) -> Exchange {
    Exchange::value(
        "list_works",
        json!({"scope": "mine", "status": "published", "keyword": record.preview.package.title, "limit": 20}),
        record.phase,
        json!({"items": items}),
    )
    .at_state(record.state)
}

fn listed_work(package: &PublicPackage) -> Value {
    json!({
        "id": WORK_ID, "public_id": "fixture-work", "kind": "mod", "creator_id": 10,
        "title": package.title, "summary": package.summary, "description": package.description,
        "mod_version": package.version, "game_version": "", "platform": "",
        "dependencies": "", "install_instructions": "", "external_url": "",
        "access_mode": "public", "price": 0, "status": "published", "review_note": "",
        "reviewed_at": null, "view_count": 0, "download_count": 0, "version": 1,
        "published_at": "2026-11-18T00:00:00Z", "created_at": "2026-11-18T00:00:00Z",
        "updated_at": "2026-11-18T00:00:00Z",
        "files": [{
            "id": 71, "file_id": FILE_ID, "name": package.archive_name,
            "size_bytes": package.archive_size, "visibility": "public", "version_type": "latest",
            "description": "", "download_count": 0, "min_tip_amount": 0,
        }],
    })
}

#[tokio::test]
async fn reconciliation_verifies_scanning_and_recovered_uploads_without_publishing() {
    for awaiting_scan in [true, false] {
        let f = Fixture::new(true);
        f.pending_at(PublicationStep::CompleteUpload);
        let record = if awaiting_scan {
            publication::await_scan(&f.engine, &f.record.id).unwrap();
            let record = f.stored();
            assert_eq!(record.phase, PublicationPhase::AwaitingScan);
            assert_eq!(record.state, PublicationState::Intended);
            record
        } else {
            let record = f.recover();
            assert_eq!(record.phase, PublicationPhase::CompleteUploadPending);
            record
        };
        let mut events = f.events();
        let remote = MockRemote::new(
            &f,
            vec![reconciliation_status(
                &record,
                status(&record.preview.package, true, true),
            )],
        );
        assert_eq!(
            f.reconcile(&remote, &record).await.unwrap(),
            ReconciliationOutcome::Verified
        );
        let stored = f.stored();
        assert_eq!(stored.phase, PublicationPhase::UploadReady);
        assert_eq!(stored.state, PublicationState::Intended);
        assert_eq!(stored.last_step, Some(PublicationStep::CompleteUpload));
        assert_eq!(stored.file_id, Some(FILE_ID));
        assert_eq!(stored.work_id, None);
        events.push(("intended".into(), "uploadReady".into()));
        assert_eq!(f.events(), events);
    }
}

#[tokio::test]
async fn reconciliation_verifies_unique_owned_work_after_restart_without_republishing() {
    let f = Fixture::new(true);
    f.pending_at(PublicationStep::PublishWork);
    let record = f.recover();
    assert_eq!(record.phase, PublicationPhase::PublishWorkPending);
    let mut events = f.events();
    let package = &record.preview.package;
    let mut items: Vec<_> = (0..18)
        .map(|i| {
            let mut item = listed_work(package);
            item["id"] = json!(WORK_ID + i + 1);
            item["files"][0]["file_id"] = json!(FILE_ID + i + 1);
            item
        })
        .collect();
    items.push(listed_work(package));
    let remote = MockRemote::new(
        &f,
        vec![
            reconciliation_status(&record, status(package, true, true)),
            reconciliation_list(&record, json!(items)),
        ],
    );
    assert_eq!(
        f.reconcile(&remote, &record).await.unwrap(),
        ReconciliationOutcome::Verified
    );
    let stored = f.stored();
    assert_eq!(stored.phase, PublicationPhase::Completed);
    assert_eq!(stored.state, PublicationState::Completed);
    assert_eq!(stored.last_step, Some(PublicationStep::PublishWork));
    assert_eq!(stored.file_id, Some(FILE_ID));
    assert_eq!(stored.work_id, Some(WORK_ID));
    events.push(("completed".into(), "completed".into()));
    assert_eq!(f.events(), events);
}

#[tokio::test]
async fn reconciliation_status_identity_errors_are_redacted_and_never_list_works() {
    let f = Fixture::new(true);
    f.pending_at(PublicationStep::PublishWork);
    let record = f.recover();
    let events = f.events();
    for (pointer, value) in [
        ("/file/id", json!(FILE_ID + 1)),
        ("/file/size_bytes", json!(1)),
        ("/file/original_name", json!(PRIVATE_ERROR)),
        ("/file/content_type", json!(PRIVATE_ERROR)),
        ("/file/sha256", json!(PRIVATE_ERROR)),
        ("/file/sha256", json!("")),
        ("/uploaded_bytes", json!(0)),
    ] {
        let mut response = status(&record.preview.package, true, true);
        *response.pointer_mut(pointer).unwrap() = value;
        let remote = MockRemote::new(&f, vec![reconciliation_status(&record, response)]);
        let error = f.reconcile(&remote, &record).await.unwrap_err().to_string();
        for secret in ["private-key", "private.invalid", "signature"] {
            assert!(!error.contains(secret), "{pointer}");
        }
        assert_eq!(f.stored(), record, "{pointer}");
        assert_eq!(f.events(), events, "{pointer}");
    }
}

#[tokio::test]
async fn reconciliation_requires_ready_status_and_positive_safety_before_listing_works() {
    let f = Fixture::new(true);
    f.pending_at(PublicationStep::PublishWork);
    let record = f.recover();
    let events = f.events();
    for (pointer, value) in [
        ("/file/status", json!("uploading")),
        ("/safety", Value::Null),
        ("/safety/verdict", json!("unknown")),
        ("/safety/scan_state", json!("skipped")),
    ] {
        let mut response = status(&record.preview.package, true, true);
        *response.pointer_mut(pointer).unwrap() = value;
        let remote = MockRemote::new(&f, vec![reconciliation_status(&record, response)]);
        assert_eq!(
            f.reconcile(&remote, &record).await.unwrap(),
            ReconciliationOutcome::Unconfirmed,
            "{pointer}"
        );
        assert_eq!(f.stored(), record, "{pointer}");
        assert_eq!(f.events(), events, "{pointer}");
    }
}

#[tokio::test]
async fn reconciliation_does_not_advance_for_ambiguous_or_inexact_own_work_evidence() {
    let f = Fixture::new(true);
    f.pending_at(PublicationStep::PublishWork);
    let record = f.recover();
    let events = f.events();
    let package = &record.preview.package;
    let exact = listed_work(package);
    let mut duplicate = exact.clone();
    duplicate["id"] = json!(WORK_ID + 1);
    let mut full_page = vec![exact.clone()];
    full_page.extend((0..19).map(|i| {
        let mut item = exact.clone();
        item["id"] = json!(WORK_ID + i + 1);
        item["files"][0]["file_id"] = json!(FILE_ID + i + 1);
        item
    }));
    let mut cases = vec![
        ("null items", Value::Null),
        ("empty items", json!([])),
        ("two matches", json!([exact, duplicate])),
        ("full page with one match", json!(full_page)),
    ];
    let attachment = &exact["files"][0];
    let mut extra_attachment = attachment.clone();
    extra_attachment["id"] = json!(72);
    extra_attachment["file_id"] = json!(FILE_ID + 1);
    for (pointer, value) in [
        ("/id", json!(0)),
        ("/status", json!("draft")),
        ("/title", json!(format!("{} copy", package.title))),
        ("/summary", json!(format!("{} copy", package.summary))),
        ("/mod_version", json!(format!("v{}", package.version))),
        ("/files", Value::Null),
        ("/files", json!([])),
        ("/files", json!([attachment, extra_attachment])),
        ("/files/0/file_id", json!(FILE_ID + 1)),
        ("/files/0/name", json!("other.zip")),
        ("/files/0/size_bytes", json!(package.archive_size + 1)),
        ("/files/0/visibility", json!("private")),
        ("/files/0/version_type", json!("old")),
    ] {
        let mut item = exact.clone();
        *item.pointer_mut(pointer).unwrap() = value;
        cases.push((pointer, json!([item])));
    }
    for (case, items) in cases {
        let remote = MockRemote::new(
            &f,
            vec![
                reconciliation_status(&record, status(package, true, true)),
                reconciliation_list(&record, items),
            ],
        );
        assert_eq!(
            f.reconcile(&remote, &record).await.unwrap(),
            ReconciliationOutcome::Unconfirmed,
            "{case}"
        );
        assert_eq!(f.stored(), record, "{case}");
        assert_eq!(f.events(), events, "{case}");
    }
}

#[tokio::test]
async fn reconciliation_unsupported_phases_and_inflight_writes_never_contact_remote() {
    for (step, recover) in [
        (PublicationStep::CreateUpload, true),
        (PublicationStep::StartUpload, true),
        (PublicationStep::PutPart, true),
        (PublicationStep::CompleteUpload, false),
        (PublicationStep::PublishWork, false),
    ] {
        let f = Fixture::new(true);
        let pending = f.pending_at(step);
        let record = if recover { f.recover() } else { pending };
        let events = f.events();
        let remote = MockRemote::new(&f, vec![]);
        assert_eq!(
            f.reconcile(&remote, &record).await.unwrap(),
            ReconciliationOutcome::Unconfirmed,
            "{step:?}"
        );
        assert_eq!(f.stored(), record);
        assert_eq!(f.events(), events);
    }
}

#[tokio::test]
async fn reconciliation_with_stale_snapshot_cannot_commit_or_append_an_event() {
    let f = Fixture::new(true);
    f.pending_at(PublicationStep::CompleteUpload);
    let stale = f.recover();
    // Both snapshots support verification, so phase eligibility alone cannot reject the stale one.
    let advanced = publication::await_scan(&f.reopen(), &stale.id).unwrap();
    assert_eq!(advanced.phase, PublicationPhase::AwaitingScan);
    let events = f.events();
    let remote = MockRemote::new(
        &f,
        vec![reconciliation_status(
            &advanced,
            status(&stale.preview.package, true, true),
        )],
    );
    assert!(matches!(
        f.reconcile(&remote, &stale).await,
        Err(AppError::Conflict(_))
    ));
    assert_eq!(f.stored(), advanced);
    assert_eq!(f.events(), events);
}

#[test]
fn upload_paths_are_only_canonical_same_origin_multipart_api_paths() {
    let path = "/api/v1/uploads/12/multipart/1";
    assert_eq!(
        upload_url(path).unwrap().as_str(),
        format!("https://www.glossmod.com{path}")
    );
    for path in [
        "https://evil.invalid/api/v1/uploads/12/multipart/1",
        "//evil.invalid/api/v1/uploads/12/multipart/1",
        "/api/v1/uploads/12/../settings",
        "/api/v1/uploads/%31%32/multipart/1",
        "/api/v1/uploads/12/multipart/1?token=x",
        "/api/v1/uploads/12/multipart/1#x",
        "/api/v1/uploads/12\\multipart/1",
        "/api/v1/admin/settings",
        "/api/v1/uploads/12/content",
        "/api/v1/uploads/12/multipart/complete",
        "/api/v1/uploads/12/multipart/1/",
        "/api/v1/uploads/012/multipart/1",
        "/api/v1/uploads/+12/multipart/1",
        "/api/v1/uploads/0/multipart/1",
        "/api/v1/uploads/12/multipart/0",
        "/api/v1/uploads/12//multipart/1",
        "/API/v1/uploads/12/multipart/1",
        "/api/v1/uploads/9223372036854775808/multipart/1",
        "/api/v1/uploads/12/multipart/1\r\nX: injected",
    ] {
        assert!(upload_url(path).is_err(), "{path}");
    }
}

#[test]
fn responses_are_bounded_typed_and_redact_remote_errors_and_escaped_credentials() {
    let valid = json!({"work":{"id":9,"status":"published"}});
    assert_eq!(
        parse_result::<PublishedWork>(json!({"structuredContent":valid}), "test-key")
            .unwrap()
            .work
            .id,
        9
    );
    assert_eq!(
        parse_result::<PublishedWork>(
            json!({"content":[{"type":"text","text":valid.to_string()}]}),
            "test-key"
        )
        .unwrap()
        .work
        .id,
        9
    );
    for raw in [
        json!({"isError":true,"content":[{"type":"text","text":PRIVATE_ERROR}]}),
        json!({"structuredContent":{"work":{"id":1,"status":"private-key"}}}),
        json!({"content":[{"type":"image","data":"abc"}]}),
        json!({"content":[]}),
        json!({"content":[{"type":"text","text":"not JSON"}]}),
        json!({"content":[{"type":"text","text":"{}"},{"type":"text","text":"{}"}]}),
        json!({"structuredContent":null}),
        json!({"structuredContent":{"padding":"x".repeat(MAX_RESPONSE + 1)}}),
    ] {
        let error = parse_result::<PublishedWork>(raw, "private-key")
            .err()
            .unwrap()
            .to_string();
        assert!(!error.contains("private") && !error.contains("signature"));
    }
    let key = "quote\"slash\\credential";
    let payload = json!({"work":{"id":1,"status":key}});
    assert!(parse_result::<PublishedWork>(json!({"structuredContent":payload}), key).is_err());
    assert!(parse_result::<PublishedWork>(
        json!({"content":[{"type":"text","text":payload.to_string()}]}),
        key
    )
    .is_err());
}

#[tokio::test]
async fn actual_http_put_sends_exact_bytes_headers_and_never_follows_redirects() {
    use axum::{
        body::Bytes,
        extract::State,
        http::{HeaderMap, Method, StatusCode, Uri},
        response::{IntoResponse, Response},
        Router,
    };
    type Requests = Arc<Mutex<Vec<(Method, Uri, HeaderMap, Vec<u8>)>>>;
    async fn handler(
        State((requests, code)): State<(Requests, u16)>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        requests
            .lock()
            .unwrap()
            .push((method, uri.clone(), headers, body.to_vec()));
        if uri.path() == "/target" {
            return StatusCode::OK.into_response();
        }
        (
            StatusCode::from_u16(code).unwrap(),
            [("location", "/target")],
            PRIVATE_ERROR,
        )
            .into_response()
    }
    for code in [200, 201, 204, 301, 302, 303, 307, 308, 400, 401, 500] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!(
            "http://{}/api/v1/uploads/42/multipart/1",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let requests = Requests::default();
        let app = Router::new()
            .fallback(handler)
            .with_state((requests.clone(), code));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let bytes = b"PK\x03\x04\x00\xffexact ZIP bytes\r\n";
        let result = put_bytes(
            &upload_client().unwrap(),
            "fake-local-key",
            url,
            bytes,
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(result.is_ok(), (200..300).contains(&code), "HTTP {code}");
        if let Err(error) = result {
            assert!(!error.to_string().contains("private"));
        }
        {
            let received = requests.lock().unwrap();
            assert_eq!(received.len(), 1, "redirect/retry for HTTP {code}");
            assert_eq!(received[0].0, Method::PUT);
            assert_eq!(received[0].1.path(), "/api/v1/uploads/42/multipart/1");
            assert_eq!(received[0].2["authorization"], "Bearer fake-local-key");
            assert_eq!(received[0].2["content-type"], "application/octet-stream");
            assert_eq!(received[0].2["content-length"], bytes.len().to_string());
            assert_eq!(received[0].3, bytes);
        }
        server.abort();
        let _ = server.await;
    }
}
