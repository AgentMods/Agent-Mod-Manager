use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const MAX_FRAME: usize = 2 * 1024 * 1024;
const MAX_COMPLETION: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub struct SseDecoder {
    pending: Vec<u8>,
    data: Vec<String>,
    frame_size: usize,
}
impl SseDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> AppResult<Vec<String>> {
        let mut out = Vec::new();
        for &byte in bytes {
            self.pending.push(byte);
            if self.pending.len() + self.frame_size > MAX_FRAME {
                return Err(AppError::Invalid("SSE 事件超过大小限制".into()));
            }
            if byte != b'\n' {
                continue;
            }
            let raw = std::mem::take(&mut self.pending);
            let line = std::str::from_utf8(&raw)
                .map_err(|_| AppError::Invalid("SSE UTF-8 无效".into()))?
                .trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    out.push(self.data.join("\n"));
                    self.data.clear();
                }
                self.frame_size = 0;
            } else {
                self.frame_size += raw.len();
                if let Some(data) = line.strip_prefix("data:") {
                    self.data
                        .push(data.strip_prefix(' ').unwrap_or(data).into());
                }
            }
        }
        Ok(out)
    }
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.data.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}
impl ToolCall {
    pub fn as_json(&self) -> Value {
        serde_json::json!({"id":self.id,"type":"function","function":{"name":self.name,"arguments":self.arguments}})
    }
}
#[derive(Default)]
pub struct Completion {
    pub content: String,
    pub reasoning: String,
    pub usage: Option<Value>,
    pub finish_reason: Option<String>,
    calls: BTreeMap<u64, ToolCall>,
    pub done: bool,
    bytes: usize,
}
#[derive(Default)]
pub struct Delta {
    pub content: String,
    pub reasoning: String,
    pub usage: Option<Value>,
}
impl Completion {
    pub fn accept(&mut self, data: &str) -> AppResult<Delta> {
        if self.done {
            return Err(AppError::Invalid("SSE 在结束后继续发送数据".into()));
        }
        if data.trim() == "[DONE]" {
            self.done = true;
            return Ok(Delta::default());
        }
        self.bytes += data.len();
        if self.bytes > MAX_COMPLETION {
            return Err(AppError::Invalid("模型回复超过大小限制".into()));
        }
        let value: Value = serde_json::from_str(data)?;
        if value.get("error").is_some() {
            return Err(AppError::Other(format!("模型服务返回流式错误\n{value:#}")));
        }
        let mut delta = Delta::default();
        if let Some(usage) = value.get("usage").filter(|u| !u.is_null()) {
            self.usage = Some(usage.clone());
            delta.usage = Some(usage.clone());
        }
        for choice in value["choices"].as_array().into_iter().flatten() {
            if choice["index"].as_u64().unwrap_or(0) != 0 {
                continue;
            }
            let d = &choice["delta"];
            if let Some(s) = d["content"].as_str() {
                self.content.push_str(s);
                delta.content.push_str(s);
            }
            if let Some(s) = d["reasoning_content"]
                .as_str()
                .or_else(|| d["reasoning"].as_str())
            {
                self.reasoning.push_str(s);
                delta.reasoning.push_str(s);
            }
            for call in d["tool_calls"].as_array().into_iter().flatten() {
                let index = call["index"]
                    .as_u64()
                    .filter(|i| *i < 64)
                    .ok_or_else(|| AppError::Invalid("tool_calls index 无效".into()))?;
                let partial = self.calls.entry(index).or_insert_with(|| ToolCall {
                    id: String::new(),
                    name: String::new(),
                    arguments: String::new(),
                });
                if let Some(id) = call["id"].as_str() {
                    if partial.id.is_empty() {
                        partial.id = id.into();
                    } else if partial.id != id {
                        return Err(AppError::Invalid("工具调用 ID 在流中改变".into()));
                    }
                }
                if let Some(name) = call["function"]["name"].as_str() {
                    partial.name.push_str(name);
                }
                if let Some(args) = call["function"]["arguments"].as_str() {
                    partial.arguments.push_str(args);
                }
            }
            if let Some(reason) = choice["finish_reason"].as_str() {
                self.finish_reason = Some(reason.into());
            }
        }
        Ok(delta)
    }
    pub fn calls(&self) -> AppResult<Vec<ToolCall>> {
        if !self.done {
            return Err(AppError::Invalid("模型流意外断开，工具不会执行".into()));
        }
        if !matches!(self.finish_reason.as_deref(), Some("stop" | "tool_calls")) {
            let reason = self.finish_reason.as_deref().unwrap_or("缺失");
            return Err(AppError::Invalid(format!(
                "模型回复未完整结束，工具不会执行 (finish_reason = {reason})"
            )));
        }
        if !self.calls.is_empty() && self.finish_reason.as_deref() != Some("tool_calls") {
            return Err(AppError::Invalid("工具调用未完整结束".into()));
        }
        let mut ids = std::collections::HashSet::new();
        for c in self.calls.values() {
            if c.id.is_empty()
                || c.id.len() > 256
                || c.name.is_empty()
                || c.name.len() > 128
                || !ids.insert(c.id.clone())
            {
                return Err(AppError::Invalid("工具调用缺少或重复 ID/名称".into()));
            }
            if !serde_json::from_str::<Value>(&c.arguments)?.is_object() {
                return Err(AppError::Invalid("工具参数必须为 JSON 对象".into()));
            }
        }
        Ok(self.calls.values().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn splits_utf8_crlf_comments_and_multiline_frames() {
        let input = ": heartbeat\r\ndata: 你好\r\ndata: again\r\n\r\ndata: [DONE]\n\n";
        let mut decoder = SseDecoder::default();
        let mut frames = Vec::new();
        for byte in input.as_bytes() {
            frames.extend(decoder.push(&[*byte]).unwrap());
        }
        assert_eq!(frames, vec!["你好\nagain", "[DONE]"]);
        assert!(decoder.is_empty());
    }
    #[test]
    fn assembles_interleaved_tool_arguments_and_late_usage() {
        let mut c = Completion::default();
        c.accept(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"read_file","arguments":"{\"path\":"}},{"index":0,"id":"a","function":{"name":"read_dir","arguments":"{}"}}]}}]}"#).unwrap();
        c.accept(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"function":{"arguments":"\"mods\"}"}}]},"finish_reason":"tool_calls"}]}"#).unwrap();
        assert!(c.calls().is_err());
        c.accept(r#"{"choices":[],"usage":{"total_tokens":12}}"#)
            .unwrap();
        c.accept("[DONE]").unwrap();
        let calls = c.calls().unwrap();
        assert_eq!(calls[0].id, "a");
        assert_eq!(calls[1].arguments, r#"{"path":"mods"}"#);
        assert_eq!(c.usage.unwrap()["total_tokens"], 12);
    }
    #[test]
    fn rejects_truncated_calls_and_oversized_frame() {
        let mut c = Completion::default();
        c.accept(r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#)
            .unwrap();
        c.accept("[DONE]").unwrap();
        assert!(c.calls().is_err());
        assert!(SseDecoder::default()
            .push(&vec![b'x'; MAX_FRAME + 1])
            .is_err());
    }
}
