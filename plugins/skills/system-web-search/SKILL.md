---
name: system-web-search
description: "用公开关键词查找游戏、模组和加载器资料，核对搜索证据与适用版本。"
---

# 搜索公开网页

1. 查找 Mod 时，用 `read_skill` 读取已启用的 `system-mod-download`，优先使用对应来源的 MCP，包括 Mod 客栈（`glossmod_http`），不因它不在管理器原生目录而跳过。没有可用来源结果，或需要补充官方文档、项目页、兼容性资料时，再用 `web_search`。
2. `web_search` 用应用内置浏览器打开 Bing（Windows 优先 Microsoft Edge），不需要搜索 API key，也不需要连接 Playwright MCP。只发送公开的游戏名、Mod 名、版本号或加载器名作为 `query`，按需指定 `count`；不发送本机路径、私人日志、凭据或个人信息。
3. 记录页面地址、发布方和适用版本，优先采用平台官方或作者发布的资料。搜索摘要不证明兼容、可下载或已保存。查找未知游戏身份时，用 `preview_game_identity` 核对搜索词，再用 `search_game_identity` 取得候选；由用户确认身份及游戏目录登记。
4. 用户还要求下载时，按 `system-mod-download` 核对精确文件并优先用来源 MCP；只有无法取得该文件，才用已连接的 Playwright MCP 从已核实的官方或公开下载入口保存，并核实本机路径。内置搜索不能下载；不要让用户先找归档或直链，不自动安装。

内置浏览器无法启动或缺少组件时，说明需重启或修复应用；遇到网络、验证码或结果无法解析时，如实报告，建议检查网络或人工核对。不要把搜索失败归因于缺少 API key 或 Playwright MCP，也不要绕过网站权限或工具审批。
