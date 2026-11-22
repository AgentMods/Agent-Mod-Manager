use super::{
    policy::Effect,
    tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition, ToolRegistry},
};
use crate::{
    download::DownloadManager,
    download_tasks::{
        agent_download_schema, agent_retry_schema, valid_retry_artifact_id, AgentDownloadArgs,
        ClassificationInput, CompletedSource, DownloadTarget,
    },
    error::{AppError, AppResult},
    settings::Settings,
    sources::{
        resolutions::{
            inbound_download_request, resolution_id, AgentResolutionId, ResolutionScope,
            Resolutions,
        },
        DependencyGraphRequest, ResolveRequest, SearchQuery, SourceKeys, Sources,
        VersionListRequest,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

pub fn sources(settings: &Settings) -> AppResult<Sources> {
    Sources::new(SourceKeys {
        curseforge: settings.read_secret("sources.curseforge")?,
        nexus: settings.read_secret("sources.nexus")?,
        three_dm: settings.read_secret("sources.3dm")?,
    })
}
const RETRY: &str = "retry_mod_library_download";

pub(crate) fn register(
    registry: &ToolRegistry,
    settings: Settings,
    resolutions: Resolutions,
) -> AppResult<()> {
    for name in [
        "search_mods",
        "list_mod_versions",
        "resolve_mod",
        "resolve_modrinth_dependency_graph",
        "download_mod",
        RETRY,
    ] {
        registry.register(Arc::new(SourceTool {
            name: name.into(),
            settings: settings.clone(),
            resolutions: resolutions.clone(),
        }))?;
    }
    Ok(())
}
struct SourceTool {
    name: String,
    settings: Settings,
    resolutions: Resolutions,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetryArgs {
    artifact_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    game_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    game_name: Option<String>,
}

impl RetryArgs {
    fn classification(&self) -> ClassificationInput {
        ClassificationInput {
            game_id: self.game_id.clone(),
            game_name: self.game_name.clone(),
            ..Default::default()
        }
    }
}

fn retry_request(
    context: &ToolContext,
    args: &Value,
) -> AppResult<(DownloadTarget, crate::artifacts::ArtifactRecord, Value)> {
    if context.cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    if !matches!(context.resolution_scope, ResolutionScope::Agent(_)) {
        return Err(AppError::Denied("下载入库重试只供桌面 Agent 使用".into()));
    }
    let args: RetryArgs = serde_json::from_value(args.clone()).map_err(|_| {
        AppError::Invalid("只接受原下载回执 artifactId 和对应游戏 gameId 或 gameName".into())
    })?;
    let target = args.classification().for_agent(context)?;
    let saved = target.saved_for_retry(&context.engine, &args.artifact_id)?;
    let intent = saved
        .native_library_intent()
        .expect("checked retry receipt");
    let preview = json!({
        "artifactId":saved.artifact.id,
        "filename":saved.artifact.filename,
        "size":saved.artifact.size,
        "sha256":saved.artifact.sha256,
        "classification":target,
        "projectPage":saved.project_page.as_ref().or(intent.project_page.as_ref()),
        "authorHint":intent.author_hint,
        "destination":"Mods/",
        "action":"仅从这一个已保存的回执继续入库；不联网、不重新下载、不安装。"
    });
    Ok((target, saved, preview))
}

impl SourceTool {
    fn download_request(
        &self,
        context: &ToolContext,
        args: &Value,
    ) -> AppResult<(uuid::Uuid, Option<DownloadTarget>, Value)> {
        let (id, classification) = if matches!(context.resolution_scope, ResolutionScope::Mcp(_)) {
            let (id, classification) = inbound_download_request(args)?;
            (
                id,
                classification
                    .map(|input| input.resolve_target(&context.engine))
                    .transpose()?,
            )
        } else {
            let args = AgentDownloadArgs::<AgentResolutionId>::parse(args)?;
            (
                resolution_id(&args.transfer.resolution_id)?,
                Some(args.classification.for_agent(context)?),
            )
        };
        let mut preview =
            serde_json::to_value(self.resolutions.preview(&context.resolution_scope, id)?)?;
        if let Some(classification) = &classification {
            preview["destination"] = json!("Mods/");
            preview["classification"] = serde_json::to_value(classification)?;
        }
        Ok((id, classification, preview))
    }
}

#[async_trait::async_trait]
impl AgentTool for SourceTool {
    fn definition(&self) -> ToolDefinition {
        if self.name == RETRY {
            return ToolDefinition {
                name: self.name.clone(),
                description: "仅在单个新原生下载已保存、但尚未成功入库时使用其 artifactId。游戏必须与原下载一致；已关联游戏的聊天自动使用登记游戏。仅重试本地入库；若已经入库则验证并返回相同回执。不能接管历史下载、联网下载、安装或修改原回执元数据。".into(),
                parameters: agent_retry_schema(),
                effect: Effect::ManagerWrite,
            };
        }
        let (description, properties, required, effect) = match self.name.as_str() {
            "search_mods" => (
                "在管理器内置目录搜索 Mod；这里只列原生提供方。本轮已连接的 Mod 平台 MCP（包括 Mod 客栈）可独立查找和下载文件。未配置 Key 的来源会降级并返回状态。game_ids 使用来源原生游戏 ID，不是 Steam App ID；3dm 的 ID 是正整数，且不支持结构化游戏版本/加载器筛选",
                json!({"query":{"type":"string"},"game_ids":{"type":"object"},"game_version":{"type":"string"},"loader":{"type":"string"},"source_ids":{"type":"array","items":{"type":"string"}},"limit":{"type":"integer"},"offset":{"type":"integer"}}),
                vec!["query"],
                Effect::Read,
            ),
            "list_mod_versions" => (
                "只读列出来源项目的发布版本；当前支持 Modrinth 和 Thunderstore。已知但不支持列出版本的来源返回 unsupported 状态，未知来源 ID 返回错误。game_id 是来源原生游戏 ID，不是本地登记 ID 或 Steam App ID。game_version 和 loader 仅按来源声明筛选，不代表已验证兼容；Thunderstore 不提供这两项声明时会返回 limited。原生目录解析使用本次查询的 source_id、project_id 和精确 version_id。常规下载按 system-mod-download 选择来源 MCP。",
                json!({
                    "source_id":{"type":"string"},
                    "project_id":{"type":"string"},
                    "game_id":{"type":["string","null"]},
                    "game_version":{"type":["string","null"]},
                    "loader":{"type":["string","null"]},
                    "offset":{"type":"integer","minimum":0,"default":0},
                    "limit":{"type":"integer","minimum":1,"maximum":50,"default":20}
                }),
                vec!["source_id", "project_id"],
                Effect::Read,
            ),
            "resolve_mod" => (
                "只读解析 Mod 版本与下载信息；有可下载文件时返回一次性 resolution_id，不返回直链。除 Modrinth 可仅凭精确 version_id 查询外，必须提供 project_id。Modrinth 的 dependencies 是选中版本的直接声明，null 表示未知，[] 表示来源明确未声明；不要视为已满足，file_name 仅供辨识。兼容条件必须由用户游戏版本与加载器确定。3dm 的 version_id 是资源文件数字 ID；manual_download 不得当作可安装的直链",
                json!({"source_id":{"type":"string"},"project_id":{"type":"string"},"version_id":{"type":"string"},"game_id":{"type":"string"},"game_version":{"type":"string"},"loader":{"type":"string"}}),
                vec!["source_id"],
                Effect::Read,
            ),
            "resolve_modrinth_dependency_graph" => (
                "从精确 Modrinth 版本读取必需依赖及其传递声明；必须提供已知的 Minecraft 版本和加载器。choice_required 时列出候选让用户选择，再用 project_pins 提供用户选定的项目到精确版本 ID 映射。固定版本声明不会被覆盖。将循环、冲突、缺失和截断明确告诉用户；请求可以取消，单次上游连接无进展时会停止等待。返回截断图时不能视为核对完毕。complete 仅表示声明图完整，不验证本地文件或加载器状态。可以用 check_modrinth_dependencies 分别对照已安装记录。此工具只读，不下载、不安装、不授予文件审批。",
                json!({
                    "project_id":{"type":["string","null"]},
                    "version_id":{"type":"string"},
                    "game_version":{"type":"string"},
                    "loader":{"type":"string"},
                    "project_pins":{"type":"object","propertyNames":{"pattern":"^[A-Za-z0-9]{1,32}$"},"additionalProperties":{"type":"string","pattern":"^[A-Za-z0-9]{1,32}$"}}
                }),
                vec!["version_id", "game_version", "loader"],
                Effect::Read,
            ),
            _ => (
                "原生目录专用，仅用于明确要求按原生解析凭证下载的场景。常规 Mod 下载走来源 MCP，来源无法下载时走 Playwright MCP。用本会话 resolve_mod 返回的 download.resolution_id 下载；校验大小和 hash，并将归档保存到 Mods 库。没有关联游戏时填写登记游戏 ID 或明确的游戏名称，作为 Mods 库游戏分类。不会自动安装。解析 ID 只能用一次；传输失败且没有回执时重新解析同一精确版本。不要提交下载 URL 或复制解析元数据。",
                json!({"resolution_id":{"type":"string","format":"uuid"}}),
                vec!["resolution_id"],
                Effect::ManagerWrite,
            ),
        };
        ToolDefinition {
            name: self.name.clone(),
            description: description.into(),
            parameters: if self.name == "download_mod" {
                agent_download_schema(properties, required)
            } else {
                json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
            },
            effect,
        }
    }
    fn inbound_definition(&self) -> ToolDefinition {
        let mut definition = self.definition();
        if self.name == "download_mod" {
            definition.description = "使用本 MCP 会话 resolve_mod 返回的 resolution_id 下载。提供一个登记游戏 ID（gameId）或明确的游戏名称（gameName）时归类到 Mods 库；两者都省略时保持旧版 downloads 回执，不入库。不会安装。".into();
            definition.parameters = json!({
                "type":"object",
                "properties":{
                    "resolution_id":{"type":"string","format":"uuid"},
                    "gameId":{"type":"string","description":"本地登记游戏 ID"},
                    "gameName":{"type":"string","description":"明确的游戏名称，可尚未登记"}
                },
                "required":["resolution_id"],
                "not":{"required":["gameId","gameName"]},
                "additionalProperties":false,
            });
        }
        definition
    }

    fn history_arguments(&self, raw: &str) -> Option<String> {
        if self.name != RETRY {
            return None;
        }
        Some(
            serde_json::from_str::<RetryArgs>(raw)
                .ok()
                .filter(|args| {
                    valid_retry_artifact_id(&args.artifact_id)
                        && args.classification().validate_metadata().is_ok()
                })
                .and_then(|args| serde_json::to_string(&args).ok())
                .unwrap_or_else(|| "{}".into()),
        )
    }

    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        let preview = match self.name.as_str() {
            "download_mod" => self.download_request(context, args)?.2,
            RETRY => retry_request(context, args)?.2,
            _ => args.clone(),
        };
        Ok(PreparedTool {
            preview,
            effect: self.definition().effect,
            plan: None,
        })
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
        match self.name.as_str() {
            "search_mods" => {
                let query = serde_json::from_value::<SearchQuery>(args)?;
                let sources = sources(&self.settings)?;
                tokio::select! {biased;_=context.cancel.cancelled()=>Err(AppError::Cancelled),result=sources.search(&query)=>Ok(json!(result?))}
            }
            "list_mod_versions" => {
                if !args.is_object() {
                    return Err(AppError::Invalid("版本列表参数必须是对象".into()));
                }
                let request = serde_json::from_value::<VersionListRequest>(args)?;
                // Supported version listings use public endpoints.
                let sources = Sources::new(SourceKeys::default())?;
                tokio::select! {
                    biased;
                    _ = context.cancel.cancelled() => Err(AppError::Cancelled),
                    result = sources.list_versions(&request) => Ok(serde_json::to_value(result?)?),
                }
            }
            "resolve_mod" => {
                let request = serde_json::from_value::<ResolveRequest>(args)?;
                let sources = sources(&self.settings)?;
                let result = tokio::select! {
                    biased;
                    _ = context.cancel.cancelled() => return Err(AppError::Cancelled),
                    result = sources.resolve(&request) => result?,
                };
                Ok(serde_json::to_value(
                    self.resolutions
                        .publish(context.resolution_scope.clone(), result)?,
                )?)
            }
            "resolve_modrinth_dependency_graph" => {
                if !args.is_object() {
                    return Err(AppError::Invalid("依赖图参数必须是对象".into()));
                }
                let request = serde_json::from_value::<DependencyGraphRequest>(args)?;
                let sources = Sources::new(SourceKeys::default())?;
                let graph = sources
                    .modrinth_dependency_graph(&request, &context.cancel)
                    .await?;
                Ok(serde_json::to_value(graph)?)
            }
            "download_mod" => {
                let (id, classification, preview) = self.download_request(context, &args)?;
                if prepared.effect != Effect::ManagerWrite
                    || prepared.plan.is_some()
                    || prepared.preview != preview
                {
                    return Err(AppError::Denied("下载与审批预览不一致".into()));
                }
                let resolved = self.resolutions.take(&context.resolution_scope, id)?;
                let progress = context.progress.clone();
                let artifact = DownloadManager::new(&context.engine.paths)?
                    .download_mod(
                        &resolved,
                        context.cancel.clone(),
                        Arc::new(move |p| {
                            if let Some(callback) = &progress {
                                callback(json!({"download":p}));
                            }
                        }),
                    )
                    .await?;
                if let Some(classification) = classification {
                    Ok(json!(classification
                        .finish(
                            &context.engine,
                            artifact,
                            CompletedSource::Catalog(&resolved)
                        )
                        .await?
                        .into_user_paths()))
                } else {
                    // Preserve the exact legacy MCP receipt when no game was
                    // supplied; it must not become eligible for native retry.
                    crate::artifacts::record(&context.engine, &artifact, &resolved)?;
                    Ok(json!(artifact.into_user_paths()))
                }
            }
            RETRY => {
                let (target, saved, preview) = retry_request(context, &args)?;
                if prepared.effect != Effect::ManagerWrite
                    || prepared.plan.is_some()
                    || prepared.preview != preview
                {
                    return Err(AppError::Denied("重试回执或游戏与审批预览不一致".into()));
                }
                let receipt = target
                    .retry_saved(&context.engine, saved, context.cancel.clone())
                    .await?;
                Ok(json!(receipt.into_user_paths()))
            }
            _ => Err(AppError::NotFound(self.name.clone())),
        }
    }
}

#[cfg(test)]
#[path = "source_tools_tests.rs"]
mod tests;
