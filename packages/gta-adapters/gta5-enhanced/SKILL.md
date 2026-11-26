---
name: gta5-enhanced
description: "为 GTA5 增强版（Grand Theft Auto V Enhanced）故事模式核对版本、逐文件安装普通模组，并在能力验证通过时通过暂存任务管理 RPF。"
---

# GTA5 增强版（Grand Theft Auto V Enhanced）· Windows 故事模式

只绑定已登记、含真实 `GTA5_Enhanced.exe` 文件的目录（Steam 3240220；Steam 清单 `name` 与 `installdir` 均为 `Grand Theft Auto V Enhanced`）。`PlayGTAV.exe`、`update/` 和另一版的主程序不是标记。不要创建标记、修改启动参数或安全设置，不用于 GTA Online。2026-11-25 本机只读观察主程序 1.0.1158.16；本机 build 晚于核对的 Script Hook V 1158.13 和 SHVDN Enhanced 说明中的 1158.13。此证据不能证明当前 build 可加载；须先取得作者明确支持该 build 的依赖。本包支持暂存副本内的游戏配置替换与独立 DLC 登记。增强版散装模型与模板尚待验证。

## Steam 适配

- 识别：`steamAppId` 为 `3240220`，必需标记是 `GTA5_Enhanced.exe`；`GTA5_Enhanced_BE.exe`、`PlayGTAV.exe` 属 Steam/BattlEye 启动链，不是模组目标。同机 Legacy 目录的 `GTA5.exe` 与本包无关，两个安装目录不得混用。
- 启动与更新：按 Steam App ID 启动；不改 Steam 启动选项或启动参数，不接管 `PlayGTAV.exe`。Steam 自动更新会替换主程序、`update/update.rpf`、`update2.rpf` 与 `rpf.cache`，更新后须重新核对依赖与 build。
- Steam「验证文件完整性」可能还原被替换的文件或删除模组文件，只是修复手段，不等于卸载记录；不要用它代替本包的卸载流程。
- `BattlEye/` 属在线反作弊组件，不修改、不删除、不作为安装目标；本包只服务故事模式。

## 普通文件：不依赖 RPF MCP

先检查所选发布文件的包装层、作者指定的游戏版与 build、所需加载器、资源子目录和现有同名文件。`manual` 表示逐文件预览，不是全包复制许可。脚本、ASI 和加载器都属于可执行代码；只生成用户所选组件的安装计划，不运行样本或安装器。

| 类型 | 已核对的发布文件 → 游戏根相对位置 | 准入边界 |
| --- | --- | --- |
| `scripthookv` | Alexander Blade `ScriptHookV_3889.0_1158.13.zip`：`bin/ScriptHookV.dll` → `ScriptHookV.dll`。归档中另有 `bin/dinput8.dll`、`bin/xinput1_4.dll`、`bin/NativeTrainer.asi` 和 `bin/args.txt`。 | 先确定本版 build 及作者要求的 ASI loader；人工确认所需的同级文件，不能把 bin/ 整目录复制。Trainer 可选；args.txt、启动器或安全组件不在此计划。 |
| `asi` | GameplayFixesV 3.4，asset `626891563`，`GameplayFixesV-v3.4.zip`：`GameplayFixesV.asi` 与 `GameplayFixesV.ini` → 游戏根同名文件。 | 作者指明根目录而非 scripts，声明双版、依赖 Script Hook V，不代表所有 build。保留已有 ini 的设置；不要覆盖其他插件。 |
| `script` | LemonUI v2.2，asset `257210514`，`LemonUI.zip`：作者要求选框架；所选 `SHVDN3/LemonUI.SHVDN3.dll`、同目录 `.pdb`/`.xml` → `scripts/` 下同名文件。 | 这是 SHVDN3 依赖的布局样本，不足以证明 Enhanced 当前 build 脚本兼容。不要选 FiveM、RPH、SHVDNC 或开发包；若包本来含 `scripts/` 就只保留一层。脚本数据目录保留相对关系。 |
| `game-root` | SHVDN Enhanced `v1.1.0.6`，asset `478323680`，`ScriptHookVDotNetEnhanced-v1.1.0.6.zip`：`ScriptHookVDotNet.asi`、`ScriptHookVDotNet.ini`、`ScriptHookVDotNet2.dll`、`ScriptHookVDotNet3.dll`、`MinHook.x64.dll` → 游戏根同名文件。 | 包内 README 明确指出两版游戏根。仅在用户确认兼容版本与根目录组件后选择；同名共享文件先审查归属，`Docs/`、`Debug/`、README 不是运行载荷。不要因一个 DLL 把整包归入脚本。 |

