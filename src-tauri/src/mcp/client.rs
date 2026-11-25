use super::browse::{credential_revision, public_modio_base, CredentialRevision};
use super::config::{McpServers, McpTransport, ServerConfig, PLAYWRIGHT_COMMAND};
use super::redaction::ConnectionSecrets;
use super::McpRuntime;
use crate::{
    agent::sse::{available_memory, update_memory_reservation},
    error::{AppError, AppResult},
    settings::Settings,
};
use futures_util::StreamExt;
use reqwest_mcp::header::{HeaderName, HeaderValue};
use rmcp::{
    model::{
        CallToolRequest, CallToolRequestParams, CallToolResult, ClientJsonRpcMessage,
        PaginatedRequestParams, ServerResult,
    },
    service::{
        ClientInitializeError, NotificationContext, PeerRequestOptions, RequestHandle,
        RunningService, ServiceError,
    },
    transport::{
        async_rw::{AsyncRwTransport, DEFAULT_MAX_LINE_LENGTH},
        common::client_side_sse::BoxedSseResponse,
        streamable_http_client::{
            SseError, StreamableHttpClient, StreamableHttpClientTransportConfig,
            StreamableHttpError, StreamableHttpPostResponse,
        },
        StreamableHttpClientTransport,
    },
    ClientHandler, RoleClient, ServiceExt,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    io,
    pin::Pin,
    process::Stdio,
    sync::{
        atomic::{AtomicUsize, Ordering::Relaxed},
        Arc, Mutex,
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, ReadBuf},
    process::{Child, Command},
    sync::{Mutex as AsyncMutex, MutexGuard},
};
use tokio_util::sync::CancellationToken;

static RESERVED_MCP_MEMORY: AtomicUsize = AtomicUsize::new(0);
pub(crate) const MARKDOWN_TOOL: &str = "convert_to_markdown";
const TOOL_PAGE_TIMEOUT: Duration = Duration::from_secs(30);
const BROWSE_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const CONNECTION_CLOSED_MESSAGE: &str = "MCP 连接已关闭；远端工具可能已经产生外部影响";
const REMOTE_ERROR_MESSAGE: &str = "MCP request failed (remote details suppressed)";
const TIMEOUT_MESSAGE: &str = "MCP request timed out; external effects may already have occurred";
const MEMORY_MESSAGE: &str = "MCP 当前可用内存预算不足；请减少并发操作或缩小单次请求。";
const HTTP_POSTS: usize = 2;
const HTTP_QUEUE: usize = 4;
// Fixed success-body input cap in the pinned rmcp reqwest backend; reserve
// raw buffer/capacity only while a POST can hold it, not while a client is idle.
const RMCP_HTTP_JSON_CAP: usize = 16 * 1024 * 1024;

enum ToolCall {
    Ordinary,
    AgentBridge,
    Browse,
}

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

// Limit termination of browser and converter process trees to their own group/job.
#[derive(Default)]
struct StdioScope {
    #[cfg(unix)]
    group: bool,
    #[cfg(windows)]
    job: Option<std::os::windows::io::OwnedHandle>,
}

impl StdioScope {
    fn new(_child: &Child, _scoped: bool) -> io::Result<Self> {
        #[cfg(unix)]
        let scope = Self { group: _scoped };
        #[cfg(windows)]
        let scope = Self {
            job: _scoped.then(|| stdio_job(_child)).transpose()?,
        };
        #[cfg(not(any(unix, windows)))]
        let scope = Self::default();
        Ok(scope)
    }

    fn stop(&self, child: &mut Child) {
        #[cfg(unix)]
        if self.group {
            // process_group(0) made the owned child the group leader. Its Child
            // handle has not been reaped, so the PID is not recycled here.
            if let Some(pid) = child.id().and_then(|id| i32::try_from(id).ok()) {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        }
        #[cfg(windows)]
        if let Some(job) = &self.job {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::JobObjects::TerminateJobObject;
            unsafe {
                TerminateJobObject(job.as_raw_handle(), 1);
            }
        }
        let _ = child.start_kill();
    }
}

#[cfg(windows)]
fn stdio_job(child: &Child) -> io::Result<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    let process = child
        .raw_handle()
        .ok_or_else(|| io::Error::other("stdio process exited"))?;
    // The unnamed, non-inheritable job handle is owned by this session only.
    let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    let job = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let configured = unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    };
    if configured == 0 || unsafe { AssignProcessToJobObject(job.as_raw_handle(), process) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(job)
}

// Like the default handler, advertises no capability for local tools, roots,
// sampling or elicitation. A changed tool catalog invalidates browse grants.
#[derive(Clone, Default)]
struct CatalogObserver(CancellationToken);

impl ClientHandler for CatalogObserver {
    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.0.cancel();
    }
}

// Some desktop consumers impose a shorter timeout than the transport. A
// dropped query future must not leave an admitted RPC running indefinitely.
struct BrowsePending(Option<RequestHandle<RoleClient>>);

impl Drop for BrowsePending {
    fn drop(&mut self) {
        if let (Some(request), Ok(runtime)) = (self.0.take(), tokio::runtime::Handle::try_current())
        {
            runtime.spawn(async move {
                let _ =
                    tokio::time::timeout(Duration::from_millis(250), request.cancel(None)).await;
            });
        }
    }
}

