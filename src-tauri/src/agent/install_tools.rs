use super::{
    policy::Effect,
    tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition, ToolRegistry},
};
use crate::{
    error::{AppError, AppResult},
    install, install_dependencies, install_file_evidence,
    sources::MAX_MODRINTH_DEPENDENCIES,
    transaction::{serialize_user_plan, FileOp},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn register(registry: &ToolRegistry) -> AppResult<()> {
    for name in [
        "list_mod_library",
        "match_mod_install",
        "install_library_mod",
        "bind_mod_library_entry",
        "forget_uninstalled_mod",
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

fn check_workspace_target(context: &ToolContext, target: &str) -> AppResult<()> {
    if let crate::sources::resolutions::ResolutionScope::Agent(conversation) =
        &context.resolution_scope
    {
        let selected = super::workspace::selection(&context.engine.db, conversation)?;
        super::workspace::resolve(selected.as_deref(), Some(target))?;
    }
    Ok(())
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

    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        let parsed: VerifyFileArgs = serde_json::from_value(args.clone())?;
        check_workspace_target(context, &parsed.game_id)?;
        Ok(PreparedTool {
            preview: args.clone(),
            effect: Effect::Read,
            plan: None,
        })
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
        check_workspace_target(context, &game_id)?;
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
        check_workspace_target(context, &request.game_id)?;
        let engine = context.engine.clone();
        let cancel = context.cancel.clone();
        tokio::task::spawn_blocking(move || {
            let report = install_dependencies::check_with_cancel(&engine, &request, &cancel)?;
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

/// A game-scoped uninstall can only act on the current reconciliation for
/// that exact installation, including when undoing an overwrite or upgrade.
fn check_uninstall_action(
    engine: &crate::transaction::FileEngine,
    args: &Value,
    rollback: bool,
) -> AppResult<()> {
    if args.get("game_id").is_none() && (!rollback || args.get("install_id").is_none()) {
        return Ok(());
    }
    let game_id = text(args, "game_id")?;
    let install_id = text(args, "install_id")?;
    let report = install::inspect_install(engine, install_id)?;
    if report.game_id != game_id {
        return Err(AppError::NotFound("安装记录不属于目标游戏".into()));
    }
    let action = &report.reconciliation;
    let allowed = if rollback {
        matches!(
            action.recommended_action.as_str(),
            "rollback_install" | "rollback_replacement"
        ) && action.rollback_transaction_id.as_deref() == Some(text(args, "transaction_id")?)
            && report
                .files
                .iter()
                .all(|file| file.state == "owned" && file.other_install_ids.is_empty())
    } else {
        action.recommended_action == "safe_uninstall"
    };
    if !allowed {
        return Err(AppError::Conflict(format!(
            "当前安装记录需要重新核对：{}",
            action.reasons.join("；")
        )));
    }
    Ok(())
}

#[async_trait::async_trait]
impl AgentTool for InstallTool {
    fn definition(&self) -> ToolDefinition {
        let (description,properties,required,effect) = match self.name.as_str() {
            "list_mod_library" => ("列出当前已登记游戏可用的 Mods 原包、artifactId、来源和作者项目页。game_id 是本机登记 ID。gameId 为空的条目下载时按游戏名分类。", json!({"game_id":{"type":"string"}}),vec!["game_id"],Effect::Read),
            "match_mod_install" => ("只读核对目标游戏的本地适配规则、已校验原包、README、归档清单及 FOMOD。用候选状态判断安装方法；需要用户选择时询问具体选项。", json!({"game_id":{"type":"string"},"artifact_id":{"type":"string"}}),vec!["game_id","artifact_id"],Effect::Read),
            "forget_uninstalled_mod" => ("仅用于用户明确要求从管理列表删除：卸载或回滚成功后，或 inspect_install 建议 forget_missing_files 时，移除同一游戏的记录。文件均已缺失时无需卸载、回滚或补回文件。保留磁盘归档和审计历史。普通卸载不要调用。",json!({"game_id":{"type":"string"},"install_id":{"type":"string"}}),vec!["game_id","install_id"],Effect::ManagerWrite),
            "bind_mod_library_entry" => ("把下载时尚未登记游戏的库条目关联到用户选定的本机游戏实例。同名候选先让用户指定目标。",json!({"game_id":{"type":"string"},"artifact_id":{"type":"string"}}),vec!["game_id","artifact_id"],Effect::ManagerWrite),
            "install_library_mod" => (
                "从目标游戏的 Mods 库安装一个原包，准备并按审批提交同一份复制计划。先运行 match_mod_install。mode=adapter 使用已匹配的 modType；mode=approvedMappings 逐文件映射作者说明或用户选项，省略 modType 时目标相对已登记的游戏根。",
                json!({"request":{"type":"object","properties":{
                    "gameId":{"type":"string"},"artifactId":{"type":"string"},
                    "mode":{"type":"string","enum":["adapter","approvedMappings"]},
                    "modType":{"type":"string"},"folderName":{"type":"string"},
                    "fomodSelection":{"type":"string","enum":["automatic"]},
                    "sourceKind":{"type":"string","enum":["singleFile","archive"]},
                    "mappings":{"type":"array","items":{"type":"object","properties":{"source":{"type":"string"},"destination":{"type":"string"}},"additionalProperties":false,"required":["source","destination"]}}
                },"required":["gameId","artifactId","mode"],"additionalProperties":false}}),vec!["request"],Effect::ManagerWrite),
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
            "inspect_install" => ("只读检查一个安装的文件指纹、缺失或修改、覆盖过的原文件和与其他安装的路径重叠。协调预览可能建议安全卸载、回滚、仅移除文件均缺失的记录（forget_missing_files）或人工审查。rollbackTransactionId 只是建议，不会自动执行；本工具不调用 reconcile、不创建计划、不修改文件或数据库",json!({"game_id":{"type":"string"},"install_id":{"type":"string"}}),vec!["game_id","install_id"],Effect::Read),
            "uninstall_mod" => ("按游戏与安装记录核对归属，将独占且未修改的文件移入回收站。覆盖过原文件时用带 game_id、install_id 的 rollback_transaction 恢复备份。",json!({"game_id":{"type":"string"},"install_id":{"type":"string"}}),vec!["install_id"],Effect::ManagerWrite),
            _ => ("回滚一个已提交文件事务，展示原计划影响；冲突时保留现场而非强行覆盖",json!({"transaction_id":{"type":"string"},"game_id":{"type":"string"},"install_id":{"type":"string"}}),vec!["transaction_id"],Effect::ManagerWrite),
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
        let cancel = context.cancel.clone();
        let scope = context.resolution_scope.clone();
        tokio::task::spawn_blocking(move || {
            // Missing legacy game_id must not bypass the installation's actual owner.
            let owner = if name == "uninstall_mod" || (name == "rollback_transaction" && args["install_id"].is_string()) {
                Some(install::get_install(&engine, text(&args, "install_id")?)?.game_id)
            } else { None };
            let target_game = owner.as_deref().or_else(|| match name.as_str() {
                "install_library_mod" | "install_mod" | "plan_install" => args["request"]["gameId"].as_str(),
                _ => args["game_id"].as_str(),
            });
            if let (crate::sources::resolutions::ResolutionScope::Agent(conversation), Some(target)) = (&scope, target_game) {
                let selected = super::workspace::selection(&engine.db, conversation)?;
                super::workspace::resolve(selected.as_deref(), Some(target))?;
            }
            let (preview, plan) = match name.as_str() {
                "forget_uninstalled_mod" => {
                    let game_id = text(&args, "game_id")?;
                    let install_id = text(&args, "install_id")?;
                    let (name, status) = crate::mod_management::preview_record_removal(&engine, game_id, install_id)?;
                    let action = if status == "enabled" { "安装文件均已缺失，仅移除管理记录，不修改游戏文件" } else { "从管理列表删除" };
                    (json!({"action":action,"gameId":game_id,"installId":install_id,"name":name,"status":status}), None)
                }
                "install_library_mod" => {
                    let request = serde_json::from_value(args["request"].clone())?;
                    let preview = install::plan_library_install_cancellable(&engine, &request, &cancel)?;
                    (json!(preview), Some(preview.plan))
                }
                "bind_mod_library_entry" => {
                    let game_id = text(&args, "game_id")?;
                    let artifact_id = text(&args, "artifact_id")?;
                    let game = crate::games::get_game(&engine.db, game_id)?;
                    let entry = crate::mod_library::list_for_game(&engine.db, game_id)?.into_iter()
                        .find(|entry| entry.id == artifact_id)
                        .ok_or_else(|| AppError::NotFound("此游戏没有对应的库条目".into()))?;
                    (json!({"gameId":game.id,"gameName":game.name,"gameRoot":game.root_path,"artifactId":entry.id,"filename":entry.filename}),None)
                }
                "install_mod" => {
                    let preview = install::plan_install_cancellable(
                        &engine,
                        &serde_json::from_value(args["request"].clone())?,
                        &cancel,
                    )?;
                    (json!(preview), Some(preview.plan))
                }
                "uninstall_mod" => {
                    check_uninstall_action(&engine, &args, false)?;
                    let plan = install::plan_uninstall(&engine, text(&args, "install_id")?)?;
                    let preview = serialize_user_plan(&plan, serde_json::value::Serializer)?;
                    (preview, Some(plan))
                }
                "rollback_transaction" => {
                    check_uninstall_action(&engine, &args, true)?;
                    let original = engine.get_plan(text(&args, "transaction_id")?)?;
                    (
                        json!({"action":"rollback","originalPlan":original.into_user_paths()}),
                        None,
                    )
                }
                _ => (args, None),
            };
            Ok(PreparedTool {
                preview,
                plan,
                effect: if matches!(
                    name.as_str(),
                    "list_mod_library" | "match_mod_install" | "plan_install" | "list_installs" | "inspect_install"
                ) {
                    Effect::Read
                } else {
                    Effect::ManagerWrite
                },
            })
        })
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
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
        let cancel = context.cancel.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(plan) = prepared.plan {
                if name == "uninstall_mod" {
                    check_uninstall_action(&engine, &args, false)?;
                }
                engine.approve_plan(&plan.id)?;
                engine.apply_file_ops(&plan.id)?;
                install::reconcile(&engine.db)?;
                if name == "install_library_mod" {
                    return Ok(json!({"status":"committed", "transactionId":plan.id, "installId":prepared.preview["installId"], "fileCount":plan.entries.len()}));
                }
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
                return Ok(json!({"plan":plan.into_user_paths(),"status":"committed","preview":prepared.preview}));
            }
            match name.as_str() {
                "forget_uninstalled_mod" => {
                    let game = text(&args,"game_id")?;
                    let id = text(&args,"install_id")?;
                    if prepared.effect != Effect::ManagerWrite || prepared.preview["gameId"] != game || prepared.preview["installId"] != id {
                        return Err(AppError::Denied("目标与批准的记录不符".into()));
                    }
                    let status = text(&prepared.preview,"status")?;
                    crate::mod_management::remove_reviewed_record(&engine,game,id,status)?;
                    Ok(json!({"status":"removed","gameId":game,"installId":id}))
                }
                "list_mod_library" => Ok(json!(crate::mod_library::list_for_game(&engine.db, text(&args, "game_id")?)?)),
                "match_mod_install" => Ok(json!(install::match_library_install_cancellable(&engine, &install::LibraryMatchRequest {
                    game_id: text(&args, "game_id")?.into(), library_entry_id: text(&args, "artifact_id")?.into(),
                }, &cancel)?)),
                "bind_mod_library_entry" => {
                    let game_id = text(&args, "game_id")?;
                    let artifact_id = text(&args, "artifact_id")?;
                    if prepared.effect != Effect::ManagerWrite || prepared.preview["gameId"] != game_id || prepared.preview["artifactId"] != artifact_id {
                        return Err(AppError::Denied("归属与批准的游戏或文件不符".into()));
                    }
                    Ok(json!(crate::mod_library::bind_entry(&engine, game_id, artifact_id)?))
                }
                "plan_install" => Ok(json!(install::plan_install_cancellable(
                    &engine,
                    &serde_json::from_value(args["request"].clone())?,
                    &cancel,
                )?)),
                "list_installs" => {
                    let installs = crate::mod_management::visible_installs(&engine, text(&args, "game_id")?)?;
                    let result: Vec<_> = installs
                        .into_iter()
                        .map(install::InstalledMod::into_user_paths)
                        .collect();
                    Ok(json!(result))
                }
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
                    {
                        let guard = crate::transaction::write_lock();
                        check_uninstall_action(&engine, &args, true)?;
                        engine.rollback_transaction_locked(&guard, text(&args, "transaction_id")?)?;
                    }
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
