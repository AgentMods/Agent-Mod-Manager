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
    Starting {
        cancel: CancellationToken,
        finished: CancellationToken,
    },
    Running {
        client: Box<McpClient>,
        names: Vec<String>,
    },
}
enum Start {
    Confirmed(bool),
    Remembered,
    Attachment,
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
                Slot::Starting { .. } => None,
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
        .map(|(info, _)| info)
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
            .map(|(info, _)| info)
    }

    /// Resolve only the user-authorized, built-in local converter for desktop uploads.
    pub(crate) async fn markitdown_client(
        &self,
        settings: &Settings,
        registry: &ToolRegistry,
    ) -> AppResult<McpClient> {
        use crate::mcp::{
            client::{namespaced_tool_name, MARKDOWN_TOOL},
            config::MARKITDOWN_ID,
            McpTransport,
        };
        let servers = McpServers::new(settings.db.clone());
        let config = servers.get(MARKITDOWN_ID).map_err(|err| match err {
            AppError::NotFound(_) => AppError::NotFound(
                "缺少内置 MarkItDown MCP，请重启应用恢复预设后，在「设置 → MCP 服务」中启动它。"
                    .into(),
            ),
            other => other,
        })?;
        if !config.builtin || config.transport != McpTransport::Stdio {
            return Err(AppError::Invalid("聊天附件需要内置的 stdio MarkItDown MCP。请在「设置 → MCP 服务」中恢复该预设并启动。".into()));
        }
        let (info, client) = self
            .start(settings.clone(), registry, config, Start::Attachment)
            .await
            .map_err(|err| match err {
                AppError::Denied(_) => AppError::Denied("此 MarkItDown MCP 配置需要启动授权。请确认 stdio 进程使用当前用户的系统权限，或前往「设置 → MCP 服务」启动它。".into()),
                other => AppError::Other(format!("无法使用 MarkItDown MCP。请在「设置 → MCP 服务」中检查连接后重试。{other}")),
            })?;
        if !info
            .tools
            .contains(&namespaced_tool_name(MARKITDOWN_ID, MARKDOWN_TOOL))
        {
            return Err(AppError::Unsupported("MarkItDown MCP 未提供 convert_to_markdown。请在「设置 → MCP 服务」中检查内置预设并重新启动。".into()));
        }
        Ok(client)
    }

    async fn start(
        &self,
        settings: Settings,
        registry: &ToolRegistry,
        expected: ServerConfig,
        start: Start,
    ) -> AppResult<(ConnectionInfo, McpClient)> {
        let servers = McpServers::new(settings.db.clone());
        let cancel = CancellationToken::new();
        let finished = CancellationToken::new();
        let _notify_waiters = finished.clone().drop_guard();
        let consent = {
            let mut slots = self.slots.lock().await;
            if let Some(slot) = slots.get(&expected.id) {
                if !matches!(start, Start::Attachment) {
                    return Err(AppError::Conflict("MCP 正在启动或已连接".into()));
                }
                // Concurrent uploads and desktop restore share one handshake. A waiter never
                // retries a failed or revoked launch and cannot hold up an explicit stop.
                if let Slot::Starting { finished, cancel } = slot {
                    let (finished, cancel) = (finished.clone(), cancel.clone());
                    drop(slots);
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return Err(AppError::Cancelled),
                        _ = finished.cancelled() => (),
                    }
                    slots = self.slots.lock().await;
                }
                if let Some(Slot::Running { client, names }) = slots.get(&expected.id) {
                    if client.server_config() == &expected {
                        return Ok((
                            ConnectionInfo {
                                server_id: expected.id,
                                tools: names.clone(),
                            },
                            client.as_ref().clone(),
                        ));
                    }
                }
                return Err(AppError::Conflict(
                    "连接已停止、启动失败或配置已更改。请在「设置 → MCP 服务」中检查 MarkItDown 状态后重试。".into(),
                ));
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
                Start::Remembered | Start::Attachment => {
                    ConnectionConsent::from_remembered(&servers, &expected)?
                }
                Start::ManagedLocal => {
                    ConnectionConsent::after_user_confirmation(&expected, false)?
                }
            };
            slots.insert(
                expected.id.clone(),
                Slot::Starting {
                    cancel: cancel.clone(),
                    finished,
                },
            );
            consent
        };
        let remember = matches!(start, Start::Confirmed(_));
        self.finish_start(settings, registry, expected, remember, cancel, consent)
            .await
            .map_err(|err| match err {
                // Authorization succeeded. Filesystem/transport denials must never reopen the consent dialog.
                AppError::Denied(reason) if matches!(start, Start::Attachment) => {
                    AppError::Other(reason)
                }
                other => other,
            })
    }

    // Never hold the lifecycle lock through a remote handshake or a stdio download.
    async fn finish_start(
        &self,
        settings: Settings,
        registry: &ToolRegistry,
        expected: ServerConfig,
        remember: bool,
        cancel: CancellationToken,
        consent: ConnectionConsent,
    ) -> AppResult<(ConnectionInfo, McpClient)> {
        let servers = McpServers::new(settings.db.clone());
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
            if remember {
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
                client: Box::new(client.clone()),
                names,
            },
        );
        Ok((info, client))
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
            Some(Slot::Starting { cancel, .. }) => cancel.cancel(),
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
            Ok((info, _)) => Ok(info),
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
