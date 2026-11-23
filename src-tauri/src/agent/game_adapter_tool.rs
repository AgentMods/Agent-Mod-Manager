use super::{
    policy::Effect,
    tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition},
};
use crate::{
    error::{AppError, AppResult},
    game_actions,
    sources::resolutions::ResolutionScope,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub(super) struct SetGameAdapter;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingArgs {
    slug: String,
    game_id: Option<String>,
    game_name: Option<String>,
}

#[async_trait::async_trait]
impl AgentTool for SetGameAdapter {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "set_game_adapter".into(),
            description: "为已登记的游戏绑定已安装且启用的适配包。为当前游戏创建并部署适配包后，调用此工具完成绑定；若包已安装，可直接调用。游戏会话默认使用当前游戏，通用会话须提供目标登记 ID。仅在用户要求更改游戏库显示名称时传入 game_name，与绑定一并提交；这不会改名适配包。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "slug": {"type": "string", "description": "已安装适配包的 slug"},
                    "game_id": {"type": "string", "description": "已登记游戏的 ID，游戏会话可省略"},
                    "game_name": {"type": "string", "description": "可选的游戏库显示名称；仅在用户要求时提供，省略则保留原名"}
                },
                "required": ["slug"],
                "additionalProperties": false
            }),
            effect: Effect::ManagerWrite,
        }
    }

    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        if context.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let ResolutionScope::Agent(conversation) = &context.resolution_scope else {
            return Err(AppError::Denied("适配包绑定仅供桌面 Agent 对话使用".into()));
        };
        let args: BindingArgs = serde_json::from_value(args.clone()).map_err(|_| {
            AppError::Invalid("绑定适配包需要 slug，可选 game_id、game_name".into())
        })?;
        let conversation = conversation.clone();
        let engine = context.engine.clone();
        let binding = tokio::task::spawn_blocking(move || {
            let selected = super::workspace::selection(&engine.db, &conversation)?;
            let id = super::workspace::resolve(selected.as_deref(), args.game_id.as_deref())?;
            game_actions::prepare_game_adapter_binding(
                &engine,
                &id,
                &args.slug,
                args.game_name.as_deref(),
            )
        })
        .await
        .map_err(|error| AppError::Other(error.to_string()))??;
        Ok(PreparedTool {
            preview: serde_json::to_value(binding)?,
            effect: Effect::ManagerWrite,
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
        let binding = serde_json::from_value(prepared.preview)?;
        let engine = context.engine.clone();
        let game = tokio::task::spawn_blocking(move || {
            game_actions::apply_game_adapter_binding(&engine, &binding)
        })
        .await
        .map_err(|error| AppError::Other(error.to_string()))??;
        Ok(json!({
            "status": "bound",
            "gameId": game.id,
            "gameName": game.name,
            "adapterSlug": game.adapter_slug,
            "adapterVersion": game.adapter_version,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{history::History, transaction::tests::setup};
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn binding_requires_an_unambiguous_registered_target() {
        let (_temp, engine, _) = setup();
        let history = History::new(engine.db.clone());
        let game_chat = history.create_conversation("Game", Some("g")).unwrap();
        let general_chat = history.create_conversation("General", None).unwrap();
        let mut context = ToolContext {
            engine,
            resolution_scope: ResolutionScope::Agent(game_chat.id),
            cancel: CancellationToken::new(),
            progress: None,
        };
        let args = json!({"slug":"minecraft-java", "game_id":"other"});
        assert!(matches!(
            SetGameAdapter.prepare(&context, &args).await,
            Err(AppError::Invalid(_))
        ));
        context.resolution_scope = ResolutionScope::Agent(general_chat.id);
        let args = json!({"slug":"minecraft-java"});
        assert!(matches!(
            SetGameAdapter.prepare(&context, &args).await,
            Err(AppError::Invalid(_))
        ));
    }
}
