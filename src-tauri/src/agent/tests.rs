use super::{
    approval::ApprovalResponse,
    events::AgentEvent,
    runtime::{AgentRuntime, TurnRequest},
};
use crate::{
    error::AppResult,
    history::History,
    settings::{SecretStore, Settings},
    transaction::tests::setup,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
};

#[path = "live_modrinth_tests.rs"]
mod live_modrinth;

#[path = "web_download_flow_tests.rs"]
mod web_download_flow;

struct NoSecrets;
impl SecretStore for NoSecrets {
    fn read(&self, _: &str) -> AppResult<Option<String>> {
        Ok(None)
    }
    fn write(&self, _: &str, _: &str) -> AppResult<()> {
        Ok(())
    }
}
fn event(content: Value) -> String {
    format!("data: {}\n\n", content)
}
fn final_reply() -> String {
    format!(
        "{}{}data: [DONE]\n\n",
        event(json!({"choices":[{"index":0,"delta":{"content":"完成"},"finish_reason":"stop"}]})),
        event(json!({"choices":[],"usage":{"total_tokens":12}}))
    )
}
async fn server(responses: Vec<String>) -> (String, tokio::task::JoinHandle<Vec<Value>>) {
    let count = responses.len();
    let mut responses = responses.into_iter();
    server_with(count, move |_, _| responses.next().unwrap()).await
}

async fn server_with(
    count: usize,
    respond: impl FnMut(usize, &Value) -> String + Send + 'static,
) -> (String, tokio::task::JoinHandle<Vec<Value>>) {
    server_with_http(count, "200 OK", "text/event-stream", respond).await
}

async fn server_with_http(
    count: usize,
    status: &'static str,
    content_type: &'static str,
    mut respond: impl FnMut(usize, &Value) -> String + Send + 'static,
) -> (String, tokio::task::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let job = tokio::spawn(async move {
        let mut bodies = Vec::new();
        for index in 0..count {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).await.unwrap();
                header.push(byte[0]);
                assert!(header.len() < 32 * 1024);
            }
            let text = String::from_utf8(header).unwrap();
            let length = text
                .lines()
                .find_map(|s| {
                    s.to_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|s| s.parse::<usize>().ok())
                })
                .unwrap();
            let mut body = vec![0; length];
            socket.read_exact(&mut body).await.unwrap();
            let body = serde_json::from_slice(&body).unwrap();
            let response = respond(index, &body);
            bodies.push(body);
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.len()
            );
            socket.write_all(header.as_bytes()).await.unwrap();
            // Chunk at awkward Unicode/tool JSON boundaries.
            for chunk in response.as_bytes().chunks(7) {
                socket.write_all(chunk).await.unwrap();
            }
        }
        bodies
    });
    (format!("http://{addr}/v1"), job)
}
async fn next(rx: &mut mpsc::UnboundedReceiver<AgentEvent>, kind: &str) -> AgentEvent {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let event = rx.recv().await.unwrap();
            if event.kind == kind {
                return event;
            }
        }
    })
    .await
    .unwrap()
}
fn runtime(
    engine: crate::transaction::FileEngine,
    url: &str,
) -> (AgentRuntime, String, mpsc::UnboundedReceiver<AgentEvent>) {
    let settings = Settings::with_secret_store(engine.db.clone(), Arc::new(NoSecrets));
    let mut config = settings.load().unwrap();
    config.base_url = url.into();
    config.model = "mock-model".into();
    settings.save(&config).unwrap();
    let conversation = History::new(engine.db.clone())
        .create_conversation("Test", None)
        .unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    let agent = AgentRuntime::new(
        engine,
        settings,
        Arc::new(move |e| {
            let _ = tx.send(e.clone());
        }),
    )
    .unwrap();
    (agent, conversation.id, rx)
}

#[tokio::test]
async fn real_sse_tool_loop_waits_for_approval_and_replays_history() {
    let (_dir, engine, game) = setup();
    let file = game.join("a");
    std::fs::write(&file, b"retain").unwrap();
    let args = json!({"paths":[file],"reason":"test"}).to_string();
    let response = format!(
        "{}data: [DONE]\n\n",
        event(
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"provider_call","type":"function","function":{"name":"trash_paths","arguments":args}}]},"finish_reason":"tool_calls"}]})
        )
    );
    let (url, job) = server(vec![response, final_reply()]).await;
    let (agent, conv, mut rx) = runtime(engine, &url);
    let turn = agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "移入回收站".into(),
            model: None,
            permission_mode: None,
            reasoning_effort: None,
        })
        .unwrap();
    let approval = next(&mut rx, "approval-request").await;
    assert!(file.exists());
    assert!(approval.payload["preview"]["entries"].is_array());
    assert!(agent
        .respond_approval(
            "wrong",
            approval.payload["approvalId"].as_str().unwrap(),
            ApprovalResponse {
                approved: true,
                answer: Value::Null
            }
        )
        .is_err());
    agent
        .respond_approval(
            &conv,
            approval.payload["approvalId"].as_str().unwrap(),
            ApprovalResponse {
                approved: true,
                answer: Value::Null,
            },
        )
        .unwrap();
    let done = next(&mut rx, "done").await;
    assert_eq!(done.turn_id, turn);
    assert_eq!(done.payload["status"], "done");
    assert!(!file.exists());
    let requests = job.await.unwrap();
    assert!(requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "tool" && m["tool_call_id"] == "provider_call"));
    let history = History::new(agent.engine.db.clone())
        .get_conversation(&conv)
        .unwrap();
    assert_eq!(history.messages.last().unwrap().content, "完成");
    let events = agent.events.replay(&conv, 0, 100).unwrap();
    assert!(events.windows(2).all(|p| p[0].seq < p[1].seq));
    assert!(events.iter().any(|e| e.kind == "usage"));
    let after = events[1].seq;
    assert!(agent
        .events
        .replay(&conv, after, 100)
        .unwrap()
        .iter()
        .all(|e| e.seq > after));
}

