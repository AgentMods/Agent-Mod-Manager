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

#[test]
fn desktop_turn_dispatch_returns_while_the_database_is_locked() {
    use std::{sync::mpsc, time::Duration};
    use tauri::{ipc::InvokeResponse, test, webview::InvokeRequest};

    let (_dir, engine, _) = setup();
    let db = engine.db.clone();
    let (agent, conv, _rx) = runtime(engine, "http://127.0.0.1:1/v1");
    // Both start and retry will be rejected *after* acquiring the database;
    // no provider or credentials are needed for this IPC-level regression.
    History::new(db.clone())
        .archive_conversation(&conv, true)
        .unwrap();
    let app = test::mock_builder()
        .manage(agent)
        .invoke_handler(tauri::generate_handler![
            crate::commands::start_turn,
            crate::commands::restart_turn,
            crate::commands::replay_agent_events,
        ])
        .build(test::mock_context(test::noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let (ready, locked) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let deadline = Duration::from_secs(5);
    let holder = std::thread::spawn(move || {
        let _guard = db.conn();
        ready.send(()).unwrap();
        released.recv_timeout(deadline)
    });
    locked.recv().unwrap();
    let (reply, replies) = mpsc::channel();
    let request = json!({"conversationId":conv,"content":"continue"});
    for (cmd, body) in [
        ("start_turn", json!({"request":request})),
        (
            "restart_turn",
            json!({"request":request,"messageId":"previous","edit":false}),
        ),
        (
            "replay_agent_events",
            json!({"conversationId":conv,"after":0,"limit":100}),
        ),
    ] {
        let reply = reply.clone();
        window.as_ref().clone().on_message(
            InvokeRequest {
                cmd: cmd.into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: (if cfg!(any(windows, target_os = "android")) {
                    "http://tauri.localhost"
                } else {
                    "tauri://localhost"
                })
                .parse()
                .unwrap(),
                body: tauri::ipc::InvokeBody::Json(body),
                headers: Default::default(),
                invoke_key: test::INVOKE_KEY.into(),
            },
            Box::new(move |_, _, response, _, _| {
                let _ = reply.send((cmd, response));
            }),
        );
    }
    let _ = release.send(());
    assert!(
        holder.join().unwrap().is_ok(),
        "IPC dispatch blocked the window thread waiting for SQLite"
    );
    for _ in 0..3 {
        match replies.recv_timeout(deadline).unwrap() {
            ("replay_agent_events", InvokeResponse::Ok(body)) => {
                assert_eq!(body.deserialize::<Value>().unwrap(), json!([]))
            }
            ("start_turn" | "restart_turn", InvokeResponse::Err(error)) => {
                assert!(error.0.as_str().unwrap().contains("请先恢复归档会话"))
            }
            _ => panic!("unexpected IPC result"),
        }
    }
}

#[tokio::test]
async fn edit_and_retry_send_forked_context_to_the_model() {
    let (_dir, engine, _game) = setup();
    let history = History::new(engine.db.clone());
    let (url, job) = server(vec![final_reply(), final_reply()]).await;
    let (agent, source, mut rx) = runtime(engine, &url);
    let add = |role, content| {
        history
            .append_message(&source, role, content, None, None)
            .unwrap()
    };
    add("user", "Earlier question");
    add("assistant", "Earlier answer");
    let user = add("user", "Original question");
    let assistant = add("assistant", "Original response");
    add("user", "Later question");
    let request = |content: &str| TurnRequest {
        conversation_id: source.clone(),
        content: content.into(),
        model: None,
        permission_mode: None,
        reasoning_effort: None,
    };
    let retry = agent
        .restart_turn(request(""), &assistant.id, false)
        .unwrap();
    let done = next(&mut rx, "done").await;
    assert_eq!(done.conversation_id, retry);
    assert_eq!(done.payload["status"], "done");
    let edit = agent
        .restart_turn(request("Edited question"), &user.id, true)
        .unwrap();
    let done = next(&mut rx, "done").await;
    assert_eq!(done.conversation_id, edit);
    assert_eq!(done.payload["status"], "done");

    let requests = job.await.unwrap();
    for (body, prompt) in requests
        .iter()
        .zip(["Original question", "Edited question"])
    {
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(
            messages[1..],
            [
                json!({"role":"user","content":"Earlier question"}),
                json!({"role":"assistant","content":"Earlier answer"}),
                json!({"role":"user","content":prompt}),
            ]
        );
    }
    assert_eq!(history.get_conversation(&source).unwrap().messages.len(), 5);
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
            || panic!("an HTTP error cannot replace a reply"),
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
async fn flaky_server(
    replies: Vec<(String, bool)>,
) -> (String, tokio::task::JoinHandle<Vec<Value>>) {
    let replies = replies
        .into_iter()
        .map(|(body, truncate)| FallbackRegressionReply {
            status: "200 OK",
            content_type: "text/event-stream",
            body,
            truncate,
        })
        .collect();
    let (url, task) = fallback_regression_server(replies).await;
    let job = tokio::spawn(async move {
        task.await
            .unwrap()
            .into_iter()
            .map(|request| request.body)
            .collect()
    });
    (url, job)
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
            || panic!("no visible output to replace"),
        )
        .await
        .expect("the retry should succeed");
    assert_eq!(completion.content, "完成");
    assert_eq!(seen, "完成");
    assert_eq!(job.await.unwrap().len(), 2);
}

#[tokio::test]
async fn model_stream_dropped_after_output_retries_only_the_current_request() {
    let (_dir, engine, game) = setup();
    let trashed = game.join("already-executed");
    let retained = game.join("interrupted-call");
    std::fs::write(&trashed, b"trash once").unwrap();
    std::fs::write(&retained, b"retain").unwrap();
    let call = |id, file: &std::path::Path| {
        event(
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":id,"function":{
            "name":"trash_paths","arguments":json!({"paths":[file],"reason":"test"}).to_string()
        }}]},"finish_reason":"tool_calls"}]}),
        )
    };
    let completed_tool = format!("{}data: [DONE]\n\n", call("committed", &trashed));
    let interrupted = format!(
        "{}{}",
        event(json!({"choices":[{"delta":{"content":"半截","reasoning_content":"旧推理"}}]})),
        call("interrupted", &retained),
    );
    let usage = json!({"prompt_tokens":20,"completion_tokens":6,"total_tokens":26});
    let fallback = fallback_regression_json_reply("完成", "stop", vec![], usage.clone());
    let (url, job) = fallback_regression_server(vec![
        FallbackRegressionReply {
            status: "200 OK",
            content_type: "text/event-stream",
            body: completed_tool,
            truncate: false,
        },
        FallbackRegressionReply {
            status: "200 OK",
            content_type: "text/event-stream",
            body: interrupted,
            truncate: true,
        },
        FallbackRegressionReply {
            status: "200 OK",
            content_type: "text/event-stream",
            body: String::new(),
            truncate: true,
        },
        FallbackRegressionReply {
            status: "200 OK",
            content_type: "application/json",
            body: fallback,
            truncate: false,
        },
    ])
    .await;
    let (agent, conv, mut rx) = runtime(engine, &url);
    agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "trash".into(),
            model: None,
            permission_mode: Some(super::policy::PermissionMode::FullTrust),
            reasoning_effort: None,
        })
        .unwrap();
    assert_eq!(next(&mut rx, "done").await.payload["status"], "done");
    assert!(!trashed.exists());
    assert!(retained.exists());

    let requests = job.await.unwrap();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[1].body, requests[2].body);
    assert_eq!(requests[1].body["stream"], true);
    assert_eq!(requests[2].body["stream"], true);
    assert_eq!(requests[3].body["stream"], false);
    assert!(requests[3].body.get("stream_options").is_none());
    assert_eq!(requests[1].body["messages"], requests[3].body["messages"]);
    assert_eq!(requests[1].body["tools"], requests[3].body["tools"]);
    assert!(requests[1].body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["tool_call_id"] == "committed"));

    let history = History::new(agent.engine.db.clone())
        .get_conversation(&conv)
        .unwrap();
    assert_eq!(history.messages.len(), 4);
    let reply = history.messages.last().unwrap();
    assert_eq!(reply.content, "完成");
    assert_eq!(reply.reasoning.as_deref(), Some(""));
    let replay = agent.events.replay(&conv, 0, 100).unwrap();
    let started_tools: Vec<_> = replay.iter().filter(|e| e.kind == "tool-start").collect();
    assert_eq!(started_tools.len(), 1);
    assert_eq!(started_tools[0].payload["providerCallId"], "committed");
    let reply_events: Vec<_> = replay
        .iter()
        .filter(|e| e.message_id.as_ref() == Some(&reply.id))
        .map(|e| (e.kind.as_str(), e.payload.clone()))
        .collect();
    assert_eq!(
        reply_events,
        vec![
            ("delta", json!({"content":"半截","reasoning":"旧推理"})),
            ("reply-reset", json!({})),
            ("delta", json!({"content":"完成","reasoning":""})),
            ("usage", usage),
        ]
    );
}

