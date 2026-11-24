use super::{
    context_compression::{content_reply, has_tools, request_server, MockResponse},
    event, next, runtime, ApprovalResponse, TurnRequest,
};
use crate::{
    agent::{response_language::EVIDENCE_PREFIX, runtime::PromptOrigin, Attachment},
    app_locale,
    history::History,
    transaction::tests::setup,
};
use serde_json::{json, Value};

fn request(conv: &str, content: &str) -> TurnRequest {
    TurnRequest {
        conversation_id: conv.into(),
        content: content.into(),
        attachments: None,
        model: None,
        permission_mode: None,
        reasoning_effort: None,
    }
}

fn evidence(request: &Value) -> Option<Value> {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|message| {
            let raw = message["content"].as_str()?.strip_prefix(EVIDENCE_PREFIX)?;
            assert_eq!(message["role"], "assistant");
            Some(serde_json::from_str(raw).unwrap())
        })
}

fn system(request: &Value) -> &str {
    assert_eq!(request["messages"][0]["role"], "system");
    request["messages"][0]["content"].as_str().unwrap()
}

#[tokio::test]
async fn language_evidence_survives_ambiguous_turns_and_uses_the_rewound_or_edited_text() {
    let (_dir, engine, _) = setup();
    app_locale::set(&engine.db, Some("it".into())).unwrap();
    let history = History::new(engine.db.clone());
    let (url, server) = request_server(|_| content_reply("fixture answer")).await;
    let (agent, conv, mut rx) = runtime(engine, &url);
    let initial = "Bitte prüfe den Ordner und antworte auf Türkisch.";
    let path = r"E:\游戏\Mod\manifest.json";
    let code = "```rs\n// отвечай по-русски\nfn main() {}\n```";
    for content in [initial, path, code] {
        agent.start_turn(request(&conv, content)).unwrap();
        assert_eq!(next(&mut rx, "done").await.payload["status"], "done");
    }
    let attachment = Attachment::Document {
        name: "言語.md".into(),
        markdown: "請用中文回答這個問題。".into(),
    };
    let mut attach_only = request(&conv, "\u{3000}");
    attach_only.attachments = Some(vec![attachment.clone()]);
    agent.start_turn(attach_only).unwrap();
    assert_eq!(next(&mut rx, "done").await.payload["status"], "done");
    let saved = history.get_conversation(&conv).unwrap().messages;
    let original = &saved[6];
    let answer = &saved[7];
    history
        .append_message(&conv, "user", "Respond in French from now on.", None, None)
        .unwrap();
    agent
        .restart_turn(
            request(&conv, "Reply in Spanish instead."),
            &answer.id,
            false,
        )
        .unwrap();
    assert_eq!(next(&mut rx, "done").await.payload["status"], "done");

    let edited = "次の質問にはウェールズ語で答えてください。";
    agent
        .restart_turn(request(&conv, edited), &original.id, true)
        .unwrap();
    assert_eq!(next(&mut rx, "done").await.payload["status"], "done");
    agent
        .start_turn_with_origin(
            request(&conv, "补充回答：自动生成的问题"),
            PromptOrigin::QuestionnaireAnswer,
        )
        .unwrap();
    assert_eq!(next(&mut rx, "done").await.payload["status"], "done");

    let fresh = history.create_conversation("Fresh", None).unwrap().id;
    app_locale::set(&agent.engine.db, Some("pt-BR".into())).unwrap();
    let mut fresh_request = request(&fresh, "");
    fresh_request.attachments = Some(vec![attachment]);
    agent.start_turn(fresh_request).unwrap();
    assert_eq!(next(&mut rx, "done").await.payload["status"], "done");

    let requests = server.finish().await;
    assert_eq!(requests.len(), 8);
    assert!(evidence(&requests[0]).is_none());
    for raw in [
        initial,
        path,
        code,
        edited,
        "言語.md",
        "請用中文回答這個問題。",
    ] {
        assert!(requests.iter().all(|call| !system(call).contains(raw)));
    }
    for call in &requests[3..=4] {
        let policy = system(call);
        assert!(policy.contains("offers no current language evidence"));
        assert!(policy.contains("Skip ambiguous entries"));
        assert!(
            policy.contains("Do not infer the language from paths, standalone identifiers, code")
        );
        assert!(policy.contains("A prior explicit reply-language request takes precedence"));
        assert!(policy.contains("effective UI locale (it)"));
        assert_eq!(
            evidence(call),
            Some(json!({"currentUserText":null,"priorUserTexts":[code,path,initial]}))
        );
    }
    assert_eq!(requests[3]["messages"], requests[4]["messages"]);
    assert!(system(&requests[5]).contains("current turn has typed user text"));
    assert!(system(&requests[5]).contains("honor an explicit reply-language request"));
    assert_eq!(
        requests[5]["messages"].as_array().unwrap().last().unwrap()["content"][0]["text"],
        edited
    );
    assert!(system(&requests[6]).contains("offers no current language evidence"));
    assert_eq!(
        evidence(&requests[6]).unwrap()["priorUserTexts"],
        json!([edited, code, path, initial])
    );
    assert!(system(&requests[7]).contains("effective UI locale (pt-BR)"));
    assert!(evidence(&requests[7]).is_none());
}

