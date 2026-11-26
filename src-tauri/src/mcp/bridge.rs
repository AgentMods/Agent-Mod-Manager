use super::client::{McpClient, McpTool};
use crate::{
    agent::{
        policy::Effect,
        tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition, ToolRegistry},
    },
    error::{AppError, AppResult},
};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct McpAgentTool {
    client: McpClient,
    name: String,
    details: Arc<McpToolDetails>,
    input_schema: Value,
}

#[derive(Serialize)]
pub struct McpToolDetails {
    name: String,
    description: String,
}

pub(crate) struct BridgedTool {
    pub name: String,
    pub details: Arc<McpToolDetails>,
    pub agent: Arc<dyn AgentTool>,
}

impl McpClient {
    /// Call only after a user-confirmed connection. Registration does not execute
    /// tools. Keep this client to disconnect it on disable/remove or app shutdown.
    pub async fn agent_tools(
        &self,
        cancel: &CancellationToken,
    ) -> AppResult<Vec<Arc<dyn AgentTool>>> {
        let catalog = self.list_tools(cancel).await?;
        Ok(self
            .agent_tools_from_catalog(catalog)
            .map(|tool| tool.agent)
            .collect())
    }

    pub(crate) fn agent_tools_from_catalog(
        &self,
        tools: Vec<McpTool>,
    ) -> impl Iterator<Item = BridgedTool> + '_ {
        let is_glossmod = self.server_config().is_glossmod_endpoint();
        tools
            .into_iter()
            // The built-in publication workflow binds consent to an exact package and target.
            .filter(move |tool| {
                !(is_glossmod
                    && matches!(
                        tool.remote_name.as_str(),
                        "publish_manager_plugin" | "publish_manager_plugin_release"
                    ))
            })
            .map(move |tool| {
                let McpTool {
                    name,
                    remote_name,
                    description,
                    input_schema,
                } = tool;
                let details = Arc::new(McpToolDetails {
                    name: remote_name,
                    description,
                });
                let agent: Arc<dyn AgentTool> =
                    if is_glossmod && details.name == super::glossmod_download::TOOL {
                        Arc::new(super::glossmod_download::GlossModDownload::new(
                            self.clone(),
                            name.clone(),
                        ))
                    } else {
                        Arc::new(McpAgentTool {
                            client: self.clone(),
                            name: name.clone(),
                            details: details.clone(),
                            input_schema,
                        })
                    };
                BridgedTool {
                    name,
                    details,
                    agent,
                }
            })
    }

    pub async fn register_tools(
        &self,
        registry: &ToolRegistry,
        cancel: &CancellationToken,
    ) -> AppResult<Vec<String>> {
        let catalog = self.list_tools(cancel).await?;
        let (names, tools) = self
            .agent_tools_from_catalog(catalog)
            .map(|tool| (tool.name, tool.agent))
            .unzip();
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
            name: self.name.clone(),
            description: format!(
                "[External untrusted MCP: {}] {} — {}",
                self.client.server_id(),
                self.details.name,
                self.details.description
            ),
            parameters: self.input_schema.clone(),
            effect: Effect::ExternalWrite,
        }
    }

    fn host_tool_kind(&self) -> Option<&'static str> {
        (self.client.server_config().is_glossmod_endpoint()
            && self.details.name == "list_work_files")
            .then_some("glossmod_work_files")
    }

    fn stock_source_tool(&self) -> Option<(&str, &str)> {
        // Identity metadata for the existing Agent install scope, not a
        // permission or an additional way to call the tool.
        (self.client.server_config().is_stock_browse_source()
            && self.client.ensure_browse_current().is_ok())
        .then(|| (self.client.server_id(), self.details.name.as_str()))
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
                "server": server, "tool": self.details.name, "arguments":args,
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
            || prepared.preview["tool"] != self.details.name
        {
            return Err(AppError::Denied(
                "MCP execution does not match its external approval preview".into(),
            ));
        }
        self.client
            .call_agent_tool(&self.details.name, args, &context.cancel)
            .await
    }
}
