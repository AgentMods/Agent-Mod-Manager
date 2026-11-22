use super::{event, final_reply, runtime, server_with, ApprovalResponse, TurnRequest};
use crate::{agent::policy::PermissionMode, artifacts, transaction::tests::setup};
use serde_json::{json, Value};
use std::{
    fs::File,
    io::{Cursor, Read, Write},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const REVIEWED_URL_HOST: &str = "127.0.0.1";

fn tool_call(name: &str, arguments: Value) -> String {
    format!(
        "{}data: [DONE]\n\n",
        event(json!({"choices":[{"index":0,"delta":{"tool_calls":[{
            "index":0,"id":"web-download-call","type":"function",
            "function":{"name":name,"arguments":arguments.to_string()}
        }]},"finish_reason":"tool_calls"}]}))
    )
}

fn inert_zip() -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "README.txt",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(b"Inert web-download fixture.\n").unwrap();
    writer.finish().unwrap().into_inner()
}

async fn request_headers(socket: &mut TcpStream) -> String {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).await.unwrap();
            bytes.push(byte[0]);
            assert!(bytes.len() < 32 * 1024, "oversized HTTP request headers");
        }
        String::from_utf8(bytes).unwrap()
    })
    .await
    .expect("timed out reading loopback HTTP request")
}

async fn next_expected(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<super::AgentEvent>,
    expected: &str,
) -> super::AgentEvent {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = rx.recv().await.expect("agent event stream closed");
            if event.kind == expected {
                return event;
            }
            assert_ne!(event.kind, "error", "agent failed: {}", event.payload);
            assert_ne!(
                event.kind, "done",
                "turn ended before {expected}: {}",
                event.payload
            );
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {expected}"))
}

