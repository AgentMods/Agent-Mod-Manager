use super::{
    policy::Effect,
    tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition, ToolRegistry},
};
use crate::{
    error::{AppError, AppResult},
    install, install_dependencies, install_file_evidence,
    sources::MAX_MODRINTH_DEPENDENCIES,
    transaction::FileOp,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn register(registry: &ToolRegistry) -> AppResult<()> {
    for name in [
        "plan_install",
        "install_mod",
        "list_installs",
        "inspect_install",
        "uninstall_mod",
        "rollback_transaction",
    ] {
        registry.register(Arc::new(InstallTool { name: name.into() }))?;
    }
    registry.register(Arc::new(CheckModrinthDependencies))?;
    registry.register(Arc::new(VerifyInstallFiles))
}

struct VerifyInstallFiles;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectInstallArgs {
    game_id: String,
    install_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifyFileArgs {
    game_id: String,
    install_id: String,
}

#[async_trait::async_trait]
impl AgentTool for VerifyInstallFiles {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "verify_install_files".into(),
            description: "用 list_installs 返回的游戏 ID 和完整部署的安装 ID，只读核验当前文件是否匹配安装清单中的大小与 SHA-256。可用来诊断某个 Mod 的缺失或修改；报告仅列计数及少量相对路径，不返回文件内容，不联网或修复。未核验不代表文件损坏；匹配不证明加载器已启用。".into(),
            parameters: json!({"type":"object", "properties":{
                "game_id":{"type":"string","maxLength":128},
                "install_id":{"type":"string","maxLength":128}
            }, "required":["game_id","install_id"], "additionalProperties":false}),
            effect: Effect::Read,
        }
    }

    async fn execute(
        &self,
        context: &ToolContext,
        args: Value,
        _: PreparedTool,
    ) -> AppResult<Value> {
        let VerifyFileArgs {
            game_id,
            install_id,
        } = serde_json::from_value(args)?;
        let engine = context.engine.clone();
        let cancel = context.cancel.clone();
        tokio::task::spawn_blocking(move || {
            let evidence = install_file_evidence::verify_managed_install(
                &engine,
                &game_id,
                &install_id,
                &cancel,
            )?;
            Ok(serde_json::to_value(evidence)?)
        })
        .await
        .map_err(|_| AppError::Other("核验安装文件失败".into()))?
    }
}

struct CheckModrinthDependencies;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DependencyCheckArgs {
    request: install_dependencies::CheckRequest,
}

#[async_trait::async_trait]
impl AgentTool for CheckModrinthDependencies {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "check_modrinth_dependencies".into(),
            description: "仅根据目标游戏的管理器安装记录，核对 Modrinth 直接依赖的精确项目或版本 ID。使用 resolve_mod 为确切 Modrinth 版本返回的原始 dependencies 和目标游戏在管理器中的 gameId；不要猜测 ID，也不要把未知的 null 改成空数组。仅在评估替换安装时传 replaceInstallId，以排除待替换的记录。不访问来源、不下载、不修改安装记录或文件、不自动安装；结果不验证哈希、加载器状态或兼容性。".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "request": {
                        "type": "object",
                        "properties": {
                            "gameId": {"type": "string"},
                            "dependencies": {
                                "type": ["array", "null"],
                                "maxItems": MAX_MODRINTH_DEPENDENCIES,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "dependency_type": {"type": "string", "enum": ["required", "optional", "embedded", "incompatible"]},
                                        "project_id": {"type": ["string", "null"]},
                                        "version_id": {"type": ["string", "null"]},
                                        "file_name": {"type": ["string", "null"], "description": "仅供显示"}
                                    },
                                    "required": ["dependency_type"],
                                    "additionalProperties": false
                                }
                            },
                            "replaceInstallId": {"type": ["string", "null"]}
                        },
                        "required": ["gameId", "dependencies"],
                        "additionalProperties": false
                    }
                },
                "required": ["request"],
                "additionalProperties": false
            }),
            effect: Effect::Read,
        }
    }

    async fn execute(
        &self,
        context: &ToolContext,
        args: Value,
        _prepared: PreparedTool,
    ) -> AppResult<Value> {
        if context.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let DependencyCheckArgs { request } = serde_json::from_value(args)?;
        let engine = context.engine.clone();
        tokio::task::spawn_blocking(move || {
            let report = install_dependencies::check(&engine, &request)?;
            Ok(serde_json::to_value(report)?)
        })
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
    }
}

struct InstallTool {
    name: String,
}
fn text<'a>(args: &'a Value, key: &str) -> AppResult<&'a str> {
    args[key]
        .as_str()
        .ok_or_else(|| AppError::Invalid(format!("缺少参数 {key}")))
}

