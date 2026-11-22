use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, sync::LazyLock};

// The runtime still consumes contiguous strings. Budget the live model buffers against
// local memory, not a provider token/count limit. Leave room for allocator growth,
// reqwest, SQLite, IPC and the rest of the desktop process. Every concurrent model
// attempt shares the reservations, including the completion until it is consumed.
static RESERVED_MODEL_MEMORY: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[derive(Clone, Copy)]
pub(super) struct ResponseLimit {
    bytes: usize,
    pool: &'static std::sync::atomic::AtomicUsize,
}

impl Default for ResponseLimit {
    fn default() -> Self {
        // A conservative fallback only for platforms where the OS probe is unavailable.
        Self {
            bytes: available_memory().map_or(128 * 1024 * 1024, |bytes| bytes / 8),
            pool: &RESERVED_MODEL_MEMORY,
        }
    }
}

/// Conservative payload capacity for a read operation, derived from the
/// same available-memory probe as model response buffering.
pub(crate) fn response_payload_capacity() -> usize {
    ResponseLimit::default().payload_capacity()
}

impl ResponseLimit {
    pub(super) fn payload_capacity(self) -> usize {
        self.bytes
    }

    fn lease(self) -> ResponseMemory {
        ResponseMemory {
            limit: self,
            held: 0,
        }
    }

    #[cfg(test)]
    pub(super) fn for_test(bytes: usize) -> Self {
        Self {
            bytes,
            pool: Box::leak(Box::new(std::sync::atomic::AtomicUsize::new(0))),
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn available_memory() -> Option<usize> {
    #[repr(C)]
    struct MemoryStatus {
        length: u32,
        load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(status: *mut MemoryStatus) -> i32;
    }
    // SAFETY: all fields are integers; the API writes this exact layout and length.
    let mut status: MemoryStatus = unsafe { std::mem::zeroed() };
    status.length = size_of::<MemoryStatus>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return None;
    }
    usize::try_from(
        status
            .avail_phys
            .min(status.avail_page_file)
            .min(status.avail_virtual),
    )
    .ok()
}

#[cfg(target_os = "linux")]
pub(crate) fn available_memory() -> Option<usize> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let available = meminfo.lines().find_map(|line| {
        let kb = line
            .strip_prefix("MemAvailable:")?
            .split_whitespace()
            .next()?;
        kb.parse::<usize>().ok()?.checked_mul(1024)
    })?;
    // cgroup v2 may give a desktop/container process less space than host RAM.
    let group = std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|value| {
            value
                .lines()
                .find_map(|line| line.strip_prefix("0::/").map(str::to_owned))
        });
    let root = std::path::Path::new("/sys/fs/cgroup");
    let mut available = available;
    let path = root.join(group.as_deref().unwrap_or_default());
    for group in path.ancestors().take_while(|path| path.starts_with(root)) {
        let read_number = |name| {
            std::fs::read_to_string(group.join(name))
                .ok()?
                .trim()
                .parse::<usize>()
                .ok()
        };
        if let (Some(limit), Some(used)) =
            (read_number("memory.max"), read_number("memory.current"))
        {
            available = available.min(limit.saturating_sub(used));
        }
    }
    Some(available)
}

#[cfg(target_os = "macos")]
pub(crate) fn available_memory() -> Option<usize> {
    // Retain one send right for the process instead of acquiring one per request.
    static HOST: LazyLock<libc::mach_port_t> = LazyLock::new(|| unsafe { libc::mach_host_self() });
    let mut stats = std::mem::MaybeUninit::<libc::vm_statistics64_data_t>::zeroed();
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: the output layout and count match HOST_VM_INFO64; read it only on success.
    let result = unsafe {
        libc::host_statistics64(
            *HOST,
            libc::HOST_VM_INFO64,
            stats.as_mut_ptr().cast(),
            &mut count,
        )
    };
    if result != 0 || count < libc::HOST_VM_INFO64_COUNT {
        return None;
    }
    let stats = unsafe { stats.assume_init() };
    let page_size = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).ok()?;
    // Free pages already include speculative pages; inactive pages can be reclaimed.
    (stats.free_count as usize)
        .saturating_add(stats.inactive_count as usize)
        .checked_mul(page_size)
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub(crate) fn available_memory() -> Option<usize> {
    None
}

struct ResponseMemory {
    limit: ResponseLimit,
    held: usize,
}

