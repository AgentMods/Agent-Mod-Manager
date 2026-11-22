//! Desktop-only conversion of bytes explicitly selected in the chat composer.
use crate::{
    agent::runtime::AgentRuntime,
    app::AppServices,
    error::{AppError, AppResult},
};
use serde_json::Value;
use std::{
    borrow::Cow,
    collections::{hash_map::Entry, HashMap},
    path::Path,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};
use tauri::{
    http::HeaderMap,
    ipc::{InvokeBody, Request},
    Runtime, State, WebviewWindow,
};
use tempfile::TempPath;
use tokio::{io::AsyncWriteExt, sync::Mutex as AsyncMutex};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const MAX_NAME_CHARS: usize = 255;
const NAME_HEADER: &str = "x-attachment-name";
const CONVERSION_HEADER: &str = "x-attachment-conversion-id";
// Only reservations that never submit bytes expire. There is no conversion deadline.
const RESERVATION_TTL: Duration = Duration::from_secs(60);
const MAX_REGISTERED_CONVERSIONS: usize = 64;

#[derive(Debug, serde::Serialize)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub(crate) enum AttachmentConversionError {
    ConsentRequired(String),
    Cancelled(String),
    ConversionFailed(String),
}

impl From<AppError> for AttachmentConversionError {
    fn from(error: AppError) -> Self {
        match error {
            AppError::Cancelled => Self::Cancelled("附件转换已取消。".into()),
            other => Self::ConversionFailed(other.to_string()),
        }
    }
}

type ConversionKey = (String, Uuid);

enum Conversion {
    Reserved(Instant),
    Running(CancellationToken),
}

#[derive(Default)]
pub(crate) struct AttachmentConversions {
    work: Mutex<HashMap<ConversionKey, Conversion>>,
    // Hold this through startup, use, and cleanup. A successor must not reuse
    // the MarkItDown process that its cancelled predecessor is still stopping.
    converter: AsyncMutex<()>,
}

impl AttachmentConversions {
    fn lock(&self) -> MutexGuard<'_, HashMap<ConversionKey, Conversion>> {
        self.work.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn register(&self, key: ConversionKey) -> AppResult<()> {
        let mut work = self.lock();
        work.retain(|_, entry| !matches!(entry, Conversion::Reserved(start) if start.elapsed() >= RESERVATION_TTL));
        if work.len() >= MAX_REGISTERED_CONVERSIONS {
            return Err(AppError::Conflict(
                "等待转换的附件过多，请移除不需要的附件后重试。".into(),
            ));
        }
        match work.entry(key) {
            Entry::Vacant(slot) => {
                slot.insert(Conversion::Reserved(Instant::now()));
                Ok(())
            }
            Entry::Occupied(_) => Err(AppError::Conflict("附件转换标识已使用。".into())),
        }
    }

    fn start(&self, key: ConversionKey) -> AppResult<ActiveConversion<'_>> {
        let mut work = self.lock();
        match work.entry(key.clone()) {
            Entry::Occupied(mut slot) => match slot.get() {
                Conversion::Reserved(start) if start.elapsed() < RESERVATION_TTL => {
                    let cancel = CancellationToken::new();
                    slot.insert(Conversion::Running(cancel.clone()));
                    Ok(ActiveConversion {
                        owner: self,
                        key,
                        cancel,
                    })
                }
                Conversion::Reserved(_) => {
                    slot.remove();
                    Err(AppError::Cancelled)
                }
                Conversion::Running(_) => Err(AppError::Conflict("此附件已提交转换。".into())),
            },
            Entry::Vacant(_) => Err(AppError::Cancelled),
        }
    }

    fn cancel(&self, key: ConversionKey) {
        if let Entry::Occupied(slot) = self.lock().entry(key) {
            match slot.get() {
                Conversion::Reserved(_) => {
                    slot.remove();
                }
                Conversion::Running(cancel) => cancel.cancel(),
            }
        }
    }

    pub(crate) fn cancel_window(&self, window: &str) {
        self.lock().retain(|(owner, _), entry| {
            if owner != window {
                return true;
            }
            match entry {
                Conversion::Reserved(_) => false,
                Conversion::Running(cancel) => {
                    cancel.cancel();
                    true
                }
            }
        });
    }

    async fn wait_for_converter(
        &self,
        cancel: &CancellationToken,
    ) -> AppResult<tokio::sync::MutexGuard<'_, ()>> {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(AppError::Cancelled),
            slot = self.converter.lock() => Ok(slot),
        }
    }
}