#[tokio::test]
async fn model_stream_tool_only_retry_replaces_partial_only_after_validation() {
    use std::cell::RefCell;

    for (arguments, valid) in [("{}", true), ("{", false)] {
        let partial = event(json!({"choices":[{"delta":{"content":"半截"}}]}));
        let tool_only = fallback_regression_json_reply(
            "",
            "tool_calls",
            vec![json!({
                "id":"replacement",
                "type":"function",
                "function":{"name":"read_dir","arguments":arguments}
            })],
            Value::Null,
        );
        let (url, job) = fallback_regression_server(vec![
            FallbackRegressionReply {
                status: "200 OK",
                content_type: "text/event-stream",
                body: partial,
                truncate: true,
            },
            FallbackRegressionReply {
                status: "200 OK",
                content_type: "text/event-stream",
                body: String::new(),
                truncate: true,
            },
            FallbackRegressionReply {
                status: "200 OK",
                content_type: "application/json",
                body: tool_only,
                truncate: false,
            },
        ])
        .await;
        let emitted = RefCell::new(Vec::new());
        let result = super::client::ModelClient::new()
            .unwrap()
            .stream(
                &mock_config(url),
                &[],
                &[],
                &tokio_util::sync::CancellationToken::new(),
                |delta, full| {
                    if !delta.content.is_empty() {
                        emitted.borrow_mut().push("text");
                    }
                    if full.done {
                        emitted.borrow_mut().push("done");
                    }
                    Ok(())
                },
                || {
                    emitted.borrow_mut().push("reset");
                    Ok(())
                },
            )
            .await;
        if valid {
            let completion = result.expect("valid tool-only replacement");
            assert!(completion.content.is_empty());
            assert_eq!(completion.calls().unwrap()[0].id, "replacement");
            assert_eq!(emitted.into_inner(), vec!["text", "done", "reset"]);
        } else {
            assert!(result.is_err());
            assert_eq!(emitted.into_inner(), vec!["text"]);
        }
        let requests = job.await.unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].body["stream"], true);
        assert_eq!(requests[1].body["stream"], true);
        assert_eq!(requests[2].body["stream"], false);
        assert!(requests[2].body.get("stream_options").is_none());
    }
}

