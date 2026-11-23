# Agent Mod Manager

一款由AI Agents 驱动的智能 Mod 管理器

## 切换 Agent 工作区

在 Agent 对话侧栏底部点击 **工作区** 下的游戏名称（窄屏点击对话标题下方的游戏名称）。勾选一款或多款游戏，或点击 **全选** / **清空**，再点 **应用**。全选会自动包含以后登记的游戏；清空只解除当前对话与游戏的关联，仍可正常聊天。

工作区按对话保存，更改后从下一轮生效。运行中的任务需先完成或停止，归档对话需先恢复。切换工作区不会更改文件权限或审批设置。

## 用 Agent 管理游戏库

可以在对话里直接说“把这个游戏目录添加到游戏库”“修改游戏名称和启动程序”“启动这个游戏”或“为这个游戏绑定已安装的适配包”。说“移除当前工作区的全部游戏”时，Agent 会列出已登记游戏，按 ID 逐个移除。移除只清理管理器登记和关联，磁盘上的游戏和存档会保留。仍有已安装 Mod 或待处理计划时，管理器会说明阻止移除的原因。

启用本地 MCP 服务后，已认证客户端也可以调用 `list_games`、`register_game`、`update_game`、`remove_game`、`launch_game` 和 `set_game_adapter`。添加时传入游戏目录的绝对路径，操作已有游戏时传入 `game_id`。桌面对话使用本轮权限，本地 MCP 使用宿主全局权限。查询可直接执行，其余五项在“逐次审批”和“游戏内自动批准”下都会展示审批，在“完全信任”下直接执行。

## 更新桌面应用

签名发行版会在启动时检查 GitHub Releases，发现新版本后在后台下载。也可以打开 **设置 → 外观与存储 → 应用更新**，点击 **检查更新**。显示「已下载」后，正常退出程序即可安装，下次打开时使用新版本。下载过程中退出，下次启动会重新检查。系统安装器可能要求管理员授权。

### 维护者发布流程

