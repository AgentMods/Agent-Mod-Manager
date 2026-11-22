//! Local HTTP and in-memory state/provenance tests; no provider write is used.
use super::*;
use crate::{db::Db, plugin_workflow, storage::AppPaths};
use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use std::sync::{Arc, Mutex};

const SLUG: &str = "manager-publication-test";
const KEY: &str = "fake-key-for-offline-test";

struct Fixture {
    _temp: tempfile::TempDir,
    engine: FileEngine,
    draft: plugin_workflow::DraftPreview,
    adapter: String,
    skill: String,
}
impl Fixture {
    fn new(large: bool) -> Self {
        Self::with_draft_origin(large, false)
    }

    fn with_draft_origin(large: bool, agent: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("manager")).unwrap();
        let engine = FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths);
        let mut input: Value = serde_json::from_str(plugins::test_fixtures::PACKAGES[0].1).unwrap();
        input["slug"] = json!(SLUG);
        input["version"] = json!("1.2.3");
        let mut adapter = serde_json::to_string_pretty(&input).unwrap();
        let mut skill = format!("---\r\nname: {SLUG}\r\ndescription: Publication test\r\n---\r\n\r\nExact text 适配\r\n");
        if large {
            adapter.push_str(&"\t".repeat(640 * 1024));
            skill.push_str(&"\"".repeat(1024 * 1024));
        }
        let draft = plugin_workflow::plan_write_draft(&engine, SLUG, &adapter, &skill).unwrap();
        if agent {
            plugin_workflow::mark_agent_draft(&engine, &draft.draft.id).unwrap();
        }
        engine.approve_plan(&draft.plan.id).unwrap();
        engine.apply_file_ops(&draft.plan.id).unwrap();
        Self {
            _temp: temp,
            engine,
            draft,
            adapter,
            skill,
        }
    }

    fn prepare(&self, target: PublicationTarget) -> PublicationRecord {
        prepare_draft(
            &self.engine,
            &self.draft.draft.id,
            target,
            vec!["中文标签".into(), "other".into()],
        )
        .unwrap()
    }

    fn approve(&self, record: &PublicationRecord) -> PublicationRecord {
        approve_publication(&self.engine, &record.id, &record.preview.digest).unwrap()
    }

    fn reopen(&self) -> FileEngine {
        let paths = (*self.engine.paths).clone();
        FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths)
    }
}

fn receipt(id: i64, release_id: i64, hidden: bool) -> Value {
    json!({"id":id,"releaseId":release_id,"slug":SLUG,"version":"1.2.3","tags":["other","中文标签"],
        "hidden":hidden,"name":"Fixture","reason":if hidden { "moderated" } else { "" },"extraProviderMetadata":true})
}

#[derive(Clone)]
struct Loopback {
    calls: Arc<Mutex<Vec<(Value, usize)>>>,
    reply: Value,
    status: StatusCode,
}

async fn post_mcp(State(state): State<Loopback>, headers: HeaderMap, bytes: Bytes) -> Response {
    assert_eq!(headers[header::AUTHORIZATION], format!("Bearer {KEY}"));
    let accept = headers[header::ACCEPT].to_str().unwrap();
    assert!(accept.contains("application/json") && accept.contains("text/event-stream"));
    let rpc: Value = serde_json::from_slice(&bytes).unwrap();
    state.calls.lock().unwrap().push((rpc.clone(), bytes.len()));
    match rpc["method"].as_str().unwrap() {
        "initialize" => {
            assert!(rpc["params"]["protocolVersion"].is_string());
            let reply = json!({"jsonrpc":"2.0","id":rpc["id"],"result":{
                "capabilities":{"tools":{}},"protocolVersion":"2025-03-26",
                "serverInfo":{"name":"loopback","version":"1"}
            }});
            (
                [
                    (header::CONTENT_TYPE, "text/event-stream"),
                    (
                        header::HeaderName::from_static("mcp-session-id"),
                        "loopback-session",
                    ),
                ],
                format!("event: message\r\ndata: {reply}\r\n\r\n"),
            )
                .into_response()
        }
        method => {
            assert_eq!(headers["mcp-protocol-version"], "2025-03-26");
            assert_eq!(headers["mcp-session-id"], "loopback-session");
            match method {
                "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
                "tools/call" if state.status == StatusCode::OK => {
                    let response = json!({"jsonrpc":"2.0","id":rpc["id"],"result":state.reply});
                    Response::builder()
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(response.to_string()))
                        .unwrap()
                }
                "tools/call" => state.status.into_response(),
                other => panic!("Unexpected procedure: {other}"),
            }
        }
    }
}

