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
        let reply = self.0.call_tool(name, args, MAX_REQUEST, cancel).await?;
        if reply["isError"] == true {
            // These exact early refusals come from the scope/creator guards,
            // before any mutation can occur. Any other failure is ambiguous.
            if reply.get("structuredContent").is_none()
                && matches!(
                    single_text(&reply),
                    Some(
                        "this api key does not have the required scope"
                            | "manager plugin action forbidden"
                    )
                )
            {
                let scope = if matches!(
                    name,
                    "list_my_manager_plugins" | "get_my_manager_plugin_texts"
                ) {
                    "manager_plugins.read"
                } else {
                    "manager_plugins.publish"
                };
                return Err(AppError::Denied(format!(
                    "Mod 客栈拒绝该请求，请检查 {scope} 权限或创作者审核状态"
                )));
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