struct ActiveConversion<'a> {
    owner: &'a AttachmentConversions,
    key: ConversionKey,
    cancel: CancellationToken,
}

impl Drop for ActiveConversion<'_> {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.owner.lock().remove(&self.key);
    }
}

fn decode_conversion_id(raw: Option<&str>) -> AppResult<Uuid> {
    raw.filter(|id| id.len() == 36)
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or_else(|| AppError::Invalid("附件转换标识无效。".into()))
}

fn conversion_id_header(headers: &HeaderMap) -> AppResult<Option<Uuid>> {
    let mut values = headers.get_all(CONVERSION_HEADER).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(AppError::Invalid("附件转换标识无效。".into()));
    }
    Ok(Some(decode_conversion_id(value.to_str().ok())?))
}

fn check_cancelled(cancel: &CancellationToken) -> AppResult<()> {
    if cancel.is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(())
    }
}

#[tauri::command]
pub(crate) fn begin_agent_attachment<R: Runtime>(
    window: WebviewWindow<R>,
    conversions: State<'_, AttachmentConversions>,
    conversion_id: String,
) -> Result<(), AttachmentConversionError> {
    Ok(conversions.register((
        window.label().into(),
        decode_conversion_id(Some(&conversion_id))?,
    ))?)
}

#[tauri::command]
pub(crate) fn cancel_agent_attachment<R: Runtime>(
    window: WebviewWindow<R>,
    conversions: State<'_, AttachmentConversions>,
    conversion_id: String,
) -> Result<(), AttachmentConversionError> {
    conversions.cancel((
        window.label().into(),
        decode_conversion_id(Some(&conversion_id))?,
    ));
    Ok(())
}

fn decode_name(headers: &HeaderMap) -> AppResult<Cow<'_, str>> {
    let invalid = || {
        AppError::Invalid(
            "附件文件名编码无效，请重新选择文件（文件名需使用 encodeURIComponent 编码）。".into(),
        )
    };
    let mut names = headers.get_all(NAME_HEADER).iter();
    let encoded = names
        .next()
        .and_then(|h| h.to_str().ok())
        .ok_or_else(invalid)?;
    // One Unicode scalar can take four UTF-8 bytes, each encoded as %XX.
    if names.next().is_some() || encoded.len() > 3 * 4 * MAX_NAME_CHARS {
        return Err(invalid());
    }
    // percent_decode_str passes malformed escapes through. Require complete hex triplets first.
    let mut bytes = encoded.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%'
            && !(bytes.next().is_some_and(|c| c.is_ascii_hexdigit())
                && bytes.next().is_some_and(|c| c.is_ascii_hexdigit()))
        {
            return Err(invalid());
        }
    }
    percent_encoding::percent_decode_str(encoded)
        .decode_utf8()
        .map_err(|_| invalid())
}

fn validate_upload(name: &str, size: usize) -> AppResult<()> {
    if name.trim().is_empty()
        || name.chars().count() > MAX_NAME_CHARS
        || name.contains(['/', '\\'])
        || crate::paths::safe_join(Path::new(""), name).is_err()
    {
        return Err(AppError::Invalid(
            "附件名称无效。请选择单个普通文件，或重命名后重试（文件名最多 255 个字符）。".into(),
        ));
    }
    if size == 0 {
        return Err(AppError::Invalid("附件为空，请选择包含内容的文件。".into()));
    }
    Ok(())
}

/// Already textual formats need no MCP conversion, so their content never
/// travels through a bounded tool-result frame. Other encodings and structured
/// formats still go through the authorized converter, as a complete file.
fn utf8_text_attachment<'a>(name: &str, bytes: &'a [u8]) -> Option<&'a str> {
    let extension = Path::new(name).extension()?.to_str()?;
    if !["txt", "md", "markdown"]
        .iter()
        .any(|ext| extension.eq_ignore_ascii_case(ext))
    {
        return None;
    }
    let text = std::str::from_utf8(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)).ok()?;
    (!text.contains('\0')).then_some(text)
}

