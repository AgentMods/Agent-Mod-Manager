use super::{
    event, final_reply, next, runtime, server, server_with, ApprovalResponse, TurnRequest,
};
use crate::{
    agent::policy::PermissionMode, games, history::History, plugins, transaction::tests::setup,
};
use serde_json::{json, Value};

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
    let mut adapter: Value = serde_json::from_str(plugins::BUILTIN_PACKAGES[0].1).unwrap();
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
async fn existing_package_binding_obeys_chat_approval() {
    for approve in [false, true] {
        let (_temp, engine, root) = setup();
        std::fs::write(root.join("options.txt"), "").unwrap();
        plugins::seed_builtins(&engine.db, &engine.paths).unwrap();
        let (url, job) = server(vec![
            tool_call(
                "set_game_adapter",
                json!({"slug":"minecraft-java","game_id":"g"}),
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
        assert!(games::get_game(&agent.engine.db, "g")
            .unwrap()
            .adapter_slug
            .is_none());
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
        next(&mut rx, "done").await;
        job.await.unwrap();
    }
}
