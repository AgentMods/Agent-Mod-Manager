use super::sse::{Completion, SseDecoder};
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
        mut on_delta: impl FnMut(&super::sse::Delta, &Completion) -> AppResult<()>,
    ) -> AppResult<Completion> {
        parse_model_base_url(&config.base_url)?;
        if config.model.trim().is_empty() {
            return Err(AppError::Invalid("请先在设置中选择模型".into()));
        }
        // A dropped connection before anything reached the user is safe to repeat:
        // nothing was shown or stored. Once text was emitted a retry would duplicate it.
        let mut attempt = 0;
        loop {
            match self
                .stream_once(config, messages, tools, cancel, &mut on_delta)
                .await
            {
                Ok(completion) => return Ok(completion),
                Err(StreamFailure::Retryable(error)) if attempt + 1 < MAX_ATTEMPTS => {
                    let delay = std::time::Duration::from_millis(RETRY_BASE_MS << attempt);
                    attempt += 1;
                    tracing::warn!("模型连接中断，第 {attempt} 次重试: {error}");
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return Err(AppError::Cancelled),
                        _ = tokio::time::sleep(delay) => {}
                    }
                }
                Err(StreamFailure::Retryable(error) | StreamFailure::Fatal(error)) => {
                    return Err(error)
                }
            }
        }
    }

    async fn stream_once(
        &self,
        config: &ModelConfig,
        messages: &[Value],
        tools: &[Value],
        cancel: &CancellationToken,
        on_delta: &mut impl FnMut(&super::sse::Delta, &Completion) -> AppResult<()>,
    ) -> Result<Completion, StreamFailure> {
        let endpoint = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
        let mut body = json!({"model":config.model,"messages":messages,"stream":true,"stream_options":{"include_usage":true}});
        if let Some(effort) = config.reasoning_effort {
            body["reasoning_effort"] = json!(effort);
        }
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        let mut request = self
            .http
            .post(&endpoint)
            .header("Accept", "text/event-stream")
            .json(&body);
        if let Some(key) = &config.api_key {
            request = request.bearer_auth(key);
        }
        let api_key = config.api_key.as_deref().filter(|key| !key.is_empty());
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AppError::Cancelled.into()),
            result = request.send() => result.map_err(|error| {
                StreamFailure::Retryable(model_error(transport_details("无法连接模型服务", error), api_key, false))
            })?,
        };
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|header| header.to_str().ok());
        let failure = if !response.status().is_success() {
            Some(format!("模型服务请求失败 (HTTP {status})"))
        } else if !content_type.is_some_and(|value| value.starts_with("text/event-stream")) {
            let content_type = content_type.unwrap_or("缺失");
            Some(format!(
                "模型服务未返回 SSE 流 (HTTP {status}, Content-Type = {content_type})"
            ))
        } else {
            None
        };
        if let Some(summary) = failure {
            return Err(response_error(response, summary, api_key, cancel)
                .await
                .into());
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
                let error = model_error(transport_details("模型流连接中断", error), api_key, false);
                if completion.content.is_empty() && completion.reasoning.is_empty() {
                    StreamFailure::Retryable(error)
                } else {
                    StreamFailure::Fatal(error)
                }
            })?;
            for frame in decoder.push(&chunk)? {
                let delta = completion
                    .accept(&frame)
                    .map_err(|error| model_error(error.to_string(), api_key, false))?;
                on_delta(&delta, &completion)?;
                if completion.done {
                    completion
                        .calls()
                        .map_err(|error| model_error(error.to_string(), api_key, false))?;
                    return Ok(completion);
                }
            }
        }
        Err(AppError::Other("模型流提前结束，未执行工具调用".into()).into())
    }
}

enum StreamFailure {
    Retryable(AppError),
    Fatal(AppError),
}
impl From<AppError> for StreamFailure {
    fn from(error: AppError) -> Self {
        Self::Fatal(error)
    }
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
