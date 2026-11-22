---
name: system-mod-uninstall
description: "卸载有管理器安装记录的 Mod，逐文件核对当前归属，移走独占文件，按安装事务恢复原有文件或前一版本。"
---

# 卸载已登记的 Mod

1. 用户从游戏详情点选时沿用 `game_id` 和 `install_id`。聊天仅给出名称时，用 `list_installs` 在目标游戏中核对版本和启用状态。没有对应安装记录时说明原因，保留游戏文件。
2. 调用 `inspect_install({game_id, install_id})` 核对实际磁盘文件、安装清单、覆盖备份和其他安装的占用情况。按 `reconciliation.recommendedAction` 决定下一步：
   - `safe_uninstall`：用 `uninstall_mod({game_id, install_id})` 将本次安装独占且未改变的文件移出游戏目录。
   - `rollback_install` 或 `rollback_replacement`：核对当前文件和冲突；使用 `rollback_transaction`，同时传入本游戏、本安装和报告给出的 `transaction_id`。向用户说明回滚将恢复哪些原文件或前一版本。
   - 文件被改动、缺失、共享、部分恢复或有后续替换时，列出冲突路径和涉及的安装。若要动到另一个安装，先让用户决定。原包仍可在游戏分类库中找到。
3. 本地清单表达不清的特殊卸载步骤，可用来源 MCP 读取同版本的作者说明，缺少正文时用 Playwright 查已核实的项目页。作者介绍仅用于解释配置或前置组件；只按已登记清单操作。用户新生成的配置、存档和其他 Mod 的文件应保留。审批被拒或事务失败时报告实际结果。
4. 提交后用 `list_installs` 核对卸载或回滚状态，报告移除文件或恢复的版本。需要重装时沿用原库条目。

用户只要求卸载时，保留安装记录和库条目，报告已卸载。用户明确要求“删除”或“从管理列表移除”时，先完成上述卸载或回滚，再对同一 `game_id` 和 `install_id` 调用 `forget_uninstalled_mod`。若卸载被拒、失败或发生冲突，保留列表记录。
