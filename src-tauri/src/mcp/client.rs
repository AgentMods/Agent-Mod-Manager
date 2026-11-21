use super::config::{McpServers, McpTransport, ServerConfig};
use super::redaction::ConnectionSecrets;
use super::McpRuntime;
use crate::{
    error::{AppError, AppResult},
    settings::Settings,
};
use rmcp::{
    model::{CallToolRequestParams, PaginatedRequestParams},
    service::{ClientInitializeError, RunningService},
    transport::{
        async_rw::AsyncRwTransport,
        streamable_http_client::{StreamableHttpClientTransportConfig, StreamableHttpError},
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
use tokio::process::Child;
use tokio_util::sync::CancellationToken;

pub const MAX_TOOLS: usize = 128;
pub const MAX_TOOL_PAGES: usize = 8;
pub const MAX_TOOL_LIST_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_SCHEMA_BYTES: usize = 64 * 1024;
pub const MAX_RESULT_BYTES: usize = 1024 * 1024;
pub const MAX_ARGUMENT_BYTES: usize = 256 * 1024;
pub const MAX_WIRE_MESSAGE_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MARKDOWN_TOOL: &str = "convert_to_markdown";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const TIMEOUT_MESSAGE: &str = "MCP request timed out; external effects may already have occurred";
const RESULT_LIMIT_MESSAGE: &str = "MCP result exceeds limits";

/// For stdio, the initialize deadline also covers anything the launcher must
/// install before an MCP server exists. This never extends tool call timeouts.
#[derive(Clone, Copy)]
struct StartupBudget {
    timeout: Duration,
    hint: &'static str,
    launcher_env: &'static [(&'static str, &'static str)],
}

impl StartupBudget {
    const HTTP: Self = Self {
        timeout: Duration::from_secs(20),
        hint: "请检查服务地址和网络连接。",
        launcher_env: &[],
    };

    fn stdio(command: &str) -> Self {
        let executable = std::path::Path::new(command)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if ["uvx", "uvx.exe"]
            .iter()
            .any(|s| executable.eq_ignore_ascii_case(s))
        {
            return Self {
                timeout: Duration::from_secs(600),
                // Fixed non-secret tuning, not inherited UV_* (indexes may contain credentials).
                launcher_env: &[("UV_HTTP_TIMEOUT", "120"), ("UV_CONCURRENT_DOWNLOADS", "4")],
                hint: "uvx 首次启动可能需要下载 Python 和依赖包，请检查网络和 uv 缓存。",
            };
        }
        let is_npx = ["npx", "npx.cmd", "npx.exe"]
            .iter()
            .any(|s| executable.eq_ignore_ascii_case(s));
        Self {
            timeout: Duration::from_secs(120),
            launcher_env: &[],
            hint: if is_npx {
                "npx 首次启动可能需要下载 npm 包，请检查网络和 npm 缓存。"
            } else {
                "请确认命令和参数正确，且服务使用 stdio 传输。"
            },
        }
    }
}

/// A single-use, non-serializable confirmation of this exact configuration.
/// Created by trusted desktop code after confirmation, or by matching its saved digest.
/// Never expose this constructor as an Agent tool or infer it from enabled=true.
pub struct ConnectionConsent {
    config: ServerConfig,
}

impl ConnectionConsent {
    pub(crate) fn from_remembered(servers: &McpServers, config: &ServerConfig) -> AppResult<Self> {
        config.validate()?;
        if !config.enabled
            || servers.startup_digest(&config.id)?.as_deref()
                != Some(&config.confirmation_digest()?[..])
        {
            return Err(AppError::Denied(
                "此 MCP 配置没有自动启动授权，请重新确认".into(),
            ));
        }
        Ok(Self {
            config: config.clone(),
        })
    }

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
    lifetime: CancellationToken,
    child: Mutex<Option<Child>>,
    secrets: ConnectionSecrets,
}

impl Session {
    fn stop(&self) {
        self.lifetime.cancel();
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
        runtime: &McpRuntime,
        cancel: &CancellationToken,
    ) -> AppResult<Self> {
        let config = servers.get(id)?;
        config.validate()?;
        if !config.enabled {
            return Err(AppError::Denied("MCP server is disabled".into()));
        }
        if !consent.config.same_connection_config(&config) {
            return Err(AppError::Conflict(
                "MCP configuration changed; confirm again".into(),
            ));
        }
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let lifetime = CancellationToken::new();
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
                let (headers, token) = http_authorization(settings, &config)?;
                resolved_secrets.extend(token);
                let secrets = ConnectionSecrets::new(resolved_secrets)?;
                let http = reqwest_mcp::Client::builder()
                    .default_headers(headers)
                    .redirect(reqwest_mcp::redirect::Policy::none())
                    .connect_timeout(Duration::from_secs(10))
                    .timeout(REQUEST_TIMEOUT)
                    .build()
                    .map_err(|_| AppError::Other("Cannot initialize MCP HTTP client".into()))?;
                let transport = StreamableHttpClientTransport::with_client(http, transport_config);
                let service = bounded_startup(
                    cancel,
                    StartupBudget::HTTP,
                    ().serve_with_ct(transport, lifetime.clone()),
                )
                .await?;
                (service, None, secrets)
            }
            McpTransport::Stdio => {
                let executable = config.command.as_deref().unwrap();
                let budget = StartupBudget::stdio(executable);
                let mut command = runtime.command(executable, cancel).await?;
                command
                    .args(&config.args)
                    .envs(budget.launcher_env.iter().copied())
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    // Never forward credential-bearing stderr into application logs.
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                // No ambient API keys, shell startup configuration or loader hooks.
                for key in [
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
                    let Some(value) = stdio_secret(settings, &config, env, key)? else {
                        continue;
                    };
                    command.env(env, &value);
                    resolved_secrets.push(value);
                }
                let secrets = ConnectionSecrets::new(resolved_secrets)?;
                let mut child = {
                    // A plugin commit and the launch of its old executable must not cross.
                    let _plugin_write = config
                        .plugin_slug
                        .as_ref()
                        .map(|_| crate::transaction::write_lock());
                    if cancel.is_cancelled() {
                        return Err(AppError::Cancelled);
                    }
                    if config.plugin_slug.is_some()
                        && !servers.get(id)?.same_connection_config(&config)
                    {
                        return Err(AppError::Conflict(
                            "配置已变化，请重新确认完整连接配置".into(),
                        ));
                    }
                    command.spawn().map_err(|err| AppError::Other(match err.kind() {
                        std::io::ErrorKind::NotFound => "找不到 MCP 启动程序。请确认已安装，并在应用的 PATH 中，或将命令设置为绝对路径。",
                        _ => "无法启动 MCP 进程，请检查启动程序及其执行权限。",
                    }.into()))?
                };
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
                let service = match bounded_startup(
                    cancel,
                    budget,
                    ().serve_with_ct(transport, lifetime.clone()),
                )
                .await
                {
                    Ok(service) => service,
                    Err(err) => {
                        let _ = child.start_kill();
                        if let Err(cleanup) = reap_child(child).await {
                            tracing::warn!(%cleanup, "could not reap MCP child after startup failure");
                        }
                        return Err(err);
                    }
                };
                (service, Some(child), secrets)
            }
        };
        Ok(Self {
            servers,
            config,
            session: Arc::new(Session {
                service,
                lifetime,
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

    pub(super) async fn disconnected(&self) {
        self.session.lifetime.cancelled().await;
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
        if let Some(child) = child {
            reap_child(child).await?;
        }
        Ok(())
    }

    pub(crate) fn ensure_current(&self) -> AppResult<()> {
        let current = match self.servers.get(&self.config.id) {
            Ok(config) => config,
            Err(error) => {
                self.disconnect();
                return Err(error);
            }
        };
        if !current.same_connection_config(&self.config) || !current.enabled {
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
        let discover = async {
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
        };
        let result = self.request(cancel, discover).await;
        if result.is_err() {
            self.disconnect();
        }
        result
    }

    /// The desktop upload command supplies only its own staged file. Keep arbitrary tool calls private.
    pub(crate) async fn call_attachment_converter(
        &self,
        file: &tempfile::TempPath,
    ) -> AppResult<Value> {
        let uri = url::Url::from_file_path(dunce::simplified(file))
            .ok()
            .filter(|uri| uri.host().is_none())
            .ok_or_else(|| {
                AppError::Invalid(
                    "附件临时目录必须是本机的绝对路径，请检查系统临时目录设置。".into(),
                )
            })?;
        self.call_tool(
            MARKDOWN_TOOL,
            serde_json::json!({"uri": uri.as_str()}),
            &CancellationToken::new(),
        )
        .await
        .map_err(|err| match err {
            AppError::Other(message) if message == TIMEOUT_MESSAGE => AppError::Other(format!(
                "MarkItDown 转换超过 {} 秒。请拆分文档，在「设置 → 工具」中重新启动 MarkItDown 后重试。",
                REQUEST_TIMEOUT.as_secs()
            )),
            AppError::Invalid(message) if message == RESULT_LIMIT_MESSAGE => AppError::Invalid(
                "MarkItDown 返回的 MCP 消息超过 1 MiB。请拆分文档，在「设置 → 工具」中重新启动 MarkItDown 后重试。".into()
            ),
            _ => AppError::Other("MarkItDown MCP 调用失败。请在「设置 → 工具」中停止并重新启动 MarkItDown，然后重试。".into()),
        })
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
        let result = self
            .request(
                cancel,
                self.session.service.peer().call_tool(
                    CallToolRequestParams::new(name.to_owned()).with_arguments(arguments),
                ),
            )
            .await;
        match result {
            Ok(result) => {
                let filtered = (|| {
                    let value = serde_json::to_value(result)
                        .map_err(|_| AppError::Other("Cannot serialize MCP result".into()))?;
                    if serde_json::to_vec(&value)?.len() > MAX_RESULT_BYTES {
                        return Err(AppError::Invalid(RESULT_LIMIT_MESSAGE.into()));
                    }
                    let value = self.session.secrets.redact_value(value)?;
                    if serde_json::to_vec(&value)?.len() > MAX_RESULT_BYTES {
                        return Err(AppError::Invalid(RESULT_LIMIT_MESSAGE.into()));
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
    async fn request<T, E>(
        &self,
        cancel: &CancellationToken,
        operation: impl Future<Output = Result<T, E>>,
    ) -> AppResult<T> {
        tokio::select! {
            biased;
            _ = self.session.lifetime.cancelled() => Err(AppError::Cancelled),
            result = bounded(cancel, REQUEST_TIMEOUT, operation) => result,
        }
    }
}

async fn reap_child(mut child: Child) -> AppResult<()> {
    tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .map_err(|_| AppError::Other("MCP child shutdown timed out".into()))?
        .map_err(|_| AppError::Other("Cannot reap MCP child".into()))?;
    Ok(())
}

/// The stock 3DM endpoint accepts a raw API key, while other HTTP connections use Bearer.
pub(super) fn http_authorization(
    settings: &Settings,
    config: &ServerConfig,
) -> AppResult<(reqwest_mcp::header::HeaderMap, Option<String>)> {
    use reqwest_mcp::header::{HeaderMap, HeaderValue, AUTHORIZATION};
    let mut headers = HeaderMap::new();
    let Some(key) = &config.bearer_secret_ref else {
        return Ok((headers, None));
    };
    let raw_key = config.uses_3dm_authorization();
    let Some(token) = read_secret(settings, key)? else {
        return if raw_key || config.is_glossmod_endpoint() {
            Ok((headers, None))
        } else {
            Err(missing_secret())
        };
    };
    if token.chars().any(char::is_control) {
        return Err(AppError::Invalid("MCP HTTP 凭据包含无效字符".into()));
    }
    let header = if raw_key {
        HeaderValue::from_str(&token)
    } else {
        HeaderValue::from_str(&format!("Bearer {token}"))
    };
    let mut header = header.map_err(|_| AppError::Invalid("MCP HTTP 凭据格式无效".into()))?;
    header.set_sensitive(true);
    headers.insert(AUTHORIZATION, header);
    Ok((headers, Some(token)))
}

fn missing_secret() -> AppError {
    AppError::Invalid("请先在「设置 → 工具」中配置该工具所需的凭据".into())
}

fn read_secret(settings: &Settings, key: &str) -> AppResult<Option<String>> {
    let value = settings
        .read_secret(key)
        .map_err(|_| AppError::Other("Cannot read encrypted MCP credential".into()))?;
    if value
        .as_ref()
        .is_some_and(|v| v.is_empty() || v.len() > 8192 || v.contains('\0'))
    {
        return Err(AppError::Invalid("Invalid MCP credential".into()));
    }
    Ok(value)
}

pub(super) fn stdio_secret(
    settings: &Settings,
    config: &ServerConfig,
    env: &str,
    key: &str,
) -> AppResult<Option<String>> {
    let value = read_secret(settings, key)?;
    // The stock Nexus service can browse public projects anonymously.
    let optional = config.builtin
        && config.id == super::config::NEXUSMODS_ID
        && env == "NEXUSMODS_API_KEY"
        && key == "mcp.nexusmods.api_key";
    if value.is_none() && !optional {
        return Err(missing_secret());
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
            Err(_) => Err(AppError::Other(TIMEOUT_MESSAGE.into())),
        }
    }
}

fn startup_failure_hint(error: &ClientInitializeError, budget: StartupBudget) -> &'static str {
    let ClientInitializeError::TransportError { error, .. } = error else {
        return budget.hint;
    };
    let Some(http) = error
        .error
        .downcast_ref::<StreamableHttpError<reqwest_mcp::Error>>()
    else {
        return budget.hint;
    };
    // Without WWW-Authenticate, rmcp 3.10 embeds the HTTP status and body in
    // UnexpectedServerResponse. Only inspect its SDK-generated status prefix.
    const UNAUTHORIZED: &str = "HTTP 401，身份验证失败，请检查此连接的密钥。";
    const FORBIDDEN: &str = "HTTP 403，服务拒绝访问，请检查密钥权限。";
    match http {
        StreamableHttpError::AuthRequired(_) => UNAUTHORIZED,
        StreamableHttpError::InsufficientScope(_) => FORBIDDEN,
        StreamableHttpError::UnexpectedServerResponse(message)
            if message.starts_with("HTTP 401 ") =>
        {
            UNAUTHORIZED
        }
        StreamableHttpError::UnexpectedServerResponse(message)
            if message.starts_with("HTTP 403 ") =>
        {
            FORBIDDEN
        }
        _ => budget.hint,
    }
}

async fn bounded_startup<T>(
    cancel: &CancellationToken,
    budget: StartupBudget,
    operation: impl Future<Output = Result<T, ClientInitializeError>>,
) -> AppResult<T> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(AppError::Cancelled),
        result = tokio::time::timeout(budget.timeout, operation) => match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(AppError::Other(format!("MCP 启动握手失败。{}未调用任何工具。", startup_failure_hint(&error, budget)))),
            Err(_) => Err(AppError::Other(format!("MCP 启动超时（{} 秒）。{}未调用任何工具。", budget.timeout.as_secs(), budget.hint))),
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

    #[test]
    fn cold_start_budget_only_extends_uvx_launches() {
        for command in ["uvx", "uvx.exe", "/tools/UVX.EXE"] {
            let budget = StartupBudget::stdio(command);
            assert_eq!(budget.timeout, Duration::from_secs(600));
            assert_eq!(
                budget.launcher_env,
                &[("UV_HTTP_TIMEOUT", "120"), ("UV_CONCURRENT_DOWNLOADS", "4")]
            );
            assert!(budget.hint.contains("uvx"));
            assert!(!budget.hint.contains("npx"));
        }
        #[cfg(windows)]
        assert_eq!(
            StartupBudget::stdio(r"C:\Program Files\uv\uvx.exe").timeout,
            Duration::from_secs(600)
        );
        for command in ["npx", "npx.cmd", "my-uvx", "/uvx/python"] {
            let budget = StartupBudget::stdio(command);
            assert_eq!(budget.timeout, Duration::from_secs(120));
            assert!(budget.launcher_env.is_empty());
            assert_eq!(budget.hint.contains("npx"), command.starts_with("npx"));
        }
        assert_eq!(StartupBudget::HTTP.timeout, Duration::from_secs(20));
    }

    #[tokio::test]
    async fn startup_reports_timeout_separately_from_failure_and_cancellation() {
        let cancel = CancellationToken::new();
        let budget = StartupBudget {
            timeout: Duration::from_millis(2),
            ..StartupBudget::stdio("uvx")
        };
        let hung = std::future::pending::<Result<(), ClientInitializeError>>();
        let timeout = bounded_startup(&cancel, budget, hung)
            .await
            .unwrap_err()
            .to_string();
        assert!(timeout.contains("超时") && timeout.contains("uvx") && !timeout.contains("npx"));
        let failed = async {
            Err::<(), _>(ClientInitializeError::ConnectionClosed(
                "private-token".into(),
            ))
        };
        let error = bounded_startup(&cancel, budget, failed)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("握手失败")
                && !error.contains("超时")
                && !error.contains("private-token")
        );
        cancel.cancel();
        assert!(matches!(
            bounded_startup(&cancel, budget, async { Ok(()) }).await,
            Err(AppError::Cancelled)
        ));
    }

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
