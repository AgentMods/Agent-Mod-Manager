//! Desktop-owned connections. Agents cannot approve a connection or launch a
//! server process. Disconnect revokes its tool namespace before closing IO.
use crate::{
    agent::tools::ToolRegistry,
    error::{AppError, AppResult},
    mcp::{ConnectionConsent, McpClient, McpServers, ServerConfig},
    settings::Settings,
};
use serde::Serialize;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
pub struct McpConnections(Arc<Mutex<BTreeMap<String, Active>>>);
struct Active {
    client: McpClient,
    names: Vec<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub server_id: String,
    pub tools: Vec<String>,
}
impl McpConnections {
    pub async fn list(&self) -> Vec<ConnectionInfo> {
        self.0
            .lock()
            .await
            .iter()
            .map(|(server, active)| ConnectionInfo {
                server_id: server.clone(),
                tools: active.names.clone(),
            })
            .collect()
    }
    pub async fn connect(
        &self,
        settings: Settings,
        registry: &ToolRegistry,
        expected: ServerConfig,
        accepts_unsandboxed_stdio: bool,
    ) -> AppResult<ConnectionInfo> {
        let mut active = self.0.lock().await;
        if active.contains_key(&expected.id) {
            return Err(AppError::Conflict("MCP已连接，请先断开".into()));
        }
        if active.len() >= 8 {
            return Err(AppError::Invalid("最多同时连接8个MCP服务器".into()));
        }
        let servers = McpServers::new(settings.db.clone());
        if servers.get(&expected.id)? != expected {
            return Err(AppError::Conflict(
                "配置已变化，请重新确认完整连接配置".into(),
            ));
        }
        let consent =
            ConnectionConsent::after_user_confirmation(&expected, accepts_unsandboxed_stdio)?;
        let cancel = CancellationToken::new();
        let client = McpClient::connect(servers, &settings, &expected.id, consent, &cancel).await?;
        let tools = match client.agent_tools(&cancel).await {
            Ok(tools) => tools,
            Err(err) => {
                client.disconnect_and_wait().await?;
                return Err(err);
            }
        };
        let names: Vec<_> = tools.iter().map(|t| t.definition().name).collect();
        if let Err(err) = registry.register_batch(tools) {
            client.disconnect_and_wait().await?;
            return Err(err);
        }
        let info = ConnectionInfo {
            server_id: expected.id.clone(),
            tools: names.clone(),
        };
        active.insert(expected.id, Active { client, names });
        Ok(info)
    }
    pub async fn disconnect(&self, registry: &ToolRegistry, id: &str) -> AppResult<()> {
        if let Some(active) = self.0.lock().await.remove(id) {
            registry.unregister(&active.names);
            active.client.disconnect_and_wait().await?;
        }
        Ok(())
    }
}
