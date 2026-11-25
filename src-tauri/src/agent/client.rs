use super::sse::{
    is_context_length_message, is_provider_context_length_error, provider_error_envelope,
    Completion, Delta, ResponseBuffer, ResponseLimit, SseDecoder,
};
use crate::{
    error::{AppError, AppResult},
    settings::{parse_model_base_url, ReasoningEffort, Settings, MODEL_API_KEY},
};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::Value;
use std::{error::Error, io, time::Duration};
use tokio_util::sync::CancellationToken;

const MAX_ERROR_BYTES: usize = 64 * 1024;
const INTERRUPTION_FALLBACK_THRESHOLD: u8 = 2;
const RETRY_BASE: Duration = Duration::from_millis(if cfg!(test) { 10 } else { 1000 });
const RETRY_MAX: Duration = Duration::from_millis(if cfg!(test) { 40 } else { 30_000 });
const ERROR_BODY_GRACE: Duration = Duration::from_millis(if cfg!(test) { 100 } else { 2000 });

pub struct ModelConfig {
    pub base_url: String,
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub api_key: Option<String>,
}
impl ModelConfig {
    pub(crate) fn configured(settings: &Settings) -> AppResult<Option<Self>> {
        let saved = settings.load()?;
        let last = settings.load_composer_preferences()?;
        let selected = saved
            .models
            .iter()
            .find(|m| Some(&m.id) == last.model.as_ref());
        let model = selected.map_or(saved.model, |m| m.id.clone());
        if model.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            model,
            reasoning_effort: last
                .reasoning_effort
                .filter(|level| selected.is_some_and(|m| m.reasoning_levels.contains(level))),
            base_url: saved.base_url,
            api_key: settings.read_secret(saved.api_key_ref.as_deref().unwrap_or(MODEL_API_KEY))?,
        }))
    }
}

#[derive(Serialize)]
struct CompletionRequest<'a> {
    model: &'a str,
    // Borrow the messages so building the request does not copy the data URLs.
    messages: &'a [Value],
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<ReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [Value]>,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

