//! The Mod Inn MCP issues a short-lived URL for a selected attachment. Consume it
//! inside the approved Agent call so credentials and signed links stay out of history.
use super::client::McpClient;
use crate::{
    agent::{
        policy::Effect,
        tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition},
    },
    download::{DownloadManager, ProgressCallback},
    download_tasks::{agent_download_schema, AgentDownloadArgs, CompletedSource, DownloadTarget},
    error::{AppError, AppResult},
    sources::resolutions::ResolutionScope,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{num::NonZeroU64, sync::Arc};

pub(super) const TOOL: &str = "get_work_file_download";

pub(super) struct GlossModDownload {
    client: McpClient,
    name: String,
}

#[derive(Deserialize, Serialize)]
struct Attachment {
    work_id: NonZeroU64,
    file_id: NonZeroU64,
}

fn selected_attachment(
    context: &ToolContext,
    args: &Value,
) -> AppResult<(Attachment, DownloadTarget)> {
    if context.cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    if !matches!(context.resolution_scope, ResolutionScope::Agent(_)) {
        return Err(AppError::Denied("此附件下载只供桌面 Agent 审批".into()));
    }
    let args = AgentDownloadArgs::<Attachment>::parse(args)?;
    Ok((args.transfer, args.classification.for_agent(context)?))
}

impl GlossModDownload {
    pub(super) fn new(client: McpClient, name: String) -> Self {
        Self { client, name }
    }

    fn preview(&self, attachment: &Attachment, classification: &DownloadTarget) -> Value {
        json!({"serverId":self.client.server_id(),"endpoint":self.client.server_config().url,"tool":TOOL,
            "work_id":attachment.work_id,"file_id":attachment.file_id,"destination":"Mods/","classification":classification,
            "sizePolicy":"按实际可用磁盘空间执行，未声明大小的响应使用较小预算",
            "warning":"由 Mod 客栈签发选定附件的下载地址，并将归档保存到本机。成功签发会计一次下载；安装需另行审核。"})
    }
}

#[async_trait::async_trait]
impl AgentTool for GlossModDownload {
    fn host_tool_kind(&self) -> Option<&'static str> {
        Some("glossmod_attachment_download")
    }
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.clone(),
            description: format!("[Mod 客栈 MCP: {}] {TOOL} — 下载 list_work_files 中已选定的附件。聊天未关联游戏时提供登记游戏 ID 或明确的游戏名称，作为 Mods 库游戏分类。按当前权限模式一次完成签发并归档，成功返回 Mods 库 artifact.path、大小和 SHA-256。签发计入下载次数，只在确需下载时调用。", self.client.server_id()),
            parameters: agent_download_schema(json!({"work_id":{"type":"integer","minimum":1},"file_id":{"type":"integer","minimum":1}}), vec!["work_id","file_id"]),
            effect: Effect::UncatalogedDownload,
        }
    }

    fn history_arguments(&self, raw: &str) -> Option<String> {
        Some(
            AgentDownloadArgs::<Attachment>::from_history(raw)
                .and_then(|args| serde_json::to_string(&args).ok())
                .unwrap_or_else(|| "{}".into()),
        )
    }

    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        let (attachment, classification) = selected_attachment(context, args)?;
        Ok(PreparedTool {
            preview: self.preview(&attachment, &classification),
            effect: Effect::UncatalogedDownload,
            plan: None,
        })
    }

    async fn execute(
        &self,
        context: &ToolContext,
        args: Value,
        prepared: PreparedTool,
    ) -> AppResult<Value> {
        let (attachment, classification) = selected_attachment(context, &args)?;
        if prepared.effect != Effect::UncatalogedDownload
            || prepared.preview != self.preview(&attachment, &classification)
            || prepared.plan.is_some()
        {
            return Err(AppError::Denied("下载的作品或附件与审批不一致".into()));
        }
        // Only send the official remote schema. Local library metadata is not
        // part of the signed attachment request and must not leave this device.
        let result = self
            .client
            .call_tool(TOOL, json!(&attachment), &context.cancel)
            .await?;
        self.client.ensure_current()?;
        save_attachment(&self.client, context, &attachment, classification, &result).await
    }
}

async fn save_attachment(
    client: &McpClient,
    context: &ToolContext,
    attachment: &Attachment,
    classification: DownloadTarget,
    result: &Value,
) -> AppResult<Value> {
    let (url, name) = signed_archive(result)?;
    let version = signed_version(&result["structuredContent"], attachment);
    let progress = context.progress.as_ref().map(|notify| {
        let notify = notify.clone();
        Arc::new(move |progress| notify(json!({"download": progress}))) as ProgressCallback
    });
    let manager = DownloadManager::new(&context.engine.paths)?;
    let cancel = context.cancel.child_token();
    let transfer = manager.download_mcp_archive(url, name, None, cancel.clone(), progress);
    tokio::pin!(transfer);
    let artifact = tokio::select! {
        biased;
        _ = client.disconnected() => { cancel.cancel(); transfer.await }
        result = &mut transfer => result,
    }
    .map_err(|error| attachment_transfer_error(error, attachment))?;
    let artifact = classification
        .finish(
            &context.engine,
            artifact,
            CompletedSource::ModInn {
                work_id: attachment.work_id.get(),
                file_id: attachment.file_id.get(),
                version,
                url,
            },
        )
        .await?;
    let id = artifact.artifact_id.clone();
    let mut artifact = serde_json::to_value(artifact.into_user_paths())?;
    if let Some(version) = version {
        artifact["version"] = json!(version);
    }
    Ok(
        json!({"artifactId":id,"artifact":artifact,"cataloged":true,"work_id":attachment.work_id,"file_id":attachment.file_id}),
    )
}

