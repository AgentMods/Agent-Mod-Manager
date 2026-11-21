//! Desktop-owned connections. Agents cannot approve a connection or launch a
//! server process. Disconnect revokes its tool namespace before closing IO.
use crate::{
    agent::tools::ToolRegistry,
    error::{AppError, AppResult},
    mcp::{ConnectionConsent, McpClient, McpRuntime, McpServers, ServerConfig},
    settings::Settings,
};
use serde::Serialize;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Fixed ID of the automatically managed entry for this app's own local MCP server.
pub const LOCAL_SERVER_ID: &str = "amm_local";

pub(crate) fn require_external(id: &str) -> AppResult<()> {
    if id == LOCAL_SERVER_ID {
        return Err(AppError::Denied(
            "请使用本地 MCP 服务的启动和停止控件".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Default)]
pub struct McpConnections {
    slots: Arc<Mutex<BTreeMap<String, Slot>>>,
    runtime: McpRuntime,
}
enum Slot {
    Starting(CancellationToken),
    Running {
        client: Box<McpClient>,
        names: Vec<String>,
    },
}
enum Start {
    Confirmed(bool),
    Remembered,
    ManagedLocal,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub server_id: String,
    pub tools: Vec<String>,
}
impl McpConnections {
    pub fn with_runtime(runtime: McpRuntime) -> Self {
        Self {
            runtime,
            ..Self::default()
        }
    }

    pub async fn list(&self) -> Vec<ConnectionInfo> {
        self.slots
            .lock()
            .await
            .iter()
            .filter_map(|(server, slot)| match slot {
                Slot::Starting(_) => None,
                Slot::Running { names, .. } => Some(ConnectionInfo {
                    server_id: server.clone(),
                    tools: names.clone(),
                }),
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
        require_external(&expected.id)?;
        self.start(
            settings,
            registry,
            expected,
            Start::Confirmed(accepts_unsandboxed_stdio),
        )
        .await
    }

    pub(crate) async fn restore_requested(
        &self,
        settings: &Settings,
        registry: &ToolRegistry,
        progress: impl Fn(),
    ) {
        use futures_util::{stream, StreamExt};
        let ids = match McpServers::new(settings.db.clone()).startup_requests() {
            Ok(ids) => ids,
            Err(error) => {
                tracing::warn!(%error, "could not read MCP startup preferences");
                return;
            }
        };
        // A slow handshake must not hold up unrelated servers or the desktop window.
        let progress = &progress;
        stream::iter(ids)
            .for_each_concurrent(4, |id| async move {
                if let Err(error) = self.restore(settings.clone(), registry, &id).await {
                    tracing::warn!(server_id = id, %error, "could not restore MCP connection");
                }
                progress();
            })
            .await;
    }

    pub(crate) async fn restore(
        &self,
        settings: Settings,
        registry: &ToolRegistry,
        id: &str,
    ) -> AppResult<ConnectionInfo> {
        require_external(id)?;
        let config = McpServers::new(settings.db.clone()).get(id)?;
        self.start(settings, registry, config, Start::Remembered)
            .await
    }

    async fn start(
        &self,
        settings: Settings,
        registry: &ToolRegistry,
        expected: ServerConfig,
        start: Start,
    ) -> AppResult<ConnectionInfo> {
        let servers = McpServers::new(settings.db.clone());
        let cancel = CancellationToken::new();
        let consent = {
            let mut slots = self.slots.lock().await;
            if slots.contains_key(&expected.id) {
                return Err(AppError::Conflict("MCP 正在启动或已连接".into()));
            }
            if slots.len() >= 8 {
                return Err(AppError::Invalid("最多同时连接8个MCP服务器".into()));
            }
            if servers.get(&expected.id)? != expected {
                return Err(AppError::Conflict(
                    "配置已变化，请重新确认完整连接配置".into(),
                ));
            }
            let consent = match start {
                Start::Confirmed(accepted) => {
                    ConnectionConsent::after_user_confirmation(&expected, accepted)?
                }
                Start::Remembered => ConnectionConsent::from_remembered(&servers, &expected)?,
                Start::ManagedLocal => {
                    ConnectionConsent::after_user_confirmation(&expected, false)?
                }
            };
            slots.insert(expected.id.clone(), Slot::Starting(cancel.clone()));
            consent
        };
        // Do not hold the lifecycle lock while a remote handshake or stdio download runs.
        let prepared = async {
            let client = McpClient::connect(
                servers.clone(),
                &settings,
                &expected.id,
                consent,
                &self.runtime,
                &cancel,
            )
            .await?;
            let tools = match client.agent_tools(&cancel).await {
                Ok(tools) => tools,
                Err(err) => {
                    client.disconnect_and_wait().await?;
                    return Err(err);
                }
            };
            Ok((client, tools))
        }
        .await;
        let mut slots = self.slots.lock().await;
        if cancel.is_cancelled() {
            // Stop/edit removed our reservation. A newer start may already own the ID.
            drop(slots);
            if let Ok((client, _)) = prepared {
                client.disconnect_and_wait().await?;
            }
            return Err(AppError::Cancelled);
        }
        slots.remove(&expected.id);
        let (client, tools) = prepared?;
        let names: Vec<_> = tools.iter().map(|t| t.definition().name).collect();
        let result = registry.register_batch(tools).and_then(|()| {
            if matches!(start, Start::Confirmed(_)) {
                if let Err(err) = servers.remember_start(&expected) {
                    registry.unregister(&names);
                    return Err(err);
                }
            }
            Ok(())
        });
        if let Err(err) = result {
            drop(slots);
            client.disconnect_and_wait().await?;
            return Err(err);
        }
        let info = ConnectionInfo {
            server_id: expected.id.clone(),
            tools: names.clone(),
        };
        slots.insert(
            expected.id,
            Slot::Running {
                client: Box::new(client),
                names,
            },
        );
        Ok(info)
    }

    /// Commit stop/edit/remove and revoke the slot together so a queued restore cannot undo it.
    pub(crate) async fn revoke(
        &self,
        registry: &ToolRegistry,
        id: &str,
        change: impl FnOnce() -> AppResult<()>,
    ) -> AppResult<()> {
        let mut slots = self.slots.lock().await;
        // Never leave live tools running just because persistence is unavailable.
        let changed = change();
        match slots.remove(id) {
            Some(Slot::Starting(cancel)) => cancel.cancel(),
            Some(Slot::Running { client, names }) => {
                registry.unregister(&names);
                client.disconnect();
                drop(slots);
                return changed.and(client.disconnect_and_wait().await);
            }
            None => (),
        }
        changed
    }

    pub(crate) async fn disconnect(
        &self,
        registry: &ToolRegistry,
        servers: &McpServers,
        id: &str,
    ) -> AppResult<()> {
        require_external(id)?;
        self.revoke(registry, id, || servers.forget_start(id)).await
    }

    /// Register the freshly started local server. Its persisted intent belongs to
    /// LocalMcpServer, not this transient endpoint and encrypted token reference.
    pub async fn connect_local(
        &self,
        settings: Settings,
        registry: &ToolRegistry,
        endpoint: &str,
    ) -> AppResult<ConnectionInfo> {
        self.disconnect_local(registry, &settings).await?;
        let mut config =
            ServerConfig::http(LOCAL_SERVER_ID, "本地 MCP（Agent Mod Manager）", endpoint);
        config.bearer_secret_ref = Some(crate::mcp_server::TOKEN_REF.into());
        config.enabled = true;
        let servers = McpServers::new(settings.db.clone());
        servers.create(&config)?;
        match self
            .start(settings, registry, config, Start::ManagedLocal)
            .await
        {
            Ok(info) => Ok(info),
            Err(err) => {
                let _ = servers.remove(LOCAL_SERVER_ID);
                Err(err)
            }
        }
    }

    pub async fn disconnect_local(
        &self,
        registry: &ToolRegistry,
        settings: &Settings,
    ) -> AppResult<()> {
        self.revoke(registry, LOCAL_SERVER_ID, || {
            match McpServers::new(settings.db.clone()).remove(LOCAL_SERVER_ID) {
                Ok(()) | Err(AppError::NotFound(_)) => Ok(()),
                Err(err) => Err(err),
            }
        })
        .await
    }
}
