---
name: gta5-enhanced
description: "为 GTA V Enhanced 故事模式核对版本、逐文件安装普通模组，并在能力验证通过时通过暂存任务管理 RPF。"
---

# Grand Theft Auto V Enhanced · Windows 故事模式

只绑定已登记、含真实 `GTA5_Enhanced.exe` 文件的目录（Steam 3240220）。`PlayGTAV.exe`、`update/` 和另一版的主程序不是标记。不要创建标记、修改启动参数或安全设置，不用于 GTA Online。2026-11-25 本机只读观察主程序 1.0.1158.16；本机 build 晚于核对的 Script Hook V 1158.13 和 SHVDN Enhanced 说明中的 1158.13。此证据不能证明当前 build 可加载；须先取得作者明确支持该 build 的依赖。Enhanced 的归档、模型与模板未经该工具实测，本包不开放 Enhanced RPF 写入。

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

- `rpf-gameconfig` → `gameconfig`：只接收已确认适配本版的具体 `gameconfig.xml`。目标是 **mods 下的 update.rpf 副本内** `common/data/gameconfig.xml`，不是一个可直拷的宿主子目录。样本：FULLYCRX 发布 `Version`，asset `478748278`，直接 XML，SHA-256 `2874c4446a2acf59bf56daec6f1d4c9dbcc91294987d4ac0c158b1150a9a221f`。作者称双版适用，但载荷注释为 3889，没有本机 Enhanced build 的归档读写证据：不准入。
- `rpf-dlc` → `dlc`：独立 `dlc.rpf` 须读其 `setup2.xml`，校验 `nameHash`。结果才可映射到 `mods/update/x64/dlcpacks/<名称>/dlc.rpf` 并在 `mods/update/update.rpf` 副本内维护 `common/data/dlclist.xml`。Weeny Issi Hardtop v3（站点文件 `204664`）作者要求选 Enhanced 的 `issi1`，且给出 dlcpacks/dlclist 目标；**未取得该发布归档内部清单**。不要凭页面或目录名认定源路径/packName，更不可选 Legacy 分支。
- `rpf-vehicle` → `vehicle`：历史映射是将经核对的 `.yft`/`.ytd` 贡献加入可信 `gmm` DLC 模板的 `x64/vehicles.rpf`。不是所有车型都能这样装：BMW M3 GTR 1.3（文件 `27765`）作者要求替换多个原生归档，不是本流程的散装 add-on 正样本；不写原版。尚无本版正样本据以推断自动映射。
- `rpf-ped` → `ped`：按实际模型选同名主体和配套 `.ydd/.yft/.yld/.ymt/.ytd`；Streamed 还要选择匹配的组件目录。共享 `gmm` 模板中的 `x64/peds.rpf` 及模型元数据必须一起核验。John Wick: Chapter 2（文件 `54979`）作者要求 AddonPeds；没有核实该归档内部路径，也不构成 gmm 或 Enhanced 兼容证明。未取得对应版、组件或模型零匹配均停止。

### 任务和卸载

1. 普通文件不请求此服务。需要 RPF 时先确认本版能力、指定 3DM 205014 发行的工具/DLL、键数据与模板已经核验，并能对该版暂存副本读写回读。Windows 服务的只读 `gta_rpf_status({})` 可在人工授权连接后查询；1.0.0 规定 Python 3.11+、Windows PowerShell 5.1、.NET Framework 4.8，单独的 205014 / 356110 原 ZIP 由用户提供。缺失则报告，不代为下载/安装。不得从 Mod 中加载同名 DLL；任何验证不通过，报告缺口，不退回普通映射。不开启工具或自动同意非沙箱 stdio；由用户在已安装适配包的工具设置中选择。只使用本游戏绑定包的 `rpf` 服务。
2. 安装先调用 native `prepare_gta_rpf_job({game_id,artifact_id,kind,options})`。`kind` 如上，`sourceDir` 可选、仅原包内相对目录。行人必须显式给 `modelName`、`sex`（`Male` / `Female`）和 `streamed` 布尔值；车辆/DLC 仅在已有 Menyoo 列表、模型匹配且用户选中时才用 `menyoo`，不创建前置。
3. 取返回的 `jobId` 调用此包 `gta_rpf_prepare({jobId})`；它只负责私有暂存，不写游戏、不登记安装。**DLC 未知 packName**：第一次 `options:{}`（需要时含 sourceDir 等）为探测；`status:'inspected'` 和 `packName` 不是成品。用同 game/artifact/kind 重做 native 准备，`options.inspectJobId` 取探测 jobId，保留原 sourceDir / menyoo，再用正式 jobId 调 MCP。明确核验过的 `options.packName` 可以直接准备，工具仍须核对 nameHash。
4. 仅正式任务成功时调用 `install_library_mod({request:{gameId,artifactId,mode:'gtaRpf',jobId}})`。由 native 重新核对游戏版、库原包、job 身份、目标白名单、基线和产物指纹；不信任 MCP 自报目标。以文件计划预览、审批、提交和已安装列表回读为准。游戏根 `update/update.rpf` 永远不写；磁盘产物不是游戏加载成功。
5. 卸载从已安装记录取 ID：`prepare_gta_rpf_job({game_id,install_id})` → 用本次 jobId 调同包 `gta_rpf_prepare({jobId})` → `uninstall_mod({game_id,install_id,rpf_job_id})`（rpf_job_id 就是本次 jobId）。从基线和其余 Mod 贡献重建共享归档，最后一份才恢复基线/撤销 AMM 首创副本。冲突、外部修改、版本不符、权限拒绝或回读失败均停止；不删除整个 mods/、加载器、Menyoo 列表、存档或玩家自留条目。