fn response_memory_error() -> AppError {
    AppError::Invalid("模型回复超出本机可安全处理的内存，未执行工具调用".into())
}

impl ResponseMemory {
    fn grow(&mut self, bytes: usize) -> AppResult<()> {
        use std::sync::atomic::Ordering::Relaxed;
        if bytes == 0 {
            return Ok(());
        }
        self.limit
            .pool
            .fetch_update(Relaxed, Relaxed, |reserved| {
                reserved
                    .checked_add(bytes)
                    .filter(|&total| total <= self.limit.bytes)
            })
            .map_err(|_| response_memory_error())?;
        self.held += bytes;
        Ok(())
    }

    fn reserve_buffer(&mut self, needed: usize, capacity: usize) -> AppResult<usize> {
        use std::sync::atomic::Ordering::Relaxed;
        let minimum = needed - capacity;
        let preferred = needed.max(capacity.saturating_mul(2)) - capacity;
        let mut extra = 0;
        // Grow geometrically up to the available budget. Avoid repeatedly
        // reallocating one byte at a time as a remote response nears the limit.
        self.limit
            .pool
            .fetch_update(Relaxed, Relaxed, |reserved| {
                let available = self.limit.bytes.checked_sub(reserved)?;
                if available < minimum {
                    return None;
                }
                extra = preferred.min(available);
                Some(reserved + extra)
            })
            .map_err(|_| response_memory_error())?;
        self.held += extra;
        Ok(extra)
    }

    fn release(&mut self, bytes: usize) {
        self.held -= bytes;
        self.limit
            .pool
            .fetch_sub(bytes, std::sync::atomic::Ordering::Relaxed);
    }

    fn json(limit: ResponseLimit, bytes: &[u8]) -> AppResult<Self> {
        let mut memory = limit.lease();
        // Account for parser copies and for the expansion of compact JSON objects
        // (e.g. [0,0,...]) into a serde tree, not just the bytes on the wire.
        memory.grow(bytes.len().saturating_mul(2))?;
        let (mut string, mut escape) = (false, false);
        let mut structure = 0_usize;
        for &byte in bytes {
            if string {
                if escape {
                    escape = false;
                } else if byte == b'\\' {
                    escape = true;
                } else if byte == b'"' {
                    string = false;
                }
            } else {
                match byte {
                    b'"' => string = true,
                    b'{' => structure = structure.saturating_add(1024),
                    b'[' | b',' | b':' => structure = structure.saturating_add(128),
                    _ => {}
                }
            }
        }
        memory.grow(structure)?;
        Ok(memory)
    }
}

impl Drop for ResponseMemory {
    fn drop(&mut self) {
        self.release(self.held);
    }
}

pub(super) struct ResponseBuffer {
    data: Vec<u8>,
    memory: ResponseMemory,
}

impl ResponseBuffer {
    pub(super) fn new(limit: ResponseLimit) -> Self {
        Self {
            data: Vec::new(),
            memory: limit.lease(),
        }
    }

    pub(super) fn extend(&mut self, bytes: &[u8]) -> AppResult<()> {
        let needed = self
            .data
            .len()
            .checked_add(bytes.len())
            .ok_or_else(response_memory_error)?;
        let capacity = self.data.capacity();
        if needed > capacity {
            // No allocation is based on a remote Content-Length.
            let extra = self.memory.reserve_buffer(needed, capacity)?;
            if self
                .data
                .try_reserve_exact(capacity + extra - self.data.len())
                .is_err()
            {
                self.memory.release(extra);
                return Err(response_memory_error());
            }
        }
        self.data.extend_from_slice(bytes);
        Ok(())
    }

    pub(super) fn bytes(&self) -> &[u8] {
        &self.data
    }

    fn clear(&mut self) {
        // Release unusually large one-off line/event allocations instead of
        // permanently tying up the shared budget until the response finishes.
        if self.data.capacity() > 64 * 1024 {
            *self = Self::new(self.memory.limit);
        } else {
            self.data.clear();
        }
    }
}