1. 首次配置时，运行 `yarn tauri signer generate -w <仓库外的私钥文件路径>` 并备份密钥。把 `.pub` 文件内容存入 GitHub Actions 仓库变量 `TAURI_UPDATER_PUBLIC_KEY`，私钥文件内容和密码分别存入 Secrets `TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。后续发行沿用同一对密钥。
2. 默认更新源是当前仓库的公开 Releases。如果源码仓库为私有，将变量 `AMM_RELEASE_REPOSITORY` 设为公开分发仓库的 `owner/repo`，并设置对该仓库有 Contents 写权限的 Secret `AMM_RELEASE_TOKEN`。客户端可匿名下载。
3. 同步 `src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`、`package.json` 及 Cargo 锁文件中的应用版本，推送对应的稳定标签（例如版本 `1.2.3` 对应 `v1.2.3`）。**Build signed desktop release** 会构建 Windows x64（NSIS、MSI）、Linux x64（AppImage、Debian）、macOS（Apple Silicon、Intel）安装包，校验签名并创建带 `latest.json` 的 Release 草稿。
4. 审核安装包后在 GitHub 发布草稿，并将其设为 **Latest**。更新器读取 `/releases/latest/download/latest.json`。此前未集成更新器的用户需要手动安装一次签名发行版。

## 安装、更新和移除插件

在 **插件中心 → 云端插件** 浏览或搜索插件，点击卡片上的 **安装** 即可，无需创建草稿。**查看详情** 可选，用于查看发布内容和声明的工具。

每次启动桌面应用时，管理器会在后台检查已安装的云端适配包，发现新版后自动升级，保留游戏绑定和启用设置。如果云端暂时不可用，下次启动时会再检查。手动制作或修改的本地适配包由用户自行更新。

在 **插件中心 → 已安装** 点击对应行的 **移除**。如果插件已绑定游戏，请先从 **游戏库 → 更多操作 → 选择适配包** 解除绑定。移除插件不会删除游戏或已安装的 Mod。

## 使用 7-Zip 压缩和解压

在桌面 Agent 对话里可以直接说：“把 `D:\Mods\材质包` 压缩成 `D:\备份\材质包.7z`”，或者“把 `D:\备份\材质包.7z` 解压到新目录 `D:\还原`”。Agent 会先展示文件计划，再按当前权限设置执行；自动批准模式下，这两种操作仍需确认。

创建归档仍只支持 `.zip` 和 `.7z`，会保留源文件夹名称和空目录。解压支持 ZIP、7z、TAR、TAR.GZ、TAR.BZ2 和 RAR。RAR4/5 仅支持读取非加密的单卷归档，可从本地 Mod 文件选择器导入，也可通过支持归档格式检查的下载入口接收；不支持创建 RAR、加密 RAR 或分卷 RAR。解压不覆盖已有文件或目录，也不删除源文件；若要撤销，可在事务历史中回滚。

## 匿名使用统计

正式桌面版默认向自托管的 Aptabase 实例 `analytics.gloscai.com` 发送匿名事件。每次桌面进程成功初始化会记录一次 `app_started`。在 Aptabase 看板选中这个事件可以查看启动次数，再按操作系统、系统版本或应用版本查看启动分布。Mod 安装以文件事务成功提交为准，事件名为 `mod_installed`。

| 功能 | Aptabase 事件 | 计数时机 |
| --- | --- | --- |
| 游戏库 | `game_registered`、`game_launch_requested` | 完成登记，或将启动请求交给系统或 Steam |
| Mod 管理 | `mod_archive_import_committed`、`mod_installed`、`mod_tags_updated` | 提交桌面归档导入、提交一次托管安装，或保存 Mod 标签 |
| Agent 对话 | `agent_turn_accepted`、`agent_turn_restart_accepted` | 接受一轮新请求，或接受编辑、重试请求 |
| 插件与恢复 | `plugin_installed`、`trash_restored`、`transaction_rolled_back` | 完成手动插件安装、提交桌面回收站恢复，或完成桌面事务回滚 |

客户端发送固定事件名、发生时间、临时会话编号、操作系统名称和版本、应用版本，以及协议要求的上报客户端版本。会话编号仅在内存中保存，每次启动和跨 UTC 日期时更新。Aptabase 服务端可根据连接 IP 推断地区。离线或退出前未送达的事件可能漏计。

如需关闭统计，在启动应用前设置环境变量 `AMM_DISABLE_ANALYTICS=1`，然后完全退出并重新打开应用。开发构建、浏览器预览和项目的安装包冒烟测试均关闭上报。

## 桌面开发与打包：7-Zip

桌面版随应用分发官方 7-Zip CLI。Windows 使用 `7z.exe` 和同目录的 `7z.dll`；macOS 使用 `7zz`；Linux 使用静态版 `7zzs`（包内命名为 `7zz`）。安装后不需要另装 7-Zip 或联网下载，也不会从 `PATH` 选择其他版本。

`yarn tauri dev`、`yarn tauri build` 和 `yarn tauri bundle` 会在构建或打包前自动准备资源。构建机需要 Python 3.11+（也可将 `AMM_BUILD_PYTHON` 设为解释器的绝对路径）和首次下载时的网络连接。使用独立 Cargo 命令或 Rust 测试脚本时，请先准备资源：

```sh
yarn prepare:seven-zip
node scripts/test-rust.mjs --locked --lib
```

准备脚本按 `TAURI_ENV_TARGET_TRIPLE`（未指定时按本机）选择 Windows MSVC、macOS 或 Linux GNU 的 x64/ARM64 资源。锁定版本、下载及提取后文件的 SHA-256 见 [`scripts/seven-zip.lock.json`](scripts/seven-zip.lock.json)；下载摘要来自 [7-Zip 官方 GitHub Release API](https://api.github.com/repos/ip7z/7zip/releases/tags/26.06)。应用的 `seven-zip/` 资源目录内也包含上游许可、完整 LGPL 文本和对应源代码归档。可用以下命令离线检查文件并运行同架构的 CLI（将路径换为安装包的资源目录也可）：

```sh
node scripts/prepare-mcp-runtime.mjs --verify-seven-zip src-tauri/resources
```