struct Session {
    service: RunningService<RoleClient, CatalogObserver>,
    catalog_observer: CatalogObserver,
    browse_credentials: Option<CredentialRevision>,
    lifetime: CancellationToken,
    child: Mutex<Option<Child>>,
    stdio_scope: StdioScope,
    secrets: ConnectionSecrets,
    has_secrets: bool,
    frames: FrameLimit,
    operation: AsyncMutex<()>,
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
            self.stdio_scope.stop(child);
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

/// The stock source MCPs configure their local destination at process startup,
/// not via a download_file argument. Keep this out of the parent environment.
pub(super) fn configure_source_download_dir(
    command: &mut Command,
    config: &ServerConfig,
    db: &crate::db::Db,
) -> AppResult<()> {
    if let Some(key) = config.source_download_dir_env() {
        let inbox =
            crate::storage::incoming_downloads(super::config::active_storage_root(&db.conn())?);
        crate::safe_fs::create_dir_all(&inbox)?;
        command.env(key, crate::paths::user_path(&inbox).as_os_str());
    }
    Ok(())
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
        let catalog_observer = CatalogObserver::default();
        // A tracking failure disables browse, not unrelated Agent MCP use.
        let mut browse_credentials = config
            .is_stock_browse_source()
            .then(|| credential_revision(&servers, &config, true).ok())
            .flatten();
        let mut resolved_secrets = Vec::new();
        let (service, child, stdio_scope, secrets, has_secrets, frames) = match config.transport {
            McpTransport::StreamableHttp => {
                let defaults =
                    StreamableHttpClientTransportConfig::with_uri(config.url.clone().unwrap());
                // The SDK default is the defensive ceiling. The HTTP adapter
                // reserves actual in-flight work and may reduce its SSE bound.
                let bytes = defaults.max_sse_event_size;
                let frames = FrameLimit::new(bytes, "HTTP SSE 事件", &lifetime);
                let mut transport_config = defaults
                    .max_concurrent_requests(HTTP_POSTS)
                    .max_sse_event_size(bytes)
                    // Never automatically replay a request that may mutate external state.
                    .reinit_on_expired_session(false)
                    .control_request_timeout(Duration::from_secs(2));
                transport_config.channel_buffer_capacity = HTTP_QUEUE;
                let (headers, token) = http_authorization(settings, &config)?;
                resolved_secrets.extend(token);
                let has_secrets = !resolved_secrets.is_empty();
                let secrets = ConnectionSecrets::new(resolved_secrets)?;
                let http = reqwest_mcp::Client::builder()
                    .default_headers(headers)
                    .redirect(reqwest_mcp::redirect::Policy::none())
                    .connect_timeout(Duration::from_secs(10))
                    .build()
                    .map_err(|_| AppError::Other("Cannot initialize MCP HTTP client".into()))?;
                let transport = StreamableHttpClientTransport::with_client(
                    BoundedHttp {
                        http,
                        frames: frames.clone(),
                    },
                    transport_config,
                );
                let service = bounded_startup(
                    cancel,
                    StartupBudget::HTTP,
                    catalog_observer
                        .clone()
                        .serve_with_ct(transport, lifetime.clone()),
                )
                .await
                .map_err(|err| frames.prefer_overflow(err))?;
                (
                    service,
                    None,
                    StdioScope::default(),
                    secrets,
                    has_secrets,
                    frames,
                )
            }
            McpTransport::Stdio => {
                let mut memory = McpMemory::new();
                let bytes = memory.frame_bytes(DEFAULT_MAX_LINE_LENGTH, 1)?;
                let frames = FrameLimit::new(bytes, "stdio 单条消息", &lifetime);
                let executable = config.command.as_deref().unwrap();
                let budget = StartupBudget::stdio(executable);
                let mut command = if executable == PLAYWRIGHT_COMMAND {
                    let (install, server) = runtime.playwright_commands(cancel).await?;
                    if uses_managed_chromium(&config.args) {
                        ensure_chromium(install, cancel).await?;
                    }
                    server
                } else if config.id == super::config::NEXUSMODS_ID
                    && config.source_download_dir_env().is_some()
                {
                    runtime.nexus_command(cancel).await?
                } else {
                    runtime.command(executable, cancel).await?
                };
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
                configure_source_download_dir(&mut command, &config, &servers.db)?;
                for (env, key) in &config.env_secret_refs {
                    let Some(value) = stdio_secret(settings, &config, env, key)? else {
                        continue;
                    };
                    if config.id == "modio"
                        && env == "MODIO_API_BASE_URL"
                        && !public_modio_base(&value)
                    {
                        browse_credentials = None;
                    }
                    command.env(env, &value);
                    resolved_secrets.push(value);
                }
                let has_secrets = !resolved_secrets.is_empty();
                let secrets = ConnectionSecrets::new(resolved_secrets)?;
                let owns_process_tree = executable == PLAYWRIGHT_COMMAND
                    || (config.builtin && config.id == super::config::MARKITDOWN_ID);
                #[cfg(unix)]
                if owns_process_tree {
                    command.process_group(0);
                }
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
                let stdio_scope = StdioScope::new(&child, owns_process_tree).map_err(|_| {
                    AppError::Other("无法为 MCP 启动程序建立可取消的进程范围。".into())
                })?;
                let stdout = child
                    .stdout
                    .take()
                    .ok_or_else(|| AppError::Other("Missing MCP stdout".into()))?;
                let stdin = child
                    .stdin
                    .take()
                    .ok_or_else(|| AppError::Other("Missing MCP stdin".into()))?;
                let transport = AsyncRwTransport::<RoleClient, _, _>::new(
                    StdioFrameReader {
                        inner: stdout,
                        line_bytes: 0,
                        frames: frames.clone(),
                        _memory: memory,
                    },
                    stdin,
                )
                .with_max_line_length(bytes);
                let service = match bounded_startup(
                    cancel,
                    budget,
                    catalog_observer
                        .clone()
                        .serve_with_ct(transport, lifetime.clone()),
                )
                .await
                {
                    Ok(service) => service,
                    Err(err) => {
                        stdio_scope.stop(&mut child);
                        if let Err(cleanup) = reap_child(child).await {
                            tracing::warn!(%cleanup, "could not reap MCP child after startup failure");
                        }
                        return Err(frames.prefer_overflow(err));
                    }
                };
                (
                    service,
                    Some(child),
                    stdio_scope,
                    secrets,
                    has_secrets,
                    frames,
                )
            }
        };
        Ok(Self {
            servers,
            config,
            session: Arc::new(Session {
                service,
                catalog_observer,
                browse_credentials,
                lifetime,
                child: Mutex::new(child),
                stdio_scope,
                secrets,
                has_secrets,
                frames,
                operation: AsyncMutex::new(()),
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

    pub(crate) fn is_closed(&self) -> bool {
        self.session.lifetime.is_cancelled() || self.session.service.is_closed()
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
        if self.is_closed() {
            return Err(AppError::Conflict(
                "MCP session closed; reconnect with confirmation".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn uses_browse_database(&self, servers: &McpServers) -> bool {
        Arc::ptr_eq(&self.servers.db.0, &servers.db.0)
    }

    pub(crate) fn ensure_browse_current(&self) -> AppResult<()> {
        self.ensure_current()?;
        if self.session.catalog_observer.0.is_cancelled() {
            return Err(AppError::Unsupported(
                "MCP 工具契约已变化；请重新连接".into(),
            ));
        }
        let Some(revision) = &self.session.browse_credentials else {
            return Err(AppError::Denied("浏览仅允许原厂公开来源".into()));
        };
        if credential_revision(&self.servers, &self.config, false)? != *revision {
            return Err(AppError::Denied(
                "MCP 凭据已变化；请重新连接并确认游览".into(),
            ));
        }
        Ok(())
    }

    /// Exercise the internal read transport against a loopback fixture only.
    /// The stock-source/discovery gate deliberately still rejects this peer.
    #[cfg(test)]
    pub(super) fn fixture_read_transport(&mut self) {
        assert!(self
            .config
            .url
            .as_deref()
            .is_some_and(|url| url.starts_with("http://127.0.0.1:")));
        Arc::get_mut(&mut self.session)
            .expect("unshared fixture client")
            .browse_credentials =
            Some(credential_revision(&self.servers, &self.config, true).unwrap());
    }

    pub async fn list_tools(&self, cancel: &CancellationToken) -> AppResult<Vec<McpTool>> {
        let _operation = self.lock_operation(cancel).await?;
        let result = self.discover_tools(cancel).await;
        if result.is_err() {
            self.disconnect();
        }
        result
    }

    async fn discover_tools(&self, cancel: &CancellationToken) -> AppResult<Vec<McpTool>> {
        check_cancelled(cancel)?;
        self.ensure_current()?;
        let mut catalog = ToolCatalog::default();
        let mut cursor = None;
        loop {
            let params = cursor.take().map(|cursor| {
                let mut params = PaginatedRequestParams::default();
                params.cursor = Some(cursor);
                params
            });
            let page = self
                .request(
                    cancel,
                    Some(TOOL_PAGE_TIMEOUT),
                    self.session.service.peer().list_tools(params),
                )
                .await?;
            let mut scratch = McpMemory::new();
            let page_bytes = self.message_bytes(&page, "工具列表单页")?;
            scratch.charge(page_bytes.saturating_mul(3))?;
            catalog.memory.refresh();
            if page.tools.is_empty() && page.next_cursor.is_some() {
                return Err(AppError::Invalid("MCP 工具分页未取得进展".into()));
            }
            for tool in page.tools {
                self.check_request_active(cancel)?;
                let remote_name = tool.name.into_owned();
                if remote_name.is_empty()
                    || remote_name.len() > 256
                    || remote_name.chars().any(char::is_control)
                    || catalog.names.contains(&remote_name)
                {
                    return Err(AppError::Invalid("MCP 工具名称无效或重复".into()));
                }
                let secrets = &self.session.secrets;
                let input_schema = Value::Object(Arc::unwrap_or_clone(tool.input_schema));
                let description_bytes = tool.description.as_deref().map_or(0, str::len);
                let mut filter = McpMemory::with_limit(scratch.limit, scratch.pool);
                if self.session.has_secrets {
                    let schema_wire = self.message_bytes(&input_schema, "工具参数定义")?;
                    filter.charge(
                        json_heap_bytes(&input_schema)
                            .saturating_mul(2)
                            .saturating_add(
                                schema_wire
                                    .saturating_add(description_bytes)
                                    .saturating_mul(12),
                            ),
                    )?;
                }
                // Do not redact names/schemas into a different tool contract.
                if secrets.contains(&remote_name)
                    || secrets.redact_value(input_schema.clone()).ok().as_ref()
                        != Some(&input_schema)
                {
                    return Err(AppError::Other(REMOTE_ERROR_MESSAGE.into()));
                }
                let description = secrets
                    .redact_value(Value::String(
                        tool.description.unwrap_or_default().into_owned(),
                    ))
                    .map_err(|_| AppError::Other(REMOTE_ERROR_MESSAGE.into()))?;
                let Value::String(description) = description else {
                    unreachable!("redacting a string yields a string");
                };
                let tool = McpTool {
                    name: namespaced_tool_name(&self.config.id, &remote_name),
                    remote_name,
                    description,
                    input_schema,
                };
                self.message_bytes(&tool, "工具定义")?;
                catalog.push(tool)?;
            }
            self.check_request_active(cancel)?;
            match page.next_cursor {
                None => return Ok(catalog.tools),
                Some(next) => {
                    catalog.record_cursor(&next)?;
                    cursor = Some(next);
                }
            }
        }
    }

    /// The desktop upload command supplies only its own staged file. Keep arbitrary tool calls private.
    pub(crate) async fn call_attachment_converter(
        &self,
        file: &tempfile::TempPath,
        cancel: &CancellationToken,
    ) -> AppResult<Value> {
        let uri = url::Url::from_file_path(dunce::simplified(file))
            .ok()
            .filter(|uri| uri.host().is_none())
            .ok_or_else(|| {
                AppError::Invalid(
                    "附件临时目录必须是本机的绝对路径，请检查系统临时目录设置。".into(),
                )
            })?;
        // MarkItDown may not honor an MCP cancellation notification while a
        // converter is running. call_tool closes the session on in-flight
        // cancellation; waiting for the direct child also releases the file.
        let result = self
            .call_tool(
                MARKDOWN_TOOL,
                serde_json::json!({"uri": uri.as_str()}),
                cancel,
            )
            .await;
        if result.is_err() && self.session.lifetime.is_cancelled() {
            // Wait for the direct child to release the staged file before the IPC
            // tries to unlink it (in particular on Windows).
            let _ = self.disconnect_and_wait().await;
        }
        result.map_err(|err| match err {
            AppError::Conflict(_) if self.session.lifetime.is_cancelled() => AppError::Cancelled,
            AppError::Other(ref reason) if reason == CONNECTION_CLOSED_MESSAGE => {
                AppError::Cancelled
            }
            other => attachment_call_error(other),
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
        self.call_tool_filtered(name, args, cancel, ToolCall::Ordinary)
            .await
    }

    /// Only the Agent bridge calls this after its existing approval checks.
    pub(super) async fn call_agent_tool(
        &self,
        name: &str,
        args: Value,
        cancel: &CancellationToken,
    ) -> AppResult<Value> {
        self.call_tool_filtered(name, args, cancel, ToolCall::AgentBridge)
            .await
    }

    /// Internal to the exact stock browse whitelist. Other modules must not
    /// get this raw dispatcher; `BrowseReader` verifies the discovered schema.
    pub(super) async fn call_browse_tool(
        &self,
        name: &str,
        args: Value,
        cancel: &CancellationToken,
    ) -> AppResult<Value> {
        self.call_tool_filtered(name, args, cancel, ToolCall::Browse)
            .await
    }

    /// Text-only MCP servers can escape an echoed credential in their nested
    /// JSON. Filter the decoded value as well as the outer wire envelope.
    pub(super) fn redact_browse_catalog(&self, catalog: Value) -> AppResult<Value> {
        if !self.session.has_secrets {
            return Ok(catalog);
        }
        let mut memory = McpMemory::new();
        let bytes = json_bytes(&catalog, BROWSE_RESPONSE_BYTES).ok_or_else(browse_size_error)?;
        memory.charge(
            bytes
                .saturating_mul(22)
                .saturating_add(json_heap_bytes(&catalog).saturating_mul(2)),
        )?;
        let filtered = self.session.secrets.redact_value(catalog)?;
        json_bytes(&filtered, BROWSE_RESPONSE_BYTES).ok_or_else(browse_size_error)?;
        Ok(filtered)
    }

    async fn call_tool_filtered(
        &self,
        name: &str,
        args: Value,
        cancel: &CancellationToken,
        call: ToolCall,
    ) -> AppResult<Value> {
        let browse = matches!(call, ToolCall::Browse);
        let _operation = self.lock_operation(cancel).await?;
        if browse {
            self.ensure_browse_current()?;
        } else {
            self.ensure_current()?;
        }
        let Value::Object(arguments) = args else {
            return Err(AppError::Invalid("MCP 工具参数必须是 JSON 对象".into()));
        };
        let mut memory = McpMemory::new();
        let args = Value::Object(arguments);
        memory.charge(json_heap_bytes(&args))?;
        let remaining = memory.limit.saturating_sub(memory.held);
        let size = json_bytes(&args, remaining / 2).ok_or_else(memory_error)?;
        memory.charge(size.saturating_mul(2))?;
        let Value::Object(arguments) = args else {
            unreachable!()
        };
        let params = CallToolRequestParams::new(name.to_owned()).with_arguments(arguments);
        let result = if browse {
            // A canceled read, timeout, or denied read does not revoke the Agent's
            // whole existing connection. Transport overflows still stop it.
            Ok(self.browse_request(params, cancel).await?)
        } else {
            self.request(cancel, None, self.session.service.peer().call_tool(params))
                .await
        };
        let filtered = result.and_then(|result| {
            memory.refresh();
            let bytes = if browse {
                json_bytes(
                    &result,
                    self.session.frames.bytes.min(BROWSE_RESPONSE_BYTES),
                )
                .ok_or_else(browse_size_error)?
            } else {
                self.message_bytes(&result, "工具结果")?
            };
            // Redaction can replace a one-byte credential with ten bytes. Allow
            // for the redacted tree and its encoded verification copy before
            // doing that work, not just after allocation.
            let copies = if self.session.has_secrets { 22 } else { 2 };
            memory.charge(bytes.saturating_mul(copies))?;
            let mut value = serde_json::to_value(result)
                .map_err(|_| AppError::Other("无法序列化 MCP 结果".into()))?;
            let heap = json_heap_bytes(&value);
            memory.charge(heap.saturating_mul(if self.session.has_secrets { 2 } else { 1 }))?;
            if matches!(call, ToolCall::AgentBridge)
                && self.session.has_secrets
                && self.session.browse_credentials.is_some()
                && self.server_config().is_stock_browse_source()
            {
                redact_stock_agent_json_text(&self.session.secrets, &mut value, &mut memory)?;
            }
            let value = self.session.secrets.redact_value(value)?;
            self.message_bytes(&value, "脱敏后的工具结果")?;
            memory.charge(json_heap_bytes(&value).saturating_sub(heap))?;
            self.check_request_active(cancel)?;
            // Untrusted data: do not follow links, read resources or honor tool calls.
            Ok(value)
        });
        if filtered.is_err() && !browse {
            self.disconnect();
        }
        if browse && filtered.is_ok() {
            self.ensure_browse_current()?;
        }
        filtered
    }

    async fn browse_request(
        &self,
        params: CallToolRequestParams,
        cancel: &CancellationToken,
    ) -> AppResult<CallToolResult> {
        self.check_request_active(cancel)?;
        let deadline = tokio::time::Instant::now() + TOOL_PAGE_TIMEOUT;
        let timeout = || AppError::Other("MCP 目录读取超时".into());
        let closed = || AppError::Conflict("MCP 连接已关闭；请重新确认游览".into());
        let changed = || AppError::Unsupported("MCP 工具契约已变化；请重新连接".into());
        let handle = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AppError::Cancelled),
            _ = self.session.catalog_observer.0.cancelled() => return Err(changed()),
            _ = self.session.lifetime.cancelled() => return Err(closed()),
            _ = tokio::time::sleep_until(deadline) => return Err(timeout()),
            res = self.session.service.peer().send_request_with_option(
                CallToolRequest::new(params).into(), PeerRequestOptions::no_options(),
            ) => res.map_err(browse_service_failure)?,
        };
        let mut pending = BrowsePending(Some(handle));
        let error = tokio::select! {
            biased;
            _ = cancel.cancelled() => AppError::Cancelled,
            _ = self.session.catalog_observer.0.cancelled() => changed(),
            _ = self.session.frames.overflow.cancelled() => return Err(self.session.frames.error()),
            _ = self.session.lifetime.cancelled() => return Err(closed()),
            _ = tokio::time::sleep_until(deadline) => timeout(),
            response = &mut pending.0.as_mut().expect("pending browse request").rx => {
                pending.0.take();
                return match response.map_err(|_| closed())?.map_err(browse_service_failure)? {
                    ServerResult::CallToolResult(result) => Ok(result),
                    _ => Err(AppError::Other(REMOTE_ERROR_MESSAGE.into())),
                };
            }
        };
        // Notify the remote tool when possible, while keeping local cancellation
        // responsive even if it stalls. Never restart/retry the tool implicitly.
        if let Some(handle) = pending.0.take() {
            let _ = tokio::time::timeout(Duration::from_millis(250), handle.cancel(None)).await;
        }
        Err(error)
    }

    fn message_bytes(&self, value: &impl Serialize, subject: &str) -> AppResult<usize> {
        let max = self.session.frames.bytes;
        json_bytes(value, max).ok_or_else(|| {
            AppError::Invalid(format!(
                "MCP {subject}超过本连接的单帧处理预算（{max} 字节）。请缩小单次结果或减少并发。"
            ))
        })
    }

    fn check_request_active(&self, cancel: &CancellationToken) -> AppResult<()> {
        if self.session.frames.overflow.is_cancelled() {
            Err(self.session.frames.error())
        } else {
            check_cancelled(cancel)?;
            if self.session.lifetime.is_cancelled() {
                return Err(connection_closed());
            }
            Ok(())
        }
    }

    async fn lock_operation(&self, cancel: &CancellationToken) -> AppResult<MutexGuard<'_, ()>> {
        tokio::select! {
            biased;
            _ = self.session.frames.overflow.cancelled() => Err(self.session.frames.error()),
            _ = cancel.cancelled() => Err(AppError::Cancelled),
            _ = self.session.lifetime.cancelled() => Err(AppError::Conflict(
                "MCP 连接已关闭；请重新确认并连接".into(),
            )),
            guard = self.session.operation.lock() => Ok(guard),
        }
    }

    async fn request<T>(
        &self,
        cancel: &CancellationToken,
        timeout: Option<Duration>,
        operation: impl Future<Output = Result<T, ServiceError>>,
    ) -> AppResult<T> {
        tokio::select! {
            biased;
            _ = self.session.frames.overflow.cancelled() => Err(self.session.frames.error()),
            _ = cancel.cancelled() => Err(AppError::Cancelled),
            _ = self.session.lifetime.cancelled() => Err(connection_closed()),
            result = bounded(cancel, timeout, async { operation.await.map_err(service_failure) }) => {
                result.map_err(|err| self.session.frames.prefer_overflow(err))
            }
        }
    }
}

fn connection_closed() -> AppError {
    AppError::Other(CONNECTION_CLOSED_MESSAGE.into())
}

fn attachment_call_error(err: AppError) -> AppError {
    const ADVICE: &str =
        "MarkItDown 不支持转换结果分页。请按页或章节拆分源文档，或另存为 UTF-8 文本。";
    match err {
        AppError::Cancelled => AppError::Cancelled,
        AppError::Invalid(message) => AppError::Invalid(format!("{message}{ADVICE}")),
        _ => AppError::Other(format!(
            "MarkItDown MCP 调用失败。{ADVICE}也可在「设置 → 工具」中重新启动 MarkItDown 后重试。"
        )),
    }
}

fn browse_size_error() -> AppError {
    AppError::Invalid("MCP 公开目录响应超过处理预算".into())
}

fn browse_service_failure(error: ServiceError) -> AppError {
    let denied = match &error {
        ServiceError::McpError(error) => matches!(error.code.0, 401 | 403 | -32001 | -32003),
        ServiceError::TransportSend(error) => {
            match error.error.downcast_ref::<HttpError>() {
                Some(
                    StreamableHttpError::AuthRequired(_)
                    | StreamableHttpError::InsufficientScope(_),
                ) => true,
                // Same SDK-generated prefix used by startup_failure_hint when
                // the rejected endpoint did not include a WWW-Authenticate challenge.
                Some(StreamableHttpError::UnexpectedServerResponse(message)) => {
                    message.starts_with("HTTP 401 ") || message.starts_with("HTTP 403 ")
                }
                _ => false,
            }
        }
        _ => false,
    };
    if denied {
        AppError::Denied("MCP 目录读取权限不足".into())
    } else {
        service_failure(error)
    }
}

fn service_failure(error: ServiceError) -> AppError {
    if let ServiceError::TransportSend(error) = error {
        match error.error.downcast_ref::<HttpError>() {
            Some(StreamableHttpError::ResponseBodyTooLarge { max_size }) => {
                return http_body_limit(*max_size)
            }
            Some(StreamableHttpError::Io(error)) if is_http_memory_error(error) => {
                return memory_error()
            }
            _ => {}
        }
    }
    AppError::Other(REMOTE_ERROR_MESSAGE.into())
}

fn http_body_limit(max_size: usize) -> AppError {
    AppError::Invalid(format!(
        "MCP HTTP 响应超过 SDK 传输上限（{max_size} 字节），请缩小单次响应。"
    ))
}

#[derive(Clone)]
struct FrameLimit {
    bytes: usize,
    kind: &'static str,
    overflow: CancellationToken,
    overflow_bytes: Arc<AtomicUsize>,
    lifetime: CancellationToken,
}

impl FrameLimit {
    fn new(bytes: usize, kind: &'static str, lifetime: &CancellationToken) -> Self {
        Self {
            bytes,
            kind,
            overflow: CancellationToken::new(),
            overflow_bytes: Arc::new(AtomicUsize::new(0)),
            lifetime: lifetime.clone(),
        }
    }

    fn exceeded(&self, bytes: usize) {
        let _ = self
            .overflow_bytes
            .compare_exchange(0, bytes, Relaxed, Relaxed);
        self.overflow.cancel();
        self.lifetime.cancel();
    }

    fn error(&self) -> AppError {
        let limit = self.overflow_bytes.load(Relaxed);
        let limit = if limit == 0 { self.bytes } else { limit };
        AppError::Invalid(format!(
            "MCP {}超过本连接的传输上限（{limit} 字节），连接已关闭。",
            self.kind
        ))
    }

    fn prefer_overflow(&self, error: AppError) -> AppError {
        if self.overflow.is_cancelled() {
            self.error()
        } else {
            error
        }
    }
}

// Keep rmcp's real HTTP implementation (including its non-configurable JSON
// and HTTP error body caps). Observe the SDK's locally generated SSE size error
// before its worker turns the failed event stream into a generic transport EOF.
#[derive(Clone)]
struct BoundedHttp {
    http: reqwest_mcp::Client,
    frames: FrameLimit,
}

impl BoundedHttp {
    fn observe_sse(
        &self,
        stream: BoxedSseResponse,
        limit: usize,
        memory: McpMemory<'static>,
    ) -> BoxedSseResponse {
        let frames = self.frames.clone();
        let expected = format!("SSE event exceeded the maximum size of {limit} bytes");
        stream
            .map(move |event| {
                let _lease = &memory; // Keep the reservation until this stream closes.
                                      // The size-error type is private to pinned rmcp. Inspect only the
                                      // local stream error, not an SSE payload or JSON-RPC error string.
                if matches!(&event, Err(SseError::Body(error)) if error.to_string() == expected) {
                    frames.exceeded(limit);
                }
                event
            })
            .boxed()
    }
}

type HttpHeaders = HashMap<HeaderName, HeaderValue>;
type HttpError = StreamableHttpError<reqwest_mcp::Error>;

#[derive(Debug, thiserror::Error)]
#[error("MCP HTTP response allocation was not admitted by the local memory budget")]
struct HttpMemoryLimit;

fn is_http_memory_error(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|cause| cause.is::<HttpMemoryLimit>())
}

fn http_reservation_error(_: AppError) -> HttpError {
    StreamableHttpError::Io(io::Error::other(HttpMemoryLimit))
}

impl StreamableHttpClient for BoundedHttp {
    type Error = reqwest_mcp::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth: Option<String>,
        headers: HttpHeaders,
    ) -> Result<StreamableHttpPostResponse, HttpError> {
        self.post_message_with_max_sse_event_size(
            uri,
            message,
            session_id,
            auth,
            headers,
            self.frames.bytes,
        )
        .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth: Option<String>,
        headers: HttpHeaders,
        limit: usize,
    ) -> Result<StreamableHttpPostResponse, HttpError> {
        let mut json_memory = McpMemory::new();
        json_memory
            .charge(RMCP_HTTP_JSON_CAP * 2)
            .map_err(http_reservation_error)?;
        let mut sse_memory = McpMemory::with_limit(json_memory.limit, json_memory.pool);
        let limit = sse_memory
            .frame_bytes(limit, 1)
            .map_err(http_reservation_error)?;
        Ok(
            match self
                .http
                .post_message_with_max_sse_event_size(
                    uri, message, session_id, auth, headers, limit,
                )
                .await?
            {
                StreamableHttpPostResponse::Sse(stream, session_id) => {
                    StreamableHttpPostResponse::Sse(
                        self.observe_sse(stream, limit, sse_memory),
                        session_id,
                    )
                }
                other => other,
            },
        )
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth: Option<String>,
        headers: HttpHeaders,
    ) -> Result<BoxedSseResponse, HttpError> {
        self.get_stream_with_max_sse_event_size(
            uri,
            session_id,
            last_event_id,
            auth,
            headers,
            self.frames.bytes,
        )
        .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth: Option<String>,
        headers: HttpHeaders,
        limit: usize,
    ) -> Result<BoxedSseResponse, HttpError> {
        let mut memory = McpMemory::new();
        let limit = memory
            .frame_bytes(limit, 1)
            .map_err(http_reservation_error)?;
        let stream = self
            .http
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth,
                headers,
                limit,
            )
            .await?;
        Ok(self.observe_sse(stream, limit, memory))
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth: Option<String>,
        headers: HttpHeaders,
    ) -> Result<(), HttpError> {
        self.http
            .delete_session(uri, session_id, auth, headers)
            .await
    }
}

// rmcp's line bound drops an oversized line and leaves pending calls waiting.
// Close this stdio session instead so a tool without an overall deadline fails.
struct StdioFrameReader<R> {
    inner: R,
    line_bytes: usize,
    frames: FrameLimit,
    _memory: McpMemory<'static>,
}

impl<R: AsyncRead + Unpin> AsyncRead for StdioFrameReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let start = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                for &byte in &buf.filled()[start..] {
                    self.line_bytes += 1;
                    if self.line_bytes > self.frames.bytes {
                        self.frames.exceeded(self.frames.bytes);
                        buf.set_filled(start);
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "MCP stdio frame exceeds size limit",
                        )));
                    }
                    if byte == b'\n' {
                        self.line_bytes = 0;
                    }
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

fn check_cancelled(cancel: &CancellationToken) -> AppResult<()> {
    if cancel.is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(())
    }
}

fn memory_error() -> AppError {
    AppError::Invalid(MEMORY_MESSAGE.into())
}

/// Estimates working allocations across overlapping MCP connections and operations.
/// The SDK's decoded messages and the published registry are not owned by the lease.
struct McpMemory<'a> {
    limit: usize,
    pool: &'a AtomicUsize,
    held: usize,
}

impl McpMemory<'static> {
    fn new() -> Self {
        Self::with_limit(Self::local_limit(), &RESERVED_MCP_MEMORY)
    }
}

