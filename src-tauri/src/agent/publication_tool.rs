use super::{
    policy::Effect,
    tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition, ToolRegistry},
};
use crate::{
    error::{AppError, AppResult},
    plugin_publish,
    settings::Settings,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn register(registry: &ToolRegistry, settings: Settings) -> AppResult<()> {
    registry.register(Arc::new(PublishPlugin { settings }))
}
struct PublishPlugin {
    settings: Settings,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishArgs {
    draft_id: String,
    metadata: plugin_publish::PublicationMetadata,
}

#[async_trait::async_trait]
impl AgentTool for PublishPlugin {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "publish_plugin".into(),
            description: "将已验证、已提交的声明式适配包草稿打包为仅含 adapter.json 与 SKILL.md 的 ZIP，向 Mod 客栈上传并发布。准备阶段展示全部文件原文、元数据、SHA-256 和固定目的地，遵守发布审批。严禁夹带游戏文件、密钥或私人数据。网络结果不确定时不会自动重发；扫描未完成时只返回待扫描状态。".into(),
            parameters: json!({"type":"object","properties":{"draft_id":{"type":"string"},"metadata":{"type":"object","properties":{"title":{"type":"string","minLength":2,"maxLength":160},"summary":{"type":"string","maxLength":500},"description":{"type":"string","maxLength":50000},"tags":{"type":"array","maxItems":5,"items":{"type":"string","maxLength":20}},"gameIds":{"type":"array","maxItems":10,"items":{"type":"integer","minimum":1}}},"required":["title","summary","description"],"additionalProperties":false}},"required":["draft_id","metadata"],"additionalProperties":false}),
            effect: Effect::Publish,
        }
    }
    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        if context.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let PublishArgs {
            draft_id: id,
            metadata,
        } = serde_json::from_value(args.clone())
            .map_err(|_| AppError::Invalid("发布参数仅接受 draft_id 与 metadata".into()))?;
        let engine = context.engine.clone();
        let record = tokio::task::spawn_blocking(move || {
            let (preview, _) = plugin_publish::prepare_package(&engine, &id, metadata)?;
            plugin_publish::create_publication(&engine, &preview)
        })
        .await
        .map_err(|_| AppError::Other("准备发布失败".into()))??;
        Ok(PreparedTool {
            preview: json!(record),
            effect: Effect::Publish,
            plan: None,
        })
    }
    async fn execute(
        &self,
        context: &ToolContext,
        _args: Value,
        prepared: PreparedTool,
    ) -> AppResult<Value> {
        if context.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        if prepared.effect != Effect::Publish || prepared.plan.is_some() {
            return Err(AppError::Denied("发布审批类型不匹配".into()));
        }
        let record: plugin_publish::PublicationRecord = serde_json::from_value(prepared.preview)?;
        let id = record.id.clone();
        let digest = record.preview.digest.clone();
        let engine = context.engine.clone();
        tokio::task::spawn_blocking(move || {
            plugin_publish::approve_publication(&engine, &id, &digest)
        })
        .await
        .map_err(|_| AppError::Other("发布审批失败".into()))??;
        let result = crate::plugin_publish_remote::publish_approved(
            &context.engine,
            &self.settings,
            &record.id,
            &record.preview.digest,
            &context.cancel,
        )
        .await?;
        Ok(
            json!({"publicationId":result.id,"state":result.state,"phase":result.phase,"fileId":result.file_id,"workId":result.work_id,"previewDigest":result.preview.digest,"archiveSha256":result.preview.package.archive_sha256}),
        )
    }
}

#[cfg(test)]
#[path = "publication_tool_tests.rs"]
mod tests;
