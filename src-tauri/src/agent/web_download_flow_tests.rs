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
        let tool_arguments = json!({"url":download_url});
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
        let max_bytes = preview["maxBytes"]
            .as_u64()
            .expect("approval preview must include a numeric maxBytes");
        assert!(max_bytes >= archive.len() as u64);
        assert!(artifacts::list(&agent.engine.db).unwrap().is_empty());
        assert_eq!(
            std::fs::read_dir(agent.engine.paths.downloads())
                .unwrap()
                .count(),
            0,
            "nothing should be downloaded while approval is pending"
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
            ["url"]
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
        let downloads_root = agent.engine.paths.downloads().canonicalize().unwrap();
        let artifact_path = record.artifact.path.canonicalize().unwrap();
        assert_eq!(
            artifact_path.parent().and_then(std::path::Path::parent),
            Some(downloads_root.as_path()),
            "artifact must be stored below the managed downloads root"
        );
        assert_eq!(
            record.artifact.path, artifact_path,
            "artifact path must not alias another path"
        );
        assert_eq!(
            result.payload["result"],
            json!(record.artifact.clone().into_user_paths())
        );
        assert!(!record.artifact.integrity_verified);
        assert!(record.artifact.requires_review);
        assert!(!record.artifact.auto_install_allowed);

        let mut downloaded = zip::ZipArchive::new(File::open(&record.artifact.path).unwrap())
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
