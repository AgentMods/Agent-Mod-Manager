use super::{InstallScope, MOD_INN_DOWNLOAD};
use crate::agent::tools::ToolRegistry;
use serde_json::{json, Value};

impl InstallScope {
    /// Keep the registry allowlist and present the parameters the model can choose.
    pub(crate) fn tool_definitions(&self, registry: &ToolRegistry) -> Vec<Value> {
        registry.select_definitions(|name, tool| {
            if !self.allows(name, tool) {
                return None;
            }
            let mut definition = tool.definition();
            let description = match name {
                "ask_user" => return Some(definition),
                "list_mod_versions" => {
                    omit_arguments(&mut definition.parameters, &["source_id", "project_id", "game_id"]);
                    "列出当前作品的发布版本。Modrinth 和 Thunderstore 支持此查询，其他来源可能返回 unsupported。game_version 和 loader 仅按来源声明筛选，Thunderstore 缺少筛选声明时返回 limited。选定版本后将精确 version_id 传给 resolve_mod。"
                }
                "resolve_mod" => {
                    omit_arguments(&mut definition.parameters, &["source_id", "project_id", "game_id"]);
                    "解析当前作品的指定版本，可下载时返回 download.resolution_id。已选定版本时须传精确 version_id，3DM 的 version_id 是数字资源 ID 字符串。只有已知时才提供 game_version 和 loader。Modrinth 的 dependencies 是来源声明，null 表示未知，空数组表示来源未声明依赖。manual_download 需要用户从原作品页选择文件并手动导入。"
                }
                "download_mod" => {
                    omit_arguments(&mut definition.parameters, &["gameId", "gameName", "author", "projectPage"]);
                    if self.key.source_id == "glossmod_3dm" {
                        "使用当前会话的 3DM 作品详情或 resolve_mod 返回的 download.resolution_id 下载并归档本作品。成功返回 artifactId，可交给 match_mod_install。解析凭证只能用一次，传输失败且未保存回执时重新解析同一精确版本。"
                    } else {
                        "使用当前会话 resolve_mod 返回的 download.resolution_id 下载并归档本作品。成功返回 artifactId，可交给 match_mod_install。解析凭证只能用一次，传输失败且未保存回执时重新解析同一精确版本。"
                    }
                }
                "retry_mod_library_download" => {
                    omit_arguments(&mut definition.parameters, &["gameId", "gameName"]);
                    "原生下载已保存但尚未成功入库时，传入错误回执中的 artifactId 重试本地归档。成功后使用返回的 artifactId 检查安装匹配。"
                }
                "list_mod_library" => {
                    omit_arguments(&mut definition.parameters, &["game_id"]);
                    "列出当前作品在目标游戏 Mods 库中的归档。使用返回的 artifactId 检查安装匹配。"
                }
                "match_mod_install" => {
                    omit_arguments(&mut definition.parameters, &["game_id"]);
                    "用本作品已归档的 artifact_id 检查目标游戏的适配规则、README、归档成员和 FOMOD。若 activeInstallId 非空，用该 ID 调用 inspect_install 核对并报告已有部署。根据 candidates 和 fomodStatus 选择安装方式，选项不明确时询问用户。手工映射须使用 archiveEntries 中的完整成员路径。清单已截断且缺少所需路径，或必须走 requiresGtaRpf 时，停止页内安装并说明原因。"
                }
                "inspect_install" => {
                    omit_arguments(&mut definition.parameters, &["game_id"]);
                    "只读检查 match_mod_install 返回的 activeInstallId。将其作为 install_id 传入，核对已有安装的文件状态与归属。"
                }
                "install_library_mod" => {
                    if let Some(request) = definition.parameters.pointer_mut("/properties/request") {
                        omit_arguments(request, &["gameId", "jobId"]);
                        request["properties"]["mode"]["enum"] = json!(["adapter", "approvedMappings"]);
                        request["oneOf"] = json!([
                            {
                                "properties":{"mode":{"const":"adapter"}},
                                "required":["modType"],
                                "not":{"anyOf":[{"required":["sourceKind"]},{"required":["mappings"]},{"required":["fomodSelection","folderName"]}]}
                            },
                            {
                                "properties":{"mode":{"const":"approvedMappings"}},
                                "required":["sourceKind","mappings"],
                                "not":{"anyOf":[{"required":["folderName"]},{"required":["fomodSelection"]}]}
                            }
                        ]);
                    }
                    "先用 match_mod_install 核对归档，再用其 artifactId 准备并按权限提交安装计划。mode=adapter 使用已匹配的 modType。requiresFolderName 使用已确认的 folderName。fomodStatus=automatic 且存在可用适配候选时，使用对应 modType 和 fomodSelection=automatic，省略 folderName 与 mappings。mode=approvedMappings 按作者说明或已确认选项映射，sourceKind=archive 使用完整包内成员路径，singleFile 只用于游戏直接读取的原文件。同一归档已有活动安装时，检查并报告现有部署。整理信息可省略。若提供 organization，须与 request 同级放在顶层。"
                }
                "file_received_mod" => {
                    let parameters = &mut definition.parameters;
                    omit_arguments(parameters, &["gameId", "gameName", "modName", "projectPage"]);
                    if let Some(schema) = parameters.as_object_mut() {
                        schema.remove("oneOf");
                        schema.remove("required");
                    }
                    "将本作品已验证的原厂下载回执归档到目标游戏 Mods 库。可省略 filename，或填回执中的精确文件名。归档核对文件名、大小和 SHA-256。RAR 仅支持非加密单卷归档。成功返回 artifactId，可交给 match_mod_install。"
                }
                _ if tool.host_tool_kind() == Some(MOD_INN_DOWNLOAD) => {
                    omit_arguments(&mut definition.parameters, &["work_id", "gameId", "gameName", "author", "projectPage"]);
                    "下载本作品附件列表中已确认的 file_id。签发会计入下载次数。下载并归档成功后返回 artifactId，可交给 match_mod_install。"
                }
                _ => {
                    let (source, remote) = tool.stock_source_tool()?;
                    let (description, parameters) = stock_tool_definition(source, remote);
                    definition.parameters = parameters;
                    description
                }
            };
            definition.description = description.into();
            Some(definition)
        })
    }
}

