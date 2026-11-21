---
name: baldurs-gate-3
description: "为 Baldur’s Gate 3 Windows 的玩家 PAK 模组核对 AppData 管理根、modsettings.lsx、依赖与存档，区分 Toolkit 创作目录。"
---

# Baldur's Gate 3 适配指导

- 通过 `bin/bg3.exe` 或 `bin/bg3_dx11.exe` 及 `Data` 辨认安装；这些标记只是候选信号。普通玩家 `.pak` 的目标是 `%LOCALAPPDATA%/Larian Studios/Baldur's Gate 3/Mods`，绝不能默认写入游戏根或 `Data/Mods`。
- 宿主必须先登记真实的 BG3 用户数据基目录，并将它加入文件引擎的管理根。`user-mods` 映射以该基目录为起点再追加 `Mods`；`profile` 映射以同一基目录追加 `PlayerProfiles/Public`。加载适配包、找到游戏或读取 discoveryHints 都不构成登记授权；缺少登记时拒绝规划用户目录写入，也不回退游戏根。
- ZIP 在暂存区安全检查后只选择所需 `.pak`，PAK 原样保留，不执行任何归档内代码。原生扩展器和松散覆盖归入手工计划；Toolkit 的游戏 `Data/Mods`、`Data/Public` 是创作结构，不是玩家 PAK 的安装位置。
- 默认配置 `PlayerProfiles/Public/modsettings.lsx` 涉及模组 UUID、版本及依赖；不能由 PAK 文件名虚构条目，不自动覆盖 LSX 或基础模块。优先让游戏内管理器处理启用。其他配置要明确选择；多人游玩需核对参与者模组版本与顺序。
- 默认剧情存档在该配置下 `Savegames/Story`。启动器诊断日志在 `%LOCALAPPDATA%/Larian Studios/Launcher/Cache`，需另登记 `launcher-cache`；查找 `steam_*_alteredFiles.txt` 或 GOG 的 `galaxy_*` 日志，通配符只用于列举后人工筛选，不能作为写入路径。
- 卸载只处理事务记载的 PAK 和备份。保留存档、配置与其他模组；官方支持中的广泛清理步骤不应自动应用到单模组卸载。

## 使用边界

先读取同目录 `adapter.json`，核对实际游戏版本、平台、目录标记、模组说明和依赖，再形成逐文件计划。`version` 是适配包版本，不是游戏兼容版本。`platforms` 仅表示本包覆盖的原生目录布局；不要把 Windows 布局套用到 macOS、Proton 或 Wine。

本技能与 adapter 都是静态声明，不含可执行脚本，也不授予执行安装器、启动游戏、运行 DLL/JAR、下载依赖或写入目录的权限。`discoveryHints` 仅供展示和人工定位，不得展开环境变量后直接写入；`registered_user` 根必须由宿主登记并加入文件引擎的管理根集合。相对路径禁止穿越、符号链接逃逸及跨根回退。

归档先由安全解包层检查；扩展名和布局提示不能代替内容检查。`manual` 规则需要审核具体文件计划，不能把整个归档覆盖到游戏根。关闭游戏后按事务记录操作，备份将被覆盖的文件，卸载只处理该安装的记录与备份，保留存档和共享依赖。

## 一手资料

- [Steam 游戏与 App ID](https://store.steampowered.com/app/1086940/)
- [Larian：手动安装 .pak 到用户 Mods 目录](https://baldursgate3.game/mods-how-to/)
- [Larian 支持：用户数据、modsettings 与启动器日志](https://larian.com/support/faqs/mod-issues_112)
- [Larian Toolkit：创作项目目录不是玩家安装目录](https://docs.baldursgate3.game/Getting_Started:_Creating_a_New_Mod)

目录与规则核对日期：2026-11-17。版本相关行为以具体游戏与加载器文档为准。
