use super::{
    event, final_reply, next, runtime, server, server_with, server_with_http, ApprovalResponse,
    TurnRequest,
};
use crate::{
    agent::policy::PermissionMode, games, history::History, plugins, transaction::tests::setup,
};
use serde_json::{json, Value};
use std::{io::Write, path::Path};
use tokio_util::sync::CancellationToken;

fn write_fixture_archive(path: &Path, entries: &[(&str, &[u8])]) {
    let mut archive = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, contents) in entries {
        archive
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(contents).unwrap();
    }
    archive.finish().unwrap();
}

fn search_archive_path(body: &Value, index: usize) -> String {
    let content = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_str())
        .find(|content| content.contains("structuredContent"))
        .unwrap();
    serde_json::from_str::<Value>(content).unwrap()["structuredContent"]["samples"][index]
        ["archive_path"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn tool_call(name: &str, args: Value) -> String {
    let call = json!({"index":0,"id":format!("call_{name}"),"type":"function",
        "function":{"name":name,"arguments":args.to_string()}});
    format!(
        "{}data: [DONE]\n\n",
        event(json!({"choices":[{"delta":{"tool_calls":[call]},"finish_reason":"tool_calls"}]}))
    )
}

fn request(conversation_id: String, mode: PermissionMode) -> TurnRequest {
    TurnRequest {
        conversation_id,
        content: "为当前游戏安装并绑定适配包".into(),
        attachments: None,
        model: None,
        permission_mode: Some(mode),
        reasoning_effort: None,
    }
}

#[tokio::test]
async fn authored_external_package_binds_to_the_registered_game_in_one_turn() {
    let (_temp, engine, root) = setup();
    std::fs::write(root.join("options.txt"), "").unwrap();
    let mut adapter: Value = serde_json::from_str(plugins::test_fixtures::PACKAGES[0].1).unwrap();
    adapter["slug"] = json!("test-game");
    let draft_args = json!({
        "slug": "test-game",
        "adapter_json": adapter.to_string(),
        "skill_markdown": "---\nname: test-game\ndescription: Test game adapter\n---\nInspect logs before planning changes.\n",
    });
    let original_args = draft_args.clone();
    let mut draft_id = Value::Null;
    let (url, job) = server_with(5, move |round, body| match round {
        0 => tool_call("write_plugin_draft", draft_args.clone()),
        1 => {
            let messages = body["messages"].as_array().unwrap();
            let content = messages.last().unwrap()["content"].as_str().unwrap();
            let receipt: Value = serde_json::from_str(content).unwrap();
            assert!(receipt["draftPath"].is_string());
            draft_id = receipt["draftId"].clone();
            tool_call("validate_plugin", json!({"draft_id": draft_id}))
        }
        2 => tool_call("install_plugin", json!({"draft_id": draft_id})),
        3 => tool_call("set_game_adapter", json!({"slug": "test-game"})),
        _ => final_reply(),
    })
    .await;
    let conv = History::new(engine.db.clone())
        .create_conversation("Game", Some("g"))
        .unwrap();
    let (agent, _, mut rx) = runtime(engine, &url);
    agent
        .start_turn(request(conv.id.clone(), PermissionMode::FullTrust))
        .unwrap();
    let done = next(&mut rx, "done").await;
    let events = agent.events.replay(&conv.id, 0, 200).unwrap();
    let error = events.iter().find(|event| event.kind == "error");
    assert_eq!(
        done.payload["status"],
        "done",
        "{:?}",
        error.map(|event| &event.payload)
    );
    let history = History::new(agent.engine.db.clone())
        .get_conversation(&conv.id)
        .unwrap();
    let calls = history
        .messages
        .iter()
        .find_map(|message| message.tool_calls.as_ref())
        .unwrap();
    let saved_args: Value =
        serde_json::from_str(calls[0]["function"]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(saved_args, original_args);
    let mut results = events.iter().filter(|event| event.kind == "tool-result");
    for name in [
        "write_plugin_draft",
        "validate_plugin",
        "install_plugin",
        "set_game_adapter",
    ] {
        let payload = &results.next().expect(name).payload;
        assert_eq!(payload["name"], name);
        assert_eq!(payload["status"], "done", "{payload}");
        if name == "set_game_adapter" {
            assert_eq!(payload["result"]["gameId"], "g");
            assert_eq!(payload["result"]["adapterSlug"], "test-game");
        }
    }
    let game = games::get_game(&agent.engine.db, "g").unwrap();
    assert_eq!(game.adapter_slug.as_deref(), Some("test-game"));
    assert_eq!(game.adapter_version.as_deref(), Some("1.0.0"));
    assert_eq!(job.await.unwrap().len(), 5);
}

#[tokio::test]
async fn existing_package_binding_and_rename_obey_one_chat_approval() {
    for approve in [false, true] {
        let (_temp, engine, root) = setup();
        std::fs::write(root.join("options.txt"), "").unwrap();
        plugins::test_fixtures::install(&engine.db, &engine.paths).unwrap();
        let original = games::get_game(&engine.db, "g").unwrap();
        let (url, job) = server(vec![
            tool_call(
                "set_game_adapter",
                json!({"slug":"minecraft-java","game_id":"g","game_name":"  游戏库新名称  "}),
            ),
            final_reply(),
        ])
        .await;
        let (agent, conv, mut rx) = runtime(engine, &url);
        agent
            .start_turn(request(conv.clone(), PermissionMode::AutoApprove))
            .unwrap();
        let approval = next(&mut rx, "approval-request").await;
        assert_eq!(approval.payload["title"], "set_game_adapter");
        assert_eq!(approval.payload["preview"]["gameName"], original.name);
        assert_eq!(
            approval.payload["preview"]["targetGameName"],
            "游戏库新名称"
        );
        let pending = games::get_game(&agent.engine.db, "g").unwrap();
        assert_eq!(pending.name, original.name);
        assert!(pending.adapter_slug.is_none());
        agent
            .respond_approval(
                &conv,
                approval.payload["approvalId"].as_str().unwrap(),
                ApprovalResponse {
                    approved: approve,
                    answer: Value::Null,
                },
            )
            .unwrap();
        let result = next(&mut rx, "tool-result").await;
        assert_eq!(
            result.payload["status"],
            if approve { "done" } else { "error" }
        );
        let game = games::get_game(&agent.engine.db, "g").unwrap();
        assert_eq!(
            game.adapter_slug.as_deref(),
            approve.then_some("minecraft-java")
        );
        assert_eq!(
            game.name,
            if approve {
                "游戏库新名称"
            } else {
                &original.name
            }
        );
        if approve {
            assert_eq!(result.payload["result"]["gameName"], game.name);
        }
        next(&mut rx, "done").await;
        assert_eq!(
            agent
                .events
                .replay(&conv, 0, 200)
                .unwrap()
                .iter()
                .filter(|event| event.kind == "approval-request")
                .count(),
            1
        );
        job.await.unwrap();
    }
}

#[tokio::test]
async fn offline_mcp_evidence_validates_two_archive_layouts_without_touching_game_files() {
    use crate::mcp::{ConnectionConsent, McpClient, McpRuntime, McpServers, ServerConfig};

    let (_temp, engine, game_root) = setup();
    std::fs::write(game_root.join("options.txt"), b"leave game intact").unwrap();
    let downloads = engine.paths.downloads();
    let jar_path = downloads.join("winter-shaders.jar");
    let zip_path = downloads.join("signal-boost.zip");
    write_fixture_archive(
        &jar_path,
        &[("fabric.mod.json", br#"{"id":"winter_shaders"}"#.as_slice())],
    );
    write_fixture_archive(
        &zip_path,
        &[
            ("pack.mcmeta", br#"{"pack":{"pack_format":1}}"#.as_slice()),
            (
                "assets/example/settings.json",
                br#"{"enabled":true}"#.as_slice(),
            ),
        ],
    );
    let jar_for_mcp = jar_path.to_string_lossy().into_owned();
    let zip_for_mcp = zip_path.to_string_lossy().into_owned();
    let (catalog_url, catalog_server) = server_with_http(
        4,
        "200 OK",
        "application/json",
        move |_, request| {
            let Some(id) = request.get("id") else {
                return String::new(); // rmcp accepts the initialized notification's empty 200.
            };
            let result = match request["method"].as_str().unwrap() {
                "initialize" => json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}),
                "tools/list" => json!({"tools":[{"name":"search_mods","description":"Find two fixture Mods with author notes and local archive paths.","inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}}]}),
                "tools/call" => {
                    let samples = json!([
                        {"project_id":"winter-shaders","project_page":"https://mods.example.test/mods/winter-shaders","file_id":"winter-1","author_notes":"Keep this JAR intact in the instance mods directory.","archive_path":jar_for_mcp},
                        {"project_id":"signal-boost","project_page":"https://mods.example.test/mods/signal-boost","file_id":"signal-1","author_notes":"Keep the ZIP root files together in a dedicated resource-pack directory.","archive_path":zip_for_mcp}
                    ]);
                    json!({"content":[{"type":"text","text":samples.to_string()}],"structuredContent":{"samples":samples},"isError":false})
                }
                _ => json!({}),
            };
            json!({"jsonrpc":"2.0","id":id,"result":result}).to_string()
        },
    )
    .await;
    let search_name = crate::mcp::client::namespaced_tool_name("fixture-catalog", "search_mods");
    let model_search_name = search_name.clone();
    let (model_url, model_server) = server_with(7, move |round, body| match round {
        0 => tool_call("read_skill", json!({"name":"system-adapter-authoring"})),
        1 => tool_call(&model_search_name, json!({"query":"two layouts"})),
        2 | 3 => tool_call(
            "extract_preview",
            json!({"path":search_archive_path(body, round - 2), "limit":10}),
        ),
        4 => tool_call("write_plugin_draft", adapter_draft_args()),
        5 => {
            let content = body["messages"].as_array().unwrap().last().unwrap()["content"]
                .as_str()
                .unwrap();
            let receipt: Value = serde_json::from_str(content).unwrap();
            tool_call("validate_plugin", json!({"draft_id":receipt["draftId"]}))
        }
        _ => final_reply(),
    })
    .await;
    let (agent, conversation, mut rx) = runtime(engine, &model_url);

    // Exercise the multi-sample tool flow with a GPT-family model.
    agent.settings.set("model", json!("gpt-4o-mini")).unwrap();
    let mut config = ServerConfig::http("fixture-catalog", "Offline catalog", &catalog_url);
    config.enabled = true;
    let servers = McpServers::new(agent.engine.db.clone());
    servers.create(&config).unwrap();
    let consent = ConnectionConsent::after_user_confirmation(&config, false).unwrap();
    let client = McpClient::connect(
        servers,
        &agent.settings,
        &config.id,
        consent,
        &McpRuntime::default(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let registered_names = client
        .register_tools(&agent.registry, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(registered_names, [search_name]);

    agent
        .start_turn(TurnRequest {
            content: "研究两种 Mod 样本的安装规则，创建并验证适配包草稿。".into(),
            ..request(conversation.clone(), PermissionMode::FullTrust)
        })
        .unwrap();
    let done = next(&mut rx, "done").await;
    let events = agent.events.replay(&conversation, 0, 200).unwrap();
    let error = events.iter().find(|event| event.kind == "error");
    assert_eq!(done.payload["status"], "done", "{error:?}");
    let previews: Vec<_> = events
        .iter()
        .filter(|event| event.kind == "tool-result" && event.payload["name"] == "extract_preview")
        .collect();
    assert_eq!(previews.len(), 2);
    assert!(previews
        .iter()
        .zip(["fabric.mod.json", "pack.mcmeta"])
        .all(|(preview, entry)| preview.payload["result"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["path"] == entry)));
    let validation = events
        .iter()
        .find(|event| event.kind == "tool-result" && event.payload["name"] == "validate_plugin")
        .unwrap();
    let validation = &validation.payload["result"];
    assert_eq!(validation["valid"], true);
    let rules = validation["package"]["adapter"]["archiveRules"]
        .as_array()
        .unwrap();
    assert!(rules.iter().any(|rule| rule["layout"] == "single_file"));
    assert!(rules.iter().any(|rule| rule["layout"] == "mod_directory"));
    let skill = validation["package"]["skillMarkdown"].as_str().unwrap();
    assert!(["winter-shaders", "winter-1", "signal-boost", "signal-1"]
        .iter()
        .all(|id| skill.contains(id)));
    client.disconnect_and_wait().await.unwrap();
    agent.registry.unregister(&registered_names);
    catalog_server.await.unwrap();
    model_server.await.unwrap();
    assert_eq!(
        std::fs::read(game_root.join("options.txt")).unwrap(),
        b"leave game intact"
    );
    assert!(!game_root.join("mods").exists());
}

fn adapter_draft_args() -> Value {
    let slug = "offline-mcp-layouts";
    let adapter = json!({
        "schemaVersion":1, "slug":slug, "name":"Fixture game", "version":"1.0.0", "platforms":["windows"],
        "markers":[{"path":"options.txt","kind":"file","required":true,"platforms":["windows"]}],
        "installRoots":[
            {"id":"mods","kind":"game","relativePath":"mods","platforms":["windows"],"discoveryHints":[],"notes":"Loader mods"},
            {"id":"packs","kind":"game","relativePath":"resourcepacks","platforms":["windows"],"discoveryHints":[],"notes":"Resource packs"}
        ],
        "modTypes":[
            {"id":"java-mod","name":"Java Mod","root":"mods","extensions":["jar"],"notes":"Copy JAR intact"},
            {"id":"resource-pack","name":"Resource pack","root":"packs","extensions":["zip"],"notes":"Preserve pack folder"}
        ],
        "archiveRules":[
            {"modType":"java-mod","layout":"single_file","requiredFiles":["fabric.mod.json"],"notes":"Intact JAR"},
            {"modType":"resource-pack","layout":"mod_directory","requiredFiles":["pack.mcmeta"],"notes":"Preserve pack folder"}
        ],
        "loadOrder":{"kind":"none","paths":[],"notes":""},"savePaths":[],"logPaths":[],"uninstallNotes":["Remove installed files"],
        "sources":[
            {"title":"Winter Shaders","url":"https://mods.example.test/mods/winter-shaders","checkedAt":"2026-11-21"},
            {"title":"Signal Boost","url":"https://mods.example.test/mods/signal-boost","checkedAt":"2026-11-21"}
        ]
    });
    let skill = "---\nname: offline-mcp-layouts\ndescription: Fixture game install rules\n---\n\
        winter-shaders, file winter-1: keep the JAR containing fabric.mod.json intact in mods/.\n\
        signal-boost, file signal-1: preserve the directory containing pack.mcmeta in resourcepacks/.";
    json!({"slug":slug,"adapter_json":adapter.to_string(),"skill_markdown":skill})
}
