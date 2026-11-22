use super::{event, final_reply, next, runtime, server_with, setup};
use crate::{
    agent::{
        policy::{Effect, PermissionMode},
        runtime::TurnRequest,
        tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition},
    },
    error::{AppError, AppResult},
    history::History,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::mpsc;

const TOOL_NAME: &str = "regression_count_step";
const LONG_TURN_TOOL_ROUNDS: usize = 41;

struct CountingTool {
    executions: Arc<AtomicUsize>,
    observed: mpsc::UnboundedSender<usize>,
    cancel_on: Option<usize>,
}

#[async_trait::async_trait]
impl AgentTool for CountingTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: TOOL_NAME.into(),
            description: "Test-only deterministic side effect counter".into(),
            parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
            effect: Effect::Read,
        }
    }

    async fn execute(
        &self,
        context: &ToolContext,
        _args: Value,
        _prepared: PreparedTool,
    ) -> AppResult<Value> {
        let execution = self.executions.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.observed.send(execution);
        if self.cancel_on == Some(execution) {
            context.cancel.cancelled().await;
            return Err(AppError::Cancelled);
        }
        Ok(json!({"execution":execution}))
    }
}

fn register_counting_tool(
    agent: &super::AgentRuntime,
    cancel_on: Option<usize>,
) -> (Arc<AtomicUsize>, mpsc::UnboundedReceiver<usize>) {
    let executions = Arc::new(AtomicUsize::new(0));
    let (observed, rx) = mpsc::unbounded_channel();
    agent
        .registry
        .register(Arc::new(CountingTool {
            executions: executions.clone(),
            observed,
            cancel_on,
        }))
        .unwrap();
    (executions, rx)
}

fn tool_reply(index: usize) -> String {
    format!(
        "{}data: [DONE]\n\n",
        event(json!({
            "choices":[{"index":0,"delta":{"tool_calls":[{
                "index":0,
                "id":format!("regression-call-{index}"),
                "type":"function",
                "function":{"name":TOOL_NAME,"arguments":"{}"}
            }]},"finish_reason":"tool_calls"}]
        }))
    )
}

fn request(conversation_id: String, content: &str) -> TurnRequest {
    TurnRequest {
        conversation_id,
        content: content.into(),
        attachments: None,
        model: None,
        permission_mode: Some(PermissionMode::FullTrust),
        reasoning_effort: None,
    }
}

#[tokio::test]
async fn one_user_message_completes_after_forty_one_tool_rounds_and_persists_history() {
    let (_dir, engine, _game) = setup();
    let response_count = LONG_TURN_TOOL_ROUNDS + 1;
    let (url, server) = server_with(response_count, move |index, _body| {
        if index < LONG_TURN_TOOL_ROUNDS {
            tool_reply(index)
        } else {
            final_reply()
        }
    })
    .await;
    let (agent, conversation, mut events) = runtime(engine, &url);
    let (executions, _observed) = register_counting_tool(&agent, None);

    let turn = agent
        .start_turn(request(conversation.clone(), "执行长工具链"))
        .unwrap();
    let done = next(&mut events, "done").await;
    assert_eq!(done.turn_id, turn);
    assert_eq!(done.payload["status"], "done");

    let requests = tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .expect("mock model service did not receive the final request")
        .unwrap();
    assert_eq!(requests.len(), response_count);
    assert_eq!(executions.load(Ordering::SeqCst), LONG_TURN_TOOL_ROUNDS);
    assert!(requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| { tool["function"]["name"] == TOOL_NAME }));

    let history = History::new(agent.engine.db.clone())
        .get_conversation(&conversation)
        .unwrap();
    assert_eq!(
        history
            .messages
            .iter()
            .filter(|message| message.role == "user" && message.content == "执行长工具链")
            .count(),
        1
    );
    assert_eq!(
        history
            .messages
            .iter()
            .filter(|message| message.role == "tool")
            .count(),
        LONG_TURN_TOOL_ROUNDS
    );
    assert_eq!(
        history
            .messages
            .iter()
            .filter(|message| message.role == "assistant" && message.tool_calls.is_some())
            .count(),
        LONG_TURN_TOOL_ROUNDS
    );
    assert_eq!(history.messages.last().unwrap().content, "完成");
    assert_eq!(
        history.conversation.message_count as usize,
        history.messages.len()
    );

    let persisted_turn_status: String = agent
        .engine
        .db
        .conn()
        .query_row(
            "SELECT status FROM agent_turns WHERE id=?1",
            [&turn],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted_turn_status, "done");
    let persisted_tool_calls: i64 = agent
        .engine
        .db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM tool_calls WHERE conversation_id=?1 AND name=?2 AND status='done' AND finished_at IS NOT NULL",
            rusqlite::params![conversation, TOOL_NAME],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(persisted_tool_calls as usize, LONG_TURN_TOOL_ROUNDS);
    assert!(agent
        .events
        .replay(&conversation, 0, 1_000)
        .unwrap()
        .iter()
        .any(|event| event.kind == "done" && event.payload["status"] == "done"));
}

#[tokio::test]
async fn cancelling_during_the_thirty_third_tool_stops_without_later_side_effects() {
    const CANCEL_AT_EXECUTION: usize = 33;

    let (_dir, engine, _game) = setup();
    let (url, server) = server_with(CANCEL_AT_EXECUTION, |index, _body| tool_reply(index)).await;
    let (agent, conversation, mut events) = runtime(engine, &url);
    let (executions, mut observed) = register_counting_tool(&agent, Some(CANCEL_AT_EXECUTION));

    let turn = agent
        .start_turn(request(conversation.clone(), "执行后取消长工具链"))
        .unwrap();
    for expected in 1..=CANCEL_AT_EXECUTION {
        let execution = tokio::time::timeout(std::time::Duration::from_secs(10), observed.recv())
            .await
            .expect("agent stopped before reaching the cancellation point")
            .expect("counting tool closed before the cancellation point");
        assert_eq!(execution, expected);
    }

    agent.cancel_turn(&conversation, &turn).unwrap();
    let done = next(&mut events, "done").await;
    assert_eq!(done.turn_id, turn);
    assert_eq!(done.payload["status"], "cancelled");
    server.await.unwrap();

    assert_eq!(executions.load(Ordering::SeqCst), CANCEL_AT_EXECUTION);
    let history = History::new(agent.engine.db.clone())
        .get_conversation(&conversation)
        .unwrap();
    assert_eq!(
        history
            .messages
            .iter()
            .filter(|message| message.role == "user" && message.content == "执行后取消长工具链")
            .count(),
        1
    );
    assert_eq!(
        history
            .messages
            .iter()
            .filter(|message| message.role == "tool")
            .count(),
        CANCEL_AT_EXECUTION
    );

    let (persisted_turn_status, terminal_tool_calls): (String, i64) = agent
        .engine
        .db
        .conn()
        .query_row(
            "SELECT (SELECT status FROM agent_turns WHERE id=?1), (SELECT COUNT(*) FROM tool_calls WHERE conversation_id=?2 AND name=?3 AND finished_at IS NOT NULL)",
            rusqlite::params![turn, conversation, TOOL_NAME],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(persisted_turn_status, "cancelled");
    assert_eq!(terminal_tool_calls as usize, CANCEL_AT_EXECUTION);
    assert!(agent
        .events
        .replay(&conversation, 0, 1_000)
        .unwrap()
        .iter()
        .any(|event| event.kind == "done" && event.payload["status"] == "cancelled"));
}
