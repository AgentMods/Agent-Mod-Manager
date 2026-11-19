use super::sse::{Completion, SseDecoder};
use crate::error::{AppError, AppResult};
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

pub struct ModelConfig {
    pub base_url: String,
    pub model: String,
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
        let base = url::Url::parse(&config.base_url)
            .map_err(|_| AppError::Invalid("模型地址无效".into()))?;
        if !matches!(base.scheme(), "https" | "http")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(AppError::Invalid(
                "模型地址必须为无凭据的 HTTP(S) URL".into(),
            ));
        }
        if config.model.trim().is_empty() {
            return Err(AppError::Invalid("请先在设置中选择模型".into()));
        }
        let endpoint = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
        let mut body = json!({"model":config.model,"messages":messages,"stream":true,"stream_options":{"include_usage":true}});
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
        let response = tokio::select! {biased;_=cancel.cancelled()=>return Err(AppError::Cancelled),response=request.send()=>response.map_err(|_|AppError::Other("无法连接模型服务".into()))?};
        if !response.status().is_success() {
            return Err(AppError::Other(format!(
                "模型服务请求失败 (HTTP {})",
                response.status().as_u16()
            )));
        }
        if !response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|t| t.starts_with("text/event-stream"))
        {
            return Err(AppError::Invalid("模型服务未返回 SSE 流".into()));
        }
        let mut stream = response.bytes_stream();
        let mut decoder = SseDecoder::default();
        let mut completion = Completion::default();
        loop {
            let next = tokio::select! {biased;_=cancel.cancelled()=>return Err(AppError::Cancelled),value=stream.next()=>value};
            let Some(chunk) = next else {
                break;
            };
            let chunk = chunk.map_err(|_| AppError::Other("模型流连接中断".into()))?;
            for frame in decoder.push(&chunk)? {
                let delta = completion.accept(&frame)?;
                on_delta(&delta, &completion)?;
                if completion.done {
                    completion.calls()?;
                    return Ok(completion);
                }
            }
        }
        Err(AppError::Other("模型流提前结束，未执行工具调用".into()))
    }
}