#[tokio::test]
async fn automatic_fomod_sse_install_applies_the_reviewed_plan_and_returns_a_receipt() {
    use crate::{games, plugins, util};
    use std::{io::Write, path::Path};

    let (_dir, engine, root) = setup();
    plugins::seed_builtins(&engine.db, &engine.paths).unwrap();
    let data = root.join("Data");
    std::fs::create_dir(&data).unwrap();
    std::fs::write(root.join("SkyrimSE.exe"), b"game").unwrap();
    std::fs::write(data.join("Skyrim.esm"), b"game").unwrap();
    let game = games::register_game(&engine.db, &root, Some("skyrim-se")).unwrap();
    let plugin = data.join("base.esp");
    let texture = data.join("textures/landscape/detail.dds");
    std::fs::write(&plugin, b"original").unwrap();

    let archive = engine.paths.downloads().join("fixture.zip");
    let xml = br#"<config><requiredInstallFiles>
        <file source="Data/base.esp" destination="base.esp"/>
        <folder source="Core" destination="textures"/>
        </requiredInstallFiles></config>"#;
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
    for (name, bytes) in [
        ("Release/fomod/ModuleConfig.xml", xml.as_slice()),
        ("Release/Data/base.esp", b"replacement"),
        ("Release/Core/landscape/detail.dds", b"texture"),
        ("Release/Other/extra.dds", b"unselected"),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
    let sha = util::sha256_file(&archive).unwrap();
    let args = json!({"request": {
        "gameId": game.id,
        "artifactPath": archive,
        "filename": "fixture.zip",
        "name": "FOMOD fixture",
        "modType": "fomod",
        "fomodSelection": "automatic",
        "expectedSha256": sha,
    }});
    let response = format!(
        "{}data: [DONE]\n\n",
        event(json!({"choices":[{"index":0,"delta":{"tool_calls":[{
            "index":0,"id":"fomod-install","type":"function",
            "function":{"name":"install_mod","arguments":args.to_string()}
        }]},"finish_reason":"tool_calls"}]}))
    );
    let (url, job) = server(vec![response, final_reply()]).await;
    let (agent, conv, mut rx) = runtime(engine, &url);
    let turn = agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "安装归档中可自动选择的 FOMOD 文件".into(),
            model: None,
            permission_mode: Some(super::policy::PermissionMode::AutoApprove),
            reasoning_effort: None,
        })
        .unwrap();

    let approval = next(&mut rx, "approval-request").await;
    assert_eq!(approval.turn_id, turn);
    assert_eq!(approval.payload["title"], "install_mod");
    let preview = &approval.payload["preview"];
    assert_eq!(preview["modType"], "fomod");
    assert_eq!(preview["artifactSha256"], sha);
    assert_eq!(preview["requiresReview"], true);
    let plan = &preview["plan"];
    let id = plan["id"].as_str().unwrap();
    assert_eq!(*plan, json!(agent.engine.get_plan(id).unwrap()));
    let entries = plan["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    for (entry, dst, archive_path, bytes) in [
        (
            &entries[0],
            &plugin,
            "Release/Data/base.esp",
            &b"replacement"[..],
        ),
        (
            &entries[1],
            &texture,
            "Release/Core/landscape/detail.dds",
            &b"texture"[..],
        ),
    ] {
        assert_eq!(entry["operation"]["op"], "copy");
        assert_eq!(entry["operation"]["dst"], json!(dst));
        let src = Path::new(entry["operation"]["src"].as_str().unwrap());
        assert!(src.starts_with(agent.engine.paths.staging()));
        assert!(src.ends_with(archive_path));
        assert_eq!(std::fs::read(src).unwrap(), bytes);
        assert_eq!(entry["source"]["sha256"], util::sha256_file(src).unwrap());
    }
    assert_eq!(std::fs::read(&plugin).unwrap(), b"original");
    assert_eq!(
        entries[0]["before"]["sha256"],
        util::sha256_file(&plugin).unwrap()
    );
    assert!(entries[1]["before"].is_null());
    assert!(!texture.exists());
    let transaction = || -> (String, String) {
        agent
            .engine
            .db
            .conn()
            .query_row(
                "SELECT status, journal_json FROM file_transactions WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    };
    assert_eq!(transaction(), ("planned".into(), "[]".into()));

    agent
        .respond_approval(
            &conv,
            approval.payload["approvalId"].as_str().unwrap(),
            ApprovalResponse {
                approved: true,
                answer: Value::Null,
            },
        )
        .unwrap();
    let receipt = json!({"status": "committed", "transactionId": id, "fileCount": 2});
    let result = next(&mut rx, "tool-result").await;
    assert_eq!(result.payload["result"], receipt);
    assert_eq!(next(&mut rx, "done").await.payload["status"], "done");
    assert_eq!(std::fs::read(plugin).unwrap(), b"replacement");
    assert_eq!(std::fs::read(texture).unwrap(), b"texture");
    let (status, journal) = transaction();
    assert_eq!(status, "committed");
    let journal: Vec<Value> = serde_json::from_str(&journal).unwrap();
    assert_eq!(journal.len(), entries.len());
    for (journal_entry, planned_entry) in journal.iter().zip(entries) {
        assert_eq!(journal_entry["entry"], *planned_entry);
    }

    let requests = job.await.unwrap();
    let tools = requests[0]["tools"].as_array().unwrap();
    for name in ["plan_install", "install_mod"] {
        let tool = tools
            .iter()
            .find(|tool| tool["function"]["name"] == name)
            .unwrap();
        let schema = &tool["function"]["parameters"]["properties"]["request"];
        assert_eq!(
            schema["properties"]["fomodSelection"]["enum"],
            json!(["automatic"])
        );
    }
    let model_result = requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool" && message["tool_call_id"] == "fomod-install")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(model_result["content"].as_str().unwrap()).unwrap(),
        receipt
    );
}

