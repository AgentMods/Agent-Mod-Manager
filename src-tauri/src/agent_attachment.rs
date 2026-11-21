//! Desktop-only conversion of bytes explicitly selected in the chat composer.
use crate::{
    agent::runtime::AgentRuntime,
    app::AppServices,
    error::{AppError, AppResult},
};
use serde_json::Value;
use std::{borrow::Cow, path::Path};
use tauri::{
    http::HeaderMap,
    ipc::{InvokeBody, Request},
    State,
};
use tempfile::TempPath;
use tokio::io::AsyncWriteExt;

const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
const MAX_NAME_CHARS: usize = 255;
const NAME_HEADER: &str = "x-attachment-name";

#[derive(Debug, serde::Serialize)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub(crate) enum AttachmentConversionError {
    ConsentRequired(String),
    ConversionFailed(String),
}

impl From<AppError> for AttachmentConversionError {
    fn from(error: AppError) -> Self {
        Self::ConversionFailed(error.to_string())
    }
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
    if size > MAX_ATTACHMENT_BYTES {
        return Err(AppError::Invalid(
            "单个附件上限为 10 MiB，请拆分文件后重试。".into(),
        ));
    }
    Ok(())
}

async fn stage_upload(directory: &Path, name: &str, bytes: &[u8]) -> std::io::Result<TempPath> {
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
    writer.write_all(bytes).await?;
    writer.flush().await?;
    Ok(path)
}

fn extract_markdown(result: Value) -> AppResult<String> {
    if result["isError"] == true {
        return Err(AppError::Invalid(
            "MarkItDown 未能转换此文件。请检查文件格式、是否损坏或加密，再重新选择附件。".into(),
        ));
    }
    let text = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.trim().is_empty() {
        return Err(AppError::Invalid(
            "MarkItDown 未提取到文本，请选择含可读取文字的文件后重试。".into(),
        ));
    }
    Ok(text)
}

#[tauri::command]
pub(crate) async fn convert_agent_attachment(
    state: State<'_, AppServices>,
    agent: State<'_, AgentRuntime>,
    request: Request<'_>,
) -> Result<String, AttachmentConversionError> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err(AppError::Invalid(
            "请通过二进制附件上传接口传入文件内容（ArrayBuffer）。".into(),
        )
        .into());
    };
    let name = decode_name(request.headers())?;
    validate_upload(&name, bytes.len())?;
    let client = state
        .mcp_connections
        .markitdown_client(&agent.settings, &agent.registry)
        .await
        .map_err(|error| match error {
            AppError::Denied(message) => AttachmentConversionError::ConsentRequired(message),
            other => other.into(),
        })?;
    let file = stage_upload(&std::env::temp_dir(), &name, bytes)
        .await
        .map_err(|_| {
            AppError::Other(
                "无法写入附件临时文件。请检查系统临时目录的权限和剩余空间后重试。".into(),
            )
        })?;
    let result = client.call_attachment_converter(&file).await;
    // TempPath also unlinks on an early drop. Report a failed explicit cleanup on normal return.
    file.close().map_err(|_| {
        AppError::Other("无法清理附件临时文件。请检查系统临时目录权限及文件占用后重试。".into())
    })?;
    Ok(extract_markdown(result?)?)
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
    fn filename_and_size_boundaries() {
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
        assert!(validate_upload("说明 #100%.docx", MAX_ATTACHMENT_BYTES).is_ok());
        assert!(validate_upload(&"文".repeat(MAX_NAME_CHARS), 1).is_ok());
        assert!(validate_upload("a.txt", 0).is_err());
        assert!(validate_upload("a.txt", MAX_ATTACHMENT_BYTES + 1).is_err());
    }

    #[tokio::test]
    async fn temporary_uploads_preserve_bytes_and_suffix_and_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let name = "私有说明 #%.docx";
        let bytes = [255, 0, 127, 13];
        let a = stage_upload(dir.path(), name, &bytes).await.unwrap();
        let b = stage_upload(dir.path(), name, b"different").await.unwrap();
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
    fn accepts_markdown_above_half_a_mebibyte_within_the_mcp_result_limit() {
        let text = "字".repeat(200_000);
        let result = json!({"content":[{"type":"text","text":text}]});
        assert!(text.len() > 512 * 1024);
        assert!(serde_json::to_vec(&result).unwrap().len() < crate::mcp::client::MAX_RESULT_BYTES);
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
            .invoke_handler(tauri::generate_handler![convert_agent_attachment])
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
        test::get_ipc_response(
            window,
            tauri::webview::InvokeRequest {
                cmd: "convert_agent_attachment".into(),
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
                invoke(&window, "a.txt", b"content"),
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
            invoke(&window, "a.txt", b"content"),
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
            invoke(&window, "a.txt", b"content"),
            "conversion_failed",
            "需要内置的 stdio",
        );
        servers.remove(MARKITDOWN_ID).unwrap();
        servers.create(&config).unwrap();
        assert_ipc_error(
            invoke(&window, "a.txt", b"content"),
            "conversion_failed",
            "需要内置",
        );
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
            invoke(&window, "a.txt", b"x"),
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
