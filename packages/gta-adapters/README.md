# GTA V 双版本外部适配包 · 本机交付

每个 `gta5-*` 目录恰好两个文本文件；不属于 AMM 内置包。两个包声明同一份离线、无 Python 运行依赖的本机 wheel，以 `rpf` 为包内 ID。AMM 生成不同的 `plugin_gta5-legacy_rpf` / `plugin_gta5-enhanced_rpf` 注册 ID；安装包不是安装、启动或同意运行 MCP。不要发布本机渲染后的清单或上传 wheel。

## 1. 检查源包，生成本机输入

PowerShell，在仓库根执行（Python 3.11+）：

```powershell
Set-Location -LiteralPath 'E:\GitHub\GlossMod\Agent-Mod-Manager'
$helper = (Resolve-Path -LiteralPath 'packages/gta-adapters/local_package.py').Path
py -3 -B $helper check
py -3 -B -m unittest discover -s packages/gta-adapters -p test_local_package.py -v
py -3 -B $helper hashes
```

`check` 只核对源码约束，不是 AMM 校验。源码 `--from` 指仓库相对的 `./tools/gta-rpf-mcp/dist/gta_rpf_mcp-1.0.0-py3-none-any.whl`，**勿直接提交源码 JSON**。`hashes` / `emit` 要求该文件已存在且未经链接重定向；检查 wheel 的名称、版本、入口和依赖后，渲染为本机绝对 `file:` URI，SHA-256 随声明记录。`uvx --offline --no-index` 不会回退到 PyPI。工具不调用 uvx 或加载 wheel；URI 中的摘要用于标识本次审查，不替代受控目录和重新核对。仓库迁移、wheel 重建或来源不可信时停止，重新审查、渲染、验证和部署，不在原路径偷换内容。对照打包方给出的最终 wheel SHA-256 后才继续。

为避免终端编码、换行或 BOM 改变审核文本，可把一个字段的原文精确复制到剪贴板：

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
3. 在**草稿**列表找到刚提交的那一行（检查 slug、草稿路径/ID、时间与写入事务），点**验证**（`validate_plugin`）。必须为验证通过、版本 `1.0.0`。点**安装预览**（`install_plugin`），核对指纹、目标及覆盖情况后，在桌面确认。目标应仅是 AMM 数据根的 `plugins/games/<slug>/{adapter.json,SKILL.md}`。失败时报告该阶段，不把草稿或预览当作已安装。若已有同 slug、同内容的已验证包，可跳过重新写入，直接回读；遇到不同内容先审查覆盖。
4. **已安装**确认 slug、版本、已验证，再到游戏库目标卡 → **选择适配包** → 指定对应包 → **应用适配包**（桌面 `set_game_adapter` 参数 `id`、`adapterSlug`）。以该登记的根为准，Legacy 只匹配 `GTA5.exe`，Enhanced 只匹配 `GTA5_Enhanced.exe`。不要新登记一个类似目录绕过缺失标记、覆盖另一绑定或改名；缺标记时记录“已安装，待绑定”。对 `gta5-enhanced` 重复 2–4。
5. 刷新游戏库和插件中心，分别核对绑定的 slug 和版本；设置 → 工具核对两个服务均未启动。**不要**点击连接、授予 stdio 启动许可、操作 Mod 或进入发布流程。若使用已由用户开放、且确认归属该 AMM WebView 的回环 CDP，可从同一桌面的只读 IPC `list_plugins`、`list_games`、`get_game_adapter({gameId})`、`mcp_list` 和 `mcp_connections` 交叉核对；不要探测其他应用端口或让独立辅助进程写库。

可追加只读比对（数据路径取自该桌面的存储设置，不要猜测默认目录）：

```powershell
py -3 -B $helper verify-installed 'E:\AMM\Agent Mod Manager' gta5-legacy
py -3 -B $helper verify-installed 'E:\AMM\Agent Mod Manager' gta5-enhanced
```

文件哈希一致只证明已部署的字节，**不证明已注册、已绑定、已授权或游戏内可用**。逐版记录：草稿验证、安装、绑定游戏及版本、MCP 连接、RPF 本版回读与游戏内验证。代表项目、确切归档成员、依赖和证据空白见各包 SKILL；不把历史 GMM 代码或作者宣传当作运行证据。
