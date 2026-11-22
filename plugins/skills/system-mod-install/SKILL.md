---
name: system-mod-install
description: "从 Mods 游戏分类库安装或重装指定 Mod。先匹配适配规则、README 和 FOMOD，本地依据不足时查作者发布说明，再生成逐文件复制计划。"
---

# 从游戏分类库安装 Mod

1. 用当前已登记游戏的 `game_id` 和 `list_mod_library` 选择用户指定的 `artifactId`。若用户已从游戏详情点选条目，沿用传入的 ID。同名项目或多个版本需要用户决定时，用 `ask_user` 列出候选。若条目尚未绑定且用户已选定本机游戏，调用 `bind_mod_library_entry`。游戏尚未登记时可先按游戏名下载，安装时由用户在游戏库登记目标目录。
2. 调用 `match_mod_install({game_id, artifact_id})`，核对原包、目录结构、适配规则、包内 README 和 FOMOD。只有一个明确匹配时直接使用 `install_library_mod` 的 `mode: adapter` 和对应 `modType`。确定性 FOMOD 传 `fomodSelection: automatic`。需要选项时，使用返回的分组和逐文件映射，让用户选择实际有分歧的组件。单文件游戏载荷保留原包，例如要完整复制到 `mods/` 的 Minecraft JAR。
3. 本地 README 已说明安装方法且与当前版本及包内文件相符时，直接整理映射。仅在本地无法确定时查看该 Mod 作者的说明：先按库条目的来源、项目 ID 和版本查询已连接的来源 MCP 的作品详情；仍缺少正文时，用已连接的 Playwright 打开核实过的作者项目页或文件页，读取安装章节及具体版本的差异。库里缺少项目页时，可用游戏名、Mod 名和版本定位官方作品页，再核对作者与文件；查询只传公开名称。搜索摘要、文件扩展名或同名项目不足以确定目标。无法取得作者正文时说明具体缺项，请用户提供作者说明或明确路径。
4. 按作者说明部署时使用 `mode: approvedMappings`。`sourceKind: archive` 的每个 `source` 是归档中实际存在的完整条目名；`singleFile` 的 `source` 是库内原始文件名。`destination` 是相对于已登记游戏根目录的文件路径；仅当明确使用某个适配类型的安装根时传 `modType`。保留作者要求的子目录。游戏没有适配包也可以使用这些映射。记录依据；说明只支持脚本安装、缺前置或目标未登记时，向用户说明尚需解决的步骤。
5. 有明确的 Minecraft 版本和加载器时，先核对 Modrinth 的必需依赖与冲突。`install_library_mod` 会生成复制计划并按权限设置提交；审批界面可核对新增、覆盖和备份。只在返回 `committed` 后报告安装记录、事务编号和实际文件数。重装时继续使用库内原包。仅请求下载时到入库回执为止。