pub struct ModelClient {
    http: reqwest::Client,
}
impl ModelClient {
    pub fn new() -> AppResult<Self> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self { http })
    }
    /// Run an optional, tool-free task with one SSE attempt. Unlike the
    /// interactive Agent, a failed background task does not retry or log the
    /// provider error; its caller decides how to fall back.
    pub(crate) async fn stream_once(
        &self,
        config: &ModelConfig,
        messages: &[Value],
        cancel: &CancellationToken,
        mut on_delta: impl FnMut(&Delta, &Completion) -> AppResult<()>,
    ) -> AppResult<Completion> {
        parse_model_base_url(&config.base_url)?;
        if config.model.trim().is_empty() {
            return Err(AppError::Invalid("请先在设置中选择模型".into()));
        }
        let completion = self
            .complete_once(config, messages, &[], true, cancel, &mut on_delta)
            .await
            .map_err(|failure| match failure {
                StreamFailure::Transient(error)
                | StreamFailure::Interrupted(error)
                | StreamFailure::Fatal(error) => error,
            })?;
        // complete_once validates the finish reason and consistency of the tool
        // list. A valid tool-call completion still isn't valid for this task.
        if completion.finish_reason.as_deref() != Some("stop") {
            return Err(AppError::Invalid("此模型请求不允许工具调用".into()));
        }
        Ok(completion)
    }

    pub async fn stream(
        &self,
        config: &ModelConfig,
        messages: &[Value],
        tools: &[Value],
        cancel: &CancellationToken,
        mut on_delta: impl FnMut(&Delta, &Completion) -> AppResult<()>,
        mut on_reset: impl FnMut() -> AppResult<()>,
    ) -> AppResult<Completion> {
        parse_model_base_url(&config.base_url)?;
        if config.model.trim().is_empty() {
            return Err(AppError::Invalid("请先在设置中选择模型".into()));
        }
        let mut attempt = 0_u64;
        let mut delay = RETRY_BASE;
        let mut interrupted_streams = 0_u8;
        let mut has_visible_reply = false;
        loop {
            // Change representation only if both prior SSE bodies were interrupted, not
            // for authentication, provider/protocol errors or failures to connect.
            let streaming = interrupted_streams < INTERRUPTION_FALLBACK_THRESHOLD;
            // Keep the last visible reply through retries that never produce output.
            let mut replace_pending = has_visible_reply;
            let result = self
                .complete_once(
                    config,
                    messages,
                    tools,
                    streaming,
                    cancel,
                    &mut |delta, completion| {
                        if !delta.content.is_empty() || !delta.reasoning.is_empty() {
                            if replace_pending {
                                on_reset()?;
                                replace_pending = false;
                            }
                            has_visible_reply = true;
                        }
                        on_delta(delta, completion)
                    },
                )
                .await;
            match result {
                Ok(completion) => {
                    // A validated reply may contain only tool calls.
                    if replace_pending {
                        on_reset()?;
                    }
                    return Ok(completion);
                }
                Err(failure) => {
                    let error = match failure {
                        StreamFailure::Interrupted(error) => {
                            if streaming {
                                interrupted_streams += 1;
                            }
                            error
                        }
                        StreamFailure::Transient(error) => error,
                        StreamFailure::Fatal(error) => return Err(error),
                    };
                    attempt = attempt.saturating_add(1);
                    tracing::warn!(?delay, "模型连接中断，第 {attempt} 次重试: {error}");
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return Err(AppError::Cancelled),
                        _ = tokio::time::sleep(delay) => {}
                    }
                    delay = delay.saturating_mul(2).min(RETRY_MAX);
                }
            }
        }
    }

    async fn complete_once(
        &self,
        config: &ModelConfig,
        messages: &[Value],
        tools: &[Value],
        streaming: bool,
        cancel: &CancellationToken,
        on_delta: &mut impl FnMut(&Delta, &Completion) -> AppResult<()>,
    ) -> Result<Completion, StreamFailure> {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled.into());
        }
        let endpoint = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
        let body = CompletionRequest {
            model: &config.model,
            messages,
            stream: streaming,
            stream_options: streaming.then_some(StreamOptions {
                include_usage: true,
            }),
            reasoning_effort: config.reasoning_effort,
            tools: (!tools.is_empty()).then_some(tools),
        };
        let accept = if streaming {
            "text/event-stream"
        } else {
            "application/json"
        };
        let mut request = self
            .http
            .post(&endpoint)
            .header("Accept", accept)
            .json(&body);
        if let Some(key) = &config.api_key {
            request = request.bearer_auth(key);
        }
        let api_key = config.api_key.as_deref().filter(|key| !key.is_empty());
        let diagnostic = |details: String| {
            if streaming {
                details
            } else {
                format!("模型流连续中断，非流式请求未完成。\n{details}")
            }
        };
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AppError::Cancelled.into()),
            result = request.send() => result.map_err(|error| {
                let transient = is_transient_transport(&error);
                let details = diagnostic(transport_details("无法连接模型服务", error));
                let error = model_error(details, api_key, false);
                if transient { StreamFailure::Transient(error) } else { StreamFailure::Fatal(error) }
            })?,
        };
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|header| header.to_str().ok());
        let failure = if !response.status().is_success() {
            Some(format!("模型服务请求失败 (HTTP {status})"))
        } else if !content_type.is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case(accept)
        }) {
            let content_type = content_type.unwrap_or("缺失");
            let expected = if streaming { "SSE 流" } else { "JSON 响应" };
            Some(format!(
                "模型服务未返回 {expected} (HTTP {status}, Content-Type = {content_type})"
            ))
        } else {
            None
        };
        if let Some(summary) = failure {
            return Err(
                response_error(response, diagnostic(summary), api_key, cancel)
                    .await
                    .into(),
            );
        }
        let limit = ResponseLimit::default();
        if !streaming {
            let completion = read_json_completion(response, cancel, limit)
                .await
                .map_err(|failure| {
                    failure.map_error(|error| completion_error(error, api_key, diagnostic))
                })?;
            let delta = completion.buffered_delta()?;
            if cancel.is_cancelled() {
                return Err(AppError::Cancelled.into());
            }
            on_delta(&delta, &completion)?;
            return Ok(completion);
        }
        let mut stream = response.bytes_stream();
        let mut decoder = SseDecoder::new(limit);
        let mut completion = Completion::new(limit);
        while let Some(chunk) = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AppError::Cancelled.into()),
            value = stream.next() => value,
        } {
            let chunk = chunk.map_err(|error| {
                body_failure(error, "模型流连接中断")
                    .map_error(|error| completion_error(error, api_key, std::convert::identity))
            })?;
            decoder.push(&chunk, |frame| {
                if cancel.is_cancelled() {
                    return Err(AppError::Cancelled);
                }
                let delta = completion
                    .accept(frame)
                    .map_err(|error| completion_error(error, api_key, std::convert::identity))?;
                on_delta(&delta, &completion)?;
                Ok(completion.done)
            })?;
            if completion.done {
                drop(decoder);
                if cancel.is_cancelled() {
                    return Err(AppError::Cancelled.into());
                }
                completion
                    .validate()
                    .map_err(|error| completion_error(error, api_key, std::convert::identity))?;
                return Ok(completion);
            }
        }
        Err(StreamFailure::Interrupted(AppError::Other(
            "模型流提前结束，未执行工具调用".into(),
        )))
    }
}