impl<'a> McpMemory<'a> {
    fn local_limit() -> usize {
        // Same conservative fallback as the model client, only if the OS probe
        // is unsupported or cannot produce a reading (not when it reports zero).
        available_memory().map_or(128 * 1024 * 1024, |bytes| bytes / 8)
    }

    fn with_limit(limit: usize, pool: &'a AtomicUsize) -> Self {
        Self {
            limit,
            pool,
            held: 0,
        }
    }

    fn refresh(&mut self) {
        self.limit = Self::local_limit();
    }

    fn charge(&mut self, bytes: usize) -> AppResult<()> {
        update_memory_reservation(self.pool, |reserved| {
            reserved
                .checked_add(bytes)
                .filter(|&total| total <= self.limit)
        })
        .map_err(|_| memory_error())?;
        self.held += bytes;
        Ok(())
    }

    fn frame_bytes(&mut self, sdk_default: usize, slots: usize) -> AppResult<usize> {
        // Allow geometric growth of the receive buffer. Allocate at most half
        // of the unreserved working budget to framing, leaving room to process
        // the resulting JSON. The SDK default is a defensive ceiling, not a
        // fixed application frame allowance. Future connections share the pool.
        let mut frame = 0;
        let copies = slots * 2;
        update_memory_reservation(self.pool, |reserved| {
            frame = (self.limit.saturating_sub(reserved) / 2 / copies).min(sdk_default);
            // Below one typical I/O buffer, fail before launching a server that
            // is unlikely to deliver even its initialization/tool schema.
            (frame >= 8192).then(|| reserved + frame * copies)
        })
        .map_err(|_| memory_error())?;
        self.held += frame * copies;
        Ok(frame)
    }
}