#[tokio::test]
async fn model_stream_exhausted_retries_keep_the_last_visible_reply() {
    let (_dir, engine, _) = setup();
    let api_key = "sk-offline-exhausted-redaction-token";
    let partial = event(json!({"choices":[{"delta":{"reasoning_content":"半截推理"}}]}));
    let provider_error = json!({
        "error":{"code":"fallback_unavailable","message":format!("credential {api_key} was rejected")}
    })
    .to_string();
    let (url, job) = fallback_regression_server(vec![
        FallbackRegressionReply {
            status: "200 OK",
            content_type: "text/event-stream",
            body: partial,
            truncate: true,
        },
        FallbackRegressionReply {
            status: "200 OK",
            content_type: "text/event-stream",
            body: String::new(),
            truncate: true,
        },
        FallbackRegressionReply {
            status: "502 Bad Gateway",
            content_type: "application/json",
            body: provider_error,
            truncate: false,
        },
    ])
    .await;
    let (agent, conv, mut rx) = runtime_with_fallback_regression_key(engine, &url, api_key);
    agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "hi".into(),
            model: None,
            permission_mode: None,
            reasoning_effort: None,
        })
        .unwrap();
    let error = next(&mut rx, "error").await;
    let diagnostic = error.payload["message"].as_str().unwrap();
    assert!(diagnostic.contains("非流式"), "{diagnostic}");
    assert!(diagnostic.contains("fallback_unavailable"), "{diagnostic}");
    assert!(
        diagnostic.contains("credential [REDACTED] was rejected"),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains(api_key));
    assert_eq!(next(&mut rx, "done").await.payload["status"], "error");

    let requests = job.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].body["stream"], true);
    assert_eq!(requests[1].body["stream"], true);
    assert_eq!(requests[2].body["stream"], false);
    assert!(requests[2].body.get("stream_options").is_none());

    let replay = agent.events.replay(&conv, 0, 100).unwrap();
    assert!(!replay.iter().any(|e| e.kind == "reply-reset"));
    let history = History::new(agent.engine.db.clone())
        .get_conversation(&conv)
        .unwrap();
    let reply = history.messages.last().unwrap();
    assert_eq!(reply.reasoning.as_deref(), Some("半截推理"));
    assert_eq!(error.message_id.as_ref(), Some(&reply.id));
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
    let incomplete_tool = event(json!({
        "choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call","function":{
            "name":"trash_paths","arguments":json!({"paths":[file]}).to_string()
        }}]},"finish_reason":"tool_calls"}]
    }));
    let fallback_error = json!({
        "error":{"code":"fallback_failed","message":"非流式服务暂不可用"}
    })
    .to_string();
    let (url, job) = fallback_regression_server(vec![
        FallbackRegressionReply {
            status: "200 OK",
            content_type: "text/event-stream",
            body: incomplete_tool.clone(),
            truncate: false,
        },
        FallbackRegressionReply {
            status: "200 OK",
            content_type: "text/event-stream",
            body: incomplete_tool,
            truncate: false,
        },
        FallbackRegressionReply {
            status: "502 Bad Gateway",
            content_type: "application/json",
            body: fallback_error,
            truncate: false,
        },
    ])
    .await;
    let (agent, conv, mut rx) = runtime(engine, &url);
    agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "trash".into(),
            model: None,
            permission_mode: Some(super::policy::PermissionMode::FullTrust),
            reasoning_effort: None,
        })
        .unwrap();
    let error = next(&mut rx, "error").await;
    let diagnostic = error.payload["message"].as_str().unwrap();
    assert!(diagnostic.contains("非流式"), "{diagnostic}");
    assert!(diagnostic.contains("fallback_failed"), "{diagnostic}");
    let done = next(&mut rx, "done").await;
    assert_eq!(done.payload["status"], "error");
    assert!(file.exists());

    let requests = job.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].body, requests[1].body);
    assert_eq!(requests[0].body["stream"], true);
    assert_eq!(requests[1].body["stream"], true);
    assert_eq!(requests[2].body["stream"], false);
    assert!(requests[2].body.get("stream_options").is_none());
    let events = agent.events.replay(&conv, 0, 100).unwrap();
    assert!(!events.iter().any(|event| event.kind == "tool-start"));
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

