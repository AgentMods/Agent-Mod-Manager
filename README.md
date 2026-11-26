# Agent Mod Manager


Agent Mod Manager（AMM）是一款由 AI Agent 驱动的智能游戏 Mod 管理器，内置了国内外7家Mod平台的资源，由AI根据你的需求自动查找并安装使用。支持一键创建游戏适配包，一键下载、安装、卸载、删除 任意Mod，自动重命名整理，

### 功能
- 自动扫描游戏库中支持的游戏
- 一键查找、下载、安装、卸载、删除 您的Mod
- 适配任意游戏的任意Mod
- 一键创建游戏适配包，无需任何技术门槛
- 国内外7家Mod平台资源，AI一键全部查找
- 自适应主题
- 自适应语言
- 简介的UI
- 免费开源
- 无毒无捆绑
- 功能全部免费开放

### 游览 Mod

在「设置 → 工具」连接需要的 Mod 来源，然后打开侧栏的「游览」。选好游戏后，游览页会自动从全部可用来源加载 Mod 卡片。要调整查询范围，展开「选择来源」，勾选需要的来源后点「应用」。来源筛选会重新查询目录，类型和标签筛选可继续加载更多匹配作品。

配置模型后，Agent 会按应用语言整理标题、简介和标签。点卡片的原名按钮可对照原标题，点封面或标题可在浏览器打开来源网页。点卡片上的「安装」即启动 Agent 安装任务。作品匹配多个游戏时，选定目标游戏即启动。安装在后台运行，卡片按钮会显示「正在安装」，确认安装成功后显示「已安装」。可从「安装任务」查看详情、回答版本问题，也可以在对应的 Agent 对话里继续安装或重试。安装沿用已保存的权限设置。选用「完全信任」时，下载和安装会自动执行。

### 用 Mod 启动器运行游戏

在游戏库中，点「启动游戏」右侧的 **⌄** 可选择本次的启动方式，不会改变主按钮原有的默认行为。菜单按本机安装情况显示可用项；例如在 Windows 上，可以用 **SMAPI** 启动《星露谷物语》，或用 **Mod Engine 2** 启动《艾尔登法环》。管理器不会自动安装这些工具。

如果菜单中没有你的启动器，选「设置 Mod 启动器」，为它命名，选择**已登记游戏目录内**的可执行程序并保存。启动参数可留空；需要时每行写一个参数（含空格的路径也写在一行，不加外层引号）。保存后在启动菜单中点击这个名称即可；「移除启动器」只清除配置，不删除文件。

例如，手动配置 **Mod Engine 2** 时，Windows 下请选择 `modengine2_launcher.exe`，不要选择 `.bat`。若使用启动器旁的标准配置 `config_eldenring.toml`，在「启动参数」中输入以下四行：

```text
-t
er
-c
config_eldenring.toml
```

### 应用更新

在「设置 → 外观与存储 → 应用更新」中，可以切换下载线路或手动点击「检查更新」。默认使用 `cdn.gh-proxy.org`，所选线路从下次检查起用于获取版本信息和下载安装包。签名版会在启动时静默检查并下载更新，下载完成后在正常退出时安装。

### Thunderstore MCP

在「设置 → 工具」启动 **Thunderstore MCP** 后，Agent 可按游戏社区查找模组、查看版本和依赖，并下载指定版本。无需 API Key；首次启动需要联网准备依赖。下载的 ZIP 进入管理器收件目录，不会自动安装。

### [支持的游戏](https://www.glossmod.com/manager-plugins)

### 支持的语言

- 简体中文
- 英语
- 日语
- 韩语
- 德语
- 法语
- 西班牙语
- 巴西葡萄牙语
- 俄语
- 意大利语

### 标星历史


<a href="https://star-history.dera.page/#GlossMod/Gloss-Mod-Manager&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://star-history.dera.page/svg?repos=GlossMod/Gloss-Mod-Manager&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://star-history.dera.page/svg?repos=GlossMod/Gloss-Mod-Manager&type=date&legend=top-left" />
   <img alt="Star History Chart" src="https://star-history.dera.page/svg?repos=GlossMod/Gloss-Mod-Manager&type=date&legend=top-left" />
 </picture>
</a>

