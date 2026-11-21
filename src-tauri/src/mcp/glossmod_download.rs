//! The Mod Inn MCP issues a short-lived URL for a selected attachment. Consume it
//! inside the approved Agent call so credentials and signed links stay out of history.
use super::client::McpClient;
use crate::{
    agent::{
        policy::Effect,
        tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition},
    },
    artifacts,
    download::{DownloadManager, ProgressCallback},
    error::{AppError, AppResult},
    sources::{resolutions::ResolutionScope, web_download::MAX_WEB_BYTES},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{num::NonZeroU64, sync::Arc};

pub(super) const TOOL: &str = "get_work_file_download";

pub(super) struct GlossModDownload {
    client: McpClient,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Attachment {
    work_id: NonZeroU64,
    file_id: NonZeroU64,
}

impl Attachment {
    fn parse(args: &Value) -> AppResult<Self> {
        serde_json::from_value(args.clone()).map_err(|_| {
            AppError::Invalid("请选择 Mod 客栈附件列表中的正整数 work_id 和 file_id".into())
        })
    }
}

fn selected_attachment(context: &ToolContext, args: &Value) -> AppResult<Attachment> {
    if context.cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    if !matches!(context.resolution_scope, ResolutionScope::Agent(_)) {
        return Err(AppError::Denied("此附件下载只供桌面 Agent 审批".into()));
    }
    Attachment::parse(args)
}

impl GlossModDownload {
    pub(super) fn new(client: McpClient, name: String) -> Self {
        Self { client, name }
    }

    fn preview(&self, attachment: &Attachment) -> Value {
        json!({"serverId":self.client.server_id(),"endpoint":self.client.server_config().url,"tool":TOOL,
            "work_id":attachment.work_id,"file_id":attachment.file_id,"destination":"downloads/","maxBytes":MAX_WEB_BYTES,
            "warning":"由 Mod 客栈签发选定附件的下载地址，并将归档保存到本机。成功签发会计一次下载；安装需另行审核。"})
    }
}

#[async_trait::async_trait]
impl AgentTool for GlossModDownload {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.clone(),
            description: format!("[Mod 客栈 MCP: {}] {TOOL} — 下载 list_work_files 中已选定的附件。一次审批完成签发并保存到管理器 downloads，成功返回本机 artifact.path、大小和 SHA-256。签发计入下载次数，只在确需下载时调用。", self.client.server_id()),
            parameters: json!({"type":"object","properties":{"work_id":{"type":"integer","minimum":1},"file_id":{"type":"integer","minimum":1}},"required":["work_id","file_id"],"additionalProperties":false}),
            effect: Effect::UncatalogedDownload,
        }
    }

    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        let attachment = selected_attachment(context, args)?;
        Ok(PreparedTool {
            preview: self.preview(&attachment),
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
        let attachment = selected_attachment(context, &args)?;
        if prepared.effect != Effect::UncatalogedDownload
            || prepared.preview != self.preview(&attachment)
            || prepared.plan.is_some()
        {
            return Err(AppError::Denied("下载的作品或附件与审批不一致".into()));
        }
        let result = self.client.call_tool(TOOL, args, &context.cancel).await?;
        self.client.ensure_current()?;
        save_attachment(&self.client, context, &attachment, &result).await
    }
}

async fn save_attachment(
    client: &McpClient,
    context: &ToolContext,
    attachment: &Attachment,
    result: &Value,
) -> AppResult<Value> {
    let (url, name) = signed_archive(result)?;
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
    }?;
    let cataloged = artifacts::record_glossmod_file(
        &context.engine,
        &artifact,
        attachment.work_id.get(),
        attachment.file_id.get(),
    )
    .is_ok();
    let mut receipt = json!({"artifact":artifact.into_user_paths(),"cataloged":cataloged,"work_id":attachment.work_id,"file_id":attachment.file_id});
    if !cataloged {
        receipt["warning"] = json!("归档已下载，但未能写入下载记录。请保留文件，不要重新下载。");
    }
    Ok(receipt)
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

#[cfg(test)]
mod tests {
    use super::*;

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
