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
    registry.register(Arc::new(PublishPlugin {
        settings,
        #[cfg(test)]
        test_preparation: None,
    }))
}
struct PublishPlugin {
    settings: Settings,
    #[cfg(test)]
    test_preparation: Option<TestPreparation>,
}

#[cfg(test)]
struct TestPreparation {
    remote: Arc<dyn manager_publication::Remote>,
    listing: crate::publication_copy::PublicationListing,
}

#[cfg(test)]
pub(crate) fn test_tool(
    settings: Settings,
    remote: Arc<dyn manager_publication::Remote>,
    listing: crate::publication_copy::PublicationListing,
) -> Arc<dyn AgentTool> {
    Arc::new(PublishPlugin {
        settings,
        test_preparation: Some(TestPreparation { remote, listing }),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishArgs {
    draft_id: Option<String>,
    slug: Option<String>,
}

#[async_trait::async_trait]
impl AgentTool for PublishPlugin {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "publish_plugin".into(),
            description: "准备发布到 Mod客栈。只提供 draft_id 或 slug 中的一项；管理器核对自动生成的发布信息和完整文件原文，并按当前权限模式提交。".into(),
            parameters: json!({"type":"object","properties":{"draft_id":{"type":"string","minLength":1},"slug":{"type":"string","minLength":1}},"oneOf":[{"required":["draft_id"]},{"required":["slug"]}],"additionalProperties":false}),
            effect: Effect::Publish,
        }
    }
    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        if context.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let args: PublishArgs = serde_json::from_value(args.clone())
            .map_err(|_| AppError::Invalid("请只提供 draft_id 或 slug 中的一项".into()))?;
        let (draft_id, slug) = match (args.draft_id, args.slug) {
            (Some(id), None) if !id.trim().is_empty() => (Some(id), None),
            (None, Some(slug)) if !slug.trim().is_empty() => (None, Some(slug)),
            _ => {
                return Err(AppError::Invalid(
                    "请只提供 draft_id 或 slug 中的一项".into(),
                ))
            }
        };
        #[cfg(test)]
        let record = match &self.test_preparation {
            Some(test) => {
                manager_publication::prepare_automatic_with(
                    &context.engine,
                    &self.settings,
                    test.remote.as_ref(),
                    draft_id,
                    slug,
                    test.listing.clone(),
                    &context.cancel,
                )
                .await?
            }
            None => {
                manager_publication::prepare_automatic(
                    &context.engine,
                    &self.settings,
                    draft_id,
                    slug,
                    &context.cancel,
                )
                .await?
            }
        };
        #[cfg(not(test))]
        let record = manager_publication::prepare_automatic(
            &context.engine,
            &self.settings,
            draft_id,
            slug,
            &context.cancel,
        )
        .await?;
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
pub(crate) mod tests;
