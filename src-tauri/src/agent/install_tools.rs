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
        "prepare_gta_rpf_job",
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
        registry.register(Arc::new(InstallTool {
            name: name.into(),
            tool_available: Arc::new(registry.tool_available()),
        }))?;
    }
    registry.register(Arc::new(GtaArchiveJobTool {
        tool_available: Arc::new(registry.tool_available()),
    }))?;
    registry.register(Arc::new(CheckModrinthDependencies))?;
    registry.register(Arc::new(VerifyInstallFiles))
}

// Optional presentation hint for previews and older callers.
fn suggestion(args: &Value) -> AppResult<Option<crate::mod_organization::OrganizationSuggestion>> {
    Ok(args
        .get("organization")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?)
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GtaJobArgs {
    game_id: String,
    artifact_id: Option<String>,
    install_id: Option<String>,
    kind: Option<String>,
    options: Option<Value>,
}
impl GtaJobArgs {
    fn validate(&self) -> AppResult<()> {
        match (
            self.artifact_id.as_deref(),
            self.install_id.as_deref(),
            self.kind.as_deref(),
        ) {
            (Some(id), None, Some("gameconfig" | "dlc" | "vehicle" | "ped")) if !id.is_empty() => {
                Ok(())
            }
            (None, Some(id), None) if !id.is_empty() && self.options.is_none() => Ok(()),
            _ => Err(AppError::Invalid(
                "GTA 安装需提供 artifact_id 与 kind；卸载仅提供 install_id".into(),
            )),
        }
    }
}

fn gta_adapter_slug(game: &crate::games::Game) -> AppResult<&str> {
    game.adapter_slug
        .as_deref()
        .filter(|slug| matches!(*slug, "gta5-legacy" | "gta5-enhanced"))
        .ok_or_else(|| AppError::Denied("游戏尚未绑定 GTA V 适配包".into()))
}

fn gta_mcp_config(
    engine: &crate::transaction::FileEngine,
    slug: &str,
) -> AppResult<crate::mcp::ServerConfig> {
    let id = format!("plugin_{slug}_rpf");
    let config = crate::mcp::McpServers::new(engine.db.clone()).get(&id)?;
    if config.plugin_slug.as_deref() != Some(slug) || !config.enabled {
        return Err(AppError::Denied(
            "请先从本游戏适配包的工具设置启用 RPF MCP".into(),
        ));
    }
    Ok(config)
}

// Use the manager's complete package-scoped identity, never a similarly named custom service.
fn gta_mcp_tool(
    engine: &crate::transaction::FileEngine,
    spec: &GtaJobArgs,
    available: &dyn Fn(&str) -> bool,
) -> AppResult<String> {
    let game = crate::games::get_game(&engine.db, &spec.game_id)?;
    let slug = gta_adapter_slug(&game)?;
    install::gta_rpf::validate_job_options(slug, spec.kind.as_deref(), spec.options.as_ref())?;
    let id = gta_mcp_config(engine, slug)?.id;
    let preferred = match spec.kind.as_deref() {
        Some("dlc")
            if spec.options.as_ref().is_some_and(|options| {
                options["packName"].is_string() || options["inspectJobId"].is_string()
            }) =>
        {
            Some("gta_rpf_register_dlc")
        }
        Some("dlc") => Some("gta_rpf_inspect_dlc"),
        Some("gameconfig") => Some("gta_rpf_replace_gameconfig"),
        Some("vehicle") => Some("gta_rpf_build_vehicle_pack"),
        Some("ped") => Some("gta_rpf_build_ped_pack"),
        _ => None,
    };
    preferred
        .into_iter()
        .chain(["gta_rpf_prepare"])
        .map(|name| crate::mcp::client::namespaced_tool_name(&id, name))
        .find(|tool| available(tool))
        .ok_or_else(|| {
            AppError::Denied(
                "RPF MCP 尚未连接或缺少此任务所需工具，请在本适配包的工具设置中确认。".into(),
            )
        })
}

struct GtaArchiveJobTool {
    tool_available: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GtaArchiveJobArgs {
    game_id: String,
    archive_path: String,
    #[serde(default)]
    source_paths: Vec<String>,
}

impl GtaArchiveJobTool {
    fn describe_tool(&self, context: &ToolContext, game_id: &str) -> AppResult<String> {
        use crate::sources::resolutions::ResolutionScope;
        if context.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        if !matches!(&context.resolution_scope, ResolutionScope::Agent(_)) {
            return Err(AppError::Denied("GTA RPF 任务仅面向本地桌面 Agent".into()));
        }
        super::workspace::tool_target(context, Some(game_id))?;
        let game = crate::games::get_game(&context.engine.db, game_id)?;
        let config = gta_mcp_config(&context.engine, gta_adapter_slug(&game)?)?;
        let tool = |name| crate::mcp::client::namespaced_tool_name(&config.id, name);
        // The add/delete release also introduced the standalone manifest.
        if !config.is_gta_rpf_launcher()
            || [
                "gta_rpf_describe_job",
                "gta_rpf_add_entry",
                "gta_rpf_delete_entry",
            ]
            .into_iter()
            .any(|name| !(self.tool_available)(&tool(name)))
        {
            return Err(AppError::Denied(
                "请连接支持独立归档任务的本游戏适配包 RPF MCP".into(),
            ));
        }
        Ok(tool("gta_rpf_describe_job"))
    }
}

#[async_trait::async_trait]
impl AgentTool for GtaArchiveJobTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "prepare_gta_rpf_archive_job".into(),
            description: "将已绑定 GTA 游戏的指定现有 RPF 和可选载荷复制到私有任务目录。支持直接读取、列出、添加、替换和删除归档条目。archive_path 是游戏目录内的相对 .rpf 文件路径，例如 mods/update/update.rpf；common/data/dlclist.xml 或 x64/example.rpf/file.xml 在 MCP 的 entry 参数中另传。只使用指定路径，缺失时返回错误。仅在用户指定或授权同一游戏目录下的载荷文件时传 source_paths。返回 jobId、archive、各载荷的 sourcePath 和用于查询任务的完整 mcpTool。独立任务只生成暂存副本，不能用于正式安装或卸载。".into(),
            parameters: json!({"type":"object","properties":{
                "game_id":{"type":"string","minLength":1},
                "archive_path":{"type":"string","minLength":1,"maxLength":512,"description":"游戏相对 .rpf 文件路径，保留实际名称。"},
                "source_paths":{"type":"array","maxItems":install::gta_rpf::MAX_ARCHIVE_PAYLOADS,
                    "items":{"type":"string","minLength":1,"maxLength":512},"description":"用户授权的同一游戏目录相对载荷文件，用于添加或替换条目。"}
            },"required":["game_id","archive_path"],"additionalProperties":false}),
            effect: Effect::ManagerWrite,
        }
    }

    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        let spec: GtaArchiveJobArgs = serde_json::from_value(args.clone())?;
        self.describe_tool(context, &spec.game_id)?;
        Ok(PreparedTool {
            preview: args.clone(),
            effect: Effect::ManagerWrite,
            plan: None,
        })
    }

    async fn execute(
        &self,
        context: &ToolContext,
        args: Value,
        prepared: PreparedTool,
    ) -> AppResult<Value> {
        if prepared.effect != Effect::ManagerWrite
            || prepared.plan.is_some()
            || prepared.preview != args
        {
            return Err(AppError::Denied("GTA 暂存任务与批准的选项不一致".into()));
        }
        let spec: GtaArchiveJobArgs = serde_json::from_value(args)?;
        let tool = self.describe_tool(context, &spec.game_id)?;
        let engine = context.engine.clone();
        let cancel = context.cancel.clone();
        tokio::task::spawn_blocking(move || {
            let job = install::gta_rpf::prepare_archive_job(
                &engine,
                &spec.game_id,
                &spec.archive_path,
                &spec.source_paths,
                &cancel,
            )?;
            let mut result = json!(job);
            result["mcpTool"] = json!(tool);
            Ok(result)
        })
        .await
        .map_err(|error| AppError::Other(error.to_string()))?
    }
}

