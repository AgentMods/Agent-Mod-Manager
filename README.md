# Agent Mod Manager

一款由AI Agents 驱动的智能 Mod 管理器

## 切换 Agent 工作区

在 Agent 对话侧栏底部点击 **工作区** 下的游戏名称（窄屏点击对话标题下方的游戏名称）。勾选一款或多款游戏，或点击 **全选** / **清空**，再点 **应用**。全选会自动包含以后登记的游戏；清空只解除当前对话与游戏的关联，仍可正常聊天。

工作区按对话保存，更改后从下一轮生效。运行中的任务需先完成或停止，归档对话需先恢复。切换工作区不会更改文件权限或审批设置。

## 安装和移除插件

在 **插件中心 → 云端插件** 浏览或搜索插件，点击卡片上的 **安装** 即可，无需创建草稿。**查看详情** 可选，用于查看发布内容和声明的工具。已经安装同标识插件时不会覆盖本地版本。

在 **插件中心 → 已安装** 点击对应行的 **移除**。如果插件已绑定游戏，请先从 **游戏库 → 更多操作 → 选择适配包** 解除绑定。移除插件不会删除游戏或已安装的 Mod。

## 使用 7-Zip 压缩和解压

在桌面 Agent 对话里可以直接说：“把 `D:\Mods\材质包` 压缩成 `D:\备份\材质包.7z`”，或者“把 `D:\备份\材质包.7z` 解压到新目录 `D:\还原`”。Agent 会先展示文件计划，再按当前权限设置执行；自动批准模式下，这两种操作仍需确认。

创建归档仍只支持 `.zip` 和 `.7z`，会保留源文件夹名称和空目录。解压支持 ZIP、7z、TAR、TAR.GZ、TAR.BZ2 和 RAR。RAR4/5 仅支持读取非加密的单卷归档，可从本地 Mod 文件选择器导入，也可通过支持归档格式检查的下载入口接收；不支持创建 RAR、加密 RAR 或分卷 RAR。解压不覆盖已有文件或目录，也不删除源文件；若要撤销，可在事务历史中回滚。

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
