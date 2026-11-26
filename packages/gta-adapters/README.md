# GTA V 双版本外部适配包 · 本机交付

两个外部适配包均为 `1.1.0`，每个 `gta5-*` 目录各有两个文本文件。清单以 `rpf` 为包内 ID，使用无凭据命令 `uvx gta-rpf-mcp@latest`。AMM 分别注册 `plugin_gta5-legacy_rpf` 和 `plugin_gta5-enhanced_rpf`。适配包启用且验证通过，并绑定到至少一个已登记游戏后，AMM 会自动启用并连接其 MCP；这两个 Windows GTA 配置属于已识别的无凭据配置，可直接连接。配置被改动时按新的完整配置重新判断授权。服务启动时，`uvx` 会刷新版本缓存，并从官方 PyPI（`https://pypi.org/simple`）解析最新可用版本；AMM 为这两个精确识别的声明覆盖通用清华镜像设置，因为该镜像尚无此包。AMM 同时向 MCP 子进程传入自身的 `LOCALAPPDATA`，让工具与 AMM 共用 `%LOCALAPPDATA%\GlossMod\AMM\gta-rpf-jobs` 工作区。包信息见 [PyPI](https://pypi.org/project/gta-rpf-mcp/)。

## 1. 检查源包，准备草稿输入

PowerShell，在仓库根执行（Python 3.11+）：

```powershell
Set-Location -LiteralPath 'E:\GitHub\GlossMod\Agent-Mod-Manager'
$helper = (Resolve-Path -LiteralPath 'packages/gta-adapters/local_package.py').Path
py -3 -B $helper check
py -3 -B -m unittest discover -s packages/gta-adapters -p test_local_package.py -v
py -3 -B $helper hashes
```

`check` 核对源包约束，AMM 草稿验证在桌面进行。`emit` 输出对应源文件的原始 UTF-8 字节，`hashes` 列出这两份文件的 SHA-256。三个命令均在本地运行，无需准备 wheel 或启动 MCP。`gta-rpf-mcp` 1.1.0 起自带 Python RPF7 后端和 GMM 模板。在工具状态中查看 `legacyJobs` 或 `enhancedJobs` 可确认具体操作。服务 1.3.0 在设置里列出 15 个工具。安装时使用 `prepare_gta_rpf_job` 返回的 `mcpTool` 和 `jobId`。单独检查游戏中的归档时，调用 `prepare_gta_rpf_archive_job` 并传入游戏 ID 和外层归档的游戏相对路径。将返回的 `jobId`、`archive` 传给 `gta_rpf_read_entry`，在 `entry` 中指定内部路径。服务分别提供列目录、读文件、新增、替换、删除和创建归档工具。

清单和 SKILL 在 Git 中均使用 LF。桌面草稿输入框会将 CRLF 转为 LF，源包检查会拒绝 CRLF，避免输入文本与源包的字节指纹不一致。以下方式可把原始 UTF-8 字段按 Base64 传给 PowerShell，再复制到剪贴板。

```powershell
function Copy-GtaDraftField($slug, $field) {
  $b64 = py -3 -B $helper emit $slug $field --base64
  if ($LASTEXITCODE -ne 0 -or !$b64) { throw '未得到已验证的本地字段，请勿继续。' }
  Set-Clipboard -Value ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($b64)))
}
# 在下一步各自的输入框中粘贴；每次只调用其中一行。
Copy-GtaDraftField 'gta5-legacy' 'adapter.json'
# Copy-GtaDraftField 'gta5-legacy' 'SKILL.md'
```

## 2. 只在现有 AMM 桌面保存、验证、安装与绑定

1. 先核对是预期的 AMM 实例、已登记游戏、存储根及已有的同 slug 包。没有活动桌面、出现恢复错误或另有人正在写入时停止，不开第二个 writer，不操作 SQLite。浏览器中的 Vite 页面不能代替 Tauri。原生桌面未开放 CDP 时直接用下列 UI，无需改启动参数或调试设置。
2. **插件中心 → 新建草稿**。Slug 填 `gta5-legacy`，分别粘贴 `emit` 得到的两份原文。**预览保存草稿**（`write_plugin_draft`）只生成计划，检查同一 slug 的两份草稿文件，再在桌面确认这份计划。不能直接调用 `apply_plan` 代替用户审核。
3. 在**草稿**列表找到刚提交的那一行（检查 slug、草稿路径/ID、时间与写入事务），点**验证**（`validate_plugin`）。必须为验证通过、版本 `1.1.0`。点**安装预览**（`install_plugin`），核对指纹、目标及覆盖情况后，在桌面确认。目标应仅是 AMM 数据根的 `plugins/games/<slug>/{adapter.json,SKILL.md}`。失败时报告该阶段，不把草稿或预览当作已安装。若已有同 slug、同内容的已验证包，可跳过重新写入，直接回读；遇到不同内容先审查覆盖。
4. **已安装**确认 slug、版本、已验证，再到游戏库目标卡 → **选择适配包** → 指定对应包 → **应用适配包**（桌面 `set_game_adapter` 参数 `id`、`adapterSlug`）。以该登记的根为准，Legacy 只匹配 `GTA5.exe`，Enhanced 只匹配 `GTA5_Enhanced.exe`。不要新登记一个类似目录绕过缺失标记、覆盖另一绑定或改名；缺标记时记录“已安装，待绑定”。对 `gta5-enhanced` 重复 2–4。
5. 刷新游戏库和插件中心，核对绑定 slug、包版本和设置 → 工具中的 MCP 状态。第 4 步完成最后绑定后，连接可能已自动开始或完成；这两个无凭据 Windows GTA 配置无需额外连接许可。其他配置若命中首次未知 stdio、含凭据、本机 HTTP，或连接非 AMM 固定公开地址的网络端点，会弹出针对完整配置的一次性授权提示。网络免授权仅适用于固定地址 `https://www.glossmod.com/api/v1/mcp` 或 `https://mod.3dmgame.com/mcp`，且配置中没有凭据引用或凭据 URL。手动停止优先于自动重连；最后解绑后撤销适配包自动启动。不要操作 Mod 或进入发布流程。若使用已由用户开放、且确认归属该 AMM WebView 的回环 CDP，可从同一桌面的只读 IPC `list_plugins`、`list_games`、`get_game_adapter({gameId})`、`mcp_list` 和 `mcp_connections` 交叉核对；不要探测其他应用端口或让独立辅助进程写库。

可追加只读比对（数据路径取自该桌面的存储设置，不要猜测默认目录）：

```powershell
py -3 -B $helper verify-installed 'E:\AMM\Agent Mod Manager' gta5-legacy
py -3 -B $helper verify-installed 'E:\AMM\Agent Mod Manager' gta5-enhanced
```

文件哈希一致只证明已部署的字节，**不证明已注册、已绑定、已连接或游戏内可用**。逐版记录：草稿验证、安装、绑定游戏及版本、MCP 连接、RPF 本版回读与游戏内验证。代表项目、确切归档成员、依赖和证据空白见各包 SKILL；不把历史 GMM 代码或作者宣传当作运行证据。