fn attachment_transfer_error(error: AppError, attachment: &Attachment) -> AppError {
    match error {
        // Keep the transfer a failure, but never copy resolver detail from a
        // request that used a signed URL into model-visible tool history.
        AppError::DownloadDns(_) => AppError::DownloadDns(format!(
            "Mod 客栈已签发所选附件（work_id={}，file_id={}），但本机 DNS 解析失败，附件未保存。\
             不要重新签发或先用 ask_user 询问。若 Playwright MCP 已连接，按正常审批流程打开已核实的 Mod 客栈官方作品页或文件页，\
             从正常入口下载同一附件。不要将本次签发的 URL 转交浏览器。",
            attachment.work_id, attachment.file_id
        )),
        other => other,
    }
}

fn signed_archive(result: &Value) -> AppResult<(&str, &str)> {
    if result["isError"] == true {
        return Err(AppError::Other(
            "Mod 客栈未签发所选附件，请核对文件状态和账号权限".into(),
        ));
    }
    // The official outputSchema requires these structured fields. Never echo the
    // text fallback, remote errors or other untrusted response fields to the model.
    let file = &result["structuredContent"];
    let (Some(url), Some(name)) = (file["url"].as_str(), file["name"].as_str()) else {
        return Err(AppError::Invalid("Mod 客栈未返回可下载的附件信息".into()));
    };
    Ok((url, name))
}

/// A successful signing response describes the one approved attachment. The
/// official schemas currently supply no display version: the list's
/// version_type (latest/history) and file_id are not substitutes. Consume only
/// an explicit attachment label if the existing signing response supplies one.
fn signed_version<'a>(file: &'a Value, attachment: &Attachment) -> Option<&'a str> {
    if file
        .get("work_id")
        .is_some_and(|id| id.as_u64() != Some(attachment.work_id.get()))
        || file
            .get("file_id")
            .is_some_and(|id| id.as_u64() != Some(attachment.file_id.get()))
    {
        return None;
    }
    file["version"]
        .as_str()
        .and_then(crate::mod_metadata::display_version)
        .filter(|label| {
            !label.contains(['/', '\\', '?'])
                && !label.contains("[REDACTED]")
                && !["latest", "history"]
                    .iter()
                    .any(|kind| label.eq_ignore_ascii_case(kind))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_attachment_dns_failure_gives_safe_page_recovery_without_resigning() {
        let attachment = serde_json::from_value(json!({"work_id":42,"file_id":103})).unwrap();
        let error = attachment_transfer_error(
            AppError::DownloadDns("https://cdn.example.test/signed?key=private-ticket".into()),
            &attachment,
        );
        assert!(matches!(error, AppError::DownloadDns(_)));
        let message = error.to_string();
        for expected in [
            "已签发",
            "work_id=42，file_id=103",
            "本机 DNS 解析失败，附件未保存",
            "不要重新签发或先用 ask_user 询问",
            "Playwright MCP 已连接",
            "按正常审批流程",
            "已核实的 Mod 客栈官方作品页或文件页",
            "从正常入口下载同一附件",
            "不要将本次签发的 URL 转交浏览器",
        ] {
            assert!(
                message.contains(expected),
                "missing {expected:?}: {message}"
            );
        }
        assert!(!message.contains("cdn.example.test"));
        assert!(!message.contains("private-ticket"));
    }

    #[test]
    fn non_dns_failures_do_not_trigger_page_recovery() {
        let attachment = serde_json::from_value(json!({"work_id":42,"file_id":103})).unwrap();
        for error in [
            AppError::Denied("没有访问附件的权限".into()),
            AppError::Denied("用户拒绝了此操作".into()),
            AppError::Denied("下载域名解析到非公网地址".into()),
            AppError::Invalid("网页下载地址格式无效或过长".into()),
            AppError::Cancelled,
            AppError::Other("下载返回 HTTP 503".into()),
            AppError::Other("本机 DNS 解析失败".into()),
        ] {
            let original_variant = std::mem::discriminant(&error);
            let original_message = error.to_string();
            let mapped = attachment_transfer_error(error, &attachment);
            assert_eq!(std::mem::discriminant(&mapped), original_variant);
            assert_eq!(mapped.to_string(), original_message);
        }
    }

    #[test]
    fn signed_version_requires_a_safe_explicit_label_for_the_approved_attachment() {
        let attachment = serde_json::from_value(json!({"work_id":42,"file_id":103})).unwrap();
        for missing in [
            json!({"file_id":103,"version_id":"103","version_type":"history","mod_version":"2.0"}),
            json!({"file_id":103,"version":103}),
            json!({"version":"history"}),
            json!({"file_id":104,"version":"1.2.3"}),
            json!({"work_id":43,"version":"1.2.3"}),
            json!({"version":" "}),
            json!({"version":"https://cdn.example.test/a?signature=private-ticket"}),
            json!({"version":"[REDACTED]"}),
        ] {
            assert_eq!(signed_version(&missing, &attachment), None);
        }
        assert_eq!(
            signed_version(
                &json!({"work_id":42,"file_id":103,"version":" 1.2.3 "}),
                &attachment
            ),
            Some("1.2.3")
        );
    }

    #[test]
    fn only_successful_structured_attachment_data_reaches_the_downloader() {
        let file =
            json!({"url":"https://cdn.example.test/signed?key=private-ticket","name":"one.zip"});
        for invalid in [
            json!({"isError":true,"structuredContent":file,"content":[{"text":"private-ticket"}]}),
            json!({"content":[{"type":"text","text":file.to_string()}]}),
            json!({"structuredContent":{"url":file["url"]}}),
        ] {
            let error = signed_archive(&invalid).unwrap_err().to_string();
            assert!(!error.contains("private-ticket"));
        }
    }
}