impl Drop for McpMemory<'_> {
    fn drop(&mut self) {
        self.pool.fetch_sub(self.held, Relaxed);
    }
}

struct ToolCatalog<'a> {
    tools: Vec<McpTool>,
    names: HashSet<String>,
    cursors: HashSet<String>,
    memory: McpMemory<'a>,
}

impl Default for ToolCatalog<'static> {
    fn default() -> Self {
        Self {
            tools: Vec::new(),
            names: HashSet::new(),
            cursors: HashSet::new(),
            memory: McpMemory::new(),
        }
    }
}

impl ToolCatalog<'_> {
    fn charge(&mut self, bytes: usize) -> AppResult<()> {
        self.memory.charge(bytes).map_err(|_| {
            AppError::Invalid(format!("MCP 工具目录超过当前内存预算。{MEMORY_MESSAGE}"))
        })
    }

    fn record_cursor(&mut self, cursor: &str) -> AppResult<()> {
        if cursor.is_empty() || cursor.len() > 4096 || self.cursors.contains(cursor) {
            return Err(AppError::Invalid("MCP 分页游标无效或循环".into()));
        }
        self.charge(4 * std::mem::size_of::<String>() + cursor.len())?;
        self.cursors.insert(cursor.to_owned());
        Ok(())
    }

    fn push(&mut self, tool: McpTool) -> AppResult<()> {
        self.charge(
            // Allow for growing collection capacity, the name index and its copy.
            2 * std::mem::size_of::<McpTool>()
                + 4 * std::mem::size_of::<String>()
                + tool.name.capacity()
                + tool.remote_name.capacity()
                + tool.remote_name.len()
                + tool.description.capacity()
                + json_heap_bytes(&tool.input_schema),
        )?;
        self.names.insert(tool.remote_name.clone());
        self.tools.push(tool);
        Ok(())
    }
}

