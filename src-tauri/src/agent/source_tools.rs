use super::{
    policy::Effect,
    tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition, ToolRegistry},
};
use crate::{
    download::DownloadManager,
    error::{AppError, AppResult},
    settings::Settings,
    sources::{
        resolutions::{request_id, Resolutions},
        DependencyGraphRequest, ResolveRequest, SearchQuery, SourceKeys, Sources,
        VersionListRequest,
    },
};
use serde_json::{json, Value};
use std::sync::Arc;

pub fn sources(settings: &Settings) -> AppResult<Sources> {
    Sources::new(SourceKeys {
        curseforge: settings.read_secret("sources.curseforge")?,
        nexus: settings.read_secret("sources.nexus")?,
        three_dm: settings.read_secret("sources.3dm")?,
    })
}
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
#[async_trait::async_trait]
impl AgentTool for SourceTool {
    fn definition(&self) -> ToolDefinition {
        let (description, properties, required, effect) = match self.name.as_str() {
            "search_mods" => (
                "在已配置来源搜索 Mod；未配置 Key 的来源会降级并返回状态。game_ids 使用来源原生游戏 ID，不是 Steam App ID；3dm 的 ID 是正整数，且不支持结构化游戏版本/加载器筛选",
                json!({"query":{"type":"string"},"game_ids":{"type":"object"},"game_version":{"type":"string"},"loader":{"type":"string"},"source_ids":{"type":"array","items":{"type":"string"}},"limit":{"type":"integer"},"offset":{"type":"integer"}}),
                vec!["query"],
                Effect::Read,
            ),
            "list_mod_versions" => (
                "只读列出来源项目的发布版本；当前支持 Modrinth 和 Thunderstore。已知但不支持列出版本的来源返回 unsupported 状态，未知来源 ID 返回错误。game_id 是来源原生游戏 ID，不是本地登记 ID 或 Steam App ID。game_version 和 loader 仅按来源声明筛选，不代表已验证兼容；Thunderstore 不提供这两项声明时会返回 limited。选定具体版本后，用本次查询的 source_id、project_id 和选中版本的精确 version_id 调用 resolve_mod，并传入已知的游戏版本和加载器；不要猜测版本 ID，也不要根据列表直接下载或安装。",
                json!({
                    "source_id":{"type":"string"},
                    "project_id":{"type":"string"},
                    "game_id":{"type":["string","null"]},
                    "game_version":{"type":["string","null"]},
                    "loader":{"type":["string","null"]},
                    "offset":{"type":"integer","minimum":0,"maximum":10000,"default":0},
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
                "从精确 Modrinth 版本读取必需依赖及其传递声明；必须提供已知的 Minecraft 版本和加载器。choice_required 时列出候选让用户选择，再用 project_pins 提供用户选定的项目到精确版本 ID 映射。固定版本声明不会被覆盖。将循环、冲突、缺失和截断明确告诉用户；complete 仅表示声明图完整，不验证本地文件或加载器状态。可以用 check_modrinth_dependencies 分别对照已安装记录。此工具只读，不下载、不安装、不授予文件审批。",
                json!({
                    "project_id":{"type":["string","null"]},
                    "version_id":{"type":"string"},
                    "game_version":{"type":"string"},
                    "loader":{"type":"string"},
                    "project_pins":{"type":"object","maxProperties":32,"additionalProperties":{"type":"string"}}
                }),
                vec!["version_id", "game_version", "loader"],
                Effect::Read,
            ),
            _ => (
                "用本会话 resolve_mod 返回的 download.resolution_id 下载到管理器 downloads；校验大小和 hash，不自动安装。ID 只能用一次；失效或重试时重新解析同一精确版本。不要提交 URL 或复制解析元数据。",
                json!({"resolution_id":{"type":"string","format":"uuid"}}),
                vec!["resolution_id"],
                Effect::ManagerWrite,
            ),
        };
        ToolDefinition {
            name: self.name.clone(),
            description: description.into(),
            parameters: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
            effect,
        }
    }
    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        let preview = if self.name == "download_mod" {
            let id = request_id(args)?;
            serde_json::to_value(self.resolutions.preview(&context.resolution_scope, id)?)?
        } else {
            args.clone()
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
                tokio::select! {
                    biased;
                    _ = context.cancel.cancelled() => Err(AppError::Cancelled),
                    result = sources.modrinth_dependency_graph(&request) => Ok(serde_json::to_value(result?)?),
                }
            }
            "download_mod" => {
                let id = request_id(&args)?;
                if prepared.effect != Effect::ManagerWrite
                    || prepared.preview["resolution_id"] != json!(id)
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
                crate::artifacts::record(&context.engine, &artifact, &resolved)?;
                Ok(json!(artifact.into_user_paths()))
            }
            _ => Err(AppError::NotFound(self.name.clone())),
        }
    }
}

#[cfg(test)]
#[path = "source_tools_tests.rs"]
mod tests;
