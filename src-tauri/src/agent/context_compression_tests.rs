use super::{next, runtime, TurnRequest};
use crate::{history::History, transaction::tests::setup};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};

const REMOTE_SUMMARY: &str = "REMOTE_CONTEXT_SUMMARY";
const LOG_MARKER: &str = "LOG_EVIDENCE_";

pub(super) struct MockResponse {
    status: &'static str,
    content_type: &'static str,
    body: String,
}

impl MockResponse {
    pub(super) fn sse(body: String) -> Self {
        Self {
            status: "200 OK",
            content_type: "text/event-stream",
            body,
        }
    }

    pub(super) fn context_limit() -> Self {
        Self {
            status: "400 Bad Request",
            content_type: "application/json",
            body: json!({
                "error": {
                    "type": "invalid_request_error",
                    "code": "context_length_exceeded",
                    "message": "maximum context length exceeded"
                }
            })
            .to_string(),
        }
    }
}

pub(super) struct RequestServer {
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
    bodies: Arc<Mutex<Vec<Value>>>,
}

impl RequestServer {
    pub(super) fn snapshot(&self) -> Vec<Value> {
        self.bodies.lock().unwrap().clone()
    }

    pub(super) async fn finish(mut self) -> Vec<Value> {
        let bodies = Arc::clone(&self.bodies);
        let _ = self.stop.take().unwrap().send(());
        self.task.await.unwrap();
        let snapshot = bodies.lock().unwrap().clone();
        snapshot
    }
}

pub(super) async fn request_server(
    mut respond: impl FnMut(&Value) -> MockResponse + Send + 'static,
) -> (String, RequestServer) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, mut stopped) = oneshot::channel();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&bodies);
    let task = tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                _ = &mut stopped => break,
                accepted = listener.accept() => accepted.unwrap(),
            };
            let (mut socket, _) = accepted;
            let (target, body) = read_request(&mut socket).await;
            assert_eq!(
                target, "/v1/chat/completions",
                "unexpected model metadata request"
            );
            captured.lock().unwrap().push(body.clone());
            let response = respond(&body);
            let headers = format!(
                "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.status,
                response.content_type,
                response.body.len()
            );
            socket.write_all(headers.as_bytes()).await.unwrap();
            for chunk in response.body.as_bytes().chunks(7) {
                socket.write_all(chunk).await.unwrap();
            }
        }
    });
    (
        format!("http://{address}/v1"),
        RequestServer {
            stop: Some(stop),
            task,
            bodies,
        },
    )
}

async fn read_request(socket: &mut TcpStream) -> (String, Value) {
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        socket.read_exact(&mut byte).await.unwrap();
        header.push(byte[0]);
        assert!(header.len() < 32 * 1024);
    }
    let header = String::from_utf8(header).unwrap();
    let mut request_line = header.lines().next().unwrap().split_whitespace();
    assert_eq!(request_line.next(), Some("POST"));
    let target = request_line.next().unwrap().to_owned();
    let length = header
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap();
    let mut body = vec![0; length];
    socket.read_exact(&mut body).await.unwrap();
    (target, serde_json::from_slice(&body).unwrap())
}

pub(super) fn has_tools(request: &Value) -> bool {
    request["tools"]
        .as_array()
        .is_some_and(|tools| !tools.is_empty())
}

pub(super) fn content_reply(content: &str) -> MockResponse {
    MockResponse::sse(format!(
        "{}data: [DONE]\n\n",
        super::event(json!({
            "choices": [{
                "index": 0,
                "delta": {"content": content},
                "finish_reason": "stop"
            }]
        }))
    ))
}

fn read_file_call(path: &std::path::Path) -> MockResponse {
    let arguments = json!({"path": path}).to_string();
    MockResponse::sse(format!(
        "{}data: [DONE]\n\n",
        super::event(json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "context-read",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": arguments
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }))
    ))
}