async fn local_transport(
    reply: Value,
    status: StatusCode,
) -> (remote::Transport, Loopback, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/v1/mcp", listener.local_addr().unwrap());
    let mock = Loopback {
        calls: Arc::default(),
        reply,
        status,
    };
    let app = Router::new()
        .route("/api/v1/mcp", post(post_mcp))
        .layer(DefaultBodyLimit::max(5 * 1024 * 1024))
        .with_state(mock.clone());
    let handle = tokio::spawn(async { axum::serve(listener, app).await.unwrap() });
    let transport = remote::Transport::connect_to(&url, KEY.into(), &CancellationToken::new())
        .await
        .unwrap();
    (transport, mock, handle)
}

#[tokio::test]
async fn real_mcp_first_publish_consumes_agent_draft_and_preserves_large_approved_payload() {
    let f = Fixture::with_draft_origin(true, true);
    let sibling = plugin_workflow::plan_write_draft(&f.engine, SLUG, &f.adapter, &f.skill).unwrap();
    plugin_workflow::mark_agent_draft(&f.engine, &sibling.draft.id).unwrap();
    f.engine.approve_plan(&sibling.plan.id).unwrap();
    f.engine.apply_file_ops(&sibling.plan.id).unwrap();
    let publication = f.approve(&f.prepare(PublicationTarget::First));
    let (transport, mock, server) = local_transport(
        json!({"structuredContent":{"plugin":receipt(9,40,false)}}),
        StatusCode::OK,
    )
    .await;
    let done = publish_with(
        &f.engine,
        &transport,
        &publication.id,
        &publication.preview.digest,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(done.state, PublicationState::Completed);
    assert_eq!(done.remote.as_ref().unwrap().release_id, 40);
    assert!(!done.remote.as_ref().unwrap().hidden);
    assert!(!f.draft.draft.path.exists());
    assert!(sibling.draft.path.is_dir());
    let reopened = f.reopen();
    assert_eq!(
        plugin_workflow::list_drafts(&reopened)
            .unwrap()
            .into_iter()
            .map(|draft| draft.id)
            .collect::<Vec<_>>(),
        vec![sibling.draft.id]
    );
    assert_eq!(get_publication(&reopened, &done.id).unwrap(), done);
    assert_eq!(
        list_publications(&reopened, None).unwrap(),
        vec![done.clone()]
    );
    // No second tools/call even if a caller submits the exact completed request.
    assert_eq!(
        publish_with(
            &reopened,
            &transport,
            &publication.id,
            &publication.preview.digest,
            &CancellationToken::new()
        )
        .await
        .unwrap(),
        done
    );
    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    let (rpc, size) = &calls[2];
    assert_eq!(rpc["params"]["name"], "publish_manager_plugin");
    assert!(
        rpc["params"]["arguments"]
            == json!({
                "adapter":f.adapter,"skill":f.skill,"tags":["中文标签","other"]
            }),
        "publish arguments must contain exactly the approved texts and tags"
    );
    assert!(f.adapter.len() > 256 * 1024);
    assert!(f.skill.len() > 128 * 1024);
    assert!(*size > 3 * 1024 * 1024);
    server.abort();
}

#[tokio::test]
async fn real_mcp_release_binding_and_hidden_receipt_and_installed_provenance() {
    let f = Fixture::new(false);
    let install = plugin_workflow::plan_install_plugin(&f.engine, &f.draft.draft.id).unwrap();
    f.engine.approve_plan(&install.plan.id).unwrap();
    f.engine.apply_file_ops(&install.plan.id).unwrap();
    plugin_workflow::reconcile_deployments(&f.engine).unwrap();
    let target = PublicationTarget::Release { plugin_id: 41 };
    let installed = prepare_installed(
        &f.engine,
        SLUG,
        target,
        vec!["中文标签".into(), "other".into()],
    )
    .unwrap();
    let different = prepare_installed(
        &f.engine,
        SLUG,
        PublicationTarget::Release { plugin_id: 42 },
        installed.preview.tags.clone(),
    )
    .unwrap();
    assert_ne!(installed.preview.digest, different.preview.digest);
    assert!(approve_publication(&f.engine, &different.id, &installed.preview.digest).is_err());
    let wire = serde_json::to_value(installed.preview.target).unwrap();
    assert_eq!(wire, json!({"kind":"release","pluginId":41}));
    let installed = f.approve(&installed);
    let reply = json!({"plugin":receipt(41,44,true)});
    let (transport, mock, server) = local_transport(
        json!({"content":[{"type":"text","text":reply.to_string()}]}),
        StatusCode::OK,
    )
    .await;
    let done = publish_with(
        &f.engine,
        &transport,
        &installed.id,
        &installed.preview.digest,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(done.remote.unwrap().hidden);
    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    let params = &calls[2].0["params"];
    assert_eq!(params["name"], "publish_manager_plugin_release");
    assert!(
        params["arguments"]
            == json!({
                "adapter":f.adapter,"skill":f.skill,"tags":["中文标签","other"],"plugin_id":41
            }),
        "release arguments must also contain the exact approved target"
    );
    server.abort();
}

#[tokio::test]
async fn known_scope_rejection_is_terminal_and_a_session_failure_is_durably_unknown() {
    let f = Fixture::new(false);
    let record = f.approve(&f.prepare(PublicationTarget::First));
    let cancel = CancellationToken::new();
    let (transport, mock, server) = local_transport(json!({"isError":true,"content":[{"type":"text","text":"this api key does not have the required scope"}]}), StatusCode::OK).await;
    let result = publish_with(
        &f.engine,
        &transport,
        &record.id,
        &record.preview.digest,
        &cancel,
    )
    .await;
    assert!(matches!(result, Err(AppError::Denied(e)) if e.contains("manager_plugins.publish")));
    assert_eq!(
        get_publication(&f.engine, &record.id).unwrap().state,
        PublicationState::Error
    );
    assert_eq!(mock.calls.lock().unwrap().len(), 3);
    server.abort();

    // A new user-reviewed attempt after a definitive rejection gets a new ID.
    let next = f.approve(&f.prepare(PublicationTarget::First));
    assert_ne!(next.id, record.id);
    let (transport, mock, server) = local_transport(Value::Null, StatusCode::NOT_FOUND).await;
    let unknown = publish_with(
        &f.engine,
        &transport,
        &next.id,
        &next.preview.digest,
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(unknown.state, PublicationState::Unknown);
    let reopened = f.reopen();
    assert_eq!(get_publication(&reopened, &next.id).unwrap(), unknown);
    assert!(publish_with(
        &reopened,
        &transport,
        &next.id,
        &next.preview.digest,
        &cancel
    )
    .await
    .is_err());
    let alternate = f.approve(
        &prepare_draft(
            &f.engine,
            &f.draft.draft.id,
            PublicationTarget::Release { plugin_id: 41 },
            vec![],
        )
        .unwrap(),
    );
    assert!(publish_with(
        &reopened,
        &transport,
        &alternate.id,
        &alternate.preview.digest,
        &cancel
    )
    .await
    .is_err());
    assert_eq!(mock.calls.lock().unwrap().len(), 3);
    server.abort();
}

#[derive(Default)]
struct Script {
    calls: Mutex<Vec<(String, Value)>>,
    replies: Mutex<std::collections::VecDeque<Value>>,
}
#[async_trait::async_trait]
impl Remote for Script {
    async fn call(&self, tool: &str, args: Value, _: &CancellationToken) -> AppResult<Value> {
        self.calls.lock().unwrap().push((tool.into(), args));
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(unavailable)
    }
}

#[tokio::test]
async fn unknown_reconciliation_requires_exact_bytes_owner_release_and_read_only_tools() {
    let f = Fixture::new(true);
    let approved = f.approve(&f.prepare(PublicationTarget::Release { plugin_id: 41 }));
    let cancel = CancellationToken::new();
    let write = Script {
        replies: Mutex::new([json!({"plugin":receipt(42,55,false)})].into()),
        ..Default::default()
    };
    let unknown = publish_with(
        &f.engine,
        &write,
        &approved.id,
        &approved.preview.digest,
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(unknown.state, PublicationState::Unknown);
    // Reconciliation reads the sent preview even after local draft no longer matches.
    std::fs::write(f.draft.draft.path.join("SKILL.md"), "stale").unwrap();
    let page = json!({"items":[receipt(41,55,true)]});
    let exact = json!({"adapter":f.adapter,"skill":f.skill,"tags":["other","中文标签"]});
    let mut changed = exact.clone();
    changed["skill"] = json!(format!("{}\n", f.skill));
    let bad_content = Script {
        replies: Mutex::new([page.clone(), changed].into()),
        ..Default::default()
    };
    assert_eq!(
        reconcile_with(&f.engine, &bad_content, unknown.clone(), &cancel)
            .await
            .unwrap(),
        ReconciliationOutcome::Unconfirmed
    );
    let raced = Script {
        replies: Mutex::new(
            [
                page.clone(),
                exact.clone(),
                json!({"items":[receipt(41,56,true)]}),
            ]
            .into(),
        ),
        ..Default::default()
    };
    assert_eq!(
        reconcile_with(&f.engine, &raced, unknown.clone(), &cancel)
            .await
            .unwrap(),
        ReconciliationOutcome::Unconfirmed
    );
    assert_eq!(
        get_publication(&f.engine, &unknown.id).unwrap().state,
        PublicationState::Unknown
    );
    let correct = Script {
        replies: Mutex::new([page.clone(), exact, page].into()),
        ..Default::default()
    };
    assert_eq!(
        reconcile_with(&f.engine, &correct, unknown.clone(), &cancel)
            .await
            .unwrap(),
        ReconciliationOutcome::Verified
    );
    assert_eq!(
        *correct.calls.lock().unwrap(),
        [
            (
                "list_my_manager_plugins".into(),
                json!({"limit":100,"offset":0})
            ),
            (
                "get_my_manager_plugin_texts".into(),
                json!({"plugin_id":41})
            ),
            (
                "list_my_manager_plugins".into(),
                json!({"limit":100,"offset":0})
            ),
        ]
    );
    let after = get_publication(&f.engine, &unknown.id).unwrap();
    assert_eq!(after.state, PublicationState::Completed);
    assert_eq!(after.remote.unwrap().release_id, 55);
}

#[tokio::test]
async fn stale_and_legacy_approvals_are_rejected_before_remote_and_recovery_is_one_way() {
    let f = Fixture::new(false);
    let target = PublicationTarget::First;
    let record = f.prepare(target);
    let metadata = provenance::PublicationMetadata {
        title: "Legacy fixture".into(),
        summary: String::new(),
        description: String::new(),
        tags: vec![],
        game_ids: vec![],
    };
    let (preview, _) = provenance::prepare_package(&f.engine, &f.draft.draft.id, metadata).unwrap();
    let legacy = provenance::create_publication(&f.engine, &preview).unwrap();
    provenance::approve_publication(&f.engine, &legacy.id, &legacy.preview.digest).unwrap();
    assert!(approve_publication(&f.engine, &record.id, &legacy.preview.digest).is_err());
    let remote = Script::default();
    let cancel = CancellationToken::new();
    assert!(publish_with(
        &f.engine,
        &remote,
        &legacy.id,
        &legacy.preview.digest,
        &cancel
    )
    .await
    .is_err());
    let approved = f.approve(&record);
    claim(&f.engine, &approved).unwrap();
    let reopened = f.reopen();
    assert_eq!(recover_pending_publications(&reopened).unwrap(), 1);
    assert_eq!(recover_pending_publications(&reopened).unwrap(), 0);
    assert_eq!(
        get_publication(&reopened, &record.id).unwrap().state,
        PublicationState::Unknown
    );
    assert!(publish_with(
        &reopened,
        &remote,
        &record.id,
        &record.preview.digest,
        &cancel
    )
    .await
    .is_err());
    assert_eq!(
        provenance::list_publications(&reopened, None).unwrap()[0].id,
        legacy.id
    );
    let distinct = f.approve(&f.prepare(PublicationTarget::Release { plugin_id: 88 }));
    std::fs::write(f.draft.draft.path.join("adapter.json"), "{}").unwrap();
    assert!(publish_with(
        &reopened,
        &remote,
        &distinct.id,
        &distinct.preview.digest,
        &cancel
    )
    .await
    .is_err());
    assert_eq!(
        get_publication(&reopened, &distinct.id).unwrap().state,
        PublicationState::Intended
    );
    assert!(remote.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn mcp_response_echo_never_persists_a_credential_or_becomes_a_receipt() {
    let f = Fixture::new(false);
    let approved = f.approve(&f.prepare(PublicationTarget::First));
    let mut plugin = receipt(41, 55, false);
    plugin["reason"] = json!(KEY);
    let (transport, mock, server) = local_transport(
        json!({"structuredContent":{"plugin":plugin}}),
        StatusCode::OK,
    )
    .await;
    let record = publish_with(
        &f.engine,
        &transport,
        &approved.id,
        &approved.preview.digest,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(record.state, PublicationState::Unknown);
    assert!(record.remote.is_none());
    assert!(!serde_json::to_string(&record).unwrap().contains(KEY));
    assert_eq!(mock.calls.lock().unwrap().len(), 3);
    server.abort();
}

#[tokio::test]
async fn dropping_an_inflight_publish_preserves_agent_draft_until_verified_reconciliation() {
    struct InFlight(tokio::sync::Notify);
    #[async_trait::async_trait]
    impl Remote for InFlight {
        async fn call(&self, _: &str, _: Value, _: &CancellationToken) -> AppResult<Value> {
            self.0.notify_one();
            std::future::pending().await
        }
    }

    let f = Fixture::with_draft_origin(false, true);
    let approved = f.approve(&f.prepare(PublicationTarget::First));
    let remote = InFlight(tokio::sync::Notify::new());
    let cancel = CancellationToken::new();
    let mut send = Box::pin(publish_with(
        &f.engine,
        &remote,
        &approved.id,
        &approved.preview.digest,
        &cancel,
    ));
    tokio::select! {
        _ = &mut send => panic!("the tool was still in flight"),
        _ = remote.0.notified() => {},
    }
    assert_eq!(
        get_publication(&f.engine, &approved.id).unwrap().state,
        PublicationState::Pending
    );
    assert!(f.draft.draft.path.is_dir());
    drop(send);
    let reopened = f.reopen();
    let unknown = get_publication(&reopened, &approved.id).unwrap();
    assert_eq!(unknown.state, PublicationState::Unknown);
    assert!(f.draft.draft.path.is_dir());
    assert!(publish_with(
        &reopened,
        &remote,
        &approved.id,
        &approved.preview.digest,
        &cancel
    )
    .await
    .is_err());

    let page = json!({"items":[receipt(41,55,false)]});
    let texts = json!({"adapter":f.adapter,"skill":f.skill,"tags":["other","中文标签"]});
    let verification = Script {
        replies: Mutex::new([page.clone(), texts, page].into()),
        ..Default::default()
    };
    assert_eq!(
        reconcile_with(&reopened, &verification, unknown, &cancel)
            .await
            .unwrap(),
        ReconciliationOutcome::Verified
    );
    assert!(!f.draft.draft.path.exists());
    let completed = get_publication(&f.engine, &approved.id).unwrap();
    assert_eq!(completed.state, PublicationState::Completed);
    assert_eq!(completed.preview, approved.preview);
    assert_eq!(completed.remote.unwrap().release_id, 55);
}

#[tokio::test]
async fn precommit_conflict_and_foreign_id_can_be_corrected_with_fresh_approval() {
    let f = Fixture::new(false);
    let cancel = CancellationToken::new();
    for (target, message, is_conflict) in [
        (
            PublicationTarget::First,
            "manager plugin slug or release version already exists",
            true,
        ),
        (
            PublicationTarget::Release { plugin_id: 90 },
            "manager plugin not found",
            false,
        ),
    ] {
        let record = f.approve(&f.prepare(target));
        let (transport, mock, server) = local_transport(
            json!({"isError":true,"content":[{"type":"text","text":message}]}),
            StatusCode::OK,
        )
        .await;
        let rejected = publish_with(
            &f.engine,
            &transport,
            &record.id,
            &record.preview.digest,
            &cancel,
        )
        .await;
        if is_conflict {
            assert!(
                matches!(rejected, Err(AppError::Conflict(message)) if message.contains("新版本"))
            );
        } else {
            assert!(
                matches!(rejected, Err(AppError::NotFound(message)) if message.contains("插件 ID"))
            );
        }
        assert_eq!(
            get_publication(&f.engine, &record.id).unwrap().state,
            PublicationState::Error
        );
        assert!(publish_with(
            &f.engine,
            &transport,
            &record.id,
            &record.preview.digest,
            &cancel
        )
        .await
        .is_err());
        assert_eq!(mock.calls.lock().unwrap().len(), 3);
        server.abort();
    }
    let amended = f.approve(&f.prepare(PublicationTarget::Release { plugin_id: 41 }));
    let (transport, mock, server) = local_transport(
        json!({"structuredContent":{"plugin":receipt(41,55,false)}}),
        StatusCode::OK,
    )
    .await;
    let done = publish_with(
        &f.engine,
        &transport,
        &amended.id,
        &amended.preview.digest,
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(done.state, PublicationState::Completed);
    assert_eq!(mock.calls.lock().unwrap().len(), 3);
    server.abort();
}