#[tokio::test]
async fn reject_approval_does_not_touch_game_files() {
    let (_dir, engine, game) = setup();
    let file = game.join("a");
    std::fs::write(&file, b"retain").unwrap();
    let response = format!(
        "{}data: [DONE]\n\n",
        event(
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call","function":{"name":"trash_paths","arguments":json!({"paths":[file]}).to_string()}}]},"finish_reason":"tool_calls"}]})
        )
    );
    let (url, job) = server(vec![response, final_reply()]).await;
    let (agent, conv, mut rx) = runtime(engine, &url);
    agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "trash".into(),
            model: None,
            permission_mode: None,
            reasoning_effort: None,
        })
        .unwrap();
    let a = next(&mut rx, "approval-request").await;
    agent
        .respond_approval(
            &conv,
            a.payload["approvalId"].as_str().unwrap(),
            ApprovalResponse {
                approved: false,
                answer: Value::Null,
            },
        )
        .unwrap();
    next(&mut rx, "done").await;
    assert_eq!(std::fs::read(file).unwrap(), b"retain");
    job.await.unwrap();
}

#[tokio::test]
async fn rejected_plugin_publication_never_grants_upload_consent_in_either_review_mode() {
    use crate::{agent::policy::PermissionMode, plugin_publish, plugin_workflow, plugins};
    for mode in [PermissionMode::RequestApproval, PermissionMode::AutoApprove] {
        let (_dir, engine, _) = setup();
        let mut adapter: Value = serde_json::from_str(plugins::BUILTIN_PACKAGES[0].1).unwrap();
        adapter["slug"] = json!("rejected-publication");
        let draft = plugin_workflow::plan_write_draft(&engine, "rejected-publication", &adapter.to_string(), "---\nname: rejected-publication\ndescription: Reviewed fixture\n---\nPublic instructions.\n").unwrap();
        engine.approve_plan(&draft.plan.id).unwrap();
        engine.apply_file_ops(&draft.plan.id).unwrap();
        let args = json!({"draft_id":draft.draft.id,"metadata":{"title":"Public fixture","summary":"Summary","description":"Description"}});
        let response = format!(
            "{}data: [DONE]\n\n",
            event(
                json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"publication-call","function":{"name":"publish_plugin","arguments":args.to_string()}}]},"finish_reason":"tool_calls"}]})
            )
        );
        let (url, job) = server(vec![response, final_reply()]).await;
        let (agent, conv, mut rx) = runtime(engine, &url);
        agent
            .start_turn(TurnRequest {
                conversation_id: conv.clone(),
                content: "发布适配包".into(),
                model: None,
                permission_mode: Some(mode),
                reasoning_effort: None,
            })
            .unwrap();
        let approval = next(&mut rx, "approval-request").await;
        assert_eq!(approval.payload["title"], "publish_plugin");
        assert_eq!(
            approval.payload["preview"]["preview"]["package"]["files"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let id = approval.payload["preview"]["id"].as_str().unwrap();
        assert!(plugin_publish::get_publication(&agent.engine, id)
            .unwrap()
            .approved_digest
            .is_none());
        agent
            .respond_approval(
                &conv,
                approval.payload["approvalId"].as_str().unwrap(),
                ApprovalResponse {
                    approved: false,
                    answer: Value::Null,
                },
            )
            .unwrap();
        next(&mut rx, "done").await;
        let record = plugin_publish::get_publication(&agent.engine, id).unwrap();
        assert!(record.approved_digest.is_none());
        assert!(record.file_id.is_none());
        assert_eq!(record.phase, plugin_publish::PublicationPhase::Prepared);
        assert_eq!(job.await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn rejected_online_identification_never_searches_or_registers_in_either_review_mode() {
    use crate::agent::policy::PermissionMode;
    for mode in [PermissionMode::RequestApproval, PermissionMode::AutoApprove] {
        let (_dir, engine, game) = setup();
        std::fs::write(game.join("FixtureGame.exe"), b"fixture").unwrap();
        let args = json!({"directory":game,"public_query":"Public Fixture game"});
        let response = format!(
            "{}data: [DONE]\n\n",
            event(
                json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"identity-call","function":{"name":"search_game_identity","arguments":args.to_string()}}]},"finish_reason":"tool_calls"}]})
            )
        );
        let (url, job) = server(vec![response, final_reply()]).await;
        let (agent, conv, mut rx) = runtime(engine, &url);
        agent
            .start_turn(TurnRequest {
                conversation_id: conv.clone(),
                content: "识别这个游戏，先审核搜索词".into(),
                model: None,
                permission_mode: Some(mode),
                reasoning_effort: None,
            })
            .unwrap();
        let approval = next(&mut rx, "approval-request").await;
        assert_eq!(approval.payload["title"], "search_game_identity");
        assert_eq!(approval.payload["preview"]["arguments"], args);
        assert_eq!(
            approval.payload["preview"]["destination"],
            crate::sources::web_search::ENDPOINT
        );
        agent
            .respond_approval(
                &conv,
                approval.payload["approvalId"].as_str().unwrap(),
                ApprovalResponse {
                    approved: false,
                    answer: Value::Null,
                },
            )
            .unwrap();
        next(&mut rx, "done").await;
        {
            let conn = agent.engine.db.conn();
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM game_identity_searches", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM games", [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
        assert_eq!(job.await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn questionnaire_waits_for_valid_answers_and_resumes_with_ordered_skips() {
    let (_dir, engine, _game) = setup();
    let questions = json!({"title":"确认适配信息","questions":[
        {"question":"平台？","options":["Windows PC","其他"]},
        {"question":"目录？"}
    ]});
    let response = format!(
        "{}data: [DONE]\n\n",
        event(
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"ask","function":{
            "name":"ask_user","arguments":questions.to_string()
        }}]},"finish_reason":"tool_calls"}]})
        )
    );
    let (url, job) = server(vec![response, final_reply()]).await;
    let (agent, conv, mut rx) = runtime(engine, &url);
    agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "确认适配".into(),
            model: None,
            permission_mode: Some(crate::agent::policy::PermissionMode::FullTrust),
            reasoning_effort: None,
        })
        .unwrap();
    let request = next(&mut rx, "approval-request").await;
    assert_eq!(request.payload["preview"], questions);
    let id = request.payload["approvalId"].as_str().unwrap();
    assert!(agent
        .respond_approval(
            &conv,
            id,
            ApprovalResponse {
                approved: true,
                answer: json!("[]")
            }
        )
        .is_err());
    agent
        .respond_approval(
            &conv,
            id,
            ApprovalResponse {
                approved: true,
                answer: json!(r#"["Windows PC",null]"#),
            },
        )
        .unwrap();
    let resolved = next(&mut rx, "approval-resolved").await;
    assert_eq!(resolved.payload["status"], "approved");
    let result = next(&mut rx, "tool-result").await;
    assert_eq!(
        result.payload["result"],
        json!({"answer":["Windows PC",null]})
    );
    assert_eq!(next(&mut rx, "done").await.payload["status"], "done");
    let requests = job.await.unwrap();
    let response = requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["tool_call_id"] == "ask")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(response["content"].as_str().unwrap()).unwrap(),
        result.payload["result"]
    );
}