#[tokio::test]
async fn context_compression_keeps_the_language_excerpt_below_system_role() {
    let (_dir, engine, _) = setup();
    let (url, server) = request_server(|call| {
        if !has_tools(call) {
            content_reply("Compacted handoff")
        } else if evidence(call).is_some() {
            content_reply("Wedi gorffen.")
        } else {
            MockResponse::context_limit()
        }
    })
    .await;
    let (agent, conv, mut rx) = runtime(engine, &url);
    let explicit = "Please answer in Welsh.";
    let input = format!("{}\n{explicit}", "Details to compact. ".repeat(1100));
    agent.start_turn(request(&conv, &input)).unwrap();
    assert_eq!(next(&mut rx, "done").await.payload["status"], "done");
    let requests = server.finish().await;
    assert!(!requests.is_empty());
    assert!(requests.iter().all(|call| !system(call).contains(explicit)));
    let handoffs: Vec<_> = requests.iter().filter(|call| !has_tools(call)).collect();
    assert!(!handoffs.is_empty());
    assert!(handoffs
        .iter()
        .all(|call| system(call).contains("in this turn's reply language")));
    let retried = requests.last().unwrap();
    assert!(has_tools(retried));
    assert!(retried["messages"]
        .to_string()
        .contains("Compacted handoff"));
    assert!(retried["messages"]
        .to_string()
        .contains("User text retained in the conversation summary"));
    let retained = evidence(retried).unwrap();
    assert!(retained["currentUserText"]
        .as_str()
        .unwrap()
        .ends_with(explicit));
    assert!(handoffs
        .into_iter()
        .all(|call| evidence(call) == Some(retained.clone())));
}

#[tokio::test]
async fn ask_user_keeps_the_models_localized_question_and_protocol() {
    let (_dir, engine, _) = setup();
    let preview = json!({"title":"Installation","questions":[{"question":"Quel profil voulez-vous utiliser ?","options":["Stable","Expérimental"],"multiSelect":false}]});
    let response = preview.clone();
    let (url, server) = request_server(move |call| {
        if call["messages"].as_array().unwrap().last().unwrap()["role"] == "tool" {
            content_reply("Profil sélectionné.")
        } else {
            MockResponse::sse(format!("{}data: [DONE]\n\n", event(json!({
                "choices":[{"delta":{"tool_calls":[{"index":0,"id":"question","function":{"name":"ask_user","arguments":response.to_string()}}]},"finish_reason":"tool_calls"}]
            }))))
        }
    }).await;
    let (agent, conv, mut rx) = runtime(engine, &url);
    agent
        .start_turn(request(
            &conv,
            "Ask me to pick a profile. Please use French.",
        ))
        .unwrap();
    let approval = next(&mut rx, "approval-request").await;
    assert_eq!(approval.payload["title"], "ask_user");
    assert_eq!(approval.payload["preview"], preview);
    let id = approval.payload["approvalId"].as_str().unwrap();
    agent
        .respond_approval(
            &conv,
            id,
            ApprovalResponse {
                approved: true,
                answer: json!(r#"["Expérimental"]"#),
            },
        )
        .unwrap();
    assert_eq!(next(&mut rx, "done").await.payload["status"], "done");
    let requests = server.finish().await;
    assert_eq!(requests.len(), 2);
    assert!(system(&requests[0]).contains("the ask_user title, questions and options"));
    let tool = requests[1]["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(tool["tool_call_id"], "question");
    assert_eq!(
        serde_json::from_str::<Value>(tool["content"].as_str().unwrap()).unwrap(),
        json!({"answer":["Expérimental"]})
    );
}