#[async_trait::async_trait]
impl AgentTool for InstallTool {
    fn definition(&self) -> ToolDefinition {
        let (description,properties,required,effect) = match self.name.as_str() {
            "plan_install" | "install_mod" => (
                "按适配包安装。用下载原始路径和文件名。安装确定性 FOMOD 时传 modType=fomod 和 fomodSelection=automatic，省略 mappings。后端从校验过的归档生成完整计划。需要人工选择时，先用 preview_fomod 并让用户确认选项及映射。plan_install 生成完整预览，install_mod 按审批策略执行，自动模式提交后返回 status、transactionId 和 fileCount。精确版本取自下载记录。用户明确要求替换已装同一项目的不同版本时，传其 replaceInstallId。旧版专有文件移入回收站，阻止覆盖用户修改及共享文件。不要据显示版本猜测兼容性。",
                json!({"request":{"type":"object","properties":{
                    "gameId":{"type":"string"},"artifactPath":{"type":"string"},"filename":{"type":"string"},"name":{"type":"string"},"modType":{"type":"string"},
                    "version":{"type":"string"},"expectedSha256":{"type":"string"},"folderName":{"type":"string"},"source":{"type":"string"},"sourceModId":{"type":"string"},"replaceInstallId":{"type":"string"},
                    "fomodSelection":{"type":"string","enum":["automatic"],"description":"仅适用于 modType=fomod。由后端从校验过的归档选定文件。使用时省略 mappings。"},
                    "mappings":{"type":"array","items":{"type":"object","properties":{"source":{"type":"string"},"destination":{"type":"string"}},"required":["source","destination"],"additionalProperties":false}}},
                    "required":["gameId","artifactPath","filename","name","modType"],"additionalProperties":false}}),
                vec!["request"], if self.name == "plan_install" {Effect::Read} else {Effect::ManagerWrite}),
            "list_installs" => ("读取游戏安装事务、逐文件清单和状态。downloadOrigin 记录部署时的来源、项目和精确版本 ID；null 表示未绑定，不要从显示版本推断。记录不证明来源真实性或加载器已启用",json!({"game_id":{"type":"string"}}),vec!["game_id"],Effect::Read),
            "inspect_install" => ("只读检查一个安装的文件指纹、缺失或修改、覆盖过的原文件和与其他安装的路径重叠，并返回协调预览：安全卸载、回滚安装、回滚替换版本或人工审查。rollbackTransactionId 只是建议，不会自动执行；本工具不调用 reconcile、不创建计划、不修改文件或数据库",json!({"game_id":{"type":"string"},"install_id":{"type":"string"}}),vec!["game_id","install_id"],Effect::Read),
            "uninstall_mod" => ("将此安装拥有且未修改的文件移入回收站；保留配置和未知文件；覆盖过既有文件的安装须回滚原事务",json!({"install_id":{"type":"string"}}),vec!["install_id"],Effect::ManagerWrite),
            _ => ("回滚一个已提交文件事务，展示原计划影响；冲突时保留现场而非强行覆盖",json!({"transaction_id":{"type":"string"}}),vec!["transaction_id"],Effect::ManagerWrite),
        };
        ToolDefinition {
            name: self.name.clone(),
            description: description.into(),
            parameters: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
            effect,
        }
    }
    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        if context.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let name = self.name.clone();
        let engine = if name == "rollback_transaction" {
            context.file_engine()
        } else {
            context.engine.clone()
        };
        let args = args.clone();
        tokio::task::spawn_blocking(move || {
            let (preview,plan) = match name.as_str() {
            "install_mod" => {
                    let preview = install::plan_install(&engine,&serde_json::from_value(args["request"].clone())?)?;
                    (json!(preview),Some(preview.plan))
                }
                "uninstall_mod" => {
                    let plan = install::plan_uninstall(&engine,text(&args,"install_id")?)?;
                    (json!(plan),Some(plan))
                }
                "rollback_transaction" => (json!({"action":"rollback","originalPlan":engine.get_plan(text(&args,"transaction_id")?)?}),None),
                _ => (args,None),
            };
            Ok(PreparedTool{preview,plan,effect:if matches!(name.as_str(), "plan_install" | "list_installs" | "inspect_install") {Effect::Read} else {Effect::ManagerWrite}})
        }).await.map_err(|e|AppError::Other(e.to_string()))?
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
        let name = self.name.clone();
        let engine = if name == "rollback_transaction" {
            context.file_engine()
        } else {
            context.engine.clone()
        };
        tokio::task::spawn_blocking(move || {
            if let Some(plan) = prepared.plan {
                engine.approve_plan(&plan.id)?;
                engine.apply_file_ops(&plan.id)?;
                install::reconcile(&engine.db)?;
                if name == "install_mod" && args["request"]["fomodSelection"] == "automatic" {
                    let file_count = plan
                        .entries
                        .iter()
                        .filter(|entry| matches!(entry.operation, FileOp::Copy { .. }))
                        .count();
                    return Ok(json!({
                        "status": "committed",
                        "transactionId": plan.id,
                        "fileCount": file_count,
                    }));
                }
                return Ok(json!({"plan":plan,"status":"committed","preview":prepared.preview}));
            }
            match name.as_str() {
                "plan_install" => Ok(json!(install::plan_install(
                    &engine,
                    &serde_json::from_value(args["request"].clone())?
                )?)),
                "list_installs" => Ok(json!(install::list_installs(
                    &engine,
                    text(&args, "game_id")?
                )?)),
                "inspect_install" => {
                    let InspectInstallArgs {
                        game_id,
                        install_id,
                    } = serde_json::from_value(args)?;
                    let report = install::inspect_install(&engine, &install_id)?;
                    if report.game_id != game_id {
                        return Err(AppError::NotFound("安装记录不属于目标游戏".into()));
                    }
                    Ok(json!(report))
                }
                "rollback_transaction" => {
                    engine.rollback_transaction(text(&args, "transaction_id")?)?;
                    install::reconcile(&engine.db)?;
                    crate::plugin_workflow::reconcile_deployments(&engine)?;
                    crate::skills::reconcile_deployments(&engine)?;
                    Ok(json!({"status":"rolled_back"}))
                }
                _ => Err(AppError::NotFound(name)),
            }
        })
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
    }
}

#[cfg(test)]
#[path = "install_tools_tests.rs"]
mod tests;