struct FallbackRegressionReply {
    status: &'static str,
    content_type: &'static str,
    body: String,
    truncate: bool,
}

struct FallbackRegressionRequest {
    target: String,
    body: Value,
}

async fn fallback_regression_server(
    replies: Vec<FallbackRegressionReply>,
) -> (
    String,
    tokio::task::JoinHandle<Vec<FallbackRegressionRequest>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for reply in replies {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut raw_headers = Vec::new();
            while !raw_headers.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).await.unwrap();
                raw_headers.push(byte[0]);
                assert!(raw_headers.len() < 32 * 1024);
            }
            let headers = String::from_utf8(raw_headers).unwrap();
            let request_line = headers.lines().next().unwrap();
            let target = request_line.split_whitespace().nth(1).unwrap().to_owned();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            let mut request_body = vec![0; content_length];
            socket.read_exact(&mut request_body).await.unwrap();
            requests.push(FallbackRegressionRequest {
                target,
                body: serde_json::from_slice(&request_body).unwrap(),
            });

            let declared_length = reply.body.len() + if reply.truncate { 4096 } else { 0 };
            let response_headers = format!(
                "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reply.status, reply.content_type, declared_length
            );
            socket.write_all(response_headers.as_bytes()).await.unwrap();
            for chunk in reply.body.as_bytes().chunks(17) {
                socket.write_all(chunk).await.unwrap();
            }
            socket.shutdown().await.ok();
        }
        requests
    });
    (format!("http://{address}/v1"), task)
}

fn fallback_regression_json_reply(
    content: &str,
    finish_reason: &str,
    tool_calls: Vec<Value>,
    usage: Value,
) -> String {
    let mut message = json!({"role":"assistant","content":content});
    if !tool_calls.is_empty() {
        message["tool_calls"] = json!(tool_calls);
    }
    json!({
        "choices":[{"index":0,"message":message,"finish_reason":finish_reason}],
        "usage":usage
    })
    .to_string()
}

