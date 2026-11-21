use super::sse::{Completion, Delta, SseDecoder, MAX_COMPLETION};
use crate::{
    error::{AppError, AppResult},
    settings::{parse_model_base_url, ReasoningEffort},
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::error::Error;
use tokio_util::sync::CancellationToken;

const MAX_ERROR_BYTES: usize = 64 * 1024;
const MAX_ATTEMPTS: u32 = 3;
const RETRY_BASE_MS: u64 = if cfg!(test) { 10 } else { 1000 };

pub struct ModelConfig {
    pub base_url: String,
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub api_key: Option<String>,
}
pub struct ModelClient {
    http: reqwest::Client,
}
impl ModelClient {
    pub fn new() -> AppResult<Self> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(20))
            .timeout(std::time::Duration::from_secs(600))
            .build()?;
        Ok(Self { http })
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
        let mut attempt = 0;
        let mut interrupted_streams = 0;
        let mut has_visible_reply = false;
        loop {
            // Change representation only if both prior SSE bodies were interrupted, not
            // for authentication, provider/protocol errors or failures to connect.
            let streaming = interrupted_streams < MAX_ATTEMPTS - 1;
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
                            interrupted_streams += 1;
                            error
                        }
                        StreamFailure::Connect(error) => error,
                        StreamFailure::Fatal(error) => return Err(error),
                    };
                    if attempt + 1 >= MAX_ATTEMPTS {
                        return Err(error);
                    }
                    let delay = std::time::Duration::from_millis(RETRY_BASE_MS << attempt);
                    attempt += 1;
                    tracing::warn!("模型连接中断，第 {attempt} 次重试: {error}");
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return Err(AppError::Cancelled),
                        _ = tokio::time::sleep(delay) => {}
                    }
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
        let endpoint = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
        let mut body = json!({"model":config.model,"messages":messages,"stream":streaming});
        if streaming {
            body["stream_options"] = json!({"include_usage":true});
        }
        if let Some(effort) = config.reasoning_effort {
            body["reasoning_effort"] = json!(effort);
        }
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
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
                format!("模型流连续中断，非流式重试也未完成。请检查模型服务或代理连接后重试。\n{details}")
            }
        };
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AppError::Cancelled.into()),
            result = request.send() => result.map_err(|error| {
                let details = diagnostic(transport_details("无法连接模型服务", error));
                StreamFailure::Connect(model_error(details, api_key, false))
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
        if !streaming {
            let completion = read_json_completion(response, cancel)
                .await
                .map_err(|error| match error {
                    AppError::Cancelled => AppError::Cancelled,
                    error => model_error(diagnostic(error.to_string()), api_key, false),
                })?;
            let delta = Delta {
                content: completion.content.clone(),
                reasoning: completion.reasoning.clone(),
            };
            on_delta(&delta, &completion)?;
            return Ok(completion);
        }
        let mut stream = response.bytes_stream();
        let mut decoder = SseDecoder::default();
        let mut completion = Completion::default();
        loop {
            let next = tokio::select! {biased;_=cancel.cancelled()=>return Err(AppError::Cancelled.into()),value=stream.next()=>value};
            let Some(chunk) = next else {
                break;
            };
            let chunk = chunk.map_err(|error| {
                StreamFailure::Interrupted(model_error(
                    transport_details("模型流连接中断", error),
                    api_key,
                    false,
                ))
            })?;
            for frame in decoder.push(&chunk)? {
                let delta = completion
                    .accept(&frame)
                    .map_err(|error| model_error(error.to_string(), api_key, false))?;
                on_delta(&delta, &completion)?;
                if completion.done {
                    completion
                        .validate()
                        .map_err(|error| model_error(error.to_string(), api_key, false))?;
                    return Ok(completion);
                }
            }
        }
        Err(StreamFailure::Interrupted(AppError::Other(
            "模型流提前结束，未执行工具调用".into(),
        )))
    }
}

enum StreamFailure {
    Connect(AppError),
    Interrupted(AppError),
    Fatal(AppError),
}
impl From<AppError> for StreamFailure {
    fn from(error: AppError) -> Self {
        Self::Fatal(error)
    }
}

/// JSON has no SSE terminator: require the whole HTTP body even when the bytes received
/// before a transport error happen to look like a complete JSON object.
async fn read_json_completion(
    response: reqwest::Response,
    cancel: &CancellationToken,
) -> AppResult<Completion> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(result) = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(AppError::Cancelled),
        result = stream.next() => result,
    } {
        let chunk = result
            .map_err(|error| AppError::Other(transport_details("读取模型响应失败", error)))?;
        if chunk.len() > MAX_COMPLETION - body.len() {
            return Err(AppError::Invalid("模型回复超过大小限制".into()));
        }
        body.extend_from_slice(&chunk);
    }
    Completion::from_json(&body)
}

async fn response_error(
    response: reqwest::Response,
    mut details: String,
    api_key: Option<&str>,
    cancel: &CancellationToken,
) -> AppError {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    let mut truncated = false;
    while let Some(result) = tokio::select! {
        biased;
        _ = cancel.cancelled() => return AppError::Cancelled,
        result = stream.next() => result,
    } {
        let chunk = match result {
            Ok(chunk) => chunk,
            Err(error) => {
                details.push('\n');
                details.push_str(&transport_details("读取错误响应失败", error));
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
    if !body.is_empty() {
        details.push('\n');
        match serde_json::from_slice::<Value>(&body) {
            Ok(value) => details.push_str(&format!("{value:#}")),
            Err(_) => details.push_str(&String::from_utf8_lossy(&body)),
        }
    }
    model_error(details, api_key, truncated)
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

fn model_error(mut details: String, api_key: Option<&str>, truncated: bool) -> AppError {
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
    AppError::Other(details)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Response;
    use futures_util::stream;
    use std::io;

    #[tokio::test]
    async fn buffered_body_must_end_cleanly_even_after_valid_json() {
        let reply = br#"{"choices":[{"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}]}"#;
        let chunks = [
            Ok(reply.as_slice()),
            Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
        ];
        let response = Response::new(reqwest::Body::wrap_stream(stream::iter(chunks))).into();
        assert!(read_json_completion(response, &CancellationToken::new())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn buffered_body_obeys_size_limit_and_cancellation() {
        let cancel = CancellationToken::new();
        let response = Response::new(reqwest::Body::from(vec![b' '; MAX_COMPLETION + 1])).into();
        assert!(matches!(
            read_json_completion(response, &cancel).await,
            Err(AppError::Invalid(_))
        ));

        let pending = stream::pending::<io::Result<&'static [u8]>>();
        let response = Response::new(reqwest::Body::wrap_stream(pending)).into();
        let read = read_json_completion(response, &cancel);
        tokio::pin!(read);
        assert!(futures_util::poll!(&mut read).is_pending());
        cancel.cancel();
        assert!(matches!(read.await, Err(AppError::Cancelled)));
    }
}
