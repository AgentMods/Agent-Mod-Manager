use super::{
    policy::Effect,
    tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition, ToolRegistry},
};
use crate::{
    error::{AppError, AppResult},
    plugin_workflow,
};
use serde_json::{json, Value};
use std::sync::Arc;

pub fn register(registry: &ToolRegistry) -> AppResult<()> {
    registry.register_batch(
        [
            "write_plugin_draft",
            "list_plugin_drafts",
            "validate_plugin",
            "install_plugin",
        ]
        .into_iter()
        .map(|name| Arc::new(PluginTool { name }) as Arc<dyn AgentTool>)
        .collect(),
    )
}

struct PluginTool {
    name: &'static str,
}

fn text<'a>(args: &'a Value, name: &str) -> AppResult<&'a str> {
    args[name]
        .as_str()
        .ok_or_else(|| AppError::Invalid(format!("缺少字符串参数: {name}")))
}

#[async_trait::async_trait]
impl AgentTool for PluginTool {
    fn definition(&self) -> ToolDefinition {
        let (description, properties, required, effect) = match self.name {
            "write_plugin_draft" => (
                "创建仅含 adapter.json 与 SKILL.md 的声明式插件草稿；展示原文和实际文件计划，批准后才提交。不执行代码，不允许覆盖内置 slug。",
                json!({"slug":{"type":"string"},"adapter_json":{"type":"string"},"skill_markdown":{"type":"string"}}),
                vec!["slug", "adapter_json", "skill_markdown"],
                Effect::ManagerWrite,
            ),
            "list_plugin_drafts" => (
                "列出已提交且可继续验证的插件草稿，不修改文件。",
                json!({}), vec![], Effect::Read,
            ),
            "validate_plugin" => (
                "校验草稿结构、路径、Skill 身份与事务哈希。通过校验不代表兼容性或指令可信，仍需用户审查。",
                json!({"draft_id":{"type":"string"}}), vec!["draft_id"], Effect::Read,
            ),
            _ => (
                "审查并安装已提交的声明式插件草稿；展示两个文件原文与覆盖影响，批准后事务提交并登记。不发布、不执行代码。",
                json!({"draft_id":{"type":"string"}}), vec!["draft_id"], Effect::ManagerWrite,
            ),
        };
        ToolDefinition {
            name: self.name.into(),
            description: description.into(),
            effect,
            parameters: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
        }
    }

    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        if context.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let engine = context.engine.clone();
        let name = self.name;
        let args = args.clone();
        let effect = self.definition().effect;
        tokio::task::spawn_blocking(move || {
            let (preview, plan) = match name {
                "write_plugin_draft" => {
                    let draft = plugin_workflow::plan_write_draft(&engine, text(&args, "slug")?, text(&args, "adapter_json")?, text(&args, "skill_markdown")?)?;
                    (json!({"draft":draft.draft,"plan":draft.plan,"adapterJson":args["adapter_json"],"skillMarkdown":args["skill_markdown"],"trust":"untrusted_instructions"}), Some(draft.plan))
                }
                "install_plugin" => {
                    let id = text(&args, "draft_id")?;
                    let validation = plugin_workflow::validate_draft(&engine, id)?;
                    let deployment = plugin_workflow::plan_install_plugin(&engine, id)?;
                    (json!({"deployment":deployment,"package":validation.package,"warnings":validation.warnings,"trust":"untrusted_instructions"}), Some(deployment.plan))
                }
                _ => (args, None),
            };
            Ok(PreparedTool { preview, plan, effect })
        }).await.map_err(|error| AppError::Other(error.to_string()))?
    }

    async fn execute(
        &self,
        context: &ToolContext,
        args: Value,
        prepared: PreparedTool,
    ) -> AppResult<Value> {
        if context.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let engine = context.engine.clone();
        let name = self.name;
        // Once a commit starts it is joined, not dropped on cancellation.
        tokio::task::spawn_blocking(move || {
            if let Some(plan) = prepared.plan {
                engine.approve_plan(&plan.id)?;
                engine.apply_file_ops(&plan.id)?;
                let reconciliation = plugin_workflow::reconcile_deployments(&engine)?;
                crate::skills::reconcile_deployments(&engine)?;
                return Ok(json!({"status":"committed","plan":plan,"preview":prepared.preview,"reconciliation":reconciliation}));
            }
            match name {
                "list_plugin_drafts" => Ok(json!(plugin_workflow::list_drafts(&engine)?)),
                "validate_plugin" => Ok(json!(plugin_workflow::validate_draft(&engine, text(&args, "draft_id")?)?)),
                _ => Err(AppError::Invalid("缺少已审查的插件事务计划".into())),
            }
        }).await.map_err(|error| AppError::Other(error.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        agent::policy::PermissionMode, db::Db, plugins, storage::AppPaths, transaction::FileEngine,
    };
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn reviewed_draft_and_deployment_use_real_file_transactions() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("data")).unwrap();
        let context = ToolContext {
            engine: FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths),
            cancel: CancellationToken::new(),
            progress: None,
            resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
        };
        let registry = ToolRegistry::default();
        register(&registry).unwrap();
        let mut adapter: Value = serde_json::from_str(plugins::BUILTIN_PACKAGES[0].1).unwrap();
        adapter["slug"] = json!("test-game");
        let args = json!({"slug":"test-game","adapter_json":adapter.to_string(),"skill_markdown":"---\nname: test-game\ndescription: Test game adapter\n---\nInspect logs before planning changes.\n"});
        let tool = registry.get("write_plugin_draft").unwrap();
        let prepared = tool.prepare(&context, &args).await.unwrap();
        assert!(PermissionMode::AutoApprove.needs_approval(prepared.effect));
        assert!(prepared.preview["adapterJson"]
            .as_str()
            .unwrap()
            .contains("test-game"));
        let draft_id = prepared.preview["draft"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(plugin_workflow::list_drafts(&context.engine)
            .unwrap()
            .is_empty());
        tool.execute(&context, args, prepared).await.unwrap();
        let tool = registry.get("install_plugin").unwrap();
        let args = json!({"draft_id":draft_id});
        let prepared = tool.prepare(&context, &args).await.unwrap();
        assert!(prepared.plan.as_ref().unwrap().requires_review);
        assert!(prepared.preview["package"]["skillMarkdown"]
            .as_str()
            .is_some());
        assert!(plugins::list_plugins(&context.engine.db)
            .unwrap()
            .is_empty());
        tool.execute(&context, args, prepared).await.unwrap();
        assert_eq!(
            plugins::list_plugins(&context.engine.db).unwrap()[0].slug,
            "test-game"
        );
    }

    #[tokio::test]
    async fn cancelled_plugin_preparation_cannot_create_a_plan() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("data")).unwrap();
        let context = ToolContext {
            engine: FileEngine::new(Db::open(&paths.db_file()).unwrap(), paths),
            cancel: CancellationToken::new(),
            progress: None,
            resolution_scope: crate::sources::resolutions::ResolutionScope::Desktop,
        };
        context.cancel.cancel();
        let tool = PluginTool {
            name: "write_plugin_draft",
        };
        assert!(matches!(
            tool.prepare(&context, &json!({})).await,
            Err(AppError::Cancelled)
        ));
        assert!(!context
            .engine
            .paths
            .staging()
            .join("plugin-scratch")
            .exists());
    }
}