fn omit_arguments(parameters: &mut Value, fields: &[&str]) {
    if let Some(properties) = parameters
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    {
        for field in fields {
            properties.remove(*field);
        }
    }
    if let Some(required) = parameters.get_mut("required").and_then(Value::as_array_mut) {
        required.retain(|field| field.as_str().is_none_or(|name| !fields.contains(&name)));
    }
}

fn stock_tool_definition(source: &str, remote: &str) -> (&'static str, Value) {
    // The scope supplies the game and project fields before calling the stock tool.
    let offset = json!({"type":"integer","minimum":0,"maximum":10000});
    let limit = json!({"type":"integer","minimum":1,"maximum":100});
    let (description, properties, required): (_, _, &[&str]) = match (source, remote) {
        ("glossmod_http", "list_work_files") => (
            "查看当前 Mod 客栈作品的附件、版本和下载权限。选定可下载附件后使用其 file_id。",
            json!({}),
            &[],
        ),
        ("glossmod_3dm", _) => (
            "查询当前 3DM 作品。返回 download.resolution_id 时交给 download_mod。无法验证官方直链时，由用户从原作品页选择文件并手动导入。",
            json!({}),
            &[],
        ),
        ("thunderstore", "download_version") => (
            "下载当前 Thunderstore 作品的精确版本。返回 status=saved 时，用 file_received_mod 归档该回执。",
            json!({"version":{"type":"string","maxLength":64,"pattern":"^[0-9]+\\.[0-9]+\\.[0-9]+$"}}),
            &["version"],
        ),
        (_, "download_file") => (
            "下载当前作品附件列表中已确认的 file_id。返回 status=saved 时，用 file_received_mod 归档该回执。",
            json!({"file_id":{"type":"integer","minimum":1,"maximum":9_007_199_254_740_991u64}}),
            &["file_id"],
        ),
        ("thunderstore", _) => (
            "查看当前 Thunderstore 作品的发布版本，确认后使用精确版本号下载。",
            json!({"offset":offset,"limit":limit}),
            &[],
        ),
        ("modio", _) => (
            "查看当前作品的附件及其版本，确认后使用 file_id 下载。",
            json!({"offset":offset,"limit":limit}),
            &[],
        ),
        ("curseforge", _) => (
            "查看当前作品的附件及其版本，确认后使用 file_id 下载。",
            json!({"index":offset,"page_size":limit}),
            &[],
        ),
        _ => (
            "查看当前作品的附件及其版本，确认后使用 file_id 下载。",
            json!({}),
            &[],
        ),
    };
    (
        description,
        json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}),
    )
}