fn configure_models(agent: &super::AgentRuntime, default_model: &str, extra: &[&str]) {
    let mut settings = agent.settings.load().unwrap();
    settings.model = default_model.to_owned();
    settings.models = std::iter::once(default_model)
        .chain(extra.iter().copied())
        .map(|id| crate::settings::AgentModelSettings {
            display_name: id.to_owned(),
            id: id.to_owned(),
            reasoning_levels: Vec::new(),
        })
        .collect();
    agent.settings.save(&settings).unwrap();
}

#[tokio::test]
async fn large_tool_result_is_summarized_and_reused_after_switching_models() {
    let (_dir, engine, game) = setup();
    let log = game.join("large.log");
    let line = format!("{LOG_MARKER}{}", "x".repeat(118));
    let contents = format!("{line}\n").repeat(1_900);
    assert!(contents.len() < 256 * 1024);
    std::fs::write(&log, contents).unwrap();

    let tool_path = log.clone();
    let (url, server) = request_server(move |request| {
        if !has_tools(request) {
            content_reply(REMOTE_SUMMARY)
        } else if request["model"] == "mock-model"
            && !request["messages"].to_string().contains(REMOTE_SUMMARY)
        {
            read_file_call(&tool_path)
        } else {
            content_reply("模型回答")
        }
    })
    .await;
    let (agent, conversation, mut events) = runtime(engine, &url);
    configure_models(&agent, "mock-model", &["large-model"]);
    agent.set_test_context_window("mock-model", Some(300_000));
    agent.set_test_context_window("large-model", Some(1_000_000));

    agent
        .start_turn(TurnRequest {
            conversation_id: conversation.clone(),
            content: "读取日志并总结证据".into(),
            attachments: None,
            model: None,
            permission_mode: None,
            reasoning_effort: None,
        })
        .unwrap();
    assert_eq!(next(&mut events, "done").await.payload["status"], "done");

    let first_turn_requests = server.snapshot();
    let summaries: Vec<_> = first_turn_requests
        .iter()
        .filter(|request| !has_tools(request))
        .collect();
    assert!(
        !summaries.is_empty(),
        "the oversized tool result was not summarized"
    );
    assert!(summaries
        .iter()
        .all(|request| request["model"] == "mock-model"));
    assert!(summaries
        .iter()
        .any(|request| { request["messages"].to_string().contains(LOG_MARKER) }));
    let summarized_request = first_turn_requests
        .iter()
        .find(|request| {
            has_tools(request) && request["messages"].to_string().contains(REMOTE_SUMMARY)
        })
        .expect("the normal model request should use the remote summary");
    assert!(!summarized_request.to_string().contains(LOG_MARKER));

    let history = History::new(agent.engine.db.clone());
    let detail = history.get_conversation(&conversation).unwrap();
    assert!(detail
        .messages
        .iter()
        .any(|message| message.role == "tool" && message.content.contains(LOG_MARKER)));
    assert!(detail
        .conversation
        .task_summary
        .as_deref()
        .is_some_and(|summary| summary.contains(REMOTE_SUMMARY)));

    let previous_request_count = first_turn_requests.len();
    agent
        .start_turn(TurnRequest {
            conversation_id: conversation.clone(),
            content: "继续处理上一轮结果".into(),
            attachments: None,
            model: Some("large-model".into()),
            permission_mode: None,
            reasoning_effort: None,
        })
        .unwrap();
    let done = next(&mut events, "done").await;
    assert_eq!(
        done.payload["status"],
        "done",
        "recorded events: {:?}",
        agent.events.replay(&conversation, 0, 100).unwrap()
    );

    let requests = server.finish().await;
    let next_turn_requests = &requests[previous_request_count..];
    assert!(!next_turn_requests.is_empty());
    assert!(next_turn_requests.iter().all(has_tools));
    let next_turn_request = next_turn_requests
        .iter()
        .find(|request| {
            request["model"] == "large-model"
                && request["messages"]
                    .to_string()
                    .contains("继续处理上一轮结果")
        })
        .expect("the switched model should receive the next turn");
    assert!(next_turn_request["messages"]
        .to_string()
        .contains(REMOTE_SUMMARY));
    assert!(!next_turn_request.to_string().contains(LOG_MARKER));
}

