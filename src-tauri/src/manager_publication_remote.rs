//! The fixed manager-plugin tool allowlist and response contract. MCP session,
//! authentication, bounded decoding, and retry policy are shared with works.
use super::{unavailable, Remote};
use crate::{
    error::{AppError, AppResult},
    plugin_publish_remote::PublicationMcp,
    settings::Settings,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

// 384 KiB of decoded file bytes can expand when JSON-escaped. Test the actual
// wire size rather than imposing the old ordinary-work 256 KiB argument cap.
const MAX_REQUEST: usize = 3 * 1024 * 1024;
// MCP can include the same data as structured JSON and escaped text fallback.
const MAX_RESPONSE: usize = 8 * 1024 * 1024;

pub(super) struct Transport(PublicationMcp);

impl Transport {
    pub(super) async fn connect(
        settings: &Settings,
        cancel: &CancellationToken,
    ) -> AppResult<Self> {
        Ok(Self(
            PublicationMcp::connect(settings, MAX_RESPONSE, cancel).await?,
        ))
    }

    #[cfg(test)]
    pub(super) async fn connect_to(
        url: &str,
        key: String,
        cancel: &CancellationToken,
    ) -> AppResult<Self> {
        Ok(Self(
            PublicationMcp::connect_to(url, key, MAX_RESPONSE, cancel).await?,
        ))
    }
}

fn single_text(reply: &Value) -> Option<&str> {
    let [item] = reply.get("content")?.as_array()?.as_slice() else {
        return None;
    };
    (item["type"] == "text")
        .then(|| item["text"].as_str())
        .flatten()
}

// Documented pre-commit failures in the provider's managerplugins store and
// author scope gate. Never return untrusted text, even for a known rejection.
fn explicit_rejection(tool: &str, message: &str) -> Option<AppError> {
    let publishing = matches!(
        tool,
        "publish_manager_plugin" | "publish_manager_plugin_release"
    );
    let scope = if publishing {
        "manager_plugins.publish"
    } else {
        "manager_plugins.read"
    };
    Some(match message {
        "this api key does not have the required scope" => {
            AppError::Denied(format!("Mod 客栈 API Key 缺少 {scope} 权限"))
        }
        "manager plugin action forbidden" if publishing => {
            AppError::Denied("Mod 客栈拒绝发布，请确认创作者身份已审核且有效".into())
        }
        "manager plugin not found"
            if matches!(
                tool,
                "publish_manager_plugin_release" | "get_my_manager_plugin_texts"
            ) =>
        {
            AppError::NotFound("该管理器插件不存在或不属于当前账号，请核对本人插件 ID".into())
        }
        "manager plugin slug or release version already exists" if publishing => {
            AppError::Conflict("该插件或版本已存在：请选择本人已有插件并提交未发布的新版本".into())
        }
        "invalid manager plugin input: cannot change a plugin slug" if publishing => {
            AppError::Invalid(
                "新版 adapter.json 的 slug 必须与目标插件保持一致，请核对目标和原文".into(),
            )
        }
        "invalid manager plugin input"
        | "invalid manager plugin input: 标签必须采用 UTF-8 编码且不能包含空字符（NUL）"
            if publishing =>
        {
            AppError::Invalid("Mod 客栈拒绝插件输入，请核对插件 ID、原文及标签后重新预览".into())
        }
        message
            if publishing
                && (message.starts_with("adapter.json：") || message.starts_with("SKILL.md：")) =>
        {
            AppError::Invalid(
                "Mod 客栈未通过插件文件校验，请检查 adapter.json 与 SKILL.md 后重新预览".into(),
            )
        }
        _ => return None,
    })
}

#[async_trait::async_trait]
impl Remote for Transport {
    async fn call(&self, name: &str, args: Value, cancel: &CancellationToken) -> AppResult<Value> {
        if !matches!(
            name,
            "publish_manager_plugin"
                | "publish_manager_plugin_release"
                | "list_my_manager_plugins"
                | "get_my_manager_plugin_texts"
        ) {
            return Err(unavailable());
        }
        if self.0.echoes_key(&args) {
            return Err(AppError::Denied(
                "拒绝将 Mod 客栈凭据作为插件正文或标签发送".into(),
            ));
        }
        // Only the already approved local text upload can exceed the old
        // request budget; incoming MCP responses retain their wire limit.
        let limit = (!matches!(
            name,
            "publish_manager_plugin" | "publish_manager_plugin_release"
        ))
        .then_some(MAX_REQUEST);
        let reply = self.0.call_tool(name, args, limit, cancel).await?;
        if reply["isError"] == true {
            if let (None, Some(message)) = (reply.get("structuredContent"), single_text(&reply)) {
                if let Some(error) = explicit_rejection(name, message) {
                    return Err(error);
                }
            }
            return Err(unavailable());
        }
        let data = match reply.get("structuredContent") {
            Some(content) => content.clone(),
            None => serde_json::from_str(single_text(&reply).ok_or_else(unavailable)?)
                .map_err(|_| unavailable())?,
        };
        if !data.is_object() || self.0.echoes_key(&data) {
            return Err(unavailable());
        }
        Ok(data)
    }
}