#[tokio::test]
async fn cancellation_unblocks_approval_and_next_turn_repairs_tool_protocol() {
    let (_dir, engine, _game) = setup();
    let response = format!(
        "{}data: [DONE]\n\n",
        event(
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"ask","function":{"name":"ask_user","arguments":"{\"question\":\"请选择\"}"}}]},"finish_reason":"tool_calls"}]})
        )
    );
    let (url, job) = server(vec![response, final_reply()]).await;
    let (agent, conv, mut rx) = runtime(engine, &url);
    let turn = agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "ask".into(),
            model: None,
            permission_mode: None,
            reasoning_effort: None,
        })
        .unwrap();
    next(&mut rx, "approval-request").await;
    assert!(agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "duplicate".into(),
            model: None,
            permission_mode: None,
            reasoning_effort: None,
        })
        .is_err());
    agent.cancel_turn(&conv, &turn).unwrap();
    let done = next(&mut rx, "done").await;
    assert_eq!(done.payload["status"], "cancelled");
    tokio::task::yield_now().await;
    agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "continue".into(),
            model: None,
            permission_mode: None,
            reasoning_effort: None,
        })
        .unwrap();
    let done = next(&mut rx, "done").await;
    assert_eq!(done.payload["status"], "done");
    let requests = job.await.unwrap();
    let messages = requests[1]["messages"].as_array().unwrap();
    let previous_tool = messages
        .iter()
        .position(|m| m["tool_call_id"] == "ask")
        .unwrap();
    let user = messages.iter().rposition(|m| m["role"] == "user").unwrap();
    assert!(previous_tool < user);
}