fn require_text(text: &str) -> AppResult<()> {
    if text.trim().is_empty() {
        return Err(AppError::Invalid(
            "附件未提取到文本，请选择含可读取文字的文件后重试。".into(),
        ));
    }
    Ok(())
}

async fn stage_upload(
    directory: &Path,
    name: &str,
    bytes: &[u8],
    cancel: &CancellationToken,
) -> AppResult<TempPath> {
    check_cancelled(cancel)?;
    // MarkItDown uses the extension as a format hint. No user path or stem reaches the server.
    let suffix = Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .filter(|s| s.len() <= 16 && s.bytes().all(|c| c.is_ascii_alphanumeric()))
        .map(|s| format!(".{s}"))
        .unwrap_or_default();
    let file = tempfile::Builder::new()
        .prefix("amm-attachment-")
        .suffix(&suffix)
        .tempfile_in(directory)?;
    let (handle, path) = file.into_parts();
    let mut writer = tokio::fs::File::from_std(handle);
    // Do not drop an in-flight OS write and try to unlink the still-open file.
    for chunk in bytes.chunks(128 * 1024) {
        check_cancelled(cancel)?;
        writer.write_all(chunk).await?;
    }
    writer.flush().await?;
    check_cancelled(cancel)?;
    Ok(path)
}

fn extract_markdown(mut result: Value) -> AppResult<String> {
    if result["isError"] == true {
        return Err(AppError::Invalid(
            "MarkItDown 未能转换此文件。请检查文件格式、是否损坏或加密，再重新选择附件。".into(),
        ));
    }
    let mut blocks = result
        .get_mut("content")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| match block["text"].take() {
            Value::String(text) => Some(text),
            _ => None,
        });
    // A typical converter returns one text block; move it rather than allocate
    // another markdown-sized copy. Preserve spacing when joining several blocks.
    let mut text = blocks.next().unwrap_or_default();
    for block in blocks {
        text.try_reserve(block.len().saturating_add(2))
            .map_err(|_| {
                AppError::Other("内存不足，无法合并附件文本。请拆分源文档后重试。".into())
            })?;
        text.push_str("\n\n");
        text.push_str(&block);
    }
    require_text(&text)?;
    Ok(text)
}

