---
name: cyberpunk-2077
description: "为 Cyberpunk 2077 Windows 的传统 archive、REDmod、redscript 与 RED4ext 包区分安装根、清单和依赖并规划安全卸载。"
---

# Cyberpunk 2077 适配指导

- 以 `bin/x64/Cyberpunk2077.exe` 确认 Windows 安装根；本包不声称支持 macOS 或自动映射 Proton。
- 传统 `.archive` 位于 `archive/pc/mod`；保留可选框架伴随数据，不能覆盖原版 `archive/pc/content`。REDmod 是另一种布局：`mods/ModName/info.json`，保留模组目录和清单。
- `.reds` 脚本通常位于 `r6/scripts`，RED4ext 插件位于 `red4ext/plugins`，插件日志在 `red4ext/logs`。CET、加载器或横跨多个目录的包必须分别审核文件落点，不能把所有载荷塞入 archive。
- REDmod 的部署和启用依赖官方工具/启动器。读取 `info.json` 不表示可以运行 `redMod.exe`；传统归档、脚本和框架也不能用一份通用顺序列表处理。
- 存档在 Windows Saved Games 已知文件夹下 `CD Projekt Red/Cyberpunk 2077`；先登记 `saves` 根。卸载仅恢复事务记录的文件并保留共享框架；不要执行官方“全新安装”流程中的广泛删除步骤来卸载单个模组。

## 使用边界

先读取同目录 `adapter.json`，核对实际游戏版本、平台、目录标记、模组说明和依赖，再形成逐文件计划。`version` 是适配包版本，不是游戏兼容版本。`platforms` 仅表示本包覆盖的原生目录布局；不要把 Windows 布局套用到 macOS、Proton 或 Wine。

本技能与 adapter 都是静态声明，不含可执行脚本，也不授予执行安装器、启动游戏、运行 DLL/JAR、下载依赖或写入目录的权限。`discoveryHints` 仅供展示和人工定位，不得展开环境变量后直接写入；`registered_user` 根必须由宿主登记并加入文件引擎的管理根集合。相对路径禁止穿越、符号链接逃逸及跨根回退。

归档先由安全解包层检查；扩展名和布局提示不能代替内容检查。`manual` 规则需要审核具体文件计划，不能把整个归档覆盖到游戏根。关闭游戏后按事务记录操作，备份将被覆盖的文件，卸载只处理该安装的记录与备份，保留存档和共享依赖。

## 一手资料

- [Steam 游戏与 App ID](https://store.steampowered.com/app/1091500/)
- [CD PROJEKT RED：REDmod 安装与 info.json](https://www.cyberpunk.net/en/modding-support)
- [RED4ext 作者：插件目录](https://docs.red4ext.com/getting-started/installing-a-plugin)
- [RED4ext 作者：脚本与插件示例](https://docs.red4ext.com/mod-developers/creating-a-plugin-with-redlib)
- [CD PROJEKT RED：存档路径](https://support.cdprojektred.com/en/cyberpunk%20/pc/sp-technical/issue/2233/how-do-i-perform-a-clean-install-of-the-game)

目录与规则核对日期：2026-11-17。版本相关行为以具体游戏与加载器文档为准。