enum StreamFailure {
    Transient(AppError),
    Interrupted(AppError),
    Fatal(AppError),
}
impl StreamFailure {
    fn map_error(self, map: impl FnOnce(AppError) -> AppError) -> Self {
        match self {
            Self::Transient(error) => Self::Transient(map(error)),
            Self::Interrupted(error) => Self::Interrupted(map(error)),
            Self::Fatal(error) => Self::Fatal(map(error)),
        }
    }
}
impl From<AppError> for StreamFailure {
    fn from(error: AppError) -> Self {
        Self::Fatal(error)
    }
}

fn is_transient_transport(error: &reqwest::Error) -> bool {
    if error.is_builder() || error.is_redirect() {
        return false;
    }
    if error.is_timeout() {
        return true;
    }
    let mut sources = error.source();
    while let Some(source) = sources {
        if let Some(io) = source.downcast_ref::<io::Error>() {
            if matches!(
                io.kind(),
                io::ErrorKind::TimedOut
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::NotConnected
                    | io::ErrorKind::BrokenPipe
                    | io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::NetworkUnreachable
                    | io::ErrorKind::HostUnreachable
                    | io::ErrorKind::NetworkDown
            ) || cfg!(windows) && io.raw_os_error() == Some(11002)
            // WSATRY_AGAIN
            {
                return true;
            }
        }
        sources = source.source();
        // reqwest doesn't expose Hyper's typed pre-header EOF. Match that
        // exact cause, not all connect failures (e.g. invalid TLS or DNS).
        if source.to_string() == "connection closed before message completed" {
            return true;
        }
    }
    false
}

fn body_failure(error: reqwest::Error, summary: &str) -> StreamFailure {
    let transient = is_transient_transport(&error);
    let error = AppError::Other(transport_details(summary, error));
    if transient {
        StreamFailure::Interrupted(error)
    } else {
        StreamFailure::Fatal(error)
    }
}

/// JSON has no SSE terminator: require the whole HTTP body even when the bytes received
/// before a transport error happen to look like a complete JSON object.
async fn read_json_completion(
    response: reqwest::Response,
    cancel: &CancellationToken,
    limit: ResponseLimit,
) -> Result<Completion, StreamFailure> {
    let mut body = ResponseBuffer::new(limit);
    let mut stream = response.bytes_stream();
    while let Some(result) = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(AppError::Cancelled.into()),
        result = stream.next() => result,
    } {
        let chunk = result.map_err(|error| body_failure(error, "读取模型响应失败"))?;
        body.extend(&chunk)?;
    }
    Ok(Completion::from_buffered_json(body.bytes(), limit)?)
}

async fn response_error(
    response: reqwest::Response,
    mut details: String,
    api_key: Option<&str>,
    cancel: &CancellationToken,
) -> AppError {
    let status = response.status();
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    let mut truncated = false;
    let mut complete = true;
    // The status or mismatched content type is already terminal. Optional
    // diagnostics must not hold it open if the server never closes this body.
    let deadline = tokio::time::sleep(ERROR_BODY_GRACE);
    tokio::pin!(deadline);
    while let Some(result) = tokio::select! {
        biased;
        _ = cancel.cancelled() => return AppError::Cancelled,
        _ = &mut deadline => {
            complete = false;
            None
        }
        result = stream.next() => result,
    } {
        let chunk = match result {
            Ok(chunk) => chunk,
            Err(error) => {
                details.push('\n');
                details.push_str(&transport_details("读取错误响应失败", error));
                complete = false;
                break;
            }
        };
        let remaining = MAX_ERROR_BYTES - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if chunk.len() > remaining {
            truncated = true;
            break;
        }
    }
    let json = serde_json::from_slice::<Value>(&body);
    let context_length = complete
        && !truncated
        && match (status.as_u16(), &json) {
            (400 | 413, Ok(value)) => {
                provider_error_envelope(value).is_some_and(is_provider_context_length_error)
                    || (value.get("error").is_none_or(Value::is_null)
                        && value.get("choices").is_none_or(Value::is_null)
                        && is_provider_context_length_error(value))
            }
            (400 | 413, Err(_)) => std::str::from_utf8(&body).is_ok_and(is_context_length_message),
            // Some servers return a JSON error envelope instead of the requested SSE stream.
            (_, Ok(value)) if status.is_success() => {
                provider_error_envelope(value).is_some_and(is_provider_context_length_error)
            }
            _ => false,
        };
    if !body.is_empty() {
        details.push('\n');
        match json {
            Ok(value) => details.push_str(&format!("{value:#}")),
            Err(_) => details.push_str(&String::from_utf8_lossy(&body)),
        }
    }
    if context_length {
        AppError::ContextLength(redact_model_error(details, api_key, false))
    } else {
        model_error(details, api_key, truncated)
    }
}