#[tauri::command]
pub(crate) async fn convert_agent_attachment<R: Runtime>(
    window: WebviewWindow<R>,
    conversions: State<'_, AttachmentConversions>,
    state: State<'_, AppServices>,
    agent: State<'_, AgentRuntime>,
    request: Request<'_>,
) -> Result<String, AttachmentConversionError> {
    // Historical clients send only the filename header. For clients with an ID
    // an absent reservation means cancellation (even if upload arrives later).
    let id = match conversion_id_header(request.headers())? {
        Some(id) => id,
        None => {
            let id = Uuid::new_v4();
            conversions.register((window.label().into(), id))?;
            id
        }
    };
    let active = conversions.start((window.label().into(), id))?;
    let cancel = &active.cancel;
    check_cancelled(cancel)?;
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err(AppError::Invalid(
            "请通过二进制附件上传接口传入文件内容（ArrayBuffer）。".into(),
        )
        .into());
    };
    let name = decode_name(request.headers())?;
    validate_upload(&name, bytes.len())?;
    if let Some(text) = utf8_text_attachment(&name, bytes) {
        require_text(text)?;
        let text = text.to_owned();
        check_cancelled(cancel)?;
        return Ok(text);
    }
    let _slot = conversions.wait_for_converter(cancel).await?;
    let client = state
        .mcp_connections
        .markitdown_client(&agent.settings, &agent.registry, cancel)
        .await
        .map_err(|error| match error {
            AppError::Denied(message) => AttachmentConversionError::ConsentRequired(message),
            other => other.into(),
        })?;
    let file = stage_upload(&std::env::temp_dir(), &name, bytes, cancel)
        .await
        .map_err(|error| match error {
            AppError::Cancelled => AppError::Cancelled,
            _ => AppError::Other(
                "无法写入附件临时文件。请检查系统临时目录的权限和剩余空间后重试。".into(),
            ),
        })?;
    let result = client.call_attachment_converter(&file, cancel).await;
    // TempPath also unlinks on an early drop. Report a failed explicit cleanup on normal return.
    let cleanup = file.close();
    if result.is_err() && cleanup.is_err() {
        tracing::warn!("无法清理转换失败后的附件临时文件");
    }
    // A locked file on shutdown must not mask the actionable conversion error.
    let result = result?;
    cleanup.map_err(|_| {
        AppError::Other("无法清理附件临时文件。请检查系统临时目录权限及文件占用后重试。".into())
    })?;
    let markdown = extract_markdown(result)?;
    check_cancelled(cancel)?;
    Ok(markdown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        mcp::{config::MARKITDOWN_ID, McpRuntime, McpServers},
        settings::{SecretStore, Settings},
    };
    use serde_json::json;
    use std::sync::Arc;
    use tauri::{test, Manager};

    #[test]
    fn filename_and_empty_upload_validation() {
        for bad in [
            "",
            ".",
            "..",
            "a/b.txt",
            r"..\bad.txt",
            r"C:\file",
            "/etc/passwd",
            "bad\0.txt",
            "x:stream",
            "NUL.md",
            "foo. ",
            "\u{3000}",
            "https://example.com",
            &"a".repeat(256),
        ] {
            assert!(validate_upload(bad, 1).is_err(), "{bad:?}");
        }
        assert!(validate_upload("说明 #100%.docx", 10 * 1024 * 1024 + 1).is_ok());
        assert!(validate_upload(&"文".repeat(MAX_NAME_CHARS), 1).is_ok());
        assert!(validate_upload("a.txt", 0).is_err());
    }

    #[test]
    fn only_utf8_plain_text_and_markdown_skip_conversion() {
        let bytes = "\u{feff}# 表格\r\n  说明\n".as_bytes();
        for file in ["a.txt", "a.MD", "a.MarkDown"] {
            assert_eq!(
                utf8_text_attachment(file, bytes),
                Some("# 表格\r\n  说明\n")
            );
        }
        assert_eq!(utf8_text_attachment("a.html", bytes), None);
        assert_eq!(utf8_text_attachment("a.txt", b"\xff\xfeA\0"), None);
        assert_eq!(utf8_text_attachment("a.md", b"A\0B"), None);
    }

    #[tokio::test]
    async fn temporary_uploads_preserve_bytes_and_suffix_and_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let name = "私有说明 #%.docx";
        let bytes = [255, 0, 127, 13];
        let a = stage_upload(dir.path(), name, &bytes, &CancellationToken::new())
            .await
            .unwrap();
        let b = stage_upload(dir.path(), name, b"different", &CancellationToken::new())
            .await
            .unwrap();
        assert_ne!(&*a, &*b);
        assert_eq!(a.parent(), Some(dir.path()));
        assert_eq!(a.extension().unwrap(), "docx");
        assert!(!a.file_name().unwrap().to_string_lossy().contains("私有"));
        assert_eq!(std::fs::read(&a).unwrap(), bytes);
        a.close().unwrap();
        drop(b);
        assert_eq!(dir.path().read_dir().unwrap().count(), 0);
    }

    #[test]
    fn keeps_markdown_formatting_and_never_returns_tool_errors_or_resource_links() {
        let content = json!([{"type":"text","text":"    code\n"}, {"type":"resource_link","uri":"file:///other"}, {"type":"text","text":"**说明**"}]);
        assert_eq!(
            extract_markdown(json!({"content":content})).unwrap(),
            "    code\n\n\n**说明**"
        );
        let error = extract_markdown(
            json!({"isError":true,"content":[{"type":"text","text":"private traceback"}]}),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("文件格式") && !error.contains("traceback"));
        for result in [
            json!({}),
            json!({"content":[{"type":"text","text":" \n"}]}),
            json!({"content":[{"type":"resource_link","uri":"file:///other"}]}),
        ] {
            assert!(extract_markdown(result).is_err());
        }
    }

    #[test]
    fn decodes_one_utf8_filename_header_and_rejects_malformed_inputs() {
        let mut headers = HeaderMap::new();
        assert!(decode_name(&headers).is_err());
        for bad in [
            "%",
            "%A",
            "%AG",
            "%x0",
            "%FF",
            "%C0%AF",
            &"%41".repeat(4 * MAX_NAME_CHARS + 1),
        ] {
            headers.insert(NAME_HEADER, bad.parse().unwrap());
            assert!(decode_name(&headers).is_err(), "{bad}");
        }
        headers.insert(
            NAME_HEADER,
            "%E8%AF%B4%E6%98%8E%20%23100%25%2B%252F.html"
                .parse()
                .unwrap(),
        );
        assert_eq!(decode_name(&headers).unwrap(), "说明 #100%+%2F.html");
        headers.append(NAME_HEADER, "another.txt".parse().unwrap());
        assert!(decode_name(&headers).is_err());
    }

    #[test]
    fn extracts_markdown_above_the_former_mcp_result_limit() {
        let text = "字".repeat(800_000);
        assert!(text.len() > 2 * 1024 * 1024);
        let result = json!({"content":[{"type":"text","text":text}]});
        assert_eq!(extract_markdown(result).unwrap(), text);
    }

    struct NoSecrets;
    impl SecretStore for NoSecrets {
        fn read(&self, _: &str) -> AppResult<Option<String>> {
            panic!("preset requested a credential")
        }
        fn write(&self, _: &str, _: &str) -> AppResult<()> {
            panic!("unexpected credential write")
        }
    }

    fn desktop(dir: &Path, archive: std::path::PathBuf) -> tauri::App<test::MockRuntime> {
        let runtime =
            McpRuntime::bundled(archive, dir.canonicalize().unwrap().join("runtime-cache"));
        let state = AppServices::initialize_paths(
            crate::storage::AppPaths::new(dir.join("data")).unwrap(),
            runtime,
        )
        .unwrap();
        let settings = Settings::with_secret_store(state.engine.db.clone(), Arc::new(NoSecrets));
        let agent = AgentRuntime::new(state.engine.clone(), settings, Arc::new(|_| {})).unwrap();
        test::mock_builder()
            .manage(state)
            .manage(agent)
            .manage(AttachmentConversions::default())
            .invoke_handler(tauri::generate_handler![
                begin_agent_attachment,
                convert_agent_attachment,
                cancel_agent_attachment,
            ])
            .build(test::mock_context(test::noop_assets()))
            .unwrap()
    }

    fn invoke(
        window: &tauri::WebviewWindow<test::MockRuntime>,
        encoded_name: &str,
        bytes: &[u8],
    ) -> Result<String, Value> {
        let mut headers = HeaderMap::new();
        headers.insert(NAME_HEADER, encoded_name.parse().unwrap());
        request(window, headers, InvokeBody::Raw(bytes.to_vec()))
    }

    fn request(
        window: &tauri::WebviewWindow<test::MockRuntime>,
        headers: HeaderMap,
        body: InvokeBody,
    ) -> Result<String, Value> {
        command(window, "convert_agent_attachment", headers, body)
    }

    fn command<T: serde::de::DeserializeOwned>(
        window: &tauri::WebviewWindow<test::MockRuntime>,
        name: &str,
        headers: HeaderMap,
        body: InvokeBody,
    ) -> Result<T, Value> {
        test::get_ipc_response(
            window,
            tauri::webview::InvokeRequest {
                cmd: name.into(),
                callback: tauri::ipc::CallbackFn(1),
                error: tauri::ipc::CallbackFn(2),
                url: (if cfg!(windows) {
                    "http://tauri.localhost"
                } else {
                    "tauri://localhost"
                })
                .parse()
                .unwrap(),
                body,
                headers,
                invoke_key: test::INVOKE_KEY.into(),
            },
        )
        .map(|r| r.deserialize().unwrap())
    }

    fn assert_ipc_error(result: Result<String, Value>, code: &str, message: &str) {
        let error = result.unwrap_err();
        assert_eq!(error["code"], code, "{error}");
        assert!(
            error["message"].as_str().unwrap().contains(message),
            "{error}"
        );
    }

    #[test]
    fn cancelling_a_reservation_prevents_late_uploads_and_does_not_affect_other_windows() {
        let dir = tempfile::tempdir().unwrap();
        let app = desktop(dir.path(), dir.path().join("unused-runtime.zip"));
        let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let other = tauri::WebviewWindowBuilder::new(&app, "other", Default::default())
            .build()
            .unwrap();
        let id = Uuid::new_v4().to_string();
        let call = |window, name| {
            command::<()>(
                window,
                name,
                HeaderMap::new(),
                InvokeBody::Json(json!({"conversionId": id})),
            )
        };
        let upload = |window| {
            let mut headers = HeaderMap::new();
            headers.insert(NAME_HEADER, "plain.md".parse().unwrap());
            headers.insert(CONVERSION_HEADER, id.parse().unwrap());
            request(
                window,
                headers,
                InvokeBody::Raw(b"plain legacy-compatible text".to_vec()),
            )
        };
        call(&main, "begin_agent_attachment").unwrap();
        call(&other, "begin_agent_attachment").unwrap();
        call(&main, "cancel_agent_attachment").unwrap();
        call(&main, "cancel_agent_attachment").unwrap();
        assert_ipc_error(upload(&main), "cancelled", "已取消");
        assert_eq!(upload(&other).unwrap(), "plain legacy-compatible text");
        assert_ipc_error(upload(&other), "cancelled", "已取消");
        assert_eq!(invoke(&main, "legacy.txt", b"legacy").unwrap(), "legacy");
    }

    #[tokio::test]
    async fn queued_conversions_and_a_destroyed_window_cancel_only_their_owned_work() {
        let conversions = AttachmentConversions::default();
        let active_key = ("active".into(), Uuid::new_v4());
        let queued_key = ("queued".into(), Uuid::new_v4());
        conversions.register(active_key.clone()).unwrap();
        conversions.register(queued_key.clone()).unwrap();
        let active = conversions.start(active_key.clone()).unwrap();
        let queued = conversions.start(queued_key).unwrap();
        let slot = conversions
            .wait_for_converter(&active.cancel)
            .await
            .unwrap();
        let waiting = conversions.wait_for_converter(&queued.cancel);
        tokio::pin!(waiting);
        assert!(futures_util::poll!(&mut waiting).is_pending());
        conversions.cancel_window("queued");
        assert!(matches!(waiting.await, Err(AppError::Cancelled)));
        assert!(!active.cancel.is_cancelled());
        conversions.cancel(active_key);
        assert!(active.cancel.is_cancelled());
        drop(slot);
    }

    #[test]
    fn ipc_needs_exact_consent_and_rejects_a_custom_server_with_the_same_id() {
        let dir = tempfile::tempdir().unwrap();
        let app = desktop(dir.path(), dir.path().join("unused-runtime.zip"));
        let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let servers = McpServers::new(app.state::<AppServices>().engine.db.clone());
        let legacy_body = InvokeBody::Json(json!({"name":"a.txt","bytes":[1]}));
        assert_ipc_error(
            request(&window, HeaderMap::new(), legacy_body),
            "conversion_failed",
            "ArrayBuffer",
        );
        assert_ipc_error(
            invoke(&window, "..%2Fbad.txt", b"x"),
            "conversion_failed",
            "附件名称无效",
        );
        for enabled in [false, true] {
            servers.set_enabled(MARKITDOWN_ID, enabled).unwrap();
            assert_ipc_error(
                invoke(&window, "a.html", b"content"),
                "consent_required",
                "确认 stdio",
            );
            assert!(servers.startup_requests().unwrap().is_empty());
            assert!(!dir.path().join("runtime-cache").exists());
        }
        // Editing even a confirmed preset invalidates its saved launch consent.
        let mut config = servers.get(MARKITDOWN_ID).unwrap();
        servers.remember_start(&config).unwrap();
        config.name.push_str(" (edited)");
        servers.update(&config).unwrap();
        assert_ipc_error(
            invoke(&window, "a.html", b"content"),
            "consent_required",
            "确认 stdio",
        );
        let mut remote =
            crate::mcp::ServerConfig::http(MARKITDOWN_ID, "remote", "http://127.0.0.1:1234/mcp");
        remote.enabled = true;
        servers.update(&remote).unwrap();
        servers
            .remember_start(&servers.get(MARKITDOWN_ID).unwrap())
            .unwrap();
        assert_ipc_error(
            invoke(&window, "a.html", b"content"),
            "conversion_failed",
            "需要内置的 stdio",
        );
        servers.remove(MARKITDOWN_ID).unwrap();
        servers.create(&config).unwrap();
        assert_ipc_error(
            invoke(&window, "a.html", b"content"),
            "conversion_failed",
            "需要内置",
        );
    }

    #[test]
    fn large_utf8_attachments_use_raw_ipc_without_a_converter() {
        let dir = tempfile::tempdir().unwrap();
        let app = desktop(dir.path(), dir.path().join("missing.zip"));
        let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let text = format!("# 大文件\r\n{}\n末尾", "x".repeat(10 * 1024 * 1024));
        assert_eq!(invoke(&window, "big.MD", text.as_bytes()).unwrap(), text);
        assert_ipc_error(
            invoke(&window, "blank.md", "\u{feff} \r\n".as_bytes()),
            "conversion_failed",
            "未提取到文本",
        );
        assert!(!dir.path().join("runtime-cache").exists());
    }

    #[test]
    fn denied_runtime_after_consent_is_a_conversion_failure() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("runtime.zip");
        std::fs::write(&archive, b"fixture").unwrap();
        let cache = dir.path().canonicalize().unwrap().join("runtime-cache");
        crate::encrypted_store::private_directory(&cache).unwrap();
        // A regular file in the digest slot exercises the runtime's real permission denial.
        std::fs::write(
            cache.join(crate::util::sha256_file(&archive).unwrap()),
            b"blocked",
        )
        .unwrap();
        let app = desktop(dir.path(), archive);
        let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let servers = McpServers::new(app.state::<AppServices>().engine.db.clone());
        servers.set_enabled(MARKITDOWN_ID, true).unwrap();
        servers
            .remember_start(&servers.get(MARKITDOWN_ID).unwrap())
            .unwrap();
        assert_ipc_error(
            invoke(&window, "a.html", b"x"),
            "conversion_failed",
            "缓存被重定向",
        );
    }

    #[test]
    #[ignore = "requires the bundled runtime and MarkItDown dependencies or network"]
    fn live_bundled_markitdown_converts_html_over_desktop_ipc() {
        let dir = tempfile::tempdir().unwrap();
        let app = desktop(
            dir.path(),
            Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/mcp-runtime/runtime.zip"),
        );
        let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let state = app.state::<AppServices>();
        let agent = app.state::<AgentRuntime>();
        let servers = McpServers::new(state.engine.db.clone());
        servers.set_enabled(MARKITDOWN_ID, true).unwrap();
        let config = servers.get(MARKITDOWN_ID).unwrap();
        tauri::async_runtime::block_on(state.mcp_connections.connect(
            agent.settings.clone(),
            &agent.registry,
            config,
            true,
        ))
        .unwrap();
        let html = b"<html><body><h1>Attachment check</h1><p>Read <strong>this</strong>.</p></body></html>";
        let convert = || {
            let result = invoke(&window, "%E8%AF%B4%E6%98%8E%20%23100%25.html", html).unwrap();
            assert!(
                result.contains("# Attachment check") && result.contains("**this**"),
                "{result}"
            );
        };
        convert();
        assert_ipc_error(
            invoke(&window, "empty.html", b"<html><body></body></html>"),
            "conversion_failed",
            "未提取到文本",
        );
        // Simulate app shutdown without revoking the saved consent. Exercise lazy restore via IPC.
        tauri::async_runtime::block_on(state.mcp_connections.revoke(
            &agent.registry,
            MARKITDOWN_ID,
            || Ok(()),
        ))
        .unwrap();
        std::thread::scope(|scope| {
            scope.spawn(convert);
            convert();
        });
        assert_eq!(
            tauri::async_runtime::block_on(state.mcp_connections.list()).len(),
            1
        );
        tauri::async_runtime::block_on(state.mcp_connections.disconnect(
            &agent.registry,
            &servers,
            MARKITDOWN_ID,
        ))
        .unwrap();
        assert_ipc_error(
            invoke(&window, "a.html", html),
            "consent_required",
            "确认 stdio",
        );
    }
}