#[tokio::test]
async fn below_threshold_tool_result_ignores_legacy_context_budget() {
    let (_dir, engine, game) = setup();
    let log = game.join("small.log");
    std::fs::write(&log, "SHORT_EVIDENCE\n").unwrap();
    let tool_path = log.clone();
    let mut read_file_returned = false;
    let (url, server) = request_server(move |request| {
        if !has_tools(request) {
            content_reply(REMOTE_SUMMARY)
        } else if !read_file_returned {
            read_file_returned = true;
            read_file_call(&tool_path)
        } else {
            content_reply("模型回答")
        }
    })
    .await;
    let (agent, conversation, mut events) = runtime(engine, &url);
    agent.set_test_context_window("mock-model", Some(128_000));
    // Simulate a stale persisted setting from versions that had a local budget.
    agent
        .engine
        .db
        .conn()
        .execute(
            "INSERT OR REPLACE INTO settings(key,value) VALUES ('context_budget','1')",
            [],
        )
        .unwrap();

    agent
        .start_turn(TurnRequest {
            conversation_id: conversation.clone(),
            content: "读取短日志".into(),
            attachments: None,
            model: None,
            permission_mode: None,
            reasoning_effort: None,
        })
        .unwrap();
    let done = next(&mut events, "done").await;
    assert_eq!(
        done.payload["status"],
        "done",
        "recorded events: {:?}",
        agent.events.replay(&conversation, 0, 100).unwrap()
    );

    let requests = server.finish().await;
    assert!(!requests.is_empty());
    assert!(requests.iter().all(has_tools));
    assert!(requests
        .iter()
        .any(|request| request["messages"].to_string().contains("SHORT_EVIDENCE")));
}

#[tokio::test]
async fn unknown_model_context_overflow_summarizes_remotely_then_retries() {
    let (_dir, engine, _) = setup();
    let (url, server) = request_server(|request| {
        if !has_tools(request) {
            content_reply(REMOTE_SUMMARY)
        } else if request["messages"].to_string().contains(REMOTE_SUMMARY) {
            content_reply("重试成功")
        } else {
            MockResponse::context_limit()
        }
    })
    .await;
    let (agent, conversation, mut events) = runtime(engine, &url);
    let history = History::new(agent.engine.db.clone());
    history
        .append_message(&conversation, "user", "OLD_HISTORY_EVIDENCE", None, None)
        .unwrap();
    history
        .append_message(&conversation, "assistant", "之前的答复", None, None)
        .unwrap();

    agent
        .start_turn(TurnRequest {
            conversation_id: conversation.clone(),
            content: "继续当前任务".into(),
            attachments: None,
            model: None,
            permission_mode: None,
            reasoning_effort: None,
        })
        .unwrap();
    assert_eq!(next(&mut events, "done").await.payload["status"], "done");

    let requests = server.finish().await;
    let summary_requests: Vec<_> = requests
        .iter()
        .filter(|request| !has_tools(request))
        .collect();
    assert!(!summary_requests.is_empty());
    assert!(summary_requests
        .iter()
        .all(|request| request["model"] == "mock-model"));
    let retried = requests
        .iter()
        .find(|request| {
            has_tools(request) && request["messages"].to_string().contains(REMOTE_SUMMARY)
        })
        .expect("the failed provider request should be retried with the summary");
    assert!(retried["messages"].to_string().contains("继续当前任务"));
    assert!(!retried.to_string().contains("OLD_HISTORY_EVIDENCE"));

    let detail = history.get_conversation(&conversation).unwrap();
    assert!(detail
        .messages
        .iter()
        .any(|message| message.content == "OLD_HISTORY_EVIDENCE"));
    assert_eq!(detail.messages.last().unwrap().content, "重试成功");
}