/// Match explicit context-limit diagnostics, not generic token or body size limits.
/// The caller must already know that this is an error message, not completion content.
pub(super) fn is_context_length_message(message: &str) -> bool {
    static CONTEXT_LENGTH: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?ix)^\s*(?:
                (?:(?:(?:this|the)\s+model['’]s|the)\s+)?maximum\s+context\s+length\s+
                    (?:is\s+[0-9,]+\s+tokens?\b|(?:of\s+[0-9,]+\s+tokens?\s+)?(?:has\s+been\s+)?exceeded\b)
                | (?:your\s+)?(?:input|prompt|request)(?:\s+length)?(?:\s*\([0-9,]+\s+tokens?\))?\s+
                    exceeds?\s+(?:(?:the|this)\s+)?(?:model['’]s\s+)?(?:maximum\s+context\s+length|context\s+window)\b
                | context\s+(?:length|window|size)(?:\s+limit)?\s+(?:has\s+been\s+)?exceeded\b
                | prompt\s+is\s+too\s+long:\s*[0-9,]+\s+tokens?\s*>\s*[0-9,]+\s+maximum\b
            )",
        )
        .expect("valid context length error pattern")
    });
    CONTEXT_LENGTH.is_match(message)
}

pub(super) fn is_provider_context_length_error(error: &Value) -> bool {
    [
        error.get("code"),
        error.get("type"),
        error.get("error_type"),
        error.pointer("/metadata/error_type"),
    ]
    .into_iter()
    .flatten()
    .any(|code| code.as_str() == Some("context_length_exceeded"))
        || error
            .as_str()
            .or_else(|| error.get("message").and_then(Value::as_str))
            .is_some_and(is_context_length_message)
}

/// Chat Completions can carry an error even after HTTP headers were committed.
pub(super) fn provider_error_envelope(value: &Value) -> Option<&Value> {
    value
        .get("error")
        .filter(|error| !error.is_null())
        .or_else(|| {
            value.get("choices")?.as_array()?.iter().find_map(|choice| {
                let index = choice.get("index");
                if choice["finish_reason"] == "error"
                    && index.is_none_or(|index| index.as_u64() == Some(0))
                {
                    choice.get("error").filter(|error| !error.is_null())
                } else {
                    None
                }
            })
        })
}

fn provider_error(summary: &str, details: &Value, error: &Value) -> AppError {
    struct BoundedDetails(Vec<u8>);
    impl std::io::Write for BoundedDetails {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > 64 * 1024 - self.0.len() {
                return Err(std::io::Error::other("model diagnostic exceeds budget"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut out = BoundedDetails(Vec::new());
    let details = match serde_json::to_writer_pretty(&mut out, details) {
        Ok(()) => format!("{summary}\n{}", String::from_utf8_lossy(&out.0)),
        // Do not report a partial credential split across the cutoff.
        Err(_) => format!("{summary}（错误详情超过 64 KiB，已省略）"),
    };
    if is_provider_context_length_error(error) {
        AppError::ContextLength(details)
    } else {
        AppError::Other(details)
    }
}

pub(super) struct SseDecoder {
    line: ResponseBuffer,
    event: ResponseBuffer,
    has_data: bool,
}
impl SseDecoder {
    pub(super) fn new(limit: ResponseLimit) -> Self {
        Self {
            line: ResponseBuffer::new(limit),
            event: ResponseBuffer::new(limit),
            has_data: false,
        }
    }

    /// Dispatch one complete event at a time; do not accumulate ready frames
    /// from the transport chunk on top of the in-progress frame.
    pub(super) fn push(
        &mut self,
        mut bytes: &[u8],
        mut on_frame: impl FnMut(&str) -> AppResult<bool>,
    ) -> AppResult<()> {
        while let Some(end) = bytes.iter().position(|&byte| byte == b'\n') {
            let (line, rest) = bytes.split_at(end + 1);
            bytes = rest;
            let raw = if self.line.bytes().is_empty() {
                line
            } else {
                self.line.extend(line)?;
                self.line.bytes()
            };
            let line = std::str::from_utf8(raw)
                .map_err(|_| AppError::Invalid("SSE UTF-8 无效".into()))?
                .trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if self.has_data {
                    // Each data field was UTF-8 validated before copying.
                    let frame = std::str::from_utf8(self.event.bytes())
                        .map_err(|_| AppError::Invalid("SSE UTF-8 无效".into()))?;
                    let done = on_frame(frame)?;
                    self.event.clear();
                    self.has_data = false;
                    if done {
                        self.line.clear();
                        return Ok(());
                    }
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                if self.has_data {
                    self.event.extend(b"\n")?;
                }
                self.event
                    .extend(data.strip_prefix(' ').unwrap_or(data).as_bytes())?;
                self.has_data = true;
            }
            self.line.clear();
        }
        self.line.extend(bytes)
    }
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.line.bytes().is_empty() && !self.has_data
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
pub struct Completion {
    pub content: String,
    pub reasoning: String,
    pub usage: Option<Value>,
    pub finish_reason: Option<String>,
    calls: BTreeMap<u64, ToolCall>,
    pub done: bool,
    memory: ResponseMemory,
    usage_bytes: usize,
}
#[derive(Default)]
pub struct Delta {
    pub content: String,
    pub reasoning: String,
    _memory: Option<ResponseMemory>,
}
impl Completion {
    pub(super) fn new(limit: ResponseLimit) -> Self {
        Self {
            content: String::new(),
            reasoning: String::new(),
            usage: None,
            finish_reason: None,
            calls: BTreeMap::new(),
            done: false,
            memory: limit.lease(),
            usage_bytes: 0,
        }
    }

    #[cfg(test)]
    pub(super) fn from_json(body: &[u8]) -> AppResult<Self> {
        Self::from_buffered_json(body, ResponseLimit::default())
    }

    pub(super) fn from_buffered_json(body: &[u8], limit: ResponseLimit) -> AppResult<Self> {
        let scratch = ResponseMemory::json(limit, body)?;
        #[derive(Deserialize)]
        struct Response {
            choices: Option<[Choice; 1]>, // We request a single completion (n = 1).
            usage: Option<Value>,
            error: Option<Value>,
        }
        #[derive(Deserialize)]
        struct Choice {
            #[serde(default)]
            index: usize,
            finish_reason: String,
            message: Message,
            error: Option<Value>,
        }
        #[derive(Deserialize)]
        struct Message {
            role: String,
            content: Option<String>,
            reasoning_content: Option<String>,
            reasoning: Option<String>,
            tool_calls: Option<Vec<Call>>,
        }
        #[derive(Deserialize)]
        struct Call {
            id: String,
            #[serde(rename = "type")]
            kind: String,
            function: Function,
        }
        #[derive(Deserialize)]
        struct Function {
            name: String,
            arguments: String,
        }
        let error_summary = "模型服务返回错误";
        let response: Response = match serde_json::from_slice(body) {
            Ok(response) => response,
            Err(parse_error) => {
                // An error envelope need not satisfy the strict success schema for choices.
                if let Ok(value) = serde_json::from_slice::<Value>(body) {
                    if let Some(error) = provider_error_envelope(&value) {
                        return Err(provider_error(error_summary, error, error));
                    }
                }
                return Err(parse_error.into());
            }
        };
        if let Some(error) = response.error {
            return Err(provider_error(error_summary, &error, &error));
        }
        let [choice] = response
            .choices
            .ok_or_else(|| AppError::Invalid("模型回复缺少 choices".into()))?;
        let message = choice.message;
        if choice.index != 0 || message.role != "assistant" {
            return Err(AppError::Invalid("模型回复不是预期的助手消息".into()));
        }
        if choice.finish_reason == "error" {
            if let Some(error) = choice.error {
                return Err(provider_error(error_summary, &error, &error));
            }
        }
        let mut completion = Self {
            content: message.content.unwrap_or_default(),
            reasoning: message
                .reasoning_content
                .or(message.reasoning)
                .unwrap_or_default(),
            usage: response.usage,
            finish_reason: Some(choice.finish_reason),
            done: true,
            ..Self::new(limit)
        };
        let calls = message.tool_calls.unwrap_or_default();
        for (index, call) in calls.into_iter().enumerate() {
            if call.kind != "function" {
                return Err(AppError::Invalid("不支持的工具调用类型".into()));
            }
            completion.calls.insert(
                index as u64,
                ToolCall {
                    id: call.id,
                    name: call.function.name,
                    arguments: call.function.arguments,
                },
            );
        }
        completion.usage_bytes = completion.usage.as_ref().map_or(0, json_heap_estimate);
        let retained = completion
            .usage_bytes
            .saturating_add(completion.content.capacity())
            .saturating_add(completion.reasoning.capacity())
            .saturating_add(
                completion
                    .finish_reason
                    .as_ref()
                    .map_or(0, String::capacity),
            );
        let retained = completion.calls.values().fold(retained, |sum, call| {
            sum.saturating_add(CALL_ALLOCATION)
                .saturating_add(call.id.capacity())
                .saturating_add(call.name.capacity())
                .saturating_add(call.arguments.capacity())
        });
        // Transfer the parser's reservation instead of dropping it while the
        // already-allocated result is still live and re-acquiring from the pool.
        completion.memory = scratch;
        if retained > completion.memory.held {
            completion.memory.grow(retained - completion.memory.held)?;
        } else {
            completion.memory.release(completion.memory.held - retained);
        }
        completion.validate()?;
        if completion.calls.is_empty() && completion.content.is_empty() {
            return Err(AppError::Invalid("模型未返回回复正文或工具调用".into()));
        }
        Ok(completion)
    }

    pub(super) fn buffered_delta(&self) -> AppResult<Delta> {
        let mut memory = self.memory.limit.lease();
        let mut delta = Delta::default();
        append_response_text(&mut memory, &mut delta.content, &self.content)?;
        append_response_text(&mut memory, &mut delta.reasoning, &self.reasoning)?;
        delta._memory = Some(memory);
        Ok(delta)
    }

    pub fn accept(&mut self, data: &str) -> AppResult<Delta> {
        if self.done {
            return Err(AppError::Invalid("SSE 在结束后继续发送数据".into()));
        }
        if data.trim() == "[DONE]" {
            self.done = true;
            return Ok(Delta::default());
        }
        let _scratch = ResponseMemory::json(self.memory.limit, data.as_bytes())?;
        let mut value: Value = serde_json::from_str(data)?;
        if let Some(error) = provider_error_envelope(&value) {
            return Err(provider_error("模型服务返回流式错误", &value, error));
        }
        let object = value
            .as_object_mut()
            .ok_or_else(|| AppError::Invalid("模型流事件必须为 JSON 对象".into()))?;
        if object
            .get("choices")
            .is_some_and(|choices| !choices.is_null() && !choices.is_array())
        {
            return Err(AppError::Invalid("模型流事件的 choices 不是数组".into()));
        }
        let mut delta = Delta::default();
        let mut delta_memory = self.memory.limit.lease();
        if let Some(usage) = object.remove("usage").filter(|usage| !usage.is_null()) {
            self.usage = None;
            self.memory.release(self.usage_bytes);
            self.usage_bytes = 0;
            let bytes = json_heap_estimate(&usage);
            self.memory.grow(bytes)?;
            self.usage = Some(usage);
            self.usage_bytes = bytes;
        }
        for choice in value["choices"].as_array().into_iter().flatten() {
            if choice["index"].as_u64().unwrap_or(0) != 0 {
                continue;
            }
            let d = &choice["delta"];
            if let Some(s) = d["content"].as_str() {
                append_response_text(&mut self.memory, &mut self.content, s)?;
                append_response_text(&mut delta_memory, &mut delta.content, s)?;
            }
            if let Some(s) = d["reasoning_content"]
                .as_str()
                .or_else(|| d["reasoning"].as_str())
            {
                append_response_text(&mut self.memory, &mut self.reasoning, s)?;
                append_response_text(&mut delta_memory, &mut delta.reasoning, s)?;
            }
            for call in d["tool_calls"].as_array().into_iter().flatten() {
                // The provider index is a sparse identity, not an allocation size.
                let index = call["index"]
                    .as_u64()
                    .ok_or_else(|| AppError::Invalid("tool_calls index 无效".into()))?;
                let partial = match self.calls.entry(index) {
                    std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        self.memory.grow(CALL_ALLOCATION)?;
                        entry.insert(ToolCall {
                            id: String::new(),
                            name: String::new(),
                            arguments: String::new(),
                        })
                    }
                };
                if let Some(id) = call["id"].as_str().filter(|id| !id.is_empty()) {
                    // OpenAI-compatible gateways are not fully consistent here: some of
                    // them repeat a regenerated id on later deltas for the same index.
                    // `index` is the stable identity during one streamed choice, so keep
                    // the first usable id and continue assembling the call. The stored
                    // id is the one used in the assistant/tool history pair.
                    if partial.id.is_empty() {
                        append_response_text(&mut self.memory, &mut partial.id, id)?;
                    }
                }
                if let Some(name) = call["function"]["name"].as_str() {
                    append_response_text(&mut self.memory, &mut partial.name, name)?;
                }
                if let Some(args) = call["function"]["arguments"].as_str() {
                    append_response_text(&mut self.memory, &mut partial.arguments, args)?;
                }
            }
            if let Some(reason) = choice["finish_reason"].as_str() {
                if let Some(previous) = self.finish_reason.take() {
                    self.memory.release(previous.capacity());
                }
                let mut finish = String::new();
                append_response_text(&mut self.memory, &mut finish, reason)?;
                self.finish_reason = Some(finish);
            }
        }
        delta._memory = Some(delta_memory);
        Ok(delta)
    }
    pub(super) fn validate(&self) -> AppResult<()> {
        if !self.done {
            return Err(AppError::Invalid("模型流意外断开，工具不会执行".into()));
        }
        match self.finish_reason.as_deref().unwrap_or("缺失") {
            "stop" if self.calls.is_empty() => (),
            "tool_calls" if !self.calls.is_empty() => (),
            "stop" | "tool_calls" => return Err(AppError::Invalid("工具调用未完整结束".into())),
            reason => {
                return Err(AppError::Invalid(format!(
                    "模型回复未完整结束，工具不会执行 (finish_reason = {reason})"
                )))
            }
        }
        let mut ids = std::collections::HashSet::new();
        for c in self.calls.values() {
            if c.id.is_empty()
                || c.id.len() > 256
                || c.name.is_empty()
                || c.name.len() > 128
                || !ids.insert(c.id.as_str())
            {
                return Err(AppError::Invalid("工具调用缺少或重复 ID/名称".into()));
            }
            let _scratch = ResponseMemory::json(self.memory.limit, c.arguments.as_bytes())?;
            if !serde_json::from_str::<Value>(&c.arguments)?.is_object() {
                return Err(AppError::Invalid("工具参数必须为 JSON 对象".into()));
            }
        }
        Ok(())
    }
    pub fn calls(&self) -> AppResult<Vec<ToolCall>> {
        self.validate()?;
        Ok(self.calls.values().cloned().collect())
    }
}

#[cfg(test)]
impl Default for Completion {
    fn default() -> Self {
        Self::new(ResponseLimit::default())
    }
}

// Include the map key, node capacity slack and metadata for sparse tool entries.
const CALL_ALLOCATION: usize = 2 * size_of::<ToolCall>() + 64;

fn append_response_text(
    memory: &mut ResponseMemory,
    text: &mut String,
    part: &str,
) -> AppResult<()> {
    let needed = text
        .len()
        .checked_add(part.len())
        .ok_or_else(response_memory_error)?;
    let capacity = text.capacity();
    if needed > capacity {
        let extra = memory.reserve_buffer(needed, capacity)?;
        if text
            .try_reserve_exact(capacity + extra - text.len())
            .is_err()
        {
            memory.release(extra);
            return Err(response_memory_error());
        }
    }
    text.push_str(part);
    Ok(())
}

fn json_heap_estimate(value: &Value) -> usize {
    match value {
        Value::String(text) => text.capacity(),
        Value::Array(items) => items.iter().fold(
            items.capacity().saturating_mul(size_of::<Value>()),
            |sum, item| sum.saturating_add(json_heap_estimate(item)),
        ),
        Value::Object(fields) => fields.iter().fold(1024_usize, |sum, (key, value)| {
            sum.saturating_add(128)
                .saturating_add(key.capacity())
                .saturating_add(json_heap_estimate(value))
        }),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn completion_error_envelopes_classify_explicit_rejections() {
        for value in [
            json!({"error":{"type":"context_length_exceeded","message":"reduce the prompt"},"choices":[]}),
            json!({"error":"Maximum context length exceeded"}),
            json!({"error":{"code":400,"metadata":{"error_type":"context_length_exceeded"}}}),
            json!({"choices":[{"index":0,"error":{"code":"context_length_exceeded"},"finish_reason":"error"}]}),
        ] {
            let body = value.to_string();
            assert!(
                matches!(
                    Completion::from_json(body.as_bytes()),
                    Err(AppError::ContextLength(_))
                ),
                "{value}"
            );
            assert!(
                matches!(
                    Completion::default().accept(&body),
                    Err(AppError::ContextLength(_))
                ),
                "{value}"
            );
        }
        let other = r#"{"error":{"code":"rate_limit_exceeded","message":"Reduce your total tokens per minute"}}"#;
        assert!(matches!(
            Completion::from_json(other.as_bytes()),
            Err(AppError::Other(_))
        ));
        assert!(matches!(
            Completion::default().accept(other),
            Err(AppError::Other(_))
        ));
    }

    #[test]
    fn normal_completion_content_and_length_finish_are_not_context_rejections() {
        let content =
            "This model's maximum context length is 4096 tokens (context_length_exceeded).";
        for finish_reason in ["stop", "length"] {
            let choice_error = json!({"code":"context_length_exceeded"});
            let body = json!({"error":null,"choices":[{
                "finish_reason":finish_reason,"error":choice_error,
                "message":{"role":"assistant","content":content},
            }]})
            .to_string();
            let buffered = Completion::from_json(body.as_bytes());
            let mut streamed = Completion::default();
            let frame = json!({"error":null,"choices":[{
                "finish_reason":finish_reason,"error":choice_error,"delta":{"content":content},
            }]})
            .to_string();
            assert_eq!(streamed.accept(&frame).unwrap().content, content);
            streamed.accept("[DONE]").unwrap();
            if finish_reason == "stop" {
                assert_eq!(buffered.ok().unwrap().content, content);
                assert!(streamed.validate().is_ok());
            } else {
                assert!(matches!(buffered, Err(AppError::Invalid(_))));
                assert!(matches!(streamed.validate(), Err(AppError::Invalid(_))));
            }
        }
    }
    #[test]
    fn buffered_completion_requires_matching_finish_reason_and_valid_calls() {
        let call =
            json!({"type":"function","id":"one","function":{"name":"read_file","arguments":"{}"}});
        for (finish, calls) in [
            ("stop", vec![call.clone()]),
            ("tool_calls", vec![]),
            ("length", vec![call.clone()]),
            ("tool_calls", vec![call.clone(), call]),
        ] {
            let body = json!({"choices":[{"finish_reason":finish,"message":{"role":"assistant","content":"text","tool_calls":calls}}]});
            assert!(Completion::from_json(body.to_string().as_bytes()).is_err());
        }
    }
    #[test]
    fn buffered_completion_accepts_more_than_64_distinct_calls() {
        let calls: Vec<_> = (0..80)
            .map(|i| json!({"type":"function","id":format!("call_{i}"),"function":{"name":"read_file","arguments":"{}"}}))
            .collect();
        let body = json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","tool_calls":calls}}]});
        let completion = Completion::from_json(body.to_string().as_bytes()).unwrap();
        let ids: Vec<_> = completion
            .calls()
            .unwrap()
            .into_iter()
            .map(|call| call.id)
            .collect();
        assert_eq!(
            ids,
            (0..80).map(|i| format!("call_{i}")).collect::<Vec<_>>()
        );
    }

    #[test]
    fn streamed_completion_accepts_more_than_64_calls_and_sparse_u64_indices() {
        let indices: Vec<_> = (0..79).chain([u64::MAX]).collect();
        let mut completion = Completion::default();
        for &index in indices.iter().rev() {
            let frame = json!({"choices":[{"delta":{"tool_calls":[{
                "index": index, "id": format!("call_{index}"),
                "function":{"name":"read_file","arguments":"{"}
            }]}}]});
            completion.accept(&frame.to_string()).unwrap();
        }
        for &index in &indices {
            let frame = json!({"choices":[{"delta":{"tool_calls":[{
                "index":index,"function":{"arguments":"}"}
            }]}}]});
            completion.accept(&frame.to_string()).unwrap();
        }
        completion
            .accept(r#"{"choices":[{"finish_reason":"tool_calls"}]}"#)
            .unwrap();
        completion.accept("[DONE]").unwrap();
        let calls = completion.calls().unwrap();
        assert_eq!(calls.len(), indices.len());
        for (call, index) in calls.into_iter().zip(indices) {
            assert_eq!(call.id, format!("call_{index}"));
            assert_eq!(call.arguments, "{}");
        }
    }

    #[test]
    fn streamed_completion_rejects_invalid_indices_and_duplicate_call_ids() {
        for index in [Value::Null, json!(-1), json!("5"), json!(1.5)] {
            let frame = json!({"choices":[{"delta":{"tool_calls":[{"index":index}]}}]});
            assert!(Completion::default().accept(&frame.to_string()).is_err());
        }
        let mut completion = Completion::default();
        let calls = [65, u64::MAX].map(|index| {
            json!({
                "index":index,"id":"duplicate","function":{"name":"read_file","arguments":"{}"}
            })
        });
        let frame =
            json!({"choices":[{"delta":{"tool_calls":calls},"finish_reason":"tool_calls"}]});
        completion.accept(&frame.to_string()).unwrap();
        completion.accept("[DONE]").unwrap();
        assert!(completion.calls().is_err());
    }

    #[test]
    fn splits_utf8_crlf_comments_and_multiline_frames() {
        let input = ": heartbeat\r\ndata: 你好\r\ndata: again\r\n\r\ndata: [DONE]\n\n";
        let mut decoder = SseDecoder::new(ResponseLimit::default());
        let mut frames = Vec::new();
        for byte in input.as_bytes() {
            decoder
                .push(&[*byte], |frame| {
                    frames.push(frame.to_owned());
                    Ok(false)
                })
                .unwrap();
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
    fn keeps_the_first_id_when_a_gateway_rewrites_it_for_the_same_index() {
        let mut c = Completion::default();
        c.accept(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"gateway-call-a","function":{"name":"read_file","arguments":"{\"path\":"}}]}}]}"#).unwrap();
        c.accept(r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"gateway-call-b","function":{"arguments":"\"mods\"}"}}]},"finish_reason":"tool_calls"}]}"#).unwrap();
        c.accept("[DONE]").unwrap();
        let calls = c.calls().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "gateway-call-a");
        assert_eq!(calls[0].arguments, r#"{"path":"mods"}"#);
    }

    #[test]
    fn large_frames_and_accumulated_content_exceed_old_limits_without_truncation() {
        // These sizes are test inputs, not the operating limit: the latter is
        // a simulated local memory budget, just as production probes the OS.
        let limit = ResponseLimit::for_test(128 * 1024 * 1024);
        let mut decoder = SseDecoder::new(limit);
        let mut completion = Completion::new(limit);
        let text = "界".repeat(750_001);
        let frame = format!(
            "data: {}\n\n",
            json!({"choices":[{"delta":{"content":text}}]})
        );
        let observe = |frame: &str, completion: &mut Completion| -> AppResult<bool> {
            let delta = completion.accept(frame)?;
            if !delta.content.is_empty() {
                assert_eq!(delta.content, text);
            }
            Ok(completion.done)
        };
        assert!(frame.len() > 2 * 1024 * 1024);
        for _ in 0..8 {
            for chunk in frame.as_bytes().chunks(16_381) {
                decoder
                    .push(chunk, |frame| observe(frame, &mut completion))
                    .unwrap();
            }
        }
        decoder
            .push(
                b"data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
                |frame| observe(frame, &mut completion),
            )
            .unwrap();
        completion.validate().unwrap();
        assert!(completion.content.len() > 16 * 1024 * 1024);
        assert_eq!(completion.content, text.repeat(8));
    }

    #[test]
    fn resource_budget_blocks_unterminated_lines_and_releases_reservations() {
        let limit = ResponseLimit::for_test(768 * 1024);
        let mut first = SseDecoder::new(limit);
        let mut second = SseDecoder::new(limit);
        let block = [b'x'; 4096];
        for _ in 0..128 {
            first
                .push(&block, |_| panic!("an incomplete line must not dispatch"))
                .unwrap();
        }
        assert!(second
            .push(&block.repeat(65), |_| panic!("out of memory"))
            .is_err());
        drop(first);
        second
            .push(&block.repeat(65), |_| panic!("still incomplete"))
            .unwrap();
        assert!(second
            .push(&block.repeat(128), |_| panic!("out of memory"))
            .is_err());
    }

    #[test]
    fn resource_budget_accounts_for_decoded_structure_and_accumulated_output() {
        let limit = ResponseLimit::for_test(128 * 1024);
        let array = "0,".repeat(2048);
        assert!(matches!(
            Completion::new(limit).accept(&format!(r#"{{"usage":[{array}0]}}"#)),
            Err(AppError::Invalid(_))
        ));
        let mut completion = Completion::new(limit);
        let frame = json!({"choices":[{"delta":{"content":"x".repeat(4096)}}]}).to_string();
        for _ in 0..12 {
            completion.accept(&frame).unwrap();
        }
        // The frame still fits by itself, but repeated retained output must not
        // accumulate unboundedly even if every individual delta is small.
        assert!((0..32).any(|_| completion.accept(&frame).is_err()));
        assert!(!completion.done);
    }

    #[test]
    fn malformed_event_shapes_are_protocol_errors() {
        for malformed in ["[]", r#"{"choices":"not a list"}"#] {
            assert!(matches!(
                Completion::default().accept(malformed),
                Err(AppError::Invalid(_))
            ));
        }
    }

    #[test]
    fn rejects_truncated_calls() {
        let mut c = Completion::default();
        c.accept(r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#)
            .unwrap();
        c.accept("[DONE]").unwrap();
        assert!(c.calls().is_err());
    }
}
