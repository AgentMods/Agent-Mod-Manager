use super::*;
use crate::{game_actions, games, plugins};
use std::{fs, path::Path};

const GAME_TOOLS: &[&str] = &[
    "list_games",
    "register_game",
    "update_game",
    "remove_game",
    "launch_game",
    "set_game_adapter",
];

fn result(response: &CallToolResult) -> &Value {
    response.structured_content.as_ref().unwrap()
}

async fn session_id(client: &RunningService<RoleClient, ()>) -> String {
    result(&call(client, "amm_session", json!({})).await)["conversationId"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn assert_audit(runtime: &AgentRuntime, name: &str, response: &Value, approved: Option<bool>) {
    let call_id = response["toolCallId"].as_str().unwrap();
    let conn = runtime.engine.db.conn();
    let (stored_name, status, approval_status, stored_result): (String, String, String, String) =
        conn.query_row(
            "SELECT name,status,approval_status,result_json FROM tool_calls WHERE id=?1",
            [call_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(stored_name, name);
    assert_eq!(
        status,
        if approved == Some(false) {
            "error"
        } else {
            "done"
        }
    );
    assert_eq!(
        approval_status,
        match approved {
            Some(true) => "approved",
            Some(false) => "denied",
            None => "none",
        }
    );
    assert_eq!(
        serde_json::from_str::<Value>(&stored_result).unwrap(),
        response["result"]
    );
    let approval: Option<(String, String, String)> = conn
        .query_row(
            "SELECT conversation_id,title,status FROM approvals WHERE tool_call_id=?1",
            [call_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .unwrap();
    assert_eq!(
        approval,
        approved.map(|approved| (
            response["conversationId"].as_str().unwrap().into(),
            name.into(),
            if approved { "approved" } else { "rejected" }.into(),
        ))
    );
}

async fn run_game_call(
    fixture: &mut Fixture,
    client: &RunningService<RoleClient, ()>,
    conv: &str,
    name: &'static str,
    args: Value,
    approved: Option<bool>,
) -> Value {
    let response = if let Some(approved) = approved {
        let target = args
            .get("game_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let peer = client.peer().clone();
        let mut job = tokio::spawn(async move {
            peer.call_tool(
                CallToolRequestParams::new(name).with_arguments(args.as_object().unwrap().clone()),
            )
            .await
            .unwrap()
        });
        let approval = tokio::select! {
            event = next_event(&mut fixture.events, "approval-request") => event,
            early = &mut job => panic!("{name} returned before desktop approval: {early:?}"),
        };
        assert_eq!(approval.conversation_id, conv);
        assert_eq!(approval.payload["title"], name);
        let id = approval.payload["approvalId"].as_str().unwrap();
        if name == "remove_game" {
            // It is a conflict while pending; once approved it must not block its own commit.
            assert!(game_actions::list_game_conflicts(
                &fixture.runtime.engine,
                target.as_deref().unwrap(),
                game_actions::GameChange::Remove,
            )
            .unwrap()
            .iter()
            .any(|conflict| serde_json::to_value(conflict).unwrap()["id"] == id));
        }
        fixture
            .runtime
            .respond_approval(
                conv,
                id,
                ApprovalResponse {
                    approved,
                    answer: Value::Null,
                },
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), job)
            .await
            .unwrap()
            .unwrap()
    } else {
        call(client, name, args).await
    };
    assert_eq!(
        response.is_error,
        Some(approved == Some(false)),
        "{name}: {response:?}"
    );
    let envelope = result(&response);
    assert_eq!(envelope["conversationId"], conv);
    assert_audit(&fixture.runtime, name, envelope, approved);
    envelope["result"].clone()
}

fn write_launch_fixture(root: &Path) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        // Real executable with no arguments or disk side effects. Do not launch Steam in tests.
        let system_root = std::env::var_os("SystemRoot").unwrap();
        let path = root.join("launch-fixture.exe");
        fs::copy(
            std::path::PathBuf::from(system_root).join("System32/whoami.exe"),
            &path,
        )
        .unwrap();
        path
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("launch-fixture");
        fs::write(&path, b"#!/bin/sh\nprintf launched > launch-marker.txt\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
}

#[tokio::test]
async fn discovery_requires_auth_and_target_tools_do_not_infer_game_from_workspace() {
    let fixture = fixture_with_native_tools(GAME_TOOLS);
    let engine = &fixture.runtime.engine;
    let root = fixture._dir.path().join("registered-game");
    fs::create_dir(&root).unwrap();
    let game = games::register_game(&engine.db, &root, None).unwrap();
    let server = LocalMcpServer::new(fixture.runtime.clone());
    let endpoint = server.start().await.unwrap();
    let http = reqwest_mcp::Client::new();
    for (method, params) in [
        ("tools/list", json!({})),
        (
            "tools/call",
            json!({"name":"remove_game","arguments":{"game_id":game.id}}),
        ),
    ] {
        let response = http
            .post(&endpoint)
            .header("accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401);
    }

    let client = connect(&server, &fixture.runtime).await;
    let listed = client.peer().list_tools(None).await.unwrap();
    assert_eq!(listed.tools.len(), GAME_TOOLS.len() + 2);
    for name in GAME_TOOLS {
        let tool = listed.tools.iter().find(|tool| tool.name == *name).unwrap();
        let required = &tool.input_schema["required"];
        match *name {
            "list_games" => {}
            "register_game" => assert_eq!(required, &json!(["directory"])),
            _ => assert!(
                required.as_array().unwrap().contains(&json!("game_id")),
                "{name}: {required}"
            ),
        }
    }
    let conv = session_id(&client).await;
    History::new(engine.db.clone())
        .set_conversation_workspace(&conv, Some(std::slice::from_ref(&game.id)))
        .unwrap();
    let listed = call(&client, "list_games", json!({})).await;
    assert_eq!(listed.is_error, Some(false));
    let body = result(&listed);
    assert_eq!(body["result"]["total"], 1);
    assert_eq!(body["result"]["next_offset"], Value::Null);
    assert_eq!(body["result"]["games"][0]["id"], game.id);
    assert_eq!(body["result"]["games"][0]["name"], game.name);
    assert_audit(&fixture.runtime, "list_games", body, None);
    for (name, args) in [
        ("update_game", json!({"name":"unexpected rename"})),
        ("remove_game", json!({})),
        ("launch_game", json!({})),
        ("set_game_adapter", json!({"slug":"minecraft-java"})),
    ] {
        let rejected = call(&client, name, args).await;
        assert_eq!(rejected.is_error, Some(true), "{name}: {rejected:?}");
    }
    assert_eq!(
        games::get_game(&engine.db, &game.id).unwrap().name,
        game.name
    );
    client.cancel().await.unwrap();
    server.stop().await.unwrap();
}

#[tokio::test]
async fn game_writes_and_launch_use_the_host_permission_matrix() {
    for (mode, approval) in [
        ("request_approval", Some(true)),
        ("auto_approve", Some(true)),
        ("full_trust", None),
    ] {
        let mut fixture = fixture_with_native_tools(GAME_TOOLS);
        set_mode(&fixture.runtime, mode);
        let root = fixture._dir.path().join("test-game");
        fs::create_dir(&root).unwrap();
        let engine = fixture.runtime.engine.clone();
        let server = LocalMcpServer::new(fixture.runtime.clone());
        server.start().await.unwrap();
        let client = connect(&server, &fixture.runtime).await;
        let conv = session_id(&client).await;
        // An MCP session's conversation override does not grant/withdraw inbound permission.
        History::new(engine.db.clone())
            .set_permission_mode(
                &conv,
                Some(if approval.is_some() {
                    "full_trust"
                } else {
                    "request_approval"
                }),
            )
            .unwrap();

        let args = json!({"directory":root});
        let registered = run_game_call(
            &mut fixture,
            &client,
            &conv,
            "register_game",
            args,
            approval,
        )
        .await;
        let id = registered["gameId"].as_str().unwrap();
        let game = games::get_game(&engine.db, id).unwrap();
        assert_eq!(registered["gameName"], game.name);
        assert!(game.adapter_slug.is_none());

        let program = write_launch_fixture(&root);
        let outside = fixture._dir.path().join(program.file_name().unwrap());
        fs::copy(&program, &outside).unwrap();
        let denied = call(
            &client,
            "update_game",
            json!({"game_id":id,"launch_path":outside}),
        )
        .await;
        assert_eq!(
            denied.is_error,
            Some(true),
            "out-of-root launch path in {mode}: {denied:?}"
        );
        assert!(games::get_game(&engine.db, id)
            .unwrap()
            .metadata
            .get("launchPath")
            .is_none());

        let args = json!({"game_id":id,"name":"Reviewed game","launch_path":program});
        if approval.is_some() {
            run_game_call(
                &mut fixture,
                &client,
                &conv,
                "update_game",
                args.clone(),
                Some(false),
            )
            .await;
            assert_eq!(games::get_game(&engine.db, id).unwrap().name, game.name);
        }
        let updated =
            run_game_call(&mut fixture, &client, &conv, "update_game", args, approval).await;
        assert_eq!(updated["gameId"], id);
        assert_eq!(
            games::get_game(&engine.db, id).unwrap().name,
            "Reviewed game"
        );

        plugins::test_fixtures::install(&engine.db, &engine.paths).unwrap();
        fs::write(root.join("options.txt"), "scoped fixture").unwrap();
        let bound = run_game_call(
            &mut fixture,
            &client,
            &conv,
            "set_game_adapter",
            json!({"game_id":id,"slug":"minecraft-java"}),
            approval,
        )
        .await;
        assert_eq!(bound["gameId"], id);
        assert_eq!(
            games::get_game(&engine.db, id)
                .unwrap()
                .adapter_slug
                .as_deref(),
            Some("minecraft-java")
        );

        let args = json!({"game_id":id});
        if approval.is_some() {
            run_game_call(
                &mut fixture,
                &client,
                &conv,
                "launch_game",
                args.clone(),
                Some(false),
            )
            .await;
            #[cfg(unix)]
            assert!(!root.join("launch-marker.txt").exists());
        }
        let launched = run_game_call(
            &mut fixture,
            &client,
            &conv,
            "launch_game",
            args.clone(),
            approval,
        )
        .await;
        assert_eq!(launched["gameId"], id);
        #[cfg(unix)]
        eventually(|| root.join("launch-marker.txt").exists()).await;

        let removed =
            run_game_call(&mut fixture, &client, &conv, "remove_game", args, approval).await;
        assert_eq!(removed["gameId"], id);
        assert!(games::list_games(&engine.db).unwrap().is_empty());
        assert!(root.join("options.txt").is_file());
        client.cancel().await.unwrap();
        server.stop().await.unwrap();
    }
}

#[tokio::test]
async fn discovery_and_rejected_registration_never_grant_file_scope() {
    let mut fixture =
        fixture_with_native_tools(&["scan_games", "register_game", "plan_file_ops", "read_file"]);
    set_mode(&fixture.runtime, "auto_approve");
    let engine = fixture.runtime.engine.clone();
    let root = fixture._dir.path().join("candidate");
    let file = root.join("options.txt");
    plugins::test_fixtures::install(&engine.db, &engine.paths).unwrap();
    fs::create_dir_all(root.join("versions")).unwrap();
    fs::write(&file, "candidate only").unwrap();
    let server = LocalMcpServer::new(fixture.runtime.clone());
    server.start().await.unwrap();
    let client = connect(&server, &fixture.runtime).await;
    let conv = session_id(&client).await;
    let scanned = call(
        &client,
        "scan_games",
        json!({"options": {
            "userRoots": [root], "includeCommonRoots":false, "maxDepth":0,
        }}),
    )
    .await;
    assert_eq!(scanned.is_error, Some(false), "{scanned:?}");
    assert_eq!(
        result(&scanned)["result"]["candidates"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let plan_args =
        json!({"operations":[{"op":"mkdir","dst":root.join("mods")}],"summary":"test scope"});
    for attempt in 0..2 {
        assert!(games::list_games(&engine.db).unwrap().is_empty());
        assert_eq!(
            call(&client, "read_file", json!({"path":file}))
                .await
                .is_error,
            Some(true)
        );
        assert_eq!(
            call(&client, "plan_file_ops", plan_args.clone())
                .await
                .is_error,
            Some(true)
        );
        if attempt == 0 {
            run_game_call(
                &mut fixture,
                &client,
                &conv,
                "register_game",
                json!({"directory":root}),
                Some(false),
            )
            .await;
        }
    }
    assert!(!root.join("mods").exists());

    set_mode(&fixture.runtime, "full_trust");
    for directory in [
        engine.paths.root.clone(),
        fixture._dir.path().to_path_buf(),
        "relative-game".into(),
    ] {
        let rejected = call(&client, "register_game", json!({"directory":directory})).await;
        assert_eq!(rejected.is_error, Some(true), "{directory:?}: {rejected:?}");
    }
    let combined = call(
        &client,
        "register_game",
        json!({"directory":root,"adapter_slug":"minecraft-java","from_discovery":true}),
    )
    .await;
    assert_eq!(combined.is_error, Some(true));
    assert!(games::list_games(&engine.db).unwrap().is_empty());
    let sibling_file = fixture._dir.path().join("sibling.txt");
    fs::write(&sibling_file, "outside every game").unwrap();
    run_game_call(
        &mut fixture,
        &client,
        &conv,
        "register_game",
        json!({"directory":root}),
        None,
    )
    .await;
    assert_eq!(
        call(&client, "read_file", json!({"path":file}))
            .await
            .is_error,
        Some(false)
    );
    let repeated = call(&client, "register_game", json!({"directory":root})).await;
    assert_eq!(repeated.is_error, Some(true));
    let games = games::list_games(&engine.db).unwrap();
    assert_eq!(games.len(), 1);
    assert!(
        games[0].adapter_slug.is_none(),
        "these partial markers do not uniquely identify an adapter"
    );
    assert_eq!(
        call(&client, "read_file", json!({"path":sibling_file}))
            .await
            .is_error,
        Some(true)
    );
    client.cancel().await.unwrap();
    server.stop().await.unwrap();
}
