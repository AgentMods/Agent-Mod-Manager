---
name: skyrim-se
description: "为 The Elder Scrolls V: Skyrim Special Edition 的 Data 模组、SKSE 插件和 FOMOD 包规划安装及卸载，核对运行时、依赖、用户配置目录和存档。"
---

# The Elder Scrolls V: Skyrim Special Edition 适配指导

- 用 `SkyrimSE.exe` 与 `Data/Skyrim.esm` 确认安装目录。适配包覆盖 Windows 桌面版本；其他发行版路径、VR、Wine/Proton 前缀要单独确认。
- 普通资源和 ESP/ESM/ESL 放在 `Data`，保留 BSA 与对应插件的文件名关系；归档含顶层 `Data` 时仅剥离这一层。FOMOD 存在选项与条件，必须先解析，不能将所有分支一起复制。
- SKSE 加载器文件应在游戏 EXE 旁，其脚本在 `Data/Scripts`；原生扩展插件通常在 `Data/SKSE/Plugins`，必须核对精确游戏运行时、发行版与扩展器版本，不能仅按“最新版”推断兼容。不要捆绑或执行扩展器安装器。
- 插件列表在 `%LOCALAPPDATA%/Skyrim Special Edition/Plugins.txt` 对应用户根；存档在 Documents 已知文件夹的 `My Games/Skyrim Special Edition/Saves`，SKSE 日志在该配置根的 `SKSE`。先分别登记 `local-profile`、`documents-profile`，Documents 重定向和非默认发行版目录名需现场确认。
- ESP/ESM/ESL 的依赖和固定排序与散文件覆盖优先级是两套机制；本包不运行 LOOT 或自行伪造完整排序。保留插件列表的启用标记、原配置和存档，移除内容模组后不能保证旧存档可继续使用。

## 使用边界

先读取同目录 `adapter.json`，核对实际游戏版本、平台、目录标记、模组说明和依赖，再形成逐文件计划。`version` 是适配包版本，不是游戏兼容版本。`platforms` 仅表示本包覆盖的原生目录布局；不要把 Windows 布局套用到 macOS、Proton 或 Wine。

本技能与 adapter 都是静态声明，不含可执行脚本，也不授予执行安装器、启动游戏、运行 DLL/JAR、下载依赖或写入目录的权限。`discoveryHints` 仅供展示和人工定位，不得展开环境变量后直接写入；`registered_user` 根必须由宿主登记并加入文件引擎的管理根集合。相对路径禁止穿越、符号链接逃逸及跨根回退。

归档先由安全解包层检查；扩展名和布局提示不能代替内容检查。`manual` 规则需要审核具体文件计划，不能把整个归档覆盖到游戏根。关闭游戏后按事务记录操作，备份将被覆盖的文件，卸载只处理该安装的记录与备份，保留存档和共享依赖。

## 一手资料

- [Steam 游戏与 App ID](https://store.steampowered.com/app/489830/)
- [SKSE 作者安装说明](https://github.com/ianpatt/skse64/blob/master/skse64_readme.txt)
- [libloadorder：插件位置与固定顺序](https://github.com/Ortham/libloadorder/blob/master/src/game_settings.rs)
- [LOOT：依赖与排序规则](https://github.com/loot/loot/blob/master/docs/app/sorting.rst)

目录与规则核对日期：2026-11-17。版本相关行为以具体游戏与加载器文档为准。
