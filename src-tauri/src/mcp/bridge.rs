use super::client::{McpClient, McpTool};
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
        self.list_tools(cancel)
            .await
            .map(|tools| self.agent_tools_from_catalog(tools))
    }

    pub(crate) fn agent_tools_from_catalog(&self, tools: Vec<McpTool>) -> Vec<Arc<dyn AgentTool>> {
        tools
            .into_iter()
            // The built-in publication workflow binds consent to an exact package and target.
            .filter(|tool| {
                !(self.server_config().is_glossmod_endpoint()
                    && matches!(
                        tool.remote_name.as_str(),
                        "publish_manager_plugin" | "publish_manager_plugin_release"
                    ))
            })
            .map(|tool| {
                if self.server_config().is_glossmod_endpoint()
                    && tool.remote_name == super::glossmod_download::TOOL
                {
                    return Arc::new(super::glossmod_download::GlossModDownload::new(
                        self.clone(),
                        tool.name,
                    )) as Arc<dyn AgentTool>;
                }
                Arc::new(McpAgentTool {
                    client: self.clone(),
                    tool,
                }) as Arc<dyn AgentTool>
            })
            .collect()
    }

    pub async fn register_tools(
        &self,
        registry: &ToolRegistry,
        cancel: &CancellationToken,
    ) -> AppResult<Vec<String>> {
        let tools = self.agent_tools(cancel).await?;
        let names = tools.iter().map(|tool| tool.definition().name).collect();
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        self.ensure_current()?;
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
                "[External untrusted MCP: {}] {} — {}",
                self.client.server_id(),
                self.tool.remote_name,
                self.tool.description
            ),
            parameters: self.tool.input_schema.clone(),
            effect: Effect::ExternalWrite,
        }
    }

    fn host_tool_kind(&self) -> Option<&'static str> {
        (self.client.server_config().is_glossmod_endpoint()
            && self.tool.remote_name == "list_work_files")
            .then_some("glossmod_work_files")
    }

    fn stock_source_tool(&self) -> Option<(&str, &str)> {
        // Identity metadata for the existing Agent install scope, not a
        // permission or an additional way to call the tool.
        (self.client.server_config().is_stock_browse_source()
            && self.client.ensure_browse_current().is_ok())
        .then(|| (self.client.server_id(), self.tool.remote_name.as_str()))
    }

    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        if context.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        if !args.is_object() {
            return Err(AppError::Invalid("MCP 参数必须是对象".into()));
        }
        let mut server = self.client.server_config().clone();
        server.description = None;
        Ok(PreparedTool {
            preview: json!({
                "server": server, "tool": self.tool.remote_name, "arguments":args,
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
            .call_agent_tool(&self.tool.remote_name, args, &context.cancel)
            .await
    }
}
