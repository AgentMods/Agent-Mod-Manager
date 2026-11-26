use super::TurnRequest;
use crate::{
    agent::{events::AgentEvent, policy::PermissionMode},
    history::History,
};
use rusqlite::params;
use serde_json::{json, Value};
use tokio::sync::mpsc;

fn browse_task_conversation(db: &crate::db::Db) -> String {
    let conversation = History::new(db.clone())
        .create_conversation_with_category("Browse install", Some("g"), Some("install"))
        .unwrap();
    db.conn()
        .execute(
            "INSERT INTO browse_install_tasks(id,conversation_id,game_id,source_id,project_id,provider_game_id)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params!["browse-task", conversation.id, "g", "curseforge", "45", "7"],
        )
        .unwrap();
    conversation.id
}

fn request(
    conversation_id: &str,
    content: &str,
    permission_mode: Option<PermissionMode>,
) -> TurnRequest {
    TurnRequest {
        conversation_id: conversation_id.into(),
        content: content.into(),
        attachments: None,
        model: None,
        permission_mode,
        reasoning_effort: None,
    }
}

fn tool_call_response(name: &str, id: &str, arguments: Value) -> String {
    format!(
        "{}data: [DONE]\n\n",
        super::event(json!({"choices":[{"index":0,"delta":{"tool_calls":[{
            "index":0,"id":id,"type":"function",
            "function":{"name":name,"arguments":arguments.to_string()}
        }]},"finish_reason":"tool_calls"}]}))
    )
}

fn tool_names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .expect("model request should carry a tool list")
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
        .collect()
}

