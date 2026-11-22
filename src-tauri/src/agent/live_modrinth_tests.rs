//! Opt-in acceptance: the model is scripted, but Modrinth and its CDN are real.
use super::{
    event, final_reply, runtime, server_with, AgentEvent, AgentRuntime, ApprovalResponse,
    TurnRequest,
};
use crate::{
    app::AppServices,
    artifacts,
    download::DownloadArtifact,
    games, install, plugins,
    sources::{SearchResult, SourceState},
    transaction::FilePlan,
    util,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha512};
use std::{fs, time::Duration};
use tokio::sync::mpsc;

const PROJECT: &str = "AANobbMI";
const VERSION: &str = "26nVNc41";
const FILENAME: &str = "sodium-fabric-0.6.9+mc1.21.1.jar";
const SIZE: u64 = 1_297_415;
const SHA512: &str = concat!(
    "892728b3c26038446b9f573695db9e791fa320994ed34f89be5c766fa2e735b46ad",
    "b9efa8f6688d5a5b7bb8e9793ff09e72f06fb5ff074137d5b833f1fe27227",
);

fn tool(call: &str, name: &str, args: Value) -> String {
    format!(
        "{}data: [DONE]\n\n",
        event(json!({"choices":[{"index":0,"delta":{"tool_calls":[{
            "index":0,"id":call,"type":"function","function":{"name":name,"arguments":args.to_string()}
        }]},"finish_reason":"tool_calls"}]}))
    )
}

fn result(request: &Value, id: &str) -> Value {
    let message = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool" && m["tool_call_id"] == id)
        .unwrap_or_else(|| panic!("model did not receive tool result {id}"));
    let result: Value = serde_json::from_str(message["content"].as_str().unwrap()).unwrap();
    assert!(result.get("error").is_none(), "tool failed: {id}");
    result
}

fn model_reply(step: usize, request: &Value, game_id: &str) -> String {
    match step {
        0 => tool(
            "find",
            "search_mods",
            json!({
                "query":"Sodium","source_ids":["modrinth"],"game_ids":{"modrinth":"minecraft"},
                "game_version":"1.21.1","loader":"fabric","limit":20,
            }),
        ),
        1 => {
            let search: SearchResult = serde_json::from_value(result(request, "find")).unwrap();
            assert_eq!(search.sources.len(), 1);
            assert_eq!(search.sources[0].state, SourceState::Available);
            let found = search
                .mods
                .iter()
                .find(|m| m.project_id == PROJECT)
                .expect("Sodium search hit");
            tool(
                "resolve",
                "resolve_mod",
                json!({
                    "project_id":found.project_id,"source_id":found.source_id,"version_id":VERSION,
                    "game_id":"minecraft","game_version":"1.21.1","loader":"fabric",
                }),
            )
        }
        2 => {
            let resolved = result(request, "resolve");
            assert_eq!(resolved["status"]["state"], "available");
            assert!(
                !resolved.to_string().contains("download_url"),
                "model-facing resolution must not expose the signed download URL"
            );
            let download = &resolved["download"];
            assert_eq!(download["source_id"], "modrinth");
            assert_eq!(download["project_id"], PROJECT);
            assert_eq!(download["version_id"], VERSION);
            assert_eq!(download["filename"], FILENAME);
            assert_eq!(download["size"], SIZE);
            assert_eq!(download["hashes"]["sha512"], SHA512);
            let resolution_id = download["resolution_id"]
                .as_str()
                .expect("resolution returns an opaque download reference");
            tool(
                "download",
                "download_mod",
                json!({"resolution_id":resolution_id,"gameId":game_id}),
            )
        }
        3 => {
            let receipt = result(request, "download");
            let artifact: DownloadArtifact = serde_json::from_value(receipt.clone()).unwrap();
            assert_eq!(receipt["artifactId"], artifact.id);
            assert_eq!(receipt["gameId"], game_id);
            assert_eq!(artifact.filename, FILENAME);
            assert!(
                artifact.integrity_verified
                    && artifact.requires_review
                    && !artifact.auto_install_allowed
            );
            assert_eq!(artifact.sha512, SHA512);
            tool(
                "install",
                "install_library_mod",
                json!({"request":{
                    "gameId":game_id,"artifactId":artifact.id,"mode":"adapter","modType":"java-mod",
                }}),
            )
        }
        4 => {
            assert_eq!(result(request, "install")["status"], "committed");
            final_reply()
        }
        5 => {
            let installed = result(request, "install");
            let transaction = installed["transactionId"].as_str().unwrap();
            tool(
                "rollback",
                "rollback_transaction",
                json!({"transaction_id":transaction}),
            )
        }
        6 => {
            assert_eq!(result(request, "rollback")["status"], "rolled_back");
            final_reply()
        }
        _ => unreachable!(),
    }
}

async fn next_live(rx: &mut mpsc::UnboundedReceiver<AgentEvent>, kind: &str) -> AgentEvent {
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let event = rx.recv().await.expect("live test event stream closed");
            if event.kind == "error"
                || event.kind == "tool-result" && event.payload["status"] == "error"
            {
                panic!("live provider/tool failure: {}", event.payload);
            }
            if event.kind == kind {
                return event;
            }
            assert_ne!(event.kind, "done", "turn ended before {kind}");
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {kind}"))
}

fn approve(agent: &AgentRuntime, conv: &str, event: &AgentEvent, expected_tool: &str) {
    assert_eq!(event.payload["title"], expected_tool);
    agent
        .respond_approval(
            conv,
            event.payload["approvalId"].as_str().unwrap(),
            ApprovalResponse {
                approved: true,
                answer: Value::Null,
            },
        )
        .unwrap();
}

