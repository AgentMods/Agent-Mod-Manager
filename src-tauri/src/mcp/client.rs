use super::config::{McpServers, McpTransport, ServerConfig};
use super::redaction::ConnectionSecrets;
use crate::{
    error::{AppError, AppResult},
    settings::Settings,
};
use rmcp::{
    model::{CallToolRequestParams, PaginatedRequestParams},
    service::RunningService,
    transport::{
        async_rw::AsyncRwTransport, streamable_http_client::StreamableHttpClientTransportConfig,
        StreamableHttpClientTransport,
    },
    RoleClient, ServiceExt,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    future::Future,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

pub const MAX_TOOLS: usize = 128;
pub const MAX_TOOL_PAGES: usize = 8;
pub const MAX_TOOL_LIST_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_SCHEMA_BYTES: usize = 64 * 1024;
pub const MAX_RESULT_BYTES: usize = 1024 * 1024;
pub const MAX_ARGUMENT_BYTES: usize = 256 * 1024;
pub const MAX_WIRE_MESSAGE_BYTES: usize = 2 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A single-use, non-serializable confirmation of this exact configuration.
/// Create only in trusted UI/IPC code after displaying its complete contents.
/// Never expose this constructor as an Agent tool or infer it from enabled=true.
pub struct ConnectionConsent {
    config: ServerConfig,
}

impl ConnectionConsent {
    pub fn after_user_confirmation(
        config: &ServerConfig,
        accepts_unsandboxed_stdio: bool,
    ) -> AppResult<Self> {
        config.validate()?;
        if config.transport == McpTransport::Stdio && !accepts_unsandboxed_stdio {
            return Err(AppError::Denied("stdio MCP runs with your OS user privileges, outside the file whitelist; explicit acknowledgement is required".into()));
        }
        Ok(Self {
            config: config.clone(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    /// Collision-resistant Agent name. Never route by splitting this string.
    pub name: String,
    pub remote_name: String,
    pub description: String,
    pub input_schema: Value,
}

struct Session {
    service: RunningService<RoleClient, ()>,
    child: Mutex<Option<Child>>,
    secrets: ConnectionSecrets,
}

impl Session {
    fn stop(&self) {
        self.service.cancellation_token().cancel();
        if let Some(child) = self
            .child
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            let _ = child.start_kill();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

/// No roots, sampling, elicitation, filesystem or execution capability is
/// advertised by the default rmcp client handler. Server requests cannot access
/// local tools. Results remain untrusted data; they are never executed or fetched.
#[derive(Clone)]
pub struct McpClient {
    servers: McpServers,
    config: ServerConfig,
    session: Arc<Session>,
}

impl McpClient {
    pub async fn connect(
        servers: McpServers,
        settings: &Settings,
        id: &str,
        consent: ConnectionConsent,
        cancel: &CancellationToken,
    ) -> AppResult<Self> {
        let config = servers.get(id)?;
        config.validate()?;
        if !config.enabled {
            return Err(AppError::Denied("MCP server is disabled".into()));
        }
        if consent.config != config {
            return Err(AppError::Conflict(
                "MCP configuration changed; confirm again".into(),
            ));
        }
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let mut resolved_secrets = Vec::new();
        let (service, child, secrets) = match config.transport {
            McpTransport::StreamableHttp => {
                let mut transport_config =
                    StreamableHttpClientTransportConfig::with_uri(config.url.clone().unwrap())
                        .max_concurrent_requests(2)
                        .max_sse_event_size(MAX_WIRE_MESSAGE_BYTES)
                        // Never automatically replay a request that may mutate external state.
                        .reinit_on_expired_session(false)
                        .control_request_timeout(Duration::from_secs(2));
                transport_config.channel_buffer_capacity = 4;
                if let Some(key) = &config.bearer_secret_ref {
                    let token = required_secret(settings, key)?;
                    if token.is_empty() || token.len() > 8192 || token.chars().any(char::is_control)
                    {
                        return Err(AppError::Invalid("Invalid MCP bearer credential".into()));
                    }
                    resolved_secrets.push(token.clone());
                    transport_config = transport_config.auth_header(token);
                }
                let secrets = ConnectionSecrets::new(resolved_secrets)?;
                let http = reqwest_mcp::Client::builder()
                    .redirect(reqwest_mcp::redirect::Policy::none())
                    .connect_timeout(Duration::from_secs(10))
                    .timeout(REQUEST_TIMEOUT)
                    .build()
                    .map_err(|_| AppError::Other("Cannot initialize MCP HTTP client".into()))?;
                let transport = StreamableHttpClientTransport::with_client(http, transport_config);
                let service = bounded(cancel, CONNECT_TIMEOUT, ().serve(transport)).await?;
                (service, None, secrets)
            }
            McpTransport::Stdio => {
                let mut command = Command::new(config.command.as_ref().unwrap());
                command
                    .args(&config.args)
                    .env_clear()
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    // Never forward credential-bearing stderr into application logs.
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                // No ambient API keys, shell startup configuration or loader hooks.
                for key in [
                    "PATH",
                    "HOME",
                    "USERPROFILE",
                    "SYSTEMROOT",
                    "WINDIR",
                    "TMPDIR",
                    "TEMP",
                    "TMP",
                ] {
                    if let Some(value) = std::env::var_os(key) {
                        command.env(key, value);
                    }
                }
                for (env, key) in &config.env_secret_refs {
                    let value = required_secret(settings, key)?;
                    command.env(env, &value);
                    resolved_secrets.push(value);
                }
                let secrets = ConnectionSecrets::new(resolved_secrets)?;
                let mut child = command
                    .spawn()
                    .map_err(|_| AppError::Other("Cannot start confirmed MCP process".into()))?;
                let stdout = child
                    .stdout
                    .take()
                    .ok_or_else(|| AppError::Other("Missing MCP stdout".into()))?;
                let stdin = child
                    .stdin
                    .take()
                    .ok_or_else(|| AppError::Other("Missing MCP stdin".into()))?;
                let transport = AsyncRwTransport::<RoleClient, _, _>::new(stdout, stdin)
                    .with_max_line_length(MAX_WIRE_MESSAGE_BYTES);
                let service = bounded(cancel, CONNECT_TIMEOUT, ().serve(transport)).await?;
                (service, Some(child), secrets)
            }
        };
        Ok(Self {
            servers,
            config,
            session: Arc::new(Session {
                service,
                child: Mutex::new(child),
                secrets,
            }),
        })
    }

    pub fn server_id(&self) -> &str {
        &self.config.id
    }

    pub fn server_config(&self) -> &ServerConfig {
        &self.config
    }

    pub fn disconnect(&self) {
        self.session.stop();
    }

    /// Wait for the direct stdio child after disconnecting. This does not claim
    /// containment or cleanup of arbitrary descendants spawned by untrusted code.
    pub async fn disconnect_and_wait(&self) -> AppResult<()> {
        self.disconnect();
        let child = self
            .session
            .child
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(mut child) = child {
            tokio::time::timeout(Duration::from_secs(3), child.wait())
                .await
                .map_err(|_| AppError::Other("MCP child shutdown timed out".into()))?
                .map_err(|_| AppError::Other("Cannot reap MCP child".into()))?;
        }
        Ok(())
    }

    fn ensure_current(&self) -> AppResult<()> {
        let current = match self.servers.get(&self.config.id) {
            Ok(config) => config,
            Err(error) => {
                self.disconnect();
                return Err(error);
            }
        };
        if current != self.config || !current.enabled {
            self.disconnect();
            return Err(AppError::Denied(
                "MCP configuration changed or was disabled; reconnect with confirmation".into(),
            ));
        }
        if self.session.service.is_closed() {
            return Err(AppError::Conflict(
                "MCP session closed; reconnect with confirmation".into(),
            ));
        }
        Ok(())
    }

    pub async fn list_tools(&self, cancel: &CancellationToken) -> AppResult<Vec<McpTool>> {
        self.ensure_current()?;
        let result = bounded(cancel, REQUEST_TIMEOUT, async {
            let mut tools = Vec::new();
            let mut cursor = None;
            let mut cursors = HashSet::new();
            let mut names = HashSet::new();
            let mut bytes = 0;
            for _ in 0..MAX_TOOL_PAGES {
                let params = cursor.take().map(|cursor| {
                    let mut params = PaginatedRequestParams::default();
                    params.cursor = Some(cursor);
                    params
                });
                let page = self
                    .session
                    .service
                    .peer()
                    .list_tools(params)
                    .await
                    .map_err(|_| AppError::Other("MCP tools/list failed".into()))?;
                bytes += serde_json::to_vec(&page)?.len();
                if bytes > MAX_TOOL_LIST_BYTES || tools.len() + page.tools.len() > MAX_TOOLS {
                    return Err(AppError::Invalid("MCP tool listing exceeds limits".into()));
                }
                for tool in page.tools {
                    let remote_name = tool.name.to_string();
                    if remote_name.is_empty()
                        || remote_name.len() > 256
                        || remote_name.chars().any(char::is_control)
                        || !names.insert(remote_name.clone())
                    {
                        return Err(AppError::Invalid(
                            "Invalid or repeated MCP tool name".into(),
                        ));
                    }
                    if self.session.secrets.contains(&remote_name) {
                        return Err(AppError::Other(
                            "MCP tool name withheld by credential filter".into(),
                        ));
                    }
                    let input_schema = serde_json::to_value(tool.input_schema)?;
                    if serde_json::to_vec(&input_schema)?.len() > MAX_SCHEMA_BYTES {
                        return Err(AppError::Invalid("MCP tool schema exceeds limits".into()));
                    }
                    // Changing schema keys/enums would alter the approved tool
                    // contract. Reject credential-bearing schemas instead.
                    if self.session.secrets.redact_value(input_schema.clone())? != input_schema {
                        return Err(AppError::Other(
                            "MCP tool schema withheld by credential filter".into(),
                        ));
                    }
                    let description = tool.description.unwrap_or_default().to_string();
                    if description.len() > 16 * 1024 {
                        return Err(AppError::Invalid("MCP description exceeds limits".into()));
                    }
                    let description = self
                        .session
                        .secrets
                        .redact_value(Value::String(description))?;
                    let description = description.as_str().unwrap().to_owned();
                    if description.len() > 16 * 1024 {
                        return Err(AppError::Invalid("MCP description exceeds limits".into()));
                    }
                    // Remote annotations cannot grant local read-only trust.
                    tools.push(McpTool {
                        name: namespaced_tool_name(&self.config.id, &remote_name),
                        remote_name,
                        description,
                        input_schema,
                    });
                }
                match page.next_cursor {
                    None => return Ok(tools),
                    Some(next) if next.len() <= 4096 && cursors.insert(next.clone()) => {
                        cursor = Some(next)
                    }
                    _ => return Err(AppError::Invalid("Invalid MCP pagination cursor".into())),
                }
            }
            Err(AppError::Invalid(
                "MCP tool pagination exceeds limits".into(),
            ))
        })
        .await;
        if result.is_err() {
            self.disconnect();
        }
        result
    }

    /// Internal bridge entry point. Approval belongs to AgentRuntime; there is
    /// deliberately no public unauthenticated call_tool endpoint for IPC/Agent.
    pub(super) async fn call_tool(
        &self,
        name: &str,
        args: Value,
        cancel: &CancellationToken,
    ) -> AppResult<Value> {
        self.ensure_current()?;
        let arguments = args
            .as_object()
            .cloned()
            .ok_or_else(|| AppError::Invalid("MCP arguments must be an object".into()))?;
        if serde_json::to_vec(&args)?.len() > MAX_ARGUMENT_BYTES {
            return Err(AppError::Invalid("MCP arguments exceed limits".into()));
        }
        let result = bounded(
            cancel,
            REQUEST_TIMEOUT,
            self.session
                .service
                .peer()
                .call_tool(CallToolRequestParams::new(name.to_owned()).with_arguments(arguments)),
        )
        .await;
        match result {
            Ok(result) => {
                let filtered = (|| {
                    let value = serde_json::to_value(result)
                        .map_err(|_| AppError::Other("Cannot serialize MCP result".into()))?;
                    if serde_json::to_vec(&value)?.len() > MAX_RESULT_BYTES {
                        return Err(AppError::Invalid("MCP result exceeds limits".into()));
                    }
                    let value = self.session.secrets.redact_value(value)?;
                    if serde_json::to_vec(&value)?.len() > MAX_RESULT_BYTES {
                        return Err(AppError::Invalid("MCP result exceeds limits".into()));
                    }
                    Ok(value)
                })();
                if filtered.is_err() {
                    self.disconnect();
                }
                // Do not read resources, follow links, execute commands, or honor
                // embedded tool calls/instructions from this untrusted result.
                filtered
            }
            Err(err) => {
                self.disconnect();
                Err(err)
            }
        }
    }
}

fn required_secret(settings: &Settings, key: &str) -> AppResult<String> {
    let value = settings
        .read_secret(key)
        .map_err(|_| AppError::Other("Cannot read encrypted MCP credential".into()))?
        .ok_or_else(|| {
            AppError::Invalid("MCP credential is not configured in encrypted storage".into())
        })?;
    if value.is_empty() || value.len() > 8192 || value.contains('\0') {
        return Err(AppError::Invalid("Invalid MCP credential".into()));
    }
    Ok(value)
}

async fn bounded<T, E>(
    cancel: &CancellationToken,
    timeout: Duration,
    operation: impl Future<Output = Result<T, E>>,
) -> AppResult<T> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(AppError::Cancelled),
        result = tokio::time::timeout(timeout, operation) => match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => Err(AppError::Other("MCP request failed (remote details suppressed)".into())),
            Err(_) => Err(AppError::Other("MCP request timed out; external effects may already have occurred".into())),
        }
    }
}

pub fn namespaced_tool_name(server: &str, tool: &str) -> String {
    fn readable(s: &str) -> String {
        s.bytes()
            .take(10)
            .map(|b| {
                if b.is_ascii_alphanumeric() {
                    b as char
                } else {
                    '_'
                }
            })
            .collect()
    }
    // Hash includes exact original names, not their normalized display form.
    // Registry.register still rejects any actual collision rather than replacing.
    let digest = Sha256::digest(format!("{server}\0{tool}").as_bytes());
    format!(
        "mcp__{}__{}_{}",
        readable(server),
        readable(tool),
        hex::encode(&digest[..16])
    )
}

#[cfg(test)]
mod timeout_tests {
    use super::*;

    #[tokio::test]
    async fn timeout_and_preexisting_cancellation_are_bounded_without_remote_error_leaks() {
        let cancel = CancellationToken::new();
        let pending = std::future::pending::<Result<(), AppError>>();
        let error = bounded(&cancel, Duration::from_millis(10), pending)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(error.to_string().contains("may already have occurred"));
        cancel.cancel();
        let operation = async {
            panic!("cancelled operation must not be polled");
            #[allow(unreachable_code)]
            Ok::<(), AppError>(())
        };
        assert!(matches!(
            bounded(&cancel, Duration::from_secs(1), operation).await,
            Err(AppError::Cancelled)
        ));
        let error = bounded(&CancellationToken::new(), Duration::from_secs(1), async {
            Err::<(), _>(AppError::Other("remote-secret-should-not-leak".into()))
        })
        .await
        .unwrap_err();
        assert!(!error.to_string().contains("remote-secret"));
    }
}
