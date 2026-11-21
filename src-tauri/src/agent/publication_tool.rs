use super::{
    policy::Effect,
    tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition, ToolRegistry},
};
use crate::{
    error::{AppError, AppResult},
    manager_publication,
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
    draft_id: Option<String>,
    slug: Option<String>,
    #[serde(default)]
    plugin_id: Option<i64>,
    #[serde(default)]
    tags: Vec<String>,
}

#[async_trait::async_trait]
impl AgentTool for PublishPlugin {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "publish_plugin".into(),
            description: "发布到 Mod客栈。已提交草稿 draft_id 与已验证安装包 slug 二选一。首发省略 plugin_id，新版提供本人目标 ID 和完整标签。审批核对目标、两份原文及摘要。".into(),
            parameters: json!({"type":"object","properties":{"draft_id":{"type":"string"},"slug":{"type":"string"},"plugin_id":{"type":"integer","minimum":1},"tags":{"type":"array","items":{"type":"string"}}},"additionalProperties":false}),
            effect: Effect::Publish,
        }
    }
    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        if context.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let args: PublishArgs = serde_json::from_value(args.clone()).map_err(|_| {
            AppError::Invalid("请选择 draft_id 或 slug，可另附 plugin_id 与 tags".into())
        })?;
        let target = match args.plugin_id {
            Some(plugin_id) => manager_publication::PublicationTarget::Release { plugin_id },
            None => manager_publication::PublicationTarget::First,
        };
        let engine = context.engine.clone();
        let record = tokio::task::spawn_blocking(move || match (args.draft_id, args.slug) {
            (Some(id), None) => manager_publication::prepare_draft(&engine, &id, target, args.tags),
            (None, Some(slug)) => {
                manager_publication::prepare_installed(&engine, &slug, target, args.tags)
            }
            _ => Err(AppError::Invalid("请选择一个草稿或已安装适配包".into())),
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
        let record: manager_publication::PublicationRecord =
            serde_json::from_value(prepared.preview)?;
        let id = record.id.clone();
        let digest = record.preview.digest.clone();
        let engine = context.engine.clone();
        tokio::task::spawn_blocking(move || {
            manager_publication::approve_publication(&engine, &id, &digest)
        })
        .await
        .map_err(|_| AppError::Other("发布审批失败".into()))??;
        let result = manager_publication::publish_approved(
            &context.engine,
            &self.settings,
            &record.id,
            &record.preview.digest,
            &context.cancel,
        )
        .await?;
        Ok(
            json!({"publicationId":result.id,"state":result.state,"plugin":result.remote,"target":result.preview.target,"previewDigest":result.preview.digest}),
        )
    }
}

#[cfg(test)]
#[path = "publication_tool_tests.rs"]
mod tests;