以上 ZIP 已核对发行字节及内部路径，未安装或执行。两个构建选件同时出现时只取本版；目标不明、嵌套混合布局、未知 DLL/EXE、工具包不准入。OIV 交给用户用兼容的 OpenIV 人工审阅，不自动解包映射、运行或记为已安装。未核实存档、日志的通用目录，不声明额外根或删除存档。

## RPF：四种分类，不是四条直接复制规则

- `rpf-gameconfig` → `gameconfig`：只接收已确认适配本版的具体 `gameconfig.xml`。目标是 **mods 下的 update.rpf 副本内** `common/data/gameconfig.xml`，不是一个可直拷的宿主子目录。样本：FULLYCRX 发布 `Version`，asset `478748278`，直接 XML，SHA-256 `2874c4446a2acf59bf56daec6f1d4c9dbcc91294987d4ac0c158b1150a9a221f`。作者称双版适用，但载荷注释为 3889，请核对该文件本身是否适配当前 build。
- `rpf-dlc` → `dlc`：独立 `dlc.rpf` 须读其 `setup2.xml`，校验 `nameHash`。结果才可映射到 `mods/update/x64/dlcpacks/<名称>/dlc.rpf` 并在 `mods/update/update.rpf` 副本内维护 `common/data/dlclist.xml`。Weeny Issi Hardtop v3（站点文件 `204664`）作者要求选 Enhanced 的 `issi1`，且给出 dlcpacks/dlclist 目标；**未取得该发布归档内部清单**。不要凭页面或目录名认定源路径/packName，更不可选 Legacy 分支。
- `rpf-vehicle` → `vehicle`：历史映射是将经核对的 `.yft`/`.ytd` 贡献加入可信 `gmm` DLC 模板的 `x64/vehicles.rpf`。不是所有车型都能这样装：BMW M3 GTR 1.3（文件 `27765`）作者要求替换多个原生归档，不是本流程的散装 add-on 正样本；不写原版。尚无本版正样本据以推断自动映射。
- `rpf-ped` → `ped`：按实际模型选同名主体和配套 `.ydd/.yft/.yld/.ymt/.ytd`；Streamed 还要选择匹配的组件目录。共享 `gmm` 模板中的 `x64/peds.rpf` 及模型元数据必须一起核验。John Wick: Chapter 2（文件 `54979`）作者要求 AddonPeds；没有核实该归档内部路径，也不构成 gmm 或 Enhanced 兼容证明。未取得对应版、组件或模型零匹配均停止。

### 任务和卸载

1. 绑定游戏和已启用的适配包后，AMM 自动连接本包 `rpf` 服务。运行 RPF 任务前查询 `gta_rpf_status({})`，确认 `ready` 为真、`backend` 为 `python-rpf7`。`enhancedJobs` 目前包括 `inspectDlc`、`dlc` 和 `gameconfig`。散装模型与 Menyoo 暂停。 `uvx gta-rpf-mcp@latest` 提供 Python 归档后端、格式表和模板。若用户已手动停用服务，尊重该设置。
2. 安装先调用 native `prepare_gta_rpf_job({game_id,artifact_id,kind,options})`。`kind` 如上，`sourceDir` 可选、仅原包内相对目录。Enhanced 仅接受 `gameconfig` 和 `dlc`，`menyoo` 保持关闭。
3. 按 native 返回的完整 `mcpTool` 名称调用，只传 `{jobId}`。DLC 探测用 `gta_rpf_inspect_dlc`，正式 DLC 用 `gta_rpf_register_dlc`，gameconfig 用 `gta_rpf_replace_gameconfig`。服务未提供对应专用工具时，native 自动返回同包的 `gta_rpf_prepare`。MCP 只写入私有暂存目录。
   - **DLC 的 `packName` 未知时**，先省略 `options.packName` 做探测。收到 `status:'inspected'` 后，用相同的 `game_id`、`artifact_id` 和 `kind` 再准备正式任务，把探测的 `jobId` 传给 `options.inspectJobId`，保留原 `sourceDir` 选项。用正式任务返回的 `mcpTool` 和 `jobId` 继续。已核验的 `options.packName` 可直接用来准备正式任务，MCP 仍会核对 `nameHash`。
   - 要检查本次安装任务的归档时，调用 `gta_rpf_describe_job({jobId})` 分页查看 `inputs`，优先将对应行的 `archive` 对象传给列出或读取工具。服务仅返回 `path` 和 `archiveName` 时，分别用作 `archive.path` 和 `archive.name`。对现有游戏归档的独立检查与编辑见下节。