/// Some approved stock tools return a single text block containing JSON. The
/// wire-level filter cannot see alternate Unicode escapes inside that string.
/// Do not strip URLs or credential-looking field names; signed downloads can
/// contain both. If no session credential matched, keep the original text.
fn redact_stock_agent_json_text(
    secrets: &ConnectionSecrets,
    envelope: &mut Value,
    memory: &mut McpMemory<'_>,
) -> AppResult<()> {
    let Some(block) = envelope
        .get_mut("content")
        .and_then(Value::as_array_mut)
        .and_then(|blocks| match blocks.as_mut_slice() {
            [block] if block["type"] == "text" => Some(block),
            _ => None,
        })
    else {
        return Ok(());
    };
    let Some(text) = block["text"].as_str() else {
        return Ok(());
    };
    if !text.trim_start().starts_with(['{', '[']) {
        return Ok(());
    }
    // Reserve before parsing: a short, dense JSON string can decode to many
    // map and array entries. Allow room for serialization and redaction too.
    memory.charge(text.len().saturating_mul(64))?;
    let Ok(decoded) = serde_json::from_str::<Value>(text) else {
        return Ok(());
    };
    let encode = |value: &Value| {
        serde_json::to_string(value).map_err(|_| AppError::Other("无法序列化 MCP 结果".into()))
    };
    if secrets.contains(&encode(&decoded)?) {
        block["text"] = Value::String(encode(&secrets.redact_value(decoded)?)?);
    }
    Ok(())
}