fn assert_browse_request_contract(body: &Value) {
    let system = body["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("单作品安装"), "{system}");
    assert!(system.contains("原生任务记录"), "{system}");

    let submitted_guidance = format!("{system}\n{}", body["tools"]);
    for unavailable_guidance in [
        "list_plugin_packages",
        "read_skill",
        "system-mod-download",
        "Playwright MCP",
        "download_web_mod",
    ] {
        assert!(
            !submitted_guidance.contains(unavailable_guidance),
            "Browse model request must not recommend unavailable or generic actions ({unavailable_guidance}): {submitted_guidance}"
        );
    }

    let names = tool_names(body);
    for allowed_action in [
        "list_mod_library",
        "match_mod_install",
        "install_library_mod",
    ] {
        assert!(
            names.iter().any(|name| name == allowed_action),
            "Browse request should expose the selected-work install path; missing {allowed_action}: {names:?}"
        );
    }
    for unavailable_action in ["list_plugin_packages", "read_skill", "plan_file_ops"] {
        assert!(
            !names.iter().any(|name| name == unavailable_action),
            "Browse request must not declare {unavailable_action}: {names:?}"
        );
    }
}

async fn assert_scoped_turn(
    rx: &mut mpsc::UnboundedReceiver<AgentEvent>,
    expected_turn_id: Option<&str>,
    expected_mode: PermissionMode,
    expected_tool: &str,
    expected_error: &str,
) {
    let start = super::next(rx, "turn-start").await;
    if let Some(expected_turn_id) = expected_turn_id {
        assert_eq!(start.turn_id, expected_turn_id);
    }
    assert_eq!(start.payload["permissionMode"], json!(expected_mode));

    let result = super::next(rx, "tool-result").await;
    assert_eq!(result.payload["name"], expected_tool);
    assert_eq!(result.payload["status"], "error");
    assert!(result.payload["result"]["error"]
        .as_str()
        .unwrap()
        .contains(expected_error));

    let done = super::next(rx, "done").await;
    assert_eq!(done.turn_id, start.turn_id);
    assert_eq!(done.payload["status"], "done");
}

#[tokio::test]
async fn existing_browse_task_can_continue_and_retry_with_its_scope_and_permission_priority() {
    let (_dir, engine, _game) = super::setup();
    let forbidden_write = tool_call_response(
        "plan_file_ops",
        "forbidden-write",
        json!({"operations":[{"op":"mkdir","dst":"outside-task-scope"}],"summary":"must be blocked"}),
    );
    let forged_catalog = tool_call_response(
        "resolve_mod",
        "forged-catalog",
        json!({"source_id":"curseforge","project_id":"999","game_id":"7"}),
    );
    let (url, model) = super::server(vec![
        forbidden_write.clone(),
        super::final_reply(),
        forged_catalog,
        super::final_reply(),
        forbidden_write,
        super::final_reply(),
        super::final_reply(),
    ])
    .await;
    let (agent, _unused_conversation, mut rx) = super::runtime(engine.clone(), &url);
    // A browse continuation inherits the saved permission mode; it must not elevate it.
    agent
        .settings
        .set("permission_mode", json!("full_trust"))
        .unwrap();
    let conversation = browse_task_conversation(&engine.db);

    let first = agent
        .start_turn(request(
            &conversation,
            "Continue this selected install",
            None,
        ))
        .unwrap();
    assert_scoped_turn(
        &mut rx,
        Some(&first),
        PermissionMode::FullTrust,
        "plan_file_ops",
        "本轮页内安装未提供此工具",
    )
    .await;

    // A lower permission supplied by this request remains authoritative.
    let second = agent
        .start_turn(request(
            &conversation,
            "Check the selected work again",
            Some(PermissionMode::RequestApproval),
        ))
        .unwrap();
    assert_scoped_turn(
        &mut rx,
        Some(&second),
        PermissionMode::RequestApproval,
        "resolve_mod",
        "超出页内安装范围",
    )
    .await;

    let user_message = History::new(engine.db.clone())
        .get_conversation(&conversation)
        .unwrap()
        .messages
        .into_iter()
        .rev()
        .find(|message| message.role == "user")
        .unwrap();
    agent
        .settings
        .set(
            &crate::settings::conversation_permission_key(&conversation),
            json!("auto_approve"),
        )
        .unwrap();
    let retried = agent
        .restart_turn(
            request(&conversation, "Retry the selected work", None),
            &user_message.id,
            false,
        )
        .unwrap();
    assert_eq!(retried, conversation);
    assert_scoped_turn(
        &mut rx,
        None,
        PermissionMode::AutoApprove,
        "plan_file_ops",
        "本轮页内安装未提供此工具",
    )
    .await;

    // A normal conversation still gets the configured permission and full tool registry.
    agent
        .settings
        .set("permission_mode", json!("request_approval"))
        .unwrap();
    let ordinary = History::new(engine.db)
        .create_conversation("Ordinary chat", None)
        .unwrap()
        .id;
    let ordinary_turn = agent
        .start_turn(request(&ordinary, "Just chat", None))
        .unwrap();
    let ordinary_start = super::next(&mut rx, "turn-start").await;
    assert_eq!(ordinary_start.turn_id, ordinary_turn);
    assert_eq!(
        ordinary_start.payload["permissionMode"],
        json!(PermissionMode::RequestApproval)
    );
    assert_eq!(super::next(&mut rx, "done").await.payload["status"], "done");

    let requests = model.await.unwrap();
    assert_eq!(requests.len(), 7);
    for index in [0, 2, 4] {
        let names = tool_names(&requests[index]);
        assert!(names.contains(&"resolve_mod".to_owned()), "{names:?}");
        assert!(
            names.contains(&"install_library_mod".to_owned()),
            "{names:?}"
        );
        assert!(!names.contains(&"plan_file_ops".to_owned()), "{names:?}");
        assert!(!names.contains(&"trash_paths".to_owned()), "{names:?}");
    }
    for (request_index, label) in [(0, "完全信任"), (2, "逐次审批"), (4, "游戏内自动批准")]
    {
        let system = requests[request_index]["messages"][0]["content"]
            .as_str()
            .unwrap();
        assert!(system.contains(&format!("本轮权限：{label}")), "{system}");
    }
    let ordinary_tools = tool_names(&requests[6]);
    assert!(ordinary_tools.contains(&"plan_file_ops".to_owned()));
    assert!(ordinary_tools.contains(&"trash_paths".to_owned()));
}

#[tokio::test]
async fn browse_model_request_uses_only_browse_guidance_and_keeps_the_install_path_available() {
    let (_dir, engine, _game) = super::setup();
    let (url, model) = super::server(vec![super::final_reply()]).await;
    let (agent, _unused_conversation, mut rx) = super::runtime(engine.clone(), &url);
    let conversation = browse_task_conversation(&engine.db);

    agent
        .start_turn(request(
            &conversation,
            "Install the selected result",
            Some(PermissionMode::FullTrust),
        ))
        .unwrap();
    assert_eq!(super::next(&mut rx, "done").await.payload["status"], "done");

    let requests = model.await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_browse_request_contract(&requests[0]);
}

#[tokio::test]
async fn undeclared_browse_tool_gets_available_actions_and_the_turn_can_continue() {
    let (_dir, engine, _game) = super::setup();
    let hallucinated = tool_call_response("list_plugin_packages", "invented-list", json!({}));
    let declared = tool_call_response(
        "list_mod_library",
        "available-library-list",
        json!({"game_id":"g"}),
    );
    let (url, model) = super::server(vec![hallucinated, declared, super::final_reply()]).await;
    let (agent, _unused_conversation, mut rx) = super::runtime(engine.clone(), &url);
    let conversation = browse_task_conversation(&engine.db);

    agent
        .start_turn(request(
            &conversation,
            "Continue this selected install",
            Some(PermissionMode::FullTrust),
        ))
        .unwrap();

    let rejected = super::next(&mut rx, "tool-result").await;
    assert_eq!(rejected.payload["name"], "list_plugin_packages");
    assert_eq!(rejected.payload["status"], "error");
    let actual_result = super::next(&mut rx, "tool-result").await;
    assert_eq!(actual_result.payload["name"], "list_mod_library");
    assert_eq!(actual_result.payload["status"], "done");
    assert_eq!(super::next(&mut rx, "done").await.payload["status"], "done");

    let requests = model.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(
        !tool_names(&requests[0])
            .iter()
            .any(|name| name == "list_plugin_packages"),
        "the hallucinated function must not have been declared"
    );
    let feedback = requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool" && message["tool_call_id"] == "invented-list")
        .and_then(|message| message["content"].as_str())
        .expect("model should receive the rejected tool result");
    assert!(
        feedback.contains("本轮页内安装未提供此工具"),
        "feedback should explain that the action is unavailable in this Browse turn: {feedback}"
    );
    for available_action in [
        "list_mod_library",
        "match_mod_install",
        "install_library_mod",
    ] {
        assert!(
            feedback.contains(available_action),
            "feedback should name the declared recovery action {available_action}: {feedback}"
        );
    }
}

#[tokio::test]
async fn retried_browse_turn_keeps_native_identity_and_scoped_prompt_after_forced_compaction() {
    const SUMMARY: &str = "BROWSE_RETRY_CONTEXT_SUMMARY";

    let (_dir, engine, _game) = super::setup();
    let (url, server) = super::context_compression::request_server(|request| {
        if !super::context_compression::has_tools(request) {
            super::context_compression::content_reply(SUMMARY)
        } else if request["messages"].to_string().contains(SUMMARY) {
            super::context_compression::content_reply("压缩后继续")
        } else {
            super::context_compression::MockResponse::context_limit()
        }
    })
    .await;
    let (agent, _unused_conversation, mut rx) = super::runtime(engine.clone(), &url);
    let conversation = browse_task_conversation(&engine.db);
    let history = History::new(engine.db.clone());
    history
        .append_message(&conversation, "user", "Earlier work note", None, None)
        .unwrap();
    history
        .append_message(&conversation, "assistant", "Earlier response", None, None)
        .unwrap();
    history
        .append_message(&conversation, "user", "Replace this text", None, None)
        .unwrap();
    let selected_user = history
        .get_conversation(&conversation)
        .unwrap()
        .messages
        .into_iter()
        .rev()
        .find(|message| message.role == "user")
        .unwrap()
        .id;

    agent
        .restart_turn(
            request(
                &conversation,
                "Retry the selected install",
                Some(PermissionMode::FullTrust),
            ),
            &selected_user,
            true,
        )
        .unwrap();
    assert_eq!(super::next(&mut rx, "done").await.payload["status"], "done");

    let requests = server.finish().await;
    let compacted_retry = requests
        .iter()
        .find(|request| {
            super::context_compression::has_tools(request)
                && request["messages"].to_string().contains(SUMMARY)
        })
        .expect("the context-overflow request should be retried after remote compaction");
    let initial_browse_request = requests
        .iter()
        .find(|request| {
            super::context_compression::has_tools(request)
                && !request["messages"].to_string().contains(SUMMARY)
        })
        .expect("the mock should capture the initial scoped Browse request");
    assert!(
        requests
            .iter()
            .any(|request| !super::context_compression::has_tools(request)),
        "forced compaction should issue a separate summary request"
    );

    assert_browse_request_contract(compacted_retry);
    assert_eq!(
        compacted_retry["messages"][0], initial_browse_request["messages"][0],
        "compression should preserve the Browse system instruction verbatim"
    );
    assert_eq!(compacted_retry["tools"], initial_browse_request["tools"]);

    let system = compacted_retry["messages"][0]["content"].as_str().unwrap();
    for native_identity_field in [
        r#""gameId":"g""#,
        r#""providerGameId":"7""#,
        r#""sourceId":"curseforge""#,
        r#""catalogSourceId":"curseforge""#,
        r#""projectId":"45""#,
    ] {
        assert!(
            system.contains(native_identity_field),
            "retry system prompt lost the native Browse identity field {native_identity_field}: {system}"
        );
    }
    let current_user_text = compacted_retry["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .and_then(|message| message["content"].as_str())
        .expect("retry request should retain its current user text");
    assert_eq!(current_user_text, "Retry the selected install");
    for native_id in ["g", "7", "curseforge", "45"] {
        assert!(
            !current_user_text.contains(native_id),
            "retry free text should not carry native identity {native_id}: {current_user_text}"
        );
    }
}