#[tokio::test]
async fn agent_turn_uses_only_reasoning_effort_enabled_for_the_active_model() {
    use crate::settings::{AgentModelSettings, ReasoningEffort};

    let (_dir, engine, _) = setup();
    let (url, job) = server(vec![final_reply(), final_reply(), final_reply()]).await;
    let (agent, conv, mut rx) = runtime(engine, &url);
    let mut settings = agent.settings.load().unwrap();
    settings.model = "second-model".into();
    settings.models = vec![
        AgentModelSettings {
            display_name: "Mock model".into(),
            id: "mock-model".into(),
            context_budget: 32_768,
            reasoning_levels: vec![ReasoningEffort::Low],
        },
        AgentModelSettings {
            display_name: "Second model".into(),
            id: "second-model".into(),
            context_budget: 128_000,
            reasoning_levels: vec![ReasoningEffort::None, ReasoningEffort::High],
        },
    ];
    agent.settings.save(&settings).unwrap();
    assert_eq!(agent.settings.load().unwrap(), settings);
    let request = |reasoning_effort| TurnRequest {
        conversation_id: conv.clone(),
        content: "hi".into(),
        model: None,
        permission_mode: None,
        reasoning_effort,
    };
    assert!(matches!(
        agent.start_turn(request(Some(ReasoningEffort::Low))),
        Err(crate::error::AppError::Invalid(_))
    ));
    for effort in [
        Some(ReasoningEffort::High),
        Some(ReasoningEffort::None),
        None,
    ] {
        agent.start_turn(request(effort)).unwrap();
        assert_eq!(next(&mut rx, "done").await.payload["status"], "done");
    }
    let requests = job.await.unwrap();
    assert!(requests
        .iter()
        .all(|request| request["model"] == "second-model"));
    assert_eq!(requests[0]["reasoning_effort"], "high");
    assert_eq!(requests[1]["reasoning_effort"], "none");
    assert!(requests[2].get("reasoning_effort").is_none());
}

#[tokio::test]
async fn per_turn_model_override_uses_configured_budget_and_preserves_saved_default() {
    use crate::settings::{AgentModelSettings, ReasoningEffort};

    let (_dir, engine, _) = setup();
    let (url, job) = server(vec![final_reply()]).await;
    let (agent, conv, mut rx) = runtime(engine, &url);
    let mut settings = agent.settings.load().unwrap();
    settings.models = vec![
        AgentModelSettings {
            display_name: "Mock model".into(),
            id: "mock-model".into(),
            context_budget: 1,
            reasoning_levels: vec![ReasoningEffort::Low],
        },
        AgentModelSettings {
            display_name: "Second model".into(),
            id: "second-model".into(),
            context_budget: 32_768,
            reasoning_levels: vec![ReasoningEffort::Ultra],
        },
    ];
    settings.model = "mock-model".into();
    agent.settings.save(&settings).unwrap();

    agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "hi".into(),
            model: Some("second-model".into()),
            permission_mode: None,
            reasoning_effort: Some(ReasoningEffort::Ultra),
        })
        .unwrap();
    assert_eq!(next(&mut rx, "done").await.payload["status"], "done");

    let requests = job.await.unwrap();
    assert_eq!(requests[0]["model"], "second-model");
    assert_eq!(requests[0]["reasoning_effort"], "ultra");
    assert_eq!(agent.settings.load().unwrap().model, "mock-model");
}

#[test]
fn per_turn_model_and_reasoning_are_validated_before_writing_messages() {
    use crate::settings::{AgentModelSettings, ReasoningEffort};

    let (_dir, engine, _) = setup();
    let (agent, conv, _rx) = runtime(engine, "http://127.0.0.1:1");
    let mut settings = agent.settings.load().unwrap();
    settings.models = vec![AgentModelSettings {
        display_name: "Second model".into(),
        id: "second-model".into(),
        context_budget: 32_768,
        reasoning_levels: vec![ReasoningEffort::High],
    }];
    settings.model = "second-model".into();
    agent.settings.save(&settings).unwrap();
    let history = History::new(agent.engine.db.clone());
    let initial_messages = history.get_conversation(&conv).unwrap().messages.len();
    let initial_turns: i64 = agent
        .engine
        .db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM agent_turns WHERE conversation_id=?1",
            [&conv],
            |row| row.get(0),
        )
        .unwrap();
    let parsed: TurnRequest = serde_json::from_value(json!({
        "conversationId": conv,
        "content": "parsed",
        "model": "second-model",
        "permissionMode": null,
        "reasoningEffort": "high"
    }))
    .unwrap();
    assert_eq!(parsed.model.as_deref(), Some("second-model"));
    assert_eq!(parsed.reasoning_effort, Some(ReasoningEffort::High));
    let parsed_default: TurnRequest = serde_json::from_value(json!({
        "conversationId": "conversation",
        "content": "parsed"
    }))
    .unwrap();
    assert!(parsed_default.model.is_none());
    let parsed_null: TurnRequest = serde_json::from_value(json!({
        "conversationId": "conversation",
        "content": "parsed",
        "model": null
    }))
    .unwrap();
    assert!(parsed_null.model.is_none());

    let invalid_model = agent.start_turn(TurnRequest {
        conversation_id: conv.clone(),
        content: "invalid model".into(),
        model: Some("not-configured".into()),
        permission_mode: None,
        reasoning_effort: None,
    });
    assert!(matches!(
        invalid_model,
        Err(crate::error::AppError::Invalid(_))
    ));

    let invalid_reasoning = agent.start_turn(TurnRequest {
        conversation_id: conv.clone(),
        content: "invalid reasoning".into(),
        model: Some("second-model".into()),
        permission_mode: None,
        reasoning_effort: Some(ReasoningEffort::Low),
    });
    assert!(matches!(
        invalid_reasoning,
        Err(crate::error::AppError::Invalid(_))
    ));
    assert_eq!(
        history.get_conversation(&conv).unwrap().messages.len(),
        initial_messages
    );
    assert_eq!(
        agent
            .engine
            .db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM agent_turns WHERE conversation_id=?1",
                [&conv],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        initial_turns
    );
}