// Estimate retained JSON allocation, including structure overhead for dense schemas.
fn json_heap_bytes(value: &Value) -> usize {
    match value {
        Value::String(text) => text.capacity(),
        Value::Array(items) => {
            items.capacity() * std::mem::size_of::<Value>()
                + items.iter().map(json_heap_bytes).sum::<usize>()
        }
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| {
                4 * std::mem::size_of::<(String, Value)>() + key.capacity() + json_heap_bytes(value)
            })
            .sum(),
        _ => 0,
    }
}

// Count serialized UTF-8 bytes without allocating another message-sized buffer.
fn json_bytes(value: &impl Serialize, max_bytes: usize) -> Option<usize> {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_sub(bytes.len())
                .ok_or_else(|| std::io::Error::other("JSON exceeds message size"))?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(max_bytes);
    serde_json::to_writer(&mut count, value).ok()?;
    Some(max_bytes - count.0)
}

fn uses_managed_chromium(args: &[String]) -> bool {
    let mut browser = None;
    let mut args = args.iter().map(String::as_str);
    while let Some(arg) = args.next() {
        let (flag, value) = arg
            .split_once('=')
            .map_or((arg, None), |(k, v)| (k, Some(v)));
        match flag {
            "--browser" => browser = value.or_else(|| args.next()),
            "--executable-path" | "--endpoint" | "--cdp-endpoint" | "--extension" => return false,
            _ => (),
        }
    }
    browser == Some("chromium")
}

