use super::{event, final_reply, next, runtime, server_with, TurnRequest};
use crate::{agent::policy::PermissionMode, games, history::History, transaction::tests::setup};
use serde_json::{json, Value};

fn calls(name: &str, arguments: Vec<Value>) -> String {
    let tools = arguments
        .into_iter()
        .enumerate()
        .map(|(i, args)| {
            json!({
                "index": i, "id": format!("{name}_{i}"), "type": "function",
                "function": {"name": name, "arguments": args.to_string()}
            })
        })
        .collect::<Vec<_>>();
    let delta = json!({"choices":[{"delta":{"tool_calls":tools},"finish_reason":"tool_calls"}]});
    format!("{}data: [DONE]\n\n", event(delta))
}

#[tokio::test]
async fn remove_workspace_games_in_one_turn_preserves_other_registrations_and_disk() {
    let (temp, engine, first_root) = setup();
    let second_root = temp.path().join("second");
    let other_root = temp.path().join("outside-selection");
    std::fs::create_dir(&second_root).unwrap();
    std::fs::create_dir(&other_root).unwrap();
    let second = games::register_game(&engine.db, &second_root, None).unwrap();
    let other = games::register_game(&engine.db, &other_root, None).unwrap();
    for root in [&first_root, &second_root] {
        std::fs::write(root.join("save.dat"), b"keep").unwrap();
    }
    let (url, job) = server_with(3, |round, body| match round {
        0 => calls("list_games", vec![json!({})]),
        1 => {
            let content = body["messages"].as_array().unwrap().last().unwrap()["content"]
                .as_str()
                .unwrap();
            let catalog: Value = serde_json::from_str(content).unwrap();
            assert_eq!(catalog["total"], 2);
            let targets = catalog["games"]
                .as_array()
                .unwrap()
                .iter()
                .map(|game| json!({"game_id":game["id"]}))
                .collect();
            calls("remove_game", targets)
        }
        _ => final_reply(),
    })
    .await;
    let (agent, conversation, mut rx) = runtime(engine, &url);
    let history = History::new(agent.engine.db.clone());
    history
        .set_conversation_workspace(&conversation, Some(&["g".into(), second.id]))
        .unwrap();
    agent
        .start_turn(TurnRequest {
            conversation_id: conversation.clone(),
            content: "从游戏库移除工作区的全部游戏".into(),
            permission_mode: Some(PermissionMode::FullTrust),
            attachments: None,
            model: None,
            reasoning_effort: None,
        })
        .unwrap();
    let done = next(&mut rx, "done").await;
    assert_eq!(done.payload["status"], "done");
    let events = agent.events.replay(&conversation, 0, 100).unwrap();
    let results = events
        .iter()
        .filter(|e| e.kind == "tool-result")
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 3);
    for result in results {
        assert_eq!(result.payload["status"], "done", "{}", result.payload);
    }
    assert!(!events.iter().any(|e| e.kind == "approval-request"));
    assert_eq!(
        history.conversation_workspace(&conversation).unwrap(),
        Some(vec![])
    );
    let remaining = games::list_games(&agent.engine.db).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, other.id);
    for root in [&first_root, &second_root] {
        assert_eq!(std::fs::read(root.join("save.dat")).unwrap(), b"keep");
    }
    assert_eq!(job.await.unwrap().len(), 3);
}
