---
name: minecraft-java
description: "为 Minecraft Java 独立实例规划 JAR 模组、资源包、世界数据包和整合包导入，核对加载器与版本并保留世界存档。"
---

# Minecraft Java Edition 适配指导

- 选择实际实例的游戏目录；官方启动器可为不同安装设置不同目录，第三方启动器也有独立实例。候选目录中的 `options.txt`、`versions`、`saves` 只是定位信号，应另行确认实例身份和加载器。
- 将 Fabric/Forge/NeoForge 对应版本的 `.jar` 作为完整文件放入 `mods`，不要解压 JAR，也不要执行 JAR 内代码来探测兼容性。依赖由元数据和加载器决定，没有通用手工加载顺序文件。
- 资源包留在 `resourcepacks`，数据包留在特定世界的 `datapacks`；后者必须先把该世界目录登记到 `world-datapacks` 映射。两者都不是加载器模组，不要放进 `mods`。
- `.mrpack` 或 ZIP 整合包需要清单与来源策略审核，不能直接解压覆盖实例。定位故障时读取实例 `logs/latest.log` 和可选 `crash-reports`；保留 `saves`。

## 使用边界

先读取同目录 `adapter.json`，核对实际游戏版本、平台、目录标记、模组说明和依赖，再形成逐文件计划。`version` 是适配包版本，不是游戏兼容版本。`platforms` 仅表示本包覆盖的原生目录布局；不要把 Windows 布局套用到 macOS、Proton 或 Wine。

本技能与 adapter 都是静态声明，不含可执行脚本，也不授予执行安装器、启动游戏、运行 DLL/JAR、下载依赖或写入目录的权限。`discoveryHints` 仅供展示和人工定位，不得展开环境变量后直接写入；`registered_user` 根必须由宿主登记并加入文件引擎的管理根集合。相对路径禁止穿越、符号链接逃逸及跨根回退。

归档先由安全解包层检查；扩展名和布局提示不能代替内容检查。`manual` 规则需要审核具体文件计划，不能把整个归档覆盖到游戏根。关闭游戏后按事务记录操作，备份将被覆盖的文件，卸载只处理该安装的记录与备份，保留存档和共享依赖。

## 一手资料

- [Fabric：安装模组与各平台 mods 路径](https://docs.fabricmc.net/players/installing-mods)
- [Fabric：独立实例游戏目录](https://wiki.fabricmc.net/player:tutorials:install_mcl:windows)
- [Fabric：提交日志](https://docs.fabricmc.net/players/troubleshooting/uploading-logs)

目录与规则核对日期：2026-11-17。版本相关行为以具体游戏与加载器文档为准。
