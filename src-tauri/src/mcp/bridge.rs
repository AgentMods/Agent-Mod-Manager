use super::client::{McpClient, McpTool, MAX_ARGUMENT_BYTES};
use crate::{
    agent::{
        policy::Effect,
        tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition, ToolRegistry},
    },
    error::{AppError, AppResult},
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct McpAgentTool {
    client: McpClient,
    tool: McpTool,
}

impl McpClient {
    /// Call only after a user-confirmed connection. Registration does not execute
    /// tools. Keep this client to disconnect it on disable/remove or app shutdown.
    pub async fn agent_tools(
        &self,
        cancel: &CancellationToken,
    ) -> AppResult<Vec<Arc<dyn AgentTool>>> {
        Ok(self
            .list_tools(cancel)
            .await?
            .into_iter()
            .map(|tool| {
                Arc::new(McpAgentTool {
                    client: self.clone(),
                    tool,
                }) as Arc<dyn AgentTool>
            })
            .collect())
    }

    pub async fn register_tools(
        &self,
        registry: &ToolRegistry,
        cancel: &CancellationToken,
    ) -> AppResult<Vec<String>> {
        let tools = self.agent_tools(cancel).await?;
        let names = tools.iter().map(|tool| tool.definition().name).collect();
        registry.register_batch(tools)?;
        Ok(names)
    }
}

#[async_trait::async_trait]
impl AgentTool for McpAgentTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.tool.name.clone(),
            description: format!(
                "[External untrusted MCP: {}] {}",
                self.client.server_id(),
                self.tool.description
            ),
            parameters: self.tool.input_schema.clone(),
            effect: Effect::ExternalWrite,
        }
    }

    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        if context.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        if !args.is_object() || serde_json::to_vec(args)?.len() > MAX_ARGUMENT_BYTES {
            return Err(AppError::Invalid(
                "Invalid or oversized MCP arguments".into(),
            ));
        }
        Ok(PreparedTool {
            preview: json!({
                "server": self.client.server_config(), "tool": self.tool.remote_name, "arguments":args,
                "trust":"untrusted_external", "localFileWhitelistEnforced":false,
                "warning":"External MCP may change remote state. Stdio runs with OS user privileges and can bypass local file guards. No remote annotation is trusted."
            }),
            effect: Effect::ExternalWrite,
            plan: None,
        })
    }

    async fn execute(
        &self,
        context: &ToolContext,
        args: Value,
        prepared: PreparedTool,
    ) -> AppResult<Value> {
        // A plan or Read effect must never smuggle execution around external policy.
        if prepared.effect != Effect::ExternalWrite
            || prepared.plan.is_some()
            || prepared.preview["arguments"] != args
            || prepared.preview["tool"] != self.tool.remote_name
        {
            return Err(AppError::Denied(
                "MCP execution does not match its external approval preview".into(),
            ));
        }
        self.client
            .call_tool(&self.tool.remote_name, args, &context.cancel)
            .await
    }
}
