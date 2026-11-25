---
name: system-mod-install
description: "从 Mods 游戏分类库安装或重装指定 Mod。先匹配适配规则、README 和 FOMOD，本地依据不足时查作者发布说明，再生成逐文件复制计划。"
---

# 从游戏分类库安装 Mod

1. 用当前已登记游戏的 `game_id` 和 `list_mod_library` 选择用户指定的 `artifactId`。若用户已从游戏详情点选条目，沿用传入的 ID。同名项目或多个版本需要用户决定时，用 `ask_user` 列出候选。若条目尚未绑定且用户已选定本机游戏，调用 `bind_mod_library_entry`。游戏尚未登记时可先按游戏名下载，安装时由用户在游戏库登记目标目录。
2. 调用 `match_mod_install({game_id, artifact_id})`，核对原包、目录结构、适配规则、包内 README 和 FOMOD。若返回非空的 `activeInstallId`，用当前 `game_id` 和该 ID 调用 `inspect_install`，先核对已有部署并按下文处理。只有一个明确匹配时使用 `install_library_mod` 的 `mode: adapter` 和对应 `modType`。确定性 FOMOD 传 `fomodSelection: automatic`。需要选项时，使用返回的分组和逐文件映射，让用户选择有分歧的组件。若 `archiveEntriesTruncated` 为 `true` 且报告缺少所需成员，先用 `extract_preview` 对已核实的原包路径分页查看清单。
   - GTA V 的 `requiresGtaRpf` **不是复制规则**。核对绑定的是 Legacy 还是 Enhanced，先读该适配包的 SKILL；只有用户已启动对应的可选 RPF MCP 且版本能力已验证时，才能 `prepare_gta_rpf_job({game_id,artifact_id,kind,options})`。将返回的 `jobId` 交给返回的 `mcpTool`。DLC 内部名字未知时，先做探测，用 `options.inspectJobId` 再准备正式任务。最后用 `install_library_mod({request:{gameId,artifactId,mode:"gtaRpf",jobId}})` 预览并审批。只以事务提交和安装记录为准，不以工具回报成功为准。
   - `unsupportedEdition` 表示该版 RPF 未验证，不启动 MCP。GTA V 混合包、多个 gameconfig 或未知布局需用户选择；普通脚本、ASI 不需要 RPF 服务。OIV 或工具包不按 Mod 复制；缺工具、缺版别证明、共享文件漂移时停下，不用逐文件映射或通用文件操作代替归档编辑。
3. 本地 README 已说明安装方法且与当前版本及包内文件相符时，直接整理映射。仅在本地无法确定时查看该 Mod 作者的说明：先按库条目的来源、项目 ID 和版本查询已连接的来源 MCP 的作品详情；仍缺少正文时，用已连接的 Playwright 打开核实过的作者项目页或文件页，读取安装章节及具体版本的差异。库里缺少项目页时，可用游戏名、Mod 名和版本定位官方作品页，再核对作者与文件；查询只传公开名称。搜索摘要、文件扩展名或同名项目不足以确定目标。无法取得作者正文时说明具体缺项，请用户提供作者说明或明确路径。
4. 按作者说明或用户确认的选项部署时，用 `install_library_mod` 的 `mode: approvedMappings`。即使游戏没有适配包，也可以使用明确的逐文件映射。
   - ZIP 等分发归档用 `sourceKind: archive`。每项 `source` 使用 `archiveEntries` 中实际存在的完整成员路径，保留包内目录和大小写。每项 `destination` 是相对安装根的文件路径，含目标文件名。省略 `modType` 时以已登记的游戏根为准，明确使用某个适配类型时才传 `modType` 并以该类型的根为准。
   - 若作者要求将 `Release/Example.pak` 安装为 `Content/Paks/~mods/Example.pak`，映射使用这两条相对路径。按说明为所需配套文件分别添加映射。`~mods` 是字面目录名。由安装工具从校验过的原包部署成员，不用 `plan_file_ops` 或 `apply_file_ops` 代替，也不把外层 ZIP 复制或改名为游戏组件。
   - 仅在适配规则或作者说明明确要求游戏直接读取原文件时，用 `sourceKind: singleFile` 原样部署，例如放入 `mods/` 的 Minecraft JAR。此时 `source` 填库内原始文件名。记录映射依据。说明只支持脚本安装、缺前置或目标未登记时，向用户说明尚需解决的步骤。
5. 安装成功后的展示名和分类由管理器整理。已有可靠的展示名与标签且需要用于预览时，可将 `organization` 与 `request` 并列传递。原包名和部署路径仍用于安装。
6. 有明确的 Minecraft 版本和加载器时，先核对 Modrinth 的必需依赖与冲突。`install_library_mod` 会生成复制计划并按权限设置提交；审批界面可核对新增、覆盖和备份。只在返回 `committed` 后报告安装记录、事务编号和实际文件数。仅请求下载时到入库回执为止。

## 重装与修复

- 用 `inspect_install` 核对同一游戏的相关安装。缺少安装 ID 或需要核对其他部署时，先用 `list_installs` 查找。当前原包已正确部署且用户只要求安装时，报告已安装。若恢复覆盖前的文件或上一个版本，按报告的 `rollback_install` 或 `rollback_replacement` 建议，携带 `game_id`、`install_id` 及报告中的事务编号调用 `rollback_transaction`。需要先卸载且报告为 `safe_uninstall` 时，用 `uninstall_mod`。核对结果后，需要重装时重新匹配同一库内原包并用 `install_library_mod` 提交新计划。
- 两个不同原包都要写入同一路径（如 `GlossTrainer.asi`）且文件内容不同，保留当前文件。用 `list_installs` 和 `inspect_install` 核对相关 Mod 和共享路径（`otherInstallIds`）。告诉用户冲突路径、可核实的版本及替换可能影响的 Mod，再用 `ask_user` 让用户选择保留哪个版本。若保留当前版本且新 Mod 必须使用另一版本，暂停新安装。按受影响安装的核对结果解决冲突后，再用 `install_library_mod` 生成新计划。
- 若先前误用了通用文件计划，尚未执行的计划应取消或拒绝审批。已提交时，先核对原事务的编号、状态和路径，再把原 `transaction_id` 交给 `rollback_transaction` 审批回滚。若报告待恢复事务，等待管理器恢复。若回滚被阻止、目标归属不明或发现文件已被修改、共享或存在其他冲突，保留现场，说明涉及的文件和记录，提交用户审核。勿用通用移动、覆盖或 `trash_paths` 清空冲突路径。