#[tokio::test]
async fn generic_web_download_is_reviewed_before_network_io_in_every_permission_mode() {
    let archive = inert_zip();

    for mode in [
        PermissionMode::RequestApproval,
        PermissionMode::AutoApprove,
        PermissionMode::FullTrust,
    ] {
        let (_dir, engine, game) = setup();
        let target = TcpListener::bind((REVIEWED_URL_HOST, 0)).await.unwrap();
        let target_address = target.local_addr().unwrap();
        let download_url = format!("http://{target_address}/archive.zip");
        let tool_arguments = json!({"url":download_url,"gameName":"Example Game","author":"Public Author","projectPage":"https://example.com/project"});
        let (model_url, model_job) = server_with(2, move |step, _| {
            if step == 0 {
                tool_call("download_web_mod", tool_arguments.clone())
            } else {
                final_reply()
            }
        })
        .await;
        let (agent, conversation, mut rx) = runtime(engine, &model_url);
        let turn = agent
            .start_turn(TurnRequest {
                conversation_id: conversation.clone(),
                content: "Download this public archive for review".into(),
                attachments: None,
                model: None,
                permission_mode: Some(mode),
                reasoning_effort: None,
            })
            .unwrap();

        let approval = next_expected(&mut rx, "approval-request").await;
        assert_eq!(approval.turn_id, turn);
        assert_eq!(approval.payload["title"], "download_web_mod");
        let preview = &approval.payload["preview"];
        assert_eq!(preview["url"], download_url);
        assert_eq!(preview["filename"], "archive.zip");
        assert_eq!(preview["classification"]["gameName"], "Example Game");
        assert!(preview.get("maxBytes").is_none());
        assert!(preview["sizePolicy"].is_string());
        assert!(artifacts::list(&agent.engine.db).unwrap().is_empty());
        let downloads = agent.engine.paths.downloads();
        assert_eq!(std::fs::read_dir(&downloads).unwrap().count(), 1);
        assert_eq!(
            std::fs::read_dir(downloads.join("incoming"))
                .unwrap()
                .count(),
            0
        );
        assert_eq!(
            std::fs::read_dir(agent.engine.paths.mod_library())
                .unwrap()
                .count(),
            0
        );

        match tokio::time::timeout(Duration::from_millis(250), target.accept()).await {
            Err(_) => {}
            Ok(Ok(_)) => panic!("download request arrived before approval in {mode:?} mode"),
            Ok(Err(error)) => panic!("loopback fixture accept failed: {error}"),
        }

        agent
            .respond_approval(
                &conversation,
                approval.payload["approvalId"].as_str().unwrap(),
                ApprovalResponse {
                    approved: true,
                    answer: Value::Null,
                },
            )
            .unwrap();

        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(10), target.accept())
            .await
            .expect("timed out waiting for approved download")
            .unwrap();
        let request = request_headers(&mut socket).await;
        assert!(
            request.starts_with("GET /archive.zip "),
            "unexpected request: {request}"
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            archive.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        socket.write_all(&archive).await.unwrap();

        let result = next_expected(&mut rx, "tool-result").await;
        let done = next_expected(&mut rx, "done").await;
        assert_eq!(done.payload["status"], "done", "mode: {mode:?}");
        assert_eq!(result.payload["status"], "done", "{}", result.payload);
        let model_requests = tokio::time::timeout(Duration::from_secs(10), model_job)
            .await
            .expect("timed out waiting for scripted model requests")
            .unwrap();
        assert_eq!(model_requests.len(), 2);

        let definitions = model_requests[0]["tools"].as_array().unwrap();
        let definition = definitions
            .iter()
            .find(|tool| tool["function"]["name"] == "download_web_mod")
            .expect("Agent request must advertise download_web_mod");
        let schema = &definition["function"]["parameters"];
        assert_eq!(schema["required"], json!(["url"]));
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(
            schema["properties"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["author", "gameId", "gameName", "projectPage", "url"]
        );

        let records = artifacts::list(&agent.engine.db).unwrap();
        let [record] = records.as_slice() else {
            panic!("expected one completed web artifact, got {}", records.len());
        };
        assert_eq!(record.source_id, "web");
        assert!(record.project_id.is_empty());
        assert!(record.version_id.is_empty());
        assert!(record.version.is_empty());
        assert!(record.origin().is_none());
        assert_eq!(record.artifact.filename, "archive.zip");
        let receipt = &result.payload["result"];
        let stored_path = std::path::Path::new(receipt["path"].as_str().unwrap());
        let canonical_library = agent.engine.paths.mod_library().canonicalize().unwrap();
        let final_path = stored_path.canonicalize().unwrap();
        assert!(final_path.starts_with(canonical_library));
        assert_eq!(record.artifact.path, final_path);
        assert_eq!(receipt["artifactId"], record.artifact.id);
        assert_eq!(receipt["id"], record.artifact.id);
        assert_eq!(receipt["sha256"], record.artifact.sha256);
        assert_eq!(receipt["projectPage"], "https://example.com/project");
        assert_eq!(receipt["authorHint"], "Public Author");
        let (page, author): (String, String) = agent.engine.db.conn().query_row(
            "SELECT project_page,author_hint FROM mod_library_entries WHERE id=?1 AND state='ready'",
            [&record.artifact.id], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(page, "https://example.com/project");
        assert_eq!(author, "Public Author");
        let persisted = record.native_library_intent().unwrap();
        assert_eq!(
            persisted.project_page.as_deref(),
            Some("https://example.com/project")
        );
        assert_eq!(persisted.author_hint.as_deref(), Some("Public Author"));
        assert!(!record.artifact.integrity_verified);
        assert!(record.artifact.requires_review);
        assert!(!record.artifact.auto_install_allowed);

        let mut downloaded = zip::ZipArchive::new(File::open(stored_path).unwrap())
            .expect("download should be a readable ZIP archive");
        let mut readme = String::new();
        downloaded
            .by_name("README.txt")
            .unwrap()
            .read_to_string(&mut readme)
            .unwrap();
        assert_eq!(readme, "Inert web-download fixture.\n");

        assert!(crate::install::list_installs(&agent.engine, "g")
            .unwrap()
            .is_empty());
        assert_eq!(std::fs::read_dir(&game).unwrap().count(), 0);
        assert_eq!(
            std::fs::read_dir(agent.engine.paths.staging())
                .unwrap()
                .count(),
            0,
            "web download must not stage an installation"
        );
        let tool_calls: i64 = agent
            .engine
            .db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM tool_calls WHERE name IN ('install_mod','apply_file_ops')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tool_calls, 0);
    }
}

#[tokio::test]
async fn web_download_rejects_unreviewed_destination_without_network_io() {
    let (_dir, engine, game) = setup();
    let target = TcpListener::bind((REVIEWED_URL_HOST, 0)).await.unwrap();
    let target_address = target.local_addr().unwrap();
    let arbitrary_path = _dir.path().join("outside-download.zip");
    let arguments = json!({
        "url":format!("http://{target_address}/archive.zip"),
        "destination":arbitrary_path.to_string_lossy().to_string(),
    });
    let (model_url, model_job) = server_with(2, move |step, _| {
        if step == 0 {
            tool_call("download_web_mod", arguments.clone())
        } else {
            final_reply()
        }
    })
    .await;
    let (agent, conversation, mut rx) = runtime(engine, &model_url);
    agent
        .start_turn(TurnRequest {
            conversation_id: conversation.clone(),
            content: "Try to choose an output path".into(),
            attachments: None,
            model: None,
            permission_mode: Some(PermissionMode::FullTrust),
            reasoning_effort: None,
        })
        .unwrap();

    let failed = next_expected(&mut rx, "tool-result").await;
    assert_eq!(failed.payload["status"], "error");
    let done = next_expected(&mut rx, "done").await;
    assert_eq!(done.payload["status"], "done");
    let events = agent.events.replay(&conversation, 0, 100).unwrap();
    assert!(!events.iter().any(|event| event.kind == "approval-request"));
    assert!(artifacts::list(&agent.engine.db).unwrap().is_empty());
    assert!(!arbitrary_path.exists());
    assert_eq!(std::fs::read_dir(&game).unwrap().count(), 0);
    assert!(
        tokio::time::timeout(Duration::from_millis(250), target.accept())
            .await
            .is_err(),
        "invalid arguments must not trigger a request"
    );
    let model_requests = tokio::time::timeout(Duration::from_secs(10), model_job)
        .await
        .expect("timed out waiting for scripted model requests")
        .unwrap();
    assert_eq!(model_requests.len(), 2);
}

#[tokio::test]
async fn web_download_requires_game_or_context_and_cancelled_approval_never_fetches() {
    for cancel_approval in [false, true] {
        let (_dir, engine, _) = setup();
        let target = TcpListener::bind((REVIEWED_URL_HOST, 0)).await.unwrap();
        let mut args =
            json!({"url": format!("http://{}/archive.zip", target.local_addr().unwrap())});
        if cancel_approval {
            args["gameName"] = json!("Unregistered Game");
        }
        let requests = if cancel_approval { 1 } else { 2 };
        let (model_url, model_job) = server_with(requests, move |step, _| {
            if step == 0 {
                tool_call("download_web_mod", args.clone())
            } else {
                final_reply()
            }
        })
        .await;
        let (agent, conversation, mut rx) = runtime(engine, &model_url);
        let turn = agent
            .start_turn(TurnRequest {
                conversation_id: conversation.clone(),
                content: "下载这个归档".into(),
                attachments: None,
                model: None,
                permission_mode: Some(PermissionMode::FullTrust),
                reasoning_effort: None,
            })
            .unwrap();
        if cancel_approval {
            next_expected(&mut rx, "approval-request").await;
            agent.cancel_turn(&conversation, &turn).unwrap();
        } else {
            let failed = next_expected(&mut rx, "tool-result").await;
            assert_eq!(failed.payload["status"], "error");
            assert!(failed.payload["result"]["error"]
                .as_str()
                .unwrap()
                .contains("gameName"));
            assert!(!agent
                .events
                .replay(&conversation, 0, 100)
                .unwrap()
                .iter()
                .any(|event| event.kind == "approval-request"));
        }
        let done = next_expected(&mut rx, "done").await;
        assert_eq!(
            done.payload["status"],
            if cancel_approval { "cancelled" } else { "done" }
        );
        assert!(artifacts::list(&agent.engine.db).unwrap().is_empty());
        assert_eq!(
            std::fs::read_dir(agent.engine.paths.mod_library())
                .unwrap()
                .count(),
            0
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(200), target.accept())
                .await
                .is_err()
        );
        assert_eq!(model_job.await.unwrap().len(), requests);
    }
}
