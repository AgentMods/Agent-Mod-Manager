//! Desktop-owned connections. Agents cannot approve a connection or launch a
//! server process. Disconnect revokes its tool namespace before closing IO.
use crate::{
    agent::tools::ToolRegistry,
    error::{AppError, AppResult},
    mcp::{
        browse::BrowseReader, ConnectionConsent, McpClient, McpRuntime, McpServers, ServerConfig,
    },
    settings::Settings,
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard},
};
use tokio_util::sync::CancellationToken;

/// Fixed ID of the automatically managed entry for this app's own local MCP server.
pub const LOCAL_SERVER_ID: &str = "amm_local";

pub(crate) fn require_external(id: &str) -> AppResult<()> {
    if id == LOCAL_SERVER_ID {
        return Err(AppError::Denied("请使用本地工具的启动和停止控件".into()));
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
        config: Box<ServerConfig>,
        requires_saved_consent: bool,
        cancel: Arc<CancellationToken>,
        finished: CancellationToken,
    },
    Running {
        client: Box<McpClient>,
        names: Vec<String>,
        browse: Option<Box<BrowseReader>>,
    },
}
impl Slot {
    fn config(&self) -> &ServerConfig {
        match self {
            Self::Starting { config, .. } => config,
            Self::Running { client, .. } => client.server_config(),
        }
    }

    fn revoke(self, registry: &ToolRegistry) -> Option<Box<McpClient>> {
        match self {
            Self::Starting { cancel, .. } => cancel.cancel(),
            Self::Running { client, names, .. } => {
                registry.unregister(&names);
                client.disconnect();
                return Some(client);
            }
        }
        None
    }
}

