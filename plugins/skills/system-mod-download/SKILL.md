---
name: system-mod-download
description: "从目录来源定位精确 Mod 版本并下载。处理手动下载、公开归档直链和后续安装预览。"
---

# 从来源下载 Mod

1. 确认目标游戏、版本、加载器及用户想下载的 Mod。调用 `search_mods` 时用来源原生游戏 ID，按所选来源的能力传入版本与加载器。把来源状态和可用性告诉用户，有多个可能的项目或版本时先核对选择。
2. 用 `list_mod_versions` 查询支持此功能的来源，或使用证据明确给出的版本 ID。把选定的 `source_id`、`project_id`、精确 `version_id` 和已知兼容条件传给 `resolve_mod`。遇到 `manual_download` 或来源缺少可下载文件时，说明原因并提供来源页面供用户处理。
3. 用户要求安装 Modrinth Mod 且已知 Minecraft 版本和加载器时，用 `resolve_modrinth_dependency_graph` 核对选定版本的必需依赖，并用 `check_modrinth_dependencies` 核对目标游戏的已登记安装记录。把待选、冲突、截断及未验证项目明确列出，依赖下载和安装分别走相应流程。
4. 用本次解析结果的 `download.resolution_id` 调用 `download_mod`，遵守审批策略。该 ID 在本会话中一次有效，失效或重试时重新解析同一精确版本。记录返回的文件名、归档路径和校验信息。下载到管理器后，文件仍需单独审核才可部署。
5. 目录来源无法下载且用户要求下载时，可以用 `download_web_mod` 提交用户提供或证据中明确出现的公开 HTTPS ZIP、JAR、7z 或 tar.gz 直链。这个工具始终需要单独审批。项目网页、猜测的地址、登录链接和带查询参数或签名的链接不适用。网页下载没有目录版本来源绑定。
6. 用户要求安装时，用已登记游戏和实际下载归档调用 `plan_install`，核对适配规则和文件影响，再按审批策略调用 `install_mod`。如果用户只要求下载，在确认归档结果后结束操作。