#[tokio::test]
async fn model_non_stream_fallback_after_two_body_errors_keeps_request_and_parses_standard_json() {
    use std::cell::RefCell;

    let draft = event(json!({"choices":[{"index":0,"delta":{"content":"暂存草稿"}}]}));
    let usage = json!({"prompt_tokens":9,"completion_tokens":5,"total_tokens":14});
    let standard_calls = vec![
        json!({"id":"read-first","type":"function","function":{"name":"read_dir","arguments":"{\"path\":\"mods\"}"}}),
        json!({"id":"read-second","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"mods/a.txt\"}"}}),
    ];
    let final_json = fallback_regression_json_reply(
        "已恢复的完整答复",
        "tool_calls",
        standard_calls,
        usage.clone(),
    );
    let (base_url, server) = fallback_regression_server(vec![
        FallbackRegressionReply {
            status: "200 OK",
            content_type: "text/event-stream",
            body: draft,
            truncate: true,
        },
        FallbackRegressionReply {
            status: "200 OK",
            content_type: "text/event-stream",
            body: String::new(),
            truncate: true,
        },
        FallbackRegressionReply {
            status: "200 OK",
            content_type: "application/json",
            body: final_json,
            truncate: false,
        },
    ])
    .await;

    let messages = vec![
        json!({"role":"system","content":"遵循工具调用协议"}),
        json!({"role":"user","content":"检查模组目录"}),
        json!({"role":"assistant","content":"","tool_calls":[{"id":"prior-call","type":"function","function":{"name":"read_dir","arguments":"{\"path\":\"mods\"}"}}]}),
        json!({"role":"tool","tool_call_id":"prior-call","content":"mods/a.txt"}),
    ];
    let tools = vec![json!({
        "type":"function",
        "function":{"name":"read_dir","description":"读取目录","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}
    })];
    let config = super::client::ModelConfig {
        base_url,
        model: "mock-model".into(),
        reasoning_effort: Some(crate::settings::ReasoningEffort::High),
        api_key: None,
    };
    let callbacks = RefCell::new(Vec::new());
    let completion = super::client::ModelClient::new()
        .unwrap()
        .stream(
            &config,
            &messages,
            &tools,
            &tokio_util::sync::CancellationToken::new(),
            |delta, _| {
                if !delta.content.is_empty() {
                    callbacks
                        .borrow_mut()
                        .push(format!("delta:{}", delta.content));
                }
                Ok(())
            },
            || {
                callbacks.borrow_mut().push("reset".into());
                Ok(())
            },
        )
        .await
        .expect("validated JSON fallback should complete the request");

    assert_eq!(completion.content, "已恢复的完整答复");
    assert_eq!(completion.usage.as_ref(), Some(&usage));
    let calls = completion.calls().unwrap();
    assert_eq!(
        calls
            .iter()
            .map(|call| call.id.as_str())
            .collect::<Vec<_>>(),
        vec!["read-first", "read-second"]
    );
    assert_eq!(
        callbacks.into_inner(),
        vec![
            "delta:暂存草稿".to_owned(),
            "reset".to_owned(),
            "delta:已恢复的完整答复".to_owned()
        ]
    );

    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests
        .iter()
        .all(|request| request.target == "/v1/chat/completions"));
    let expected_sse = json!({
        "model":"mock-model",
        "messages":messages,
        "tools":tools,
        "reasoning_effort":"high",
        "stream":true,
        "stream_options":{"include_usage":true}
    });
    assert_eq!(requests[0].body, expected_sse);
    assert_eq!(requests[1].body, expected_sse);
    let mut expected_fallback = expected_sse;
    expected_fallback["stream"] = json!(false);
    assert!(expected_fallback
        .as_object_mut()
        .unwrap()
        .remove("stream_options")
        .is_some());
    assert_eq!(requests[2].body, expected_fallback);
}

struct FallbackRegressionSecrets(String);

impl SecretStore for FallbackRegressionSecrets {
    fn read(&self, key: &str) -> AppResult<Option<String>> {
        Ok((key == crate::settings::MODEL_API_KEY).then(|| self.0.clone()))
    }

    fn write(&self, _: &str, _: &str) -> AppResult<()> {
        Ok(())
    }
}

fn runtime_with_fallback_regression_key(
    engine: crate::transaction::FileEngine,
    url: &str,
    api_key: &str,
) -> (AgentRuntime, String, mpsc::UnboundedReceiver<AgentEvent>) {
    let settings = Settings::with_secret_store(
        engine.db.clone(),
        Arc::new(FallbackRegressionSecrets(api_key.into())),
    );
    settings
        .write_secret(crate::settings::MODEL_API_KEY, api_key)
        .unwrap();
    let mut config = settings.load().unwrap();
    config.base_url = url.into();
    config.model = "mock-model".into();
    config.api_key_ref = Some(crate::settings::MODEL_API_KEY.into());
    settings.save(&config).unwrap();
    let conversation = History::new(engine.db.clone())
        .create_conversation("Fallback failure", None)
        .unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    let agent = AgentRuntime::new(
        engine,
        settings,
        Arc::new(move |event| {
            let _ = tx.send(event.clone());
        }),
    )
    .unwrap();
    (agent, conversation.id, rx)
}
