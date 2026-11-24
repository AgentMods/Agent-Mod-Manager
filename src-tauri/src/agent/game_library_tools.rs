use super::{
    game_adapter_tool::SetGameAdapter,
    policy::Effect,
    tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition, ToolRegistry},
    workspace,
};
use crate::{
    error::{AppError, AppResult},
    game_actions::{self, LaunchPathUpdate},
    games::Game,
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) fn register(registry: &ToolRegistry) -> AppResult<()> {
    use LibraryTool::*;
    registry.register_batch(vec![
        Arc::new(List),
        Arc::new(Register),
        Arc::new(Update),
        Arc::new(Remove),
        Arc::new(Launch),
        Arc::new(SetGameAdapter),
    ])
}

#[derive(Clone, Copy)]
enum LibraryTool {
    List,
    Register,
    Update,
    Remove,
    Launch,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageArgs {
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterArgs {
    directory: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetArgs {
    game_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    game_id: Option<String>,
    name: Option<String>,
    launch_path: Option<String>,
}

fn parse<T: DeserializeOwned>(args: Value) -> AppResult<T> {
    serde_json::from_value(args)
        .map_err(|err| AppError::Invalid(format!("游戏库工具参数无效：{err}")))
}

fn receipt(status: &str, game: Game) -> Value {
    let launch_path = game.metadata.get("launchPath").and_then(Value::as_str);
    json!({
        "status": status,
        "gameId": game.id,
        "gameName": game.name,
        "rootPath": crate::paths::display_path(Path::new(&game.root_path)),
        "adapterSlug": game.adapter_slug,
        "adapterVersion": game.adapter_version,
        "launchPath": launch_path.map(|path| crate::paths::display_path(Path::new(path))),
    })
}

fn list_games(context: &ToolContext, page: PageArgs) -> AppResult<Value> {
    let limit = page.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(AppError::Invalid("limit 必须在 1 到 100 之间".into()));
    }
    let workspace = workspace::tool_selection(context)?
        .map(|ids| serde_json::to_string(&ids))
        .transpose()?;
    let filter = "WHERE ?1 IS NULL OR id IN (SELECT value FROM json_each(?1))";
    let conn = context.engine.db.conn();
    let total: usize = conn.query_row(
        &format!("SELECT count(*) FROM games {filter}"),
        [&workspace],
        |row| row.get(0),
    )?;
    let start = page.offset.unwrap_or(0).min(total);
    let mut statement = conn.prepare(&format!(
        "SELECT id, name, root_path, adapter_slug, adapter_version FROM games {filter}
         ORDER BY name COLLATE NOCASE, id LIMIT ?2 OFFSET ?3"
    ))?;
    let rows = statement.query_map(rusqlite::params![workspace, limit, start], |row| {
        let root: String = row.get(2)?;
        Ok(json!({
            "id": row.get::<_, String>(0)?, "name": row.get::<_, String>(1)?,
            "rootPath": crate::paths::display_path(Path::new(&root)),
            "adapterSlug": row.get::<_, Option<String>>(3)?, "adapterVersion": row.get::<_, Option<String>>(4)?,
        }))
    })?.collect::<Result<Vec<_>, _>>()?;
    let end = start + rows.len();
    Ok(json!({"games": rows, "total": total, "next_offset": (end < total).then_some(end)}))
}

impl LibraryTool {
    fn describe(self, inbound: bool) -> ToolDefinition {
        let id = if inbound {
            "已登记游戏的 ID"
        } else {
            "已登记游戏的 ID；单选工作区可省略"
        };
        let target = json!({"type":"string","minLength":1,"description":id});
        let target_required = if inbound { vec!["game_id"] } else { Vec::new() };
        let (name, description, effect, properties, required) = match self {
            Self::List => (
                "list_games",
                "列出工作区已登记的游戏、目录、适配包和 ID。可按 next_offset 翻页，批量操作请先取全目标 ID。",
                Effect::Read,
                json!({"offset": {"type":"integer","minimum":0}, "limit": {"type":"integer","minimum":1,"maximum":100}}),
                vec![],
            ),
            Self::Register => (
                "register_game",
                "将用户明确指定的本地游戏目录添加到游戏库。登记后该目录进入管理器的游戏文件操作范围。返回新登记的游戏 ID 和匹配的适配包。",
                Effect::ManagerWrite,
                json!({"directory": {"type":"string","minLength":1,"description":"已存在的游戏根目录绝对路径"}}),
                vec!["directory"],
            ),
            Self::Update => (
                "update_game",
                "编辑游戏的显示名称或自定义启动程序，至少提供一项。省略的字段保留原值；launch_path 传 null 可清除自定义启动程序。",
                Effect::ManagerWrite,
                json!({
                    "game_id": target,
                    "name": {"type":"string","minLength":1},
                    "launch_path": {"type":["string","null"],"description":"游戏目录内启动程序的绝对路径"}
                }),
                target_required,
            ),
            Self::Remove => (
                "remove_game",
                "从游戏库移除一项登记并解除其工作区与用户目录关联，保留磁盘文件和历史。存在已安装 Mod、待处理计划或未恢复事务时会返回冲突原因。",
                Effect::ManagerWrite,
                json!({"game_id": target}),
                target_required,
            ),
            Self::Launch => (
                "launch_game",
                "按已登记游戏的自定义启动程序或 Steam App ID 向系统请求启动游戏。",
                Effect::LaunchGame,
                json!({"game_id": target}),
                target_required,
            ),
        };
        ToolDefinition {
            name: name.into(),
            description: description.into(),
            effect,
            parameters: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
        }
    }

    fn preview(self, context: &ToolContext, args: Value) -> AppResult<Value> {
        let engine = &context.engine;
        match self {
            Self::List => list_games(context, parse(args)?),
            Self::Register => {
                let args: RegisterArgs = parse(args)?;
                Ok(serde_json::to_value(game_actions::prepare_registration(
                    engine,
                    &args.directory,
                )?)?)
            }
            Self::Update => {
                let changes_launch = args.get("launch_path").is_some();
                let args: UpdateArgs = parse(args)?;
                if args.name.is_none() && !changes_launch {
                    return Err(AppError::Invalid(
                        "编辑游戏至少需要 name 或 launch_path".into(),
                    ));
                }
                let launch = match args.launch_path.as_deref() {
                    Some(path) => LaunchPathUpdate::Set(path),
                    None if changes_launch => LaunchPathUpdate::Clear,
                    None => LaunchPathUpdate::Keep,
                };
                let id = workspace::tool_target(context, args.game_id.as_deref())?;
                Ok(serde_json::to_value(game_actions::prepare_update(
                    engine,
                    &id,
                    args.name.as_deref(),
                    launch,
                )?)?)
            }
            Self::Remove | Self::Launch => {
                let args: TargetArgs = parse(args)?;
                let id = workspace::tool_target(context, args.game_id.as_deref())?;
                if matches!(self, Self::Remove) {
                    Ok(serde_json::to_value(game_actions::prepare_removal(
                        engine, &id,
                    )?)?)
                } else {
                    Ok(serde_json::to_value(game_actions::prepare_launch(
                        engine, &id,
                    )?)?)
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl AgentTool for LibraryTool {
    fn definition(&self) -> ToolDefinition {
        self.describe(false)
    }
    fn inbound_definition(&self) -> ToolDefinition {
        self.describe(true)
    }

    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        let (tool, context, args) = (*self, context.clone(), args.clone());
        let preview = tokio::task::spawn_blocking(move || {
            if context.cancel.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            tool.preview(&context, args)
        })
        .await
        .map_err(|error| AppError::Other(error.to_string()))??;
        Ok(PreparedTool {
            preview,
            effect: self.definition().effect,
            plan: None,
        })
    }

    async fn execute(
        &self,
        context: &ToolContext,
        _args: Value,
        prepared: PreparedTool,
    ) -> AppResult<Value> {
        let (tool, context) = (*self, context.clone());
        tokio::task::spawn_blocking(move || {
            if context.cancel.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            let engine = &context.engine;
            let preview = prepared.preview;
            match tool {
                Self::List => Ok(preview),
                Self::Register => {
                    let reviewed = serde_json::from_value(preview)?;
                    let game = game_actions::apply_registration(engine, &reviewed)?;
                    Ok(receipt("registered", game))
                }
                Self::Update => {
                    let reviewed = serde_json::from_value(preview)?;
                    let game = game_actions::apply_update(engine, &reviewed)?;
                    Ok(receipt("updated", game))
                }
                Self::Remove => {
                    let reviewed = serde_json::from_value(preview)?;
                    game_actions::apply_removal(engine, &reviewed)?;
                    Ok(json!({"status":"removed", "gameId":reviewed.game_id,"gameName":reviewed.game_name}))
                }
                Self::Launch => {
                    let reviewed = serde_json::from_value(preview)?;
                    game_actions::apply_launch(engine, &reviewed)?;
                    Ok(json!({"status":"launch_requested", "gameId":reviewed.game_id,"gameName":reviewed.game_name}))
                }
            }
        }).await.map_err(|error| AppError::Other(error.to_string()))?
    }
}