fn transport_details(summary: &str, error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut details = format!("{summary}\n{error}");
    let mut cause = error.source();
    while let Some(error) = cause {
        details.push('\n');
        details.push_str(&error.to_string());
        cause = error.source();
    }
    details
}

fn completion_error(
    error: AppError,
    api_key: Option<&str>,
    diagnostic: impl FnOnce(String) -> String,
) -> AppError {
    match error {
        AppError::Cancelled => AppError::Cancelled,
        AppError::ContextLength(details) => {
            AppError::ContextLength(redact_model_error(diagnostic(details), api_key, false))
        }
        error => model_error(diagnostic(error.to_string()), api_key, false),
    }
}

fn model_error(details: String, api_key: Option<&str>, truncated: bool) -> AppError {
    AppError::Other(redact_model_error(details, api_key, truncated))
}

fn redact_model_error(mut details: String, api_key: Option<&str>, truncated: bool) -> String {
    if let Some(key) = api_key {
        let encoded = Value::String(key.to_owned()).to_string();
        let escaped = &encoded[1..encoded.len() - 1];
        details = details.replace(escaped, "[REDACTED]");
        if escaped != key {
            details = details.replace(key, "[REDACTED]");
        }
    }
    if truncated || details.len() > MAX_ERROR_BYTES {
        let mut end = details.len().min(MAX_ERROR_BYTES);
        while !details.is_char_boundary(end) {
            end -= 1;
        }
        details.truncate(end);
        details.push_str("\n（错误详情超过 64 KiB，已截断）");
    }
    details
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Response;
    use futures_util::stream;
    use serde_json::json;
    use std::{
        io,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
    };

    const KEY: &str = r#"sk-offline-"context"\test"#;

    #[tokio::test]
    async fn http_errors_distinguish_context_rejections_from_unrelated_failures() {
        let envelope = json!({"error":{"code":"context_length_exceeded",
            "message":format!("please shorten the request for {KEY}")}})
        .to_string();
        for (status, body, context_length) in [
            (400, envelope.as_str(), true),
            (413, envelope.as_str(), true),
            (
                400,
                r#"{"object":"error","message":"This model's maximum context length is 4096 tokens; received 7000.","code":400}"#,
                true,
            ),
            (413, "Maximum context length exceeded", true),
            (413, "Payload Too Large", false),
            (401, envelope.as_str(), false),
            (429, envelope.as_str(), false),
            (
                400,
                r#"{"error":{"code":"invalid_request_error","message":"Unknown field: maximum context length","request":{"code":"context_length_exceeded"}}}"#,
                false,
            ),
            (
                400,
                r#"{"choices":[{"message":{"content":"This model's maximum context length is 4096 tokens"}}]}"#,
                false,
            ),
            (400, "<html>Maximum context length exceeded</html>", false),
        ] {
            let response = Response::builder()
                .status(status)
                .body(reqwest::Body::from(body.to_owned()))
                .unwrap()
                .into();
            let error = response_error(
                response,
                format!("模型服务请求失败 (HTTP {status})"),
                Some(KEY),
                &CancellationToken::new(),
            )
            .await;
            assert_eq!(
                matches!(&error, AppError::ContextLength(_)),
                context_length,
                "{error}"
            );
            let details = error.to_string();
            assert!(details.contains(&format!("HTTP {status}")), "{details}");
            if body == envelope {
                assert!(!details.contains("sk-offline-"), "{details}");
                assert!(details.contains("request for [REDACTED]"), "{details}");
            }
        }
    }

    #[tokio::test]
    async fn incomplete_http_error_bodies_are_not_treated_as_context_rejections() {
        const PREFIX: &[u8] = b"Maximum context length exceeded";
        let chunks = [
            Ok(PREFIX),
            Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
        ];
        let response = Response::builder()
            .status(413)
            .body(reqwest::Body::wrap_stream(stream::iter(chunks)))
            .unwrap()
            .into();
        let error =
            response_error(response, "HTTP 413".into(), None, &CancellationToken::new()).await;
        assert!(matches!(error, AppError::Other(_)), "{error}");
    }

    #[tokio::test]
    async fn stream_returns_typed_redacted_context_errors_without_retrying_them() {
        let error = json!({"error":{"message":format!(
            "This model's maximum context length is 4096 tokens; request for {KEY} used 7000."
        )}})
        .to_string();
        let event = format!("event: error\ndata: {error}\n\n");
        // A clean EOF without [DONE] counts as interruption. Two such failures cause
        // the client to request buffered JSON on the third attempt.
        for replies in [
            vec![(400, "application/json", error.clone())],
            vec![(200, "text/event-stream", event)],
            vec![(200, "application/json", error.clone())],
            vec![
                (200, "text/event-stream", String::new()),
                (200, "text/event-stream", String::new()),
                (200, "application/json", error),
            ],
        ] {
            let expected_requests = replies.len();
            let requests = Arc::new(AtomicUsize::new(0));
            let requests_in_handler = requests.clone();
            let route = axum::routing::post(move || {
                let index = requests_in_handler.fetch_add(1, Ordering::Relaxed);
                let (status, content_type, body) =
                    replies.get(index).cloned().expect("unexpected retry");
                async move {
                    Response::builder()
                        .status(status)
                        .header("content-type", content_type)
                        .body(axum::body::Body::from(body))
                        .unwrap()
                }
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                axum::serve(
                    listener,
                    axum::Router::new().route("/v1/chat/completions", route),
                )
                .await
                .unwrap();
            });
            let result = ModelClient::new()
                .unwrap()
                .stream(
                    &ModelConfig {
                        base_url,
                        model: "offline-model".into(),
                        reasoning_effort: None,
                        api_key: Some(KEY.into()),
                    },
                    &[],
                    &[],
                    &CancellationToken::new(),
                    |_, _| panic!("an error cannot emit a delta"),
                    || panic!("an error cannot replace a reply"),
                )
                .await;
            server.abort();
            assert_eq!(requests.load(Ordering::Relaxed), expected_requests);
            let error = result.err().expect("context rejection expected");
            let AppError::ContextLength(details) = error else {
                panic!("expected ContextLength, got {error}");
            };
            assert!(!details.contains("sk-offline-"), "{details}");
            assert!(
                details.contains("request for [REDACTED] used 7000"),
                "{details}"
            );
            if expected_requests > 1 {
                assert!(details.contains("非流式"), "{details}");
            }
        }
    }

    #[tokio::test]
    async fn retries_continue_across_pre_header_sse_and_buffered_transport_interruptions() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for attempt in 0..9 {
                let (mut conn, _) = listener.accept().await.unwrap();
                let mut request = vec![];
                let mut buf = [0; 2048];
                loop {
                    let len = conn.read(&mut buf).await.unwrap();
                    assert!(len > 0);
                    request.extend_from_slice(&buf[..len]);
                    if request.ends_with(b"}") && request.windows(4).any(|s| s == b"\r\n\r\n") {
                        break;
                    }
                }
                let body_start = request.windows(4).position(|s| s == b"\r\n\r\n").unwrap() + 4;
                let sent: Value = serde_json::from_slice(&request[body_start..]).unwrap();
                assert_eq!(sent["stream"], attempt < 6);
                assert_eq!(sent.get("stream_options").is_some(), attempt < 6);
                let buffered = r#"{"choices":[{"message":{"role":"assistant","content":"recovered"},"finish_reason":"stop"}]}"#;
                let body = match attempt {
                    0..=3 => None,
                    4..=5 => Some(("text/event-stream", "", true)),
                    // The full JSON syntax is visible, but HTTP framing proves
                    // the body was interrupted and its contents must not emit.
                    6..=7 => Some(("application/json", buffered, true)),
                    _ => Some(("application/json", buffered, false)),
                };
                if let Some((content_type, body, broken)) = body {
                    let len = body.len() + usize::from(broken);
                    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}");
                    conn.write_all(response.as_bytes()).await.unwrap();
                }
                conn.shutdown().await.unwrap();
            }
        });
        let mut deltas = 0;
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            ModelClient::new().unwrap().stream(
                &ModelConfig {
                    base_url,
                    model: "offline-model".into(),
                    reasoning_effort: None,
                    api_key: None,
                },
                &[],
                &[],
                &CancellationToken::new(),
                |delta, _| {
                    assert_eq!(delta.content, "recovered");
                    deltas += 1;
                    Ok(())
                },
                || panic!("no interrupted body should have emitted a partial"),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(deltas, 1);
        assert_eq!(result.content, "recovered");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn invalid_request_reports_immediately_and_connection_retries_are_cancellable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let client = ModelClient::new().unwrap();
        let mut config = ModelConfig {
            base_url,
            model: "offline-model".into(),
            reasoning_effort: None,
            api_key: Some("invalid\nkey".into()),
        };
        let cancel = CancellationToken::new();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            client.stream(
                &config,
                &[],
                &[],
                &cancel,
                |_, _| panic!("no successful response"),
                || Ok(()),
            ),
        )
        .await
        .expect("request construction errors must not retry");
        assert!(matches!(result, Err(AppError::Other(_))));

        // Leave a refused endpoint to exercise the actual connector's IO error
        // path (not an HTTP status). The future must continue until cancelled.
        drop(listener);
        config.api_key = None;
        let pending = client.stream(
            &config,
            &[],
            &[],
            &cancel,
            |_, _| panic!("no response"),
            || Ok(()),
        );
        tokio::pin!(pending);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(200)) => cancel.cancel(),
            _ = &mut pending => panic!("transient connection retries stopped before cancellation"),
        }
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), pending)
                .await
                .unwrap(),
            Err(AppError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn terminal_http_error_does_not_wait_for_an_unending_diagnostic_body() {
        let pending = stream::pending::<io::Result<&'static [u8]>>();
        let response = Response::builder()
            .status(401)
            .body(reqwest::Body::wrap_stream(pending))
            .unwrap()
            .into();
        let error = tokio::time::timeout(
            Duration::from_secs(1),
            response_error(response, "HTTP 401".into(), None, &CancellationToken::new()),
        )
        .await
        .expect("terminal HTTP status should not wait for an unending error body");
        assert!(matches!(error, AppError::Other(message) if message.contains("HTTP 401")));
    }

    #[tokio::test]
    async fn buffered_json_larger_than_old_limit_is_collected_in_chunks() {
        const PAGE: &[u8] = &[b'x'; 16 * 1024];
        let pages = 1100;
        let chunks = std::iter::once(
            br#"{"choices":[{"message":{"role":"assistant","content":""#.as_slice(),
        )
        .chain(std::iter::repeat_n(PAGE, pages))
        .chain(std::iter::once(
            br#""},"finish_reason":"stop"}]}"#.as_slice(),
        ))
        .map(Ok::<_, io::Error>);
        let response = Response::new(reqwest::Body::wrap_stream(stream::iter(chunks))).into();
        let reply = read_json_completion(
            response,
            &CancellationToken::new(),
            ResponseLimit::for_test(128 * 1024 * 1024),
        )
        .await
        .ok()
        .unwrap();
        assert_eq!(reply.content.len(), PAGE.len() * pages);
        assert!(reply.content.bytes().all(|byte| byte == b'x'));
    }

    #[tokio::test]
    async fn buffered_body_must_end_cleanly_even_after_valid_json() {
        let reply = br#"{"choices":[{"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}]}"#;
        let chunks = [
            Ok(reply.as_slice()),
            Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
        ];
        let response = Response::new(reqwest::Body::wrap_stream(stream::iter(chunks))).into();
        assert!(read_json_completion(
            response,
            &CancellationToken::new(),
            ResponseLimit::default()
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn buffered_body_obeys_resource_budget_and_cancellation() {
        let cancel = CancellationToken::new();
        let limit = ResponseLimit::for_test(32 * 1024);
        let response = Response::new(reqwest::Body::from(vec![b' '; 32 * 1024 + 1])).into();
        assert!(matches!(
            read_json_completion(response, &cancel, limit).await,
            Err(StreamFailure::Fatal(AppError::Invalid(_)))
        ));

        let pending = stream::pending::<io::Result<&'static [u8]>>();
        let response = Response::new(reqwest::Body::wrap_stream(pending)).into();
        let read = read_json_completion(response, &cancel, limit);
        tokio::pin!(read);
        assert!(futures_util::poll!(&mut read).is_pending());
        cancel.cancel();
        assert!(matches!(
            read.await,
            Err(StreamFailure::Fatal(AppError::Cancelled))
        ));
    }
}
