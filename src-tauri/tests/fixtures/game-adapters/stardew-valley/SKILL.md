---
name: stardew-valley
description: "为 Stardew Valley 的 SMAPI 模组和内容包规划目录部署、依赖核对、日志诊断及保留存档的卸载。"
---

# Stardew Valley 适配指导

- 选择包含 `Stardew Valley.dll` 的 SMAPI 实际游戏目录；macOS 安装可能需要进入 `Contents/MacOS`。本包使用默认 `Mods`，自定义 `--mods-path` 需要另登记和适配。
- 每个模组保留自己的目录和 `manifest.json`；读取 UniqueID、版本、依赖和 EntryDll 或 ContentPackFor。内容包依赖对应框架，不可仅因没有 DLL 就判为无效。
- 由 SMAPI 按清单依赖安排加载顺序，不通过文件夹名称猜顺序。本包不安装或执行 SMAPI，不运行任何模组 DLL。
- Windows Steam/GOG 的数据目录为 `%APPDATA%/StardewValley`；macOS/Linux 为 `~/.config/StardewValley`。先登记 `user-data`，其下 `Saves` 为存档，`ErrorLogs/SMAPI-latest.txt` 与可选 `SMAPI-crash.txt` 用于诊断。
- 旧 XNB 内容替换只按人工审核的逐文件计划处理，备份原文件；优先使用现代内容包。移除新增物品、地点或地图的模组可能影响存档。

## 使用边界

先读取同目录 `adapter.json`，核对实际游戏版本、平台、目录标记、模组说明和依赖，再形成逐文件计划。`version` 是适配包版本，不是游戏兼容版本。`platforms` 仅表示本包覆盖的原生目录布局；不要把 Windows 布局套用到 macOS、Proton 或 Wine。

本技能与 adapter 都是静态声明，不含可执行脚本，也不授予执行安装器、启动游戏、运行 DLL/JAR、下载依赖或写入目录的权限。`discoveryHints` 仅供展示和人工定位，不得展开环境变量后直接写入；`registered_user` 根必须由宿主登记并加入文件引擎的管理根集合。相对路径禁止穿越、符号链接逃逸及跨根回退。

归档先由安全解包层检查；扩展名和布局提示不能代替内容检查。`manual` 规则需要审核具体文件计划，不能把整个归档覆盖到游戏根。关闭游戏后按事务记录操作，备份将被覆盖的文件，卸载只处理该安装的记录与备份，保留存档和共享依赖。

## 一手资料

- [Steam 游戏与 App ID](https://store.steampowered.com/app/413150/)
- [SMAPI 作者：模组打包及默认 Mods 路径](https://github.com/Pathoschild/SMAPI/blob/develop/docs/technical/mod-package.md)
- [SMAPI 作者：路径常量与存档](https://github.com/Pathoschild/SMAPI/blob/develop/src/SMAPI/Constants.cs)
- [SMAPI 日志位置](https://smapi.io/log)
- [SMAPI 作者：依赖排序配置](https://github.com/Pathoschild/SMAPI/blob/develop/src/SMAPI/SMAPI.config.json)

目录与规则核对日期：2026-11-17。版本相关行为以具体游戏与加载器文档为准。