struct InstallTool {
    tool_available: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    name: String,
}
fn text<'a>(args: &'a Value, key: &str) -> AppResult<&'a str> {
    args[key]
        .as_str()
        .ok_or_else(|| AppError::Invalid(format!("缺少参数 {key}")))
}

fn library_install_commit_error(error: AppError) -> AppError {
    match error {
        AppError::Conflict(reason) => AppError::Conflict(format!(
            "{reason}\n请保留冲突文件。用 match_mod_install 重新检查原包，若有 activeInstallId，用 inspect_install 核对现有安装。预览过期时，重新匹配并提交新安装计划。"
        )),
        error => error,
    }
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
            && report.files.iter().all(|file| {
                file.state == "owned"
                    && (action.recommended_action == "rollback_install"
                        || file.other_install_ids.is_empty())
            })
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
        let (description,mut properties,required,effect) = match self.name.as_str() {
            "list_mod_library" => ("列出当前已登记游戏可用的 Mods 原包、artifactId、来源和作者项目页。game_id 是本机登记 ID。gameId 为空的条目下载时按游戏名分类。", json!({"game_id":{"type":"string"}}),vec!["game_id"],Effect::Read),
            "match_mod_install" => ("只读核对目标游戏的本地适配规则、已校验原包、README、归档清单及 FOMOD。若 activeInstallId 非空，先用当前 game_id 和该安装 ID 调用 inspect_install，按报告核对已有部署。根据候选状态选择安装方法，需要用户选择时询问具体选项。手工映射时从 archiveEntries 取实际文件的完整成员路径（含包内目录）。archiveEntriesTruncated 为 true 时先补查所需成员，勿猜文件名或用外层 ZIP 文件名代替。", json!({"game_id":{"type":"string"},"artifact_id":{"type":"string"}}),vec!["game_id","artifact_id"],Effect::Read),
            "prepare_gta_rpf_job" => (
                "仅供已绑定 GTA 适配包的本地游戏。先确认该包的 RPF MCP 已连接；按批准的选项，把校验过的输入复制到独立任务目录，不修改游戏、不启动 MCP。安装提供 artifact_id 和 kind；卸载仅提供 install_id。Enhanced 仅支持 gameconfig 和独立 DLC，menyoo 保持关闭。使用返回的完整 mcpTool 名称调用该工具，只传入 {jobId}。DLC 探测成功后，用 options.inspectJobId 准备正式任务。正式安装或卸载任务成功后，再用 install_library_mod 的 gtaRpf 模式或 uninstall_mod 的 rpf_job_id 提交。",
                json!({"game_id":{"type":"string"},"artifact_id":{"type":"string"},"install_id":{"type":"string"},
                    "kind":{"type":"string","enum":["dlc","gameconfig","vehicle","ped"]}, "options":{"type":"object","additionalProperties":false,
                    "properties":{"inspectJobId":{"type":"string","pattern":"^[a-f0-9]{32}$","description":"DLC 暂存探测任务返回的 jobId；与 packName 互斥。"},"sourceDir":{"type":"string"},"packName":{"type":"string"},"modelName":{"type":"string"},
                    "sex":{"type":"string","enum":["Male","Female"]},"streamed":{"type":"boolean"},"menyoo":{"type":"boolean"}}}}),vec!["game_id"],Effect::ManagerWrite),
            "forget_uninstalled_mod" => ("仅用于用户明确要求从管理列表删除：卸载或回滚成功后，或 inspect_install 建议 forget_missing_files 时，移除同一游戏的记录。文件均已缺失时无需卸载、回滚或补回文件。保留磁盘归档和审计历史。普通卸载不要调用。",json!({"game_id":{"type":"string"},"install_id":{"type":"string"}}),vec!["game_id","install_id"],Effect::ManagerWrite),
            "bind_mod_library_entry" => ("把下载时尚未登记游戏的库条目关联到用户选定的本机游戏实例。同名候选先让用户指定目标。",json!({"game_id":{"type":"string"},"artifact_id":{"type":"string"}}),vec!["game_id","artifact_id"],Effect::ManagerWrite),
            "install_library_mod" => (
                "从目标游戏的 Mods 库安装或重装原包，准备并按审批提交同一份复制计划。先运行 match_mod_install。GTA 归档走 mode=gtaRpf 与已处理的 jobId，不用整包复制。mode=adapter 使用已匹配的 modType。mode=approvedMappings 按作者说明或用户选项逐文件映射。部署 ZIP 成员时用 sourceKind=archive，不能复制外层 ZIP 来代替或冒充组件。仅在适配规则或作者说明明确要求游戏读取原文件时，才用 sourceKind=singleFile 原样部署，例如 Minecraft JAR。省略 modType 时目标相对已登记的游戏根。匹配结果中的 activeInstallId 非空时先用 inspect_install 核对该安装及建议操作。其他目标冲突可用 list_installs 和 inspect_install 核对归属。",
                json!({"request":{"type":"object","properties":{
                    "gameId":{"type":"string"},"artifactId":{"type":"string"},
                    "mode":{"type":"string","enum":["adapter","approvedMappings","gtaRpf"]},
                    "jobId":{"type":"string","pattern":"^[a-f0-9]{32}$","description":"仅 gtaRpf 模式；由 prepare_gta_rpf_job 签发并经该适配包的 MCP 处理。"},
                    "modType":{"type":"string"},"folderName":{"type":"string"},
                    "fomodSelection":{"type":"string","enum":["automatic"]},
                    "sourceKind":{"type":"string","enum":["singleFile","archive"],"description":"仅 mode=approvedMappings 使用。archive 逐文件部署归档成员，singleFile 原样部署一个游戏原生载荷。"},
                    "mappings":{"type":"array","items":{"type":"object","properties":{
                        "source":{"type":"string","description":"archive 填 archiveEntries 中的完整成员路径（含包内目录），不填外层包名或磁盘路径。singleFile 填库内原始文件名。"},
                        "destination":{"type":"string","description":"安装根下的相对文件路径，含目标文件名。传 modType 时以适配类型的根为准，否则以登记的游戏根为准。"}
                    },"additionalProperties":false,"required":["source","destination"]}}
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
            "uninstall_mod" => ("按游戏与安装记录核对归属，将独占且未修改的文件移入回收站。GTA RPF 需先重建其余贡献，并提供 rpf_job_id。其他覆盖过原文件的普通安装按报告用 rollback_transaction 恢复备份。",json!({"game_id":{"type":"string"},"install_id":{"type":"string"},"rpf_job_id":{"type":"string","pattern":"^[a-f0-9]{32}$"}}),vec!["install_id"],Effect::ManagerWrite),
            _ => ("回滚一个已提交文件事务，展示原计划影响；冲突时保留现场而非强行覆盖",json!({"transaction_id":{"type":"string"},"game_id":{"type":"string"},"install_id":{"type":"string"}}),vec!["transaction_id"],Effect::ManagerWrite),
        };
        if matches!(
            self.name.as_str(),
            "install_library_mod" | "install_mod" | "plan_install"
        ) {
            // Keep organization outside the deployment request. The library request's
            // tagged union rejects unknown fields in each installation mode.
            properties["organization"] = json!({"type":"object","additionalProperties":false,
                "description":"可选的展示名和分类建议，用于预览和兼容旧请求。仅传已核实的名称和标签。安装成功后的整理由管理器负责。",
                "properties":{"displayName":{"type":"string","minLength":1,"maxLength":200},
                              "tags":{"type":"array","minItems":1,"maxItems":3,"items":{"type":"string","minLength":1,"maxLength":48}}},
                "required":["displayName","tags"]});
        }
        ToolDefinition {
            name: self.name.clone(),
            description: description.into(),
            parameters: json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
            effect,
        }
    }
    fn inbound_definition(&self) -> ToolDefinition {
        let mut definition = self.definition();
        if self.name == "uninstall_mod" {
            if let Some(properties) = definition.parameters["properties"].as_object_mut() {
                properties.remove("rpf_job_id");
            }
            definition.description =
                "按已登记清单安全卸载普通 Mod。GTA RPF 贡献只能由桌面适配包工作流重建。".into();
        }
        definition
    }
    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        if context.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let name = self.name.clone();
        let available = self.tool_available.clone();
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
            if matches!(scope, crate::sources::resolutions::ResolutionScope::Mcp(_))
                && (name == "prepare_gta_rpf_job" || args.get("rpf_job_id").is_some() || args["request"]["mode"] == "gtaRpf") {
                return Err(AppError::Denied("GTA RPF 任务仅面向本地桌面 Agent".into()));
            }
            let (preview, plan) = match name.as_str() {
                "prepare_gta_rpf_job" => {
                    let spec: GtaJobArgs = serde_json::from_value(args.clone())?;
                    spec.validate()?;
                    gta_mcp_tool(&engine, &spec, &*available)?;
                    (args.clone(), None)
                }
                "forget_uninstalled_mod" => {
                    let game_id = text(&args, "game_id")?;
                    let install_id = text(&args, "install_id")?;
                    let (name, status) = crate::mod_management::preview_record_removal(&engine, game_id, install_id)?;
                    let action = if status == "enabled" { "安装文件均已缺失，仅移除管理记录，不修改游戏文件" } else { "从管理列表删除" };
                    (json!({"action":action,"gameId":game_id,"installId":install_id,"name":name,"status":status}), None)
                }
                "install_library_mod" => {
                    let request = serde_json::from_value(args["request"].clone())?;
                    let preview = install::plan_library_install_with_organization(&engine, &request, suggestion(&args)?, &cancel)?;
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
                    let preview = install::plan_install_with_organization(
                        &engine,
                        &serde_json::from_value(args["request"].clone())?,
                        suggestion(&args)?,
                        &cancel,
                    )?;
                    (json!(preview), Some(preview.plan))
                }
                "uninstall_mod" => {
                    let plan = if let Some(job) = args.get("rpf_job_id") {
                        install::gta_rpf::plan_uninstall(&engine, text(&args, "game_id")?, text(&args, "install_id")?, job.as_str().ok_or_else(|| AppError::Invalid("rpf_job_id 无效".into()))?, &cancel)?
                    } else {
                        check_uninstall_action(&engine, &args, false)?;
                        install::plan_uninstall(&engine, text(&args, "install_id")?)?
                    };
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
        let available = self.tool_available.clone();
        let engine = if name == "rollback_transaction" {
            context.file_engine()
        } else {
            context.engine.clone()
        };
        let cancel = context.cancel.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(plan) = prepared.plan {
                if name == "uninstall_mod" && args.get("rpf_job_id").is_none() {
                    check_uninstall_action(&engine, &args, false)?;
                }
                engine.approve_plan(&plan.id)
                    .and_then(|()| engine.apply_file_ops(&plan.id))
                    .map_err(|error| {
                        if name == "install_library_mod" {
                            library_install_commit_error(error)
                        } else {
                            error
                        }
                    })?;
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
                "prepare_gta_rpf_job" => {
                    if prepared.effect != Effect::ManagerWrite || prepared.preview != args {
                        return Err(AppError::Denied("GTA 暂存任务与批准的选项不一致".into()));
                    }
                    let spec: GtaJobArgs = serde_json::from_value(args)?;
                    spec.validate()?;
                    let tool = gta_mcp_tool(&engine, &spec, &*available)?;
                    let job = install::gta_rpf::prepare_job(&engine, &spec.game_id, spec.artifact_id.as_deref(), spec.install_id.as_deref(), spec.kind.as_deref(), spec.options.as_ref(), &cancel)?;
                    let mut receipt = json!(job);
                    receipt["mcpTool"] = json!(tool);
                    Ok(receipt)
                }
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
                "match_mod_install" => {
                    let game_id = text(&args, "game_id")?;
                    let mut result = json!(install::match_library_install_cancellable(&engine, &install::LibraryMatchRequest {
                        game_id: game_id.into(), library_entry_id: text(&args, "artifact_id")?.into(),
                    }, &cancel)?);
                    result["organizationLocale"] = json!(crate::settings::mod_organization_locale(&engine.db));
                    result["existingTags"] = json!(crate::mod_organization::list_mod_tags(&engine.db, game_id)?
                        .into_iter().map(|tag| tag.name).collect::<Vec<_>>());
                    Ok(result)
                }
                "bind_mod_library_entry" => {
                    let game_id = text(&args, "game_id")?;
                    let artifact_id = text(&args, "artifact_id")?;
                    if prepared.effect != Effect::ManagerWrite || prepared.preview["gameId"] != game_id || prepared.preview["artifactId"] != artifact_id {
                        return Err(AppError::Denied("归属与批准的游戏或文件不符".into()));
                    }
                    Ok(json!(crate::mod_library::bind_entry(&engine, game_id, artifact_id)?))
                }
                "plan_install" => Ok(json!(install::plan_install_with_organization(
                    &engine,
                    &serde_json::from_value(args["request"].clone())?,
                    suggestion(&args)?,
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