async fn ensure_chromium(mut install: Command, cancel: &CancellationToken) -> AppResult<()> {
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    install
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    install.process_group(0);
    let mut child = install.spawn().map_err(|_| {
        AppError::Other("无法启动内置 Chromium 安装程序，请检查应用运行时。".into())
    })?;
    let scope = StdioScope::new(&child, true)
        .map_err(|_| AppError::Other("无法为 Chromium 安装程序建立可取消的进程范围。".into()))?;
    let error = tokio::select! {
        biased;
        _ = cancel.cancelled() => AppError::Cancelled,
        result = tokio::time::timeout(Duration::from_secs(600), child.wait()) => match result {
            Ok(Ok(status)) if status.success() => return Ok(()),
            Ok(Ok(status)) => return Err(AppError::Other(format!(
                "Playwright 自动安装 Chromium 失败（{status}）。请检查网络、磁盘空间和浏览器缓存的写入权限，然后重新启动该工具。"
            ))),
            Ok(Err(_)) => AppError::Other("无法读取 Chromium 安装结果。".into()),
            Err(_) => AppError::Other("Playwright 自动安装 Chromium 超时，请检查网络后重新启动该工具。".into()),
        }
    };
    scope.stop(&mut child);
    reap_child(child).await?;
    Err(error)
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

async fn bounded<T>(
    cancel: &CancellationToken,
    timeout: Option<Duration>,
    operation: impl Future<Output = AppResult<T>>,
) -> AppResult<T> {
    let response = async {
        match timeout {
            Some(timeout) => tokio::time::timeout(timeout, operation)
                .await
                .map_err(|_| AppError::Other(TIMEOUT_MESSAGE.into()))?,
            None => operation.await,
        }
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(AppError::Cancelled),
        result = response => result,
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
        StreamableHttpError::ResponseBodyTooLarge { .. } => "握手 HTTP 响应超过 SDK 传输上限。",
        StreamableHttpError::Io(error) if is_http_memory_error(error) => MEMORY_MESSAGE,
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
    fn stock_agent_json_text_redacts_decoded_credentials_but_preserves_signed_downloads() {
        let secrets = ConnectionSecrets::new(vec!["fixture-private-key".into()]).unwrap();
        let pool = AtomicUsize::new(0);
        let mut memory = McpMemory::with_limit(64 * 1024, &pool);
        let signed =
            r#"{ "download_url": "https://downloads.example/a.zip?token=fixture-signed-url" }"#;
        let escaped = r#"{"\u0066ixture-private-key":["\u0066ixture-private-key"],"download_url":"https://downloads.example/a.zip?token=fixture-signed-url"}"#;
        let envelope = |text| serde_json::json!({"content":[{"type":"text","text":text}]});
        let mut unchanged = envelope(signed);
        redact_stock_agent_json_text(&secrets, &mut unchanged, &mut memory).unwrap();
        assert_eq!(unchanged, envelope(signed));

        let mut value = envelope(escaped);
        redact_stock_agent_json_text(&secrets, &mut value, &mut memory).unwrap();
        let decoded: Value =
            serde_json::from_str(value["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(decoded["[REDACTED]"][0], "[REDACTED]");
        assert_eq!(
            decoded["download_url"],
            "https://downloads.example/a.zip?token=fixture-signed-url"
        );
        assert!(!value.to_string().contains("fixture-private-key"));
    }

    #[test]
    fn playwright_provisions_only_explicit_managed_chromium() {
        let uses = |args: &[&str]| {
            uses_managed_chromium(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        assert!(uses(&["--browser=chromium", "--headless"]));
        assert!(uses(&["--browser", "chromium"]));
        for args in [
            vec![],
            vec!["--browser=msedge"],
            vec!["--browser=chromium", "--browser=firefox"],
            vec!["--browser=chromium", "--executable-path", "custom"],
            vec!["--cdp-endpoint=custom", "--browser=chromium"],
        ] {
            assert!(!uses(&args));
        }
    }

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

    #[test]
    fn catalog_budget_accounts_for_retained_json_and_cursor_history() {
        let tool = McpTool {
            name: "local_name".into(),
            remote_name: "remote_name".into(),
            description: String::new(),
            input_schema: serde_json::json!({"enum": vec![0; 128]}),
        };
        assert!(json_heap_bytes(&tool.input_schema) >= 128 * std::mem::size_of::<Value>());
        let pool = AtomicUsize::new(0);
        let mut catalog = ToolCatalog {
            tools: Vec::new(),
            names: HashSet::new(),
            cursors: HashSet::new(),
            memory: McpMemory::with_limit(1024, &pool),
        };
        assert!(catalog.push(tool).is_err());
        assert!(catalog.tools.is_empty());
        assert!(catalog.names.is_empty());
        catalog.record_cursor("remaining").unwrap();
        assert!(catalog.record_cursor("remaining").is_err());
        assert!(catalog.record_cursor(&"x".repeat(1024)).is_err());
        assert_eq!(catalog.cursors.len(), 1);
    }

    #[tokio::test]
    async fn stdio_frame_reader_does_not_wait_for_newline_after_exceeding_the_limit() {
        use tokio::io::AsyncReadExt;

        let limit = 64 * 1024;
        let lifetime = CancellationToken::new();
        let frames = FrameLimit::new(limit, "stdio 单条消息", &lifetime);
        let reader = |inner| StdioFrameReader {
            inner,
            line_bytes: 0,
            frames: frames.clone(),
            _memory: McpMemory::new(),
        };
        let mut data = vec![b'x'; limit - 1];
        data.push(b'\n');
        data.extend_from_slice(b"{}\n");
        let mut read = Vec::new();
        reader(data.as_slice())
            .read_to_end(&mut read)
            .await
            .unwrap();
        assert_eq!(read.len(), limit + 3);
        assert!(!lifetime.is_cancelled());

        let data = vec![b'x'; limit + 1];
        assert_eq!(
            reader(data.as_slice())
                .read_to_end(&mut Vec::new())
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData,
        );
        assert!(lifetime.is_cancelled());
        assert!(frames.error().to_string().contains(&limit.to_string()));
    }

    #[test]
    fn transport_frames_adjust_to_available_memory_and_concurrent_reservations() {
        let pool = AtomicUsize::new(0);
        let budget = 16 * 1024 * 1024;
        let mut first = McpMemory::with_limit(budget, &pool);
        let mut concurrent = McpMemory::with_limit(budget, &pool);
        assert_eq!(
            first.frame_bytes(DEFAULT_MAX_LINE_LENGTH, 1).unwrap(),
            budget / 4
        );
        assert_eq!(
            concurrent.frame_bytes(DEFAULT_MAX_LINE_LENGTH, 1).unwrap(),
            budget / 8
        );
        drop(first);
        drop(concurrent);
        assert_eq!(pool.load(Relaxed), 0);
        let mut low = McpMemory::with_limit(1024, &pool);
        assert!(low.frame_bytes(DEFAULT_MAX_LINE_LENGTH, 1).is_err());
    }

    #[test]
    fn catalogue_can_grow_past_old_fixed_limit_but_contends_with_other_work() {
        let pool = AtomicUsize::new(0);
        let mib = 1024 * 1024;
        let mut catalog = ToolCatalog {
            tools: Vec::new(),
            names: HashSet::new(),
            cursors: HashSet::new(),
            memory: McpMemory::with_limit(96 * mib, &pool),
        };
        catalog.charge(65 * mib).unwrap();
        let mut concurrent = McpMemory::with_limit(96 * mib, &pool);
        concurrent.charge(30 * mib).unwrap();
        assert!(catalog.charge(2 * mib).is_err());
        drop(concurrent);
        catalog.charge(2 * mib).unwrap();
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
    async fn timeout_and_preexisting_cancellation_are_bounded() {
        let cancel = CancellationToken::new();
        let pending = std::future::pending::<Result<(), AppError>>();
        let error = bounded(&cancel, Some(Duration::from_millis(10)), pending)
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
            bounded(&cancel, None, operation).await,
            Err(AppError::Cancelled)
        ));
    }

    #[test]
    fn attachment_errors_report_the_actual_guard_without_remote_details() {
        assert!(matches!(
            attachment_call_error(AppError::Cancelled),
            AppError::Cancelled
        ));
        let error = attachment_call_error(
            FrameLimit::new(123456, "stdio 单条消息", &CancellationToken::new()).error(),
        )
        .to_string();
        assert!(error.contains("123456") && error.contains("不支持转换结果分页"));
        let error = attachment_call_error(AppError::Other("remote-secret".into())).to_string();
        assert!(!error.contains("remote-secret"));
    }
}