#[derive(Clone, Copy)]
enum Start<'a> {
    Confirmed(bool),
    Remembered,
    Attachment(&'a CancellationToken),
    ManagedLocal,
}
impl Start<'_> {
    fn requires_saved_consent(self) -> bool {
        matches!(self, Self::Remembered | Self::Attachment(_))
    }
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

    // No awaits while held: committed plugin changes also revoke slots synchronously.
    fn lock_slots(&self) -> MutexGuard<'_, BTreeMap<String, Slot>> {
        self.slots.lock().unwrap_or_else(|err| err.into_inner())
    }

    pub async fn list(&self) -> Vec<ConnectionInfo> {
        self.lock_slots()
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

    /// Desktop browse can borrow only the restricted reader of an existing
    /// slot. It cannot start a process, rediscover, or obtain the raw client.
    pub(crate) fn browse_reader(&self, servers: &McpServers, id: &str) -> AppResult<BrowseReader> {
        let slots = self.lock_slots();
        let disconnected = || AppError::NotFound("尚未连接 MCP 来源".into());
        let current = servers.get(id)?;
        if !current.is_stock_browse_source() {
            return Err(AppError::Unsupported("游览不使用非原厂 MCP 配置".into()));
        }
        let Some(Slot::Running { client, browse, .. }) = slots.get(id) else {
            return Err(disconnected());
        };
        if !current.enabled
            || !client.uses_browse_database(servers)
            || client.is_closed()
            || !current.same_connection_config(client.server_config())
        {
            return Err(disconnected());
        }
        client.ensure_browse_current()?;
        browse
            .as_deref()
            .cloned()
            .ok_or_else(|| AppError::Unsupported("MCP 未提供匹配的公开目录工具契约".into()))
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
        cancel: &CancellationToken,
    ) -> AppResult<McpClient> {
        use crate::mcp::{
            client::{namespaced_tool_name, MARKDOWN_TOOL},
            config::MARKITDOWN_ID,
            McpTransport,
        };
        let servers = McpServers::new(settings.db.clone());
        let config = servers.get(MARKITDOWN_ID).map_err(|err| match err {
            AppError::NotFound(_) => AppError::NotFound(
                "缺少内置 MarkItDown MCP，请重启应用恢复预设后，在「设置 → 工具」中启动它。".into(),
            ),
            other => other,
        })?;
        if !config.builtin || config.transport != McpTransport::Stdio {
            return Err(AppError::Invalid(
                "聊天附件需要内置的 stdio MarkItDown MCP。请在「设置 → 工具」中恢复该预设并启动。"
                    .into(),
            ));
        }
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        // A cancelled in-flight conversion stops the direct stdio child. Clear
        // only that stopped slot; keep the exact saved launch authorization.
        let stopped = {
            let mut slots = self.lock_slots();
            if matches!(slots.get(MARKITDOWN_ID), Some(Slot::Running { client, .. }) if client.is_closed())
            {
                slots
                    .remove(MARKITDOWN_ID)
                    .and_then(|slot| slot.revoke(registry))
            } else {
                None
            }
        };
        if let Some(stopped) = stopped {
            stopped.disconnect_and_wait().await?;
        }
        let (info, client) = self
            .start(settings.clone(), registry, config, Start::Attachment(cancel))
            .await
            .map_err(|err| match err {
                AppError::Denied(_) => AppError::Denied("此 MarkItDown MCP 配置需要启动授权。请确认 stdio 进程使用当前用户的系统权限，或前往「设置 → 工具」启动它。".into()),
                AppError::Cancelled => AppError::Cancelled,
                other => AppError::Other(format!("无法使用 MarkItDown MCP。请在「设置 → 工具」中检查连接后重试。{other}")),
            })?;
        if !info
            .tools
            .contains(&namespaced_tool_name(MARKITDOWN_ID, MARKDOWN_TOOL))
        {
            return Err(AppError::Unsupported("MarkItDown MCP 未提供 convert_to_markdown。请在「设置 → 工具」中检查内置预设并重新启动。".into()));
        }
        Ok(client)
    }

    async fn start(
        &self,
        settings: Settings,
        registry: &ToolRegistry,
        expected: ServerConfig,
        start: Start<'_>,
    ) -> AppResult<(ConnectionInfo, McpClient)> {
        let servers = McpServers::new(settings.db.clone());
        let cancel = Arc::new(match start {
            Start::Attachment(caller) => caller.child_token(),
            _ => CancellationToken::new(),
        });
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let finished = CancellationToken::new();
        let _notify_waiters = finished.clone().drop_guard();
        let reservation = {
            // Match-and-enable and slot reservations must not cross a plugin commit.
            // Lock order: file write lock -> slots -> database (never across network IO).
            let _plugin_write = expected
                .plugin_slug
                .as_ref()
                .map(|_| crate::transaction::write_lock());
            let mut slots = self.lock_slots();
            match slots.get(&expected.id) {
                Some(_) if !matches!(start, Start::Attachment(_)) => {
                    return Err(AppError::Conflict("MCP 正在启动或已连接".into()));
                }
                Some(Slot::Starting {
                    finished, cancel, ..
                }) => Err((finished.clone(), cancel.clone())),
                Some(slot) => return Self::attached(Some(slot), &expected),
                None => {
                    let mut current = servers.get(&expected.id)?;
                    let enable_owned = matches!(start, Start::Confirmed(_))
                        && current.plugin_slug.is_some()
                        && !current.enabled
                        && expected.enabled;
                    if enable_owned {
                        current.enabled = true;
                    }
                    if !current.same_connection_config(&expected) {
                        return Err(AppError::Conflict(
                            "配置已变化，请重新确认完整连接配置".into(),
                        ));
                    }
                    let consent = match start {
                        Start::Confirmed(accepted) => {
                            ConnectionConsent::after_user_confirmation(&expected, accepted)?
                        }
                        Start::Remembered | Start::Attachment(_) => {
                            ConnectionConsent::from_remembered(&servers, &expected)?
                        }
                        Start::ManagedLocal => {
                            ConnectionConsent::after_user_confirmation(&expected, false)?
                        }
                    };
                    // Only the enable flag may differ, and a declined stdio warning cannot enable it.
                    if enable_owned {
                        servers.set_enabled(&expected.id, true)?;
                    }
                    slots.insert(
                        expected.id.clone(),
                        Slot::Starting {
                            config: Box::new(expected.clone()),
                            requires_saved_consent: start.requires_saved_consent(),
                            cancel: cancel.clone(),
                            finished,
                        },
                    );
                    Ok(consent)
                }
            }
        };
        let consent = match reservation {
            Ok(consent) => consent,
            Err((finished, shared_cancel)) => {
                // Concurrent uploads and desktop restore share one handshake. A waiter never
                // retries a failed or revoked launch and cannot hold up an explicit stop.
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(AppError::Cancelled),
                    _ = shared_cancel.cancelled() => return Err(AppError::Cancelled),
                    _ = finished.cancelled() => (),
                }
                return Self::attached(self.lock_slots().get(&expected.id), &expected);
            }
        };
        self.finish_start(settings, registry, expected, start, cancel, consent)
            .await
            .map_err(|err| match err {
                // Authorization succeeded. Filesystem/transport denials must never reopen the consent dialog.
                AppError::Denied(reason) if matches!(start, Start::Attachment(_)) => {
                    AppError::Other(reason)
                }
                other => other,
            })
    }

    fn attached(
        slot: Option<&Slot>,
        expected: &ServerConfig,
    ) -> AppResult<(ConnectionInfo, McpClient)> {
        if let Some(Slot::Running { client, names, .. }) = slot {
            if client.server_config().same_connection_config(expected) {
                return Ok((
                    ConnectionInfo {
                        server_id: expected.id.clone(),
                        tools: names.clone(),
                    },
                    client.as_ref().clone(),
                ));
            }
        }
        Err(AppError::Conflict(
            "连接已停止、启动失败或配置已更改。请在「设置 → 工具」中检查 MarkItDown 状态后重试。"
                .into(),
        ))
    }

    // Never hold the lifecycle lock through a remote handshake or a stdio download.
    async fn finish_start(
        &self,
        settings: Settings,
        registry: &ToolRegistry,
        expected: ServerConfig,
        start: Start<'_>,
        cancel: Arc<CancellationToken>,
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
            let catalog = match client.list_tools(&cancel).await {
                Ok(tools) => tools,
                Err(err) => {
                    client.disconnect_and_wait().await?;
                    return Err(err);
                }
            };
            let browse = BrowseReader::from_discovery(&client, &catalog).map(Box::new);
            let tools = client.agent_tools_from_catalog(catalog);
            Ok((client, tools, browse))
        }
        .await;
        let mut client_to_close = None;
        let finished = (|| {
            let _plugin_write = expected
                .plugin_slug
                .as_ref()
                .map(|_| crate::transaction::write_lock());
            let mut slots = self.lock_slots();
            if cancel.is_cancelled() {
                // An attachment may cancel its own launch. Stop/edit may have
                // removed the slot and allowed a newer start to claim the ID.
                if matches!(slots.get(&expected.id), Some(Slot::Starting { cancel: owner, .. }) if Arc::ptr_eq(owner, &cancel))
                {
                    slots.remove(&expected.id);
                }
                client_to_close = prepared.ok().map(|(client, _, _)| client);
                return Err(AppError::Cancelled);
            }
            slots.remove(&expected.id);
            let (client, tools, browse) = prepared?;
            let names: Vec<_> = tools.iter().map(|t| t.definition().name).collect();
            let result = (|| {
                client.ensure_current()?;
                if start.requires_saved_consent() {
                    ConnectionConsent::from_remembered(&servers, &expected)?;
                }
                registry.register_batch(tools)?;
                if matches!(start, Start::Confirmed(_)) {
                    if let Err(err) = servers.remember_start(&expected) {
                        registry.unregister(&names);
                        return Err(err);
                    }
                }
                Ok(())
            })();
            if let Err(err) = result {
                client_to_close = Some(client);
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
                    browse,
                },
            );
            Ok((info, client))
        })();
        if let Some(client) = client_to_close {
            client.disconnect_and_wait().await?;
        }
        finished
    }

    /// Revoke stale slots for this plugin after a committed change. Other plugins,
    /// manual services, and matching launch configurations/consents are untouched.
    pub(crate) fn invalidate_plugin(
        &self,
        registry: &ToolRegistry,
        servers: &McpServers,
        slug: &str,
    ) {
        let mut slots = self.lock_slots();
        let stale: Vec<_> = slots
            .iter()
            .filter_map(|(id, slot)| {
                let expected = slot.config();
                if expected.plugin_slug.as_deref() != Some(slug) {
                    return None;
                }
                let requires_saved_consent = match slot {
                    Slot::Starting {
                        requires_saved_consent,
                        ..
                    } => *requires_saved_consent,
                    Slot::Running { .. } => true,
                };
                let keep = servers.get(id).is_ok_and(|current| {
                    current.enabled && current.same_connection_config(expected)
                }) && (!requires_saved_consent
                    || ConnectionConsent::from_remembered(servers, expected).is_ok());
                (!keep).then(|| id.clone())
            })
            .collect();
        for id in stale {
            if let Some(slot) = slots.remove(&id) {
                // No blocking wait in the plugin commit path: disconnect requests a
                // kill immediately, and the child has kill_on_drop as a fallback.
                drop(slot.revoke(registry));
            }
        }
    }

    /// Commit stop/edit/remove and revoke the slot together so a queued restore cannot undo it.
    pub(crate) async fn revoke(
        &self,
        registry: &ToolRegistry,
        id: &str,
        change: impl FnOnce() -> AppResult<()>,
    ) -> AppResult<()> {
        self.revoke_if_needed(registry, id, || change().map(|()| true))
            .await
    }

    /// A successful metadata-only edit does not revoke an existing slot.
    pub(crate) async fn revoke_if_needed(
        &self,
        registry: &ToolRegistry,
        id: &str,
        change: impl FnOnce() -> AppResult<bool>,
    ) -> AppResult<()> {
        let (changed, client) = {
            let mut slots = self.lock_slots();
            let changed = match change() {
                Ok(false) => return Ok(()),
                result => result.map(|_| ()),
            };
            // A denied manual update/removal of a plugin-owned record is not a stop.
            if matches!(changed, Err(AppError::Denied(_)))
                && slots
                    .get(id)
                    .is_some_and(|slot| slot.config().plugin_slug.is_some())
            {
                return changed;
            }
            // Never leave live tools running just because persistence is unavailable.
            let client = slots.remove(id).and_then(|slot| slot.revoke(registry));
            (changed, client)
        };
        if let Some(client) = client {
            return changed.and(client.disconnect_and_wait().await);
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