4. 仅正式任务成功时调用 `install_library_mod({request:{gameId,artifactId,mode:'gtaRpf',jobId}})`。由 native 重新核对游戏版、库原包、job 身份、目标白名单、基线和产物指纹；不信任 MCP 自报目标。以文件计划预览、审批、提交和已安装列表回读为准。游戏根 `update/update.rpf` 永远不写；磁盘产物不是游戏加载成功。
5. 卸载从已安装记录取 ID：`prepare_gta_rpf_job({game_id,install_id})` → 用本次 `jobId` 调返回的 `mcpTool`（同包 `gta_rpf_prepare`） → `uninstall_mod({game_id,install_id,rpf_job_id})`（rpf_job_id 就是本次 jobId）。从基线和其余 Mod 贡献重建共享归档，最后一份才恢复基线/撤销 AMM 首创副本。冲突、外部修改、版本不符、权限拒绝或回读失败均停止；不删除整个 mods/、加载器、Menyoo 列表、存档或玩家自留条目。

### 独立读取和编辑游戏归档

1. 确认当前工作区的 GTA 游戏和本包的 `rpf` 工具已连接。调用 native `prepare_gta_rpf_archive_job({game_id,archive_path,source_paths?})`。例如用户给出 `mods/update/update.rpf/common/data/dlclist.xml` 时，传 `archive_path: "mods/update/update.rpf"`，将 `common/data/dlclist.xml` 作为后续 MCP 调用的 `entry`。跨嵌套归档时，`entry` 包含内部归档名，例如 `x64/models.rpf/car.yft`。
2. 准备任务使用实际存在的游戏相对 `.rpf` 外层文件，保留磁盘上的文件名。指定的 `mods` 副本缺失时，报告该路径缺失并停在此处。读取、列出和删除可省略 `source_paths`。添加或替换时，`source_paths` 只接受用户明确指定或授权的同一游戏安装根下的相对普通文件。准备任务依据 AMM 的管理器写入审批策略生成私有快照。
3. 把返回的 `jobId` 和 `archive` 原样传给 `gta_rpf_list_entries` 或 `gta_rpf_read_entry`。也可用返回的完整 `mcpTool` 调用 `gta_rpf_describe_job({jobId})`，按 `nextOffset` 分页，获取 `inputs[].archive` 和载荷引用。读取 XML 可传 `encoding: "utf8"`，较大的条目按工具分页读取。
4. 添加调用 `gta_rpf_add_entry`（`{jobId,archive,entry,sourcePath}`），替换调用 `gta_rpf_write_entry`（`{jobId,archive,entry,sourcePath}`）。`sourcePath` 使用 native 返回的 `payloads[].sourcePath` 或任务清单中对应载荷的 `inputs[].path`。删除现有文件调用 `gta_rpf_delete_entry`（`{jobId,archive,entry}`）。按需使用 `gta_rpf_create_archive` 和载荷生成新归档。每次修改后使用返回的完整 `archive` 引用继续编辑或重新打开并读取验证，删除可列目录或确认目标条目缺失。
5. 独立任务的产物仅在私有暂存目录中，游戏文件保持原样。正式安装和卸载只能使用上节的安装或卸载任务 ID。