#[tokio::test]
async fn model_http_error_keeps_status_and_provider_details_with_key_redacted() {
    use super::client::{ModelClient, ModelConfig};

    let (base_url, job) = server_with_http(1, "429 Too Many Requests", "application/json", |_, _| {
        r#"{"error":{"code":"rate_limit_exceeded","message":"Key local-test-credential hit the limit","details":{"retry_after":42}}}"#.into()
    })
    .await;
    let error = ModelClient::new()
        .unwrap()
        .stream(
            &ModelConfig {
                reasoning_effort: None,
                base_url,
                model: "mock-model".into(),
                api_key: Some("local-test-credential".into()),
            },
            &[],
            &[],
            &tokio_util::sync::CancellationToken::new(),
            |_, _| panic!("an HTTP error cannot produce a delta"),
        )
        .await
        .err()
        .expect("expected HTTP error")
        .to_string();
    assert!(error.contains("HTTP 429"), "{error}");
    assert!(
        error.contains(r#""code": "rate_limit_exceeded""#),
        "{error}"
    );
    assert!(error.contains("Key [REDACTED] hit the limit"), "{error}");
    assert!(error.contains(r#""retry_after": 42"#), "{error}");
    assert!(!error.contains("local-test-credential"));
    job.await.unwrap();
}

/// Serves each scripted reply on its own connection. A reply is cut short (declared
/// length larger than what is sent) when `truncate` is true, which reqwest reports as a body error.
async fn flaky_server(replies: Vec<(String, bool)>) -> (String, tokio::task::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let job = tokio::spawn(async move {
        let mut served = 0;
        for (reply, truncate) in replies {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).await.unwrap();
                header.push(byte[0]);
            }
            let length = String::from_utf8(header)
                .unwrap()
                .lines()
                .find_map(|s| {
                    s.to_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|s| s.parse::<usize>().ok())
                })
                .unwrap();
            let mut body = vec![0; length];
            socket.read_exact(&mut body).await.unwrap();
            let declared = reply.len() + if truncate { 4096 } else { 0 };
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n");
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(reply.as_bytes()).await.unwrap();
            socket.shutdown().await.ok();
            served += 1;
        }
        served
    });
    (format!("http://{addr}/v1"), job)
}

fn mock_config(base_url: String) -> super::client::ModelConfig {
    super::client::ModelConfig {
        reasoning_effort: None,
        base_url,
        model: "mock-model".into(),
        api_key: None,
    }
}

#[tokio::test]
async fn model_stream_dropped_before_any_output_is_retried() {
    let (base_url, job) = flaky_server(vec![(String::new(), true), (final_reply(), false)]).await;
    let mut seen = String::new();
    let completion = super::client::ModelClient::new()
        .unwrap()
        .stream(
            &mock_config(base_url),
            &[],
            &[],
            &tokio_util::sync::CancellationToken::new(),
            |delta, _| {
                seen.push_str(&delta.content);
                Ok(())
            },
        )
        .await
        .expect("the retry should succeed");
    assert_eq!(completion.content, "完成");
    assert_eq!(seen, "完成");
    assert_eq!(job.await.unwrap(), 2);
}