#[tokio::test]
#[ignore = "live anonymous Modrinth API and CDN; temp directories only; no code execution"]
async fn opt_in_live_modrinth_agent_install_and_rollback() {
    let temp = tempfile::tempdir().unwrap();
    let app = AppServices::initialize(temp.path().join("data")).unwrap();
    plugins::test_fixtures::install(&app.engine.db, &app.engine.paths).unwrap();
    let root = temp.path().join("minecraft");
    fs::create_dir_all(root.join("mods")).unwrap();
    fs::write(root.join("options.txt"), b"fullscreen:false\n").unwrap();
    let game = games::register_game(&app.engine.db, &root, Some("minecraft-java")).unwrap();
    let dst = root.canonicalize().unwrap().join("mods").join(FILENAME);
    let original = b"existing fixture, not an executable jar\n";
    fs::write(&dst, original).unwrap();

    let game_id = game.id.clone();
    let (url, job) =
        server_with(7, move |step, request| model_reply(step, request, &game_id)).await;
    let (agent, conv, mut rx) = runtime(app.engine.clone(), &url);
    let turn = agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: format!(
                "请从 Modrinth 安装适用于 Fabric 1.21.1 的 Sodium {VERSION} 到测试实例。"
            ),
            attachments: None,
            model: None,
            permission_mode: None,
            reasoning_effort: None,
        })
        .unwrap();

    let approval = next_live(&mut rx, "approval-request").await;
    assert_eq!(approval.turn_id, turn);
    assert!(artifacts::list(&app.engine.db).unwrap().is_empty());
    let downloads = app.engine.paths.downloads();
    assert_eq!(fs::read_dir(&downloads).unwrap().count(), 1);
    assert_eq!(fs::read_dir(downloads.join("incoming")).unwrap().count(), 0);
    assert_eq!(
        fs::read_dir(app.engine.paths.mod_library())
            .unwrap()
            .count(),
        0
    );
    assert_eq!(fs::read(&dst).unwrap(), original);
    approve(&agent, &conv, &approval, "download_mod");
    println!(
        "Live search and exact version resolution accepted; separate download approval confirmed."
    );

    let approval = next_live(&mut rx, "approval-request").await;
    assert_eq!(approval.turn_id, turn);
    let records = artifacts::list(&app.engine.db).unwrap();
    let [record] = records.as_slice() else {
        panic!("expected one completed download")
    };
    let origin = record.origin().unwrap();
    assert_eq!(
        (
            origin.source_id.as_str(),
            origin.project_id.as_str(),
            origin.version_id.as_str()
        ),
        ("modrinth", PROJECT, VERSION)
    );
    let artifact = &record.artifact;
    let bytes = fs::read(&artifact.path).unwrap();
    assert!(artifact.path.starts_with(app.engine.paths.mod_library()));
    assert_eq!(bytes.len() as u64, SIZE);
    assert_eq!(hex::encode(Sha512::digest(&bytes)), SHA512);
    assert_eq!(
        fs::read(&dst).unwrap(),
        original,
        "download and preview must not deploy"
    );
    assert_eq!(approval.payload["preview"]["downloadOrigin"], json!(origin));
    let plan: FilePlan =
        serde_json::from_value(approval.payload["preview"]["plan"].clone()).unwrap();
    assert!(plan.requires_review);
    let [entry] = plan.entries.as_slice() else {
        panic!("expected one reviewed file")
    };
    assert_eq!(
        entry.operation.dst(),
        crate::paths::user_path(&dst).as_ref()
    );
    assert_eq!(
        entry.before.as_ref().unwrap().sha256,
        util::sha256_file(&dst).unwrap()
    );
    assert_eq!(entry.source.as_ref().unwrap().sha256, artifact.sha256);
    approve(&agent, &conv, &approval, "install_library_mod");

    let done = next_live(&mut rx, "done").await;
    assert_eq!(done.turn_id, turn);
    assert_eq!(done.payload["status"], "done");
    assert_eq!(fs::read(&dst).unwrap(), bytes);
    let installs = install::list_installs(&app.engine, &game.id).unwrap();
    let [installed] = installs.as_slice() else {
        panic!("expected one install")
    };
    assert_eq!(installed.status, "enabled");
    assert_eq!(installed.download_origin.as_ref(), Some(&origin));
    println!(
        "Installed reviewed artifact: {PROJECT}/{VERSION}, {SIZE} bytes, SHA-256 {}",
        artifact.sha256
    );

    let undo = agent
        .start_turn(TurnRequest {
            conversation_id: conv.clone(),
            content: "请回滚刚才的安装。".into(),
            attachments: None,
            model: None,
            permission_mode: None,
            reasoning_effort: None,
        })
        .unwrap();
    let approval = next_live(&mut rx, "approval-request").await;
    assert_eq!(approval.turn_id, undo);
    assert_eq!(approval.payload["preview"]["originalPlan"]["id"], plan.id);
    assert_eq!(fs::read(&dst).unwrap(), bytes);
    approve(&agent, &conv, &approval, "rollback_transaction");
    let done = next_live(&mut rx, "done").await;
    assert_eq!(done.turn_id, undo);
    assert_eq!(done.payload["status"], "done");
    assert_eq!(fs::read(&dst).unwrap(), original);
    assert_eq!(fs::read(&artifact.path).unwrap(), bytes);
    assert_eq!(
        install::list_installs(&app.engine, &game.id).unwrap()[0].status,
        "rolled_back"
    );
    assert_eq!(job.await.unwrap().len(), 7);
    println!("Approved rollback restored the prior file byte for byte and retained the download.");
}
