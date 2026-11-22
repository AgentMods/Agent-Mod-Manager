---
name: system-mod-download
description: "优先通过对应来源 MCP 查找并下载指定 Mod，仅在来源无法取得选定文件时用 Playwright MCP 从官方页面下载。"
---

# 查找并下载指定 Mod

先核对游戏、版本、加载器和平台。已知项目页或文件 ID 时直接核对，不重复搜索；多个主文件、补丁或同名项目仍有歧义时用 `ask_user` 确认。只要求查找时，返回官方项目页、精确文件及已核实的兼容条件，不触发下载。为适配包研究而取样时，选定代表文件，结合作者说明、依赖和 `extract_preview` 的内部目录核实，到证据核实为止。

## 优先使用对应来源 MCP

从本轮已注册的 `mcp__…` 工具中按服务 ID 和**上游工具名**匹配；下表是实测名称，当前工具的 schema、权限和返回结果才决定实际能力。平台自己的 `search_mods` 不是管理器原生同名工具。Mod 客栈即使不在原生目录里，只要 `glossmod_http` 已连接且工具可用，就能按同一流程查找和下载。

| 平台 · 服务 ID | 查询及精确文件 |
| --- | --- |
| Mod 客栈 · `glossmod_http` | `list_works` → `list_work_files`；核对作品、附件版本和 `entitlement`。确认下载已选且可访问的文件后，调用 `get_work_file_download({work_id, file_id})`。 |
| 3DM · `glossmod_3dm` | `get-game-list` → `get-mod-list` → `get-mod-detail`，确认接口成功后，从 `mods_resource` 核对资源 ID、版本及下载入口；Minecraft 专区用 `get-minecraft-mod-list` / `get-minecraft-mod-detail`，按实际结构核对。详情中的资源链接不是已保存文件。 |
| mod.io · `modio` | `search_games` → `search_mods` → `get_mod`；用 `list_mod_files` / `get_mod_file` 选文件、`get_mod_dependencies` 查前置，再用 `download_file` 下载选定文件。 |
| CurseForge · `curseforge` | `list_games` → `search_mods` → `get_mod`；按加载器筛选时同时传游戏版本。用 `list_mod_files` / `get_mod_file` 选文件、`get_file_dependencies` 查前置。优先用可保存文件的 `download_file`；`get_file_download_url` 只提供链接，不表示已保存。 |
| GameBanana · `gamebanana` | `search_games` → `search_mods` → `get_mod`；用 `list_files` / `get_file` 选文件、`get_requirements` 查前置，再用 `download_file` 下载选定文件。 |
| Nexus Mods · `nexusmods` | `find_games` 取得 `domainName`，再用 `find_mods` / `get_mod`；用 `list_files` / `get_file` 选文件、`get_file_requirements` 查前置，再按账户权限用 `download_file`。 |

- 官方 Mod 客栈的 `get_work_file_download` 每次成功签发都会计一次下载，链接有效 5 分钟。管理器客户端负责消费链接并保存归档，收到工具成功响应立即检查**受管归档回执**，不要重复签发。已完成就结束下载，不把文件再交浏览器；超时或结果不明时先查是否已落盘，不盲目重试。若运行时只返回链接，就不能算已下载；不要把签名地址、密钥或 `nxm://` 授权信息写进回复或转交浏览器。
- 其他来源若能保存选定文件，遵守外部工具审批，只保存到普通下载目录（不写游戏目录）。确认已完成、取得本机可读的绝对路径，核对文件名及可用的大小、校验值。仅有链接、远端服务路径或未完成的传输都不是本机文件。区分未连接、查询失败、权限不足与零结果。来源凭据缺失时，先尝试官方页面允许的下载方式；确实需要用户授权时，再说明缺少哪项权限。

## 仅在来源无法取得文件时用 Playwright MCP

- Mod 客栈工具明确报告已签发、但本机 DNS 解析失败且附件未保存时，不再调用 `get_work_file_download`，也不先用 `ask_user`。若 Playwright MCP 已连接，按其原有审批流程，直接打开已核实的 Mod 客栈官方作品页或文件页，从正常下载入口取得同一附件。不要把被下载器拦截的签名 URL（含查询参数）转发给浏览器或其他工具。若尚未核实页面，先通过官方查询核对，不猜页面或 CDN 地址。
- 账号或文件权限不足、审批被拒绝/取消/未获批准、下载地址被判不安全，都不是 DNS 故障，不得当作上述恢复的理由或通过浏览器绕过。普通网络错误、超时或结果不明不表示已确认 DNS 故障；先核实结果，再按实际失败原因处理。
- 没有来源下载能力、只有链接或已确认本次未取得文件时，才用**已连接**的 Playwright MCP。按实际工具 schema（如 `browser_navigate`、`browser_click`）打开已核实的官方项目/文件页，使用同一文件的正常下载入口，也可使用已核实且可公开访问的该文件下载地址。不要要求用户先找 ZIP 或直链，不猜 CDN 地址，不用 `download_web_mod`、脚本或其他工具旁路。未经允许不换平台或文件。
- 观察真实的浏览器下载事件并等待保存完成（如 `Downloaded file … to …`），取得可由本机读取的绝对路径；若只返回相对路径，基于已知的浏览器输出目录解析，用 `read_dir` 核实。页面打开、按钮被点击、截图、链接或下载开始都不是完成证据；无法验证本机文件就如实报告，不反复点击。若 Playwright 未连接、缺少下载/保存能力，或需要登录、验证码、会员/作者授权，说明具体限制、给出官方页面，让用户连接工具或完成正常授权；不得绕过网站权限或换工具避开已拒绝、取消或未获批准的操作。

## 归入对应游戏的 Mods 库

已登记游戏沿用当前游戏或明确指定 `gameId`；只知道游戏名称时传 `gameName`，允许先下载、以后再登记安装目录。来源工具若能写入本机，在调用前用 `list_mod_receipt_folders` 取得管理器的 `incoming` 或原厂 Playwright 输出目录。在真实下载完成后用 `file_received_mod` 传精确文件名及游戏归属，核对入库回执。来源只支持其他外部路径时，由用户在游戏详情的桌面文件选择器审核导入。

原生或 Mod 客栈下载已返回 `artifactId` 和游戏分类库路径时，直接报告原包位置。传输成功且已有下载回执但归类失败时，用 `retry_mod_library_download` 传原回执 ID 和同一游戏重试入库。该工具使用已保存的原包完成入库，无需重复签发下载票据。只要求下载时到入库为止。用户同时要求安装时读取 `system-mod-install`，使用同一条目和已登记游戏。