#[tokio::test]
async fn model_stream_dropped_after_output_is_not_retried() {
    let partial = event(json!({"choices":[{"index":0,"delta":{"content":"半截"}}]}));
    let (base_url, job) = flaky_server(vec![(partial, true)]).await;
    let error = super::client::ModelClient::new()
        .unwrap()
        .stream(
            &mock_config(base_url),
            &[],
            &[],
            &tokio_util::sync::CancellationToken::new(),
            |_, _| Ok(()),
        )
        .await
        .err()
        .expect("a partial reply cannot be replayed")
        .to_string();
    assert!(error.contains("模型流连接中断"), "{error}");
    assert_eq!(job.await.unwrap(), 1);
}
#[tokio::test]
async fn unconfigured_model_reports_why_the_turn_failed_without_a_delta() {
    let (_dir, engine, _) = setup();
    let (agent, conv, mut rx) = runtime(engine, "http://127.0.0.1:1/v1");
    agent.settings.set("model", json!("")).unwrap();
    let turn = agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "hi".into(),
            model: None,
            permission_mode: None,
            reasoning_effort: None,
        })
        .unwrap();
    let error = next(&mut rx, "error").await;
    assert_eq!(error.turn_id, turn);
    assert_eq!(error.payload["message"], "无效输入: 请先在设置中选择模型");
    assert_eq!(next(&mut rx, "done").await.payload["status"], "error");
    let events = agent.events.replay(&conv, 0, 100).unwrap();
    assert!(!events.iter().any(|event| event.kind == "delta"));
    let history = History::new(agent.engine.db.clone())
        .get_conversation(&conv)
        .unwrap();
    assert_eq!(error.message_id.as_ref(), Some(&history.messages[1].id));
}

#[tokio::test]
async fn model_sse_error_replays_on_the_partial_reply_without_entering_history() {
    let (_dir, engine, _) = setup();
    let partial = "已经生成的内容";
    let response = format!(
        "{}{}",
        event(json!({"choices":[{"delta":{"content":partial}}]})),
        event(
            json!({"error":{"code":"overloaded","message":"模型过载","details":{"retry_after":12}}})
        ),
    );
    let (url, job) = server(vec![response]).await;
    let (agent, conv, mut rx) = runtime(engine, &url);
    let turn = agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "hi".into(),
            model: None,
            permission_mode: None,
            reasoning_effort: None,
        })
        .unwrap();
    let delta = next(&mut rx, "delta").await;
    let error = next(&mut rx, "error").await;
    assert_eq!(error.turn_id, turn);
    assert_eq!(error.message_id, delta.message_id);
    assert_eq!(next(&mut rx, "done").await.payload["status"], "error");
    let diagnostic = error.payload["message"].as_str().unwrap();
    assert!(
        diagnostic.contains(r#""code": "overloaded""#),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("模型过载"), "{diagnostic}");
    assert!(diagnostic.contains(r#""retry_after": 12"#), "{diagnostic}");
    let replay = agent.events.replay(&conv, delta.seq, 100).unwrap();
    assert_eq!(replay[0].seq, error.seq);
    assert_eq!(replay[0].payload, error.payload);
    let history = History::new(agent.engine.db.clone())
        .get_conversation(&conv)
        .unwrap();
    assert_eq!(history.messages[1].content, partial);
    job.await.unwrap();
}

#[tokio::test]
async fn truncated_tool_stream_never_executes() {
    let (_dir, engine, game) = setup();
    let file = game.join("a");
    std::fs::write(&file, b"retain").unwrap();
    let response = event(
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call","function":{"name":"trash_paths","arguments":json!({"paths":[file]}).to_string()}}]},"finish_reason":"tool_calls"}]}),
    );
    let (url, job) = server(vec![response]).await;
    let (agent, conv, mut rx) = runtime(engine, &url);
    agent
        .start_turn(TurnRequest {
            conversation_id: conv,
            content: "trash".into(),
            model: None,
            permission_mode: Some(super::policy::PermissionMode::FullTrust),
            reasoning_effort: None,
        })
        .unwrap();
    let done = next(&mut rx, "done").await;
    assert_eq!(done.payload["status"], "error");
    assert!(file.exists());
    job.await.unwrap();
}

#[tokio::test]
async fn context_compression_preserves_full_history_and_summarizes_only_older_turns() {
    let (_dir, engine, _game) = setup();
    let (url, job) = server(vec![final_reply(), final_reply()]).await;
    let (agent, conv, mut rx) = runtime(engine, &url);
    let history = History::new(agent.engine.db.clone());
    let old = "历史任务".repeat(6000);
    history
        .append_message(&conv, "user", &old, None, None)
        .unwrap();
    history
        .append_message(&conv, "assistant", "previous answer", None, None)
        .unwrap();
    // The input budget now includes the registered tool schemas. Keep enough
    // room for the tool contract while making the deliberately long history
    // force a compression pass.
    agent.settings.set("context_budget", json!(32768)).unwrap();
    agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "继续安装".into(),
            model: None,
            permission_mode: None,
            reasoning_effort: None,
        })
        .unwrap();
    assert_eq!(next(&mut rx, "done").await.payload["status"], "done");
    let bodies = job.await.unwrap();
    assert!(bodies[0].get("tools").is_none());
    let final_request = bodies[1]["messages"].as_array().unwrap();
    assert!(final_request.iter().any(|m| m["content"] == "继续安装"));
    assert!(!final_request.iter().any(|m| m["content"] == old));
    let detail = history.get_conversation(&conv).unwrap();
    assert!(detail.messages.iter().any(|m| m.content == old));
    assert_eq!(detail.conversation.task_summary.as_deref(), Some("完成"));
}

#[tokio::test]
async fn cancel_stalled_model_request_finishes_without_waiting_for_network_timeout() {
    let (_dir, engine, _game) = setup();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (tx, accepted) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        tx.send(()).unwrap();
        let _socket = socket;
        std::future::pending::<()>().await;
    });
    let (agent, conv, mut rx) = runtime(engine, &format!("http://{address}/v1"));
    let turn = agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "request".into(),
            model: None,
            permission_mode: None,
            reasoning_effort: None,
        })
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), accepted)
        .await
        .unwrap()
        .unwrap();
    agent.cancel_turn(&conv, &turn).unwrap();
    assert_eq!(next(&mut rx, "done").await.payload["status"], "cancelled");
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn auto_approve_applies_only_registered_game_plan_without_prompt() {
    let (_dir, engine, game) = setup();
    let source = game.join("existing-mod.txt");
    std::fs::write(&source, b"mod").unwrap();
    let dst = game.join("mod.txt");
    let plan = engine
        .plan_file_ops(
            vec![crate::transaction::FileOp::Copy {
                src: source,
                dst: dst.clone(),
            }],
            "Install".into(),
        )
        .unwrap();
    let response = format!(
        "{}data: [DONE]\n\n",
        event(
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"install","function":{"name":"apply_file_ops","arguments":json!({"plan_id":plan.id}).to_string()}}]},"finish_reason":"tool_calls"}]})
        )
    );
    let (url, job) = server(vec![response, final_reply()]).await;
    let (agent, conv, mut rx) = runtime(engine, &url);
    agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "安装".into(),
            model: None,
            permission_mode: Some(super::policy::PermissionMode::AutoApprove),
            reasoning_effort: None,
        })
        .unwrap();
    assert_eq!(next(&mut rx, "done").await.payload["status"], "done");
    assert_eq!(std::fs::read(dst).unwrap(), b"mod");
    assert!(!agent
        .events
        .replay(&conv, 0, 100)
        .unwrap()
        .iter()
        .any(|e| e.kind == "approval-request"));
    job.await.unwrap();
}

#[tokio::test]
async fn auto_approve_requires_confirmation_for_move_out_of_manager_storage() {
    let (_d, e, g) = setup();
    let src = e.paths.downloads().join("a");
    std::fs::write(&src, b"keep").unwrap();
    let plan = e
        .plan_file_ops(
            vec![crate::transaction::FileOp::Move {
                src: src.clone(),
                dst: g.join("a"),
            }],
            "Move".into(),
        )
        .unwrap();
    let response = format!(
        "{}data: [DONE]\n\n",
        event(
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"move","function":{"name":"apply_file_ops","arguments":json!({"plan_id":plan.id}).to_string()}}]},"finish_reason":"tool_calls"}]})
        )
    );
    let (url, job) = server(vec![response, final_reply()]).await;
    let (agent, conv, mut rx) = runtime(e, &url);
    agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "Move".into(),
            model: None,
            permission_mode: Some(super::policy::PermissionMode::AutoApprove),
            reasoning_effort: None,
        })
        .unwrap();
    let approval = next(&mut rx, "approval-request").await;
    assert!(src.exists());
    agent
        .respond_approval(
            &conv,
            approval.payload["approvalId"].as_str().unwrap(),
            ApprovalResponse {
                approved: false,
                answer: Value::Null,
            },
        )
        .unwrap();
    next(&mut rx, "done").await;
    assert!(src.exists());
    assert!(!g.join("a").exists());
    job.await.unwrap();
}

#[test]
fn restart_replay_includes_interrupted_turn_and_cancelled_approval() {
    let (_d, e, _g) = setup();
    let history = History::new(e.db.clone());
    let conv = history.create_conversation("Restart", None).unwrap().id;
    e.db.conn().execute("INSERT INTO agent_turns(id,conversation_id,status,created_at) VALUES ('turn',?1,'running','now')",[&conv]).unwrap();
    e.db.conn().execute("INSERT INTO approvals(id,conversation_id,kind,title,status,created_at) VALUES ('approval',?1,'tool','test','pending','now')",[&conv]).unwrap();
    let settings = Settings::with_secret_store(e.db.clone(), Arc::new(NoSecrets));
    let agent = AgentRuntime::new(e, settings, Arc::new(|_| {})).unwrap();
    let events = agent.events.replay(&conv, 0, 100).unwrap();
    assert!(events
        .iter()
        .any(|e| e.kind == "done" && e.payload["status"] == "interrupted"));
    assert!(events
        .iter()
        .any(|e| e.kind == "approval-resolved" && e.payload["status"] == "cancelled"));
}

#[tokio::test]
async fn source_read_tools_honor_preexisting_cancellation() {
    let (_d, e, _g) = setup();
    let (agent, _conv, _rx) = runtime(e, "http://127.0.0.1:1/v1");
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    let context = super::tools::ToolContext {
        engine: agent.engine.clone(),
        cancel: token,
        progress: None,
        resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
    };
    for (name, args) in [
        ("search_mods", json!({"query":"test"})),
        (
            "resolve_mod",
            json!({"source_id":"modrinth","project_id":"test"}),
        ),
    ] {
        let tool = agent.registry.get(name).unwrap();
        let prepared = tool.prepare(&context, &args).await.unwrap();
        assert!(matches!(
            tool.execute(&context, args, prepared).await,
            Err(crate::error::AppError::Cancelled)
        ));
    }
}
