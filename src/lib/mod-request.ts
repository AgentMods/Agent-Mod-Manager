import type { BatchAction } from './mod-management';

export const batchLabels = { install: '批量安装', uninstall: '批量卸载', remove: '批量删除' };
const prefix = '我已在游戏详情中明确选择';
const targetLabel = '。目标（精确 ID）：';

export type ModRequestTarget = { gameId: string } & ({ artifactId: string } | { installId: string });
export type ModRequest = { kind: BatchAction; batch: boolean; gameId: string; ids: string[] };

function intentLabel(kind: BatchAction, count: number, batch: boolean) {
  if (batch) return `${batchLabels[kind]}以下 ${count} 个 Mod`;
  if (kind === 'install') return '为此游戏安装这一项 Mod';
  return `${kind === 'remove' ? '从管理列表删除' : '卸载并保留在管理中'}这一条安装记录`;
}

const sharedRemovalOrder = '批量卸载或删除遇到共享路径时，先处理本批中可回滚的记录。每成功一项，重新检查受影响的已选记录，再继续处理。';

function workflowText(kind: BatchAction, batch: boolean) {
  const installing = kind === 'install';
  const removing = kind === 'remove';
  const workflow = installing
    ? '请按系统安装 Skill，先用 match_mod_install 核对本地归档和适配规则。仅在本地证据不足时，先通过来源 MCP 查阅作者说明；若 MCP 无法提供足够说明，再用 Playwright 查阅对应发布页。只有本地证据和作者说明都不足以确定安装方式，或需要我决定可选偏好时，才向我提问。确定后用 install_library_mod 申请安装，不要替换成其他游戏或归档。'
    : '请先读取 system-mod-uninstall Skill，再用 inspect_install 核对每条记录，按 reconciliation.recommendedAction 和 Skill 处理。不要按名称匹配或操作清单外的安装记录。'
      + '如果报告为 forget_missing_files（清单文件全部缺失，且无覆盖、共享或替换关系），'
      + (removing ? '直接调用 forget_uninstalled_mod({game_id, install_id})，仅移除这条管理记录，无需卸载或回滚。'
        : '只说明清单文件已不在，保留管理记录和库条目，不执行写操作，不报告卸载成功。')
      + '仅在 safe_uninstall 时用 uninstall_mod；在 rollback_install 或 rollback_replacement 时，用报告中的事务 ID（transaction_id）和相同的 game_id、install_id 申请 rollback_transaction。'
      + (removing ? '在这些卸载或回滚分支中，成功后才对同一记录调用 forget_uninstalled_mod。' : '')
      + '其他异常先说明原因和冲突并提交审核，未解决前保留管理记录。';
  return workflow + (batch ? '请在本次会话逐项处理上述目标，每次工具调用仅使用当前项的精确 ID。某项无法处理时记录原因，继续处理其余独立项。最终按项汇总实际结果。' : '');
}

export function formatModRequest(kind: BatchAction, targets: ModRequestTarget[], batch: boolean) {
  const intent = intentLabel(kind, targets.length, batch);
  const order = batch && kind !== 'install' ? sharedRemovalOrder : '';
  return `${prefix}${intent}${targetLabel}${JSON.stringify(batch ? targets : targets[0])}。${workflowText(kind, batch)}${order}`;
}

// Only fold complete app-generated requests. An edited message must keep all its wording visible.
export function parseModRequest(content: string): ModRequest | null {
  if (!content.startsWith(prefix)) return null;
  const labelAt = content.indexOf(targetLabel, prefix.length);
  if (labelAt < 0) return null;
  const intent = content.slice(prefix.length, labelAt);
  const batchCount = intent.match(/以下 ([1-9]\d*) 个 Mod$/)?.[1];
  const batch = batchCount !== undefined;
  const count = batch ? Number(batchCount) : 1;
  const legacyRemoval = intent === '卸载并从管理列表删除这一条安装记录';
  const kind = legacyRemoval ? 'remove' : (['install', 'uninstall', 'remove'] as const)
    .find(action => intent === intentLabel(action, count, batch));
  if (!kind) return null;

  const workflow = workflowText(kind, batch);
  const endings = [workflow];
  if (batch && kind !== 'install') endings.push(workflow + sharedRemovalOrder);
  if (!batch && kind !== 'install') {
    // Earlier desktop versions persisted these two single-record templates.
    endings.push(workflow.replace('核对每条记录', '核对该记录').replace('操作清单外的安装记录', '操作其他安装记录'));
    endings.push('请先读取 system-mod-uninstall Skill，再用 inspect_install 核对该记录，按 reconciliation.recommendedAction 和 Skill 处理：仅在 safe_uninstall 时用 uninstall_mod；在 rollback_install 或 rollback_replacement 时，用报告中的事务 ID（transaction_id）和相同的 game_id、install_id 申请 rollback_transaction；存在冲突时先说明。不要按名称匹配或操作其他安装记录。'
      + (kind === 'remove' ? '卸载或回滚成功后，再用 forget_uninstalled_mod 对相同游戏和记录执行从管理列表删除。' : ''));
  }
  const ending = endings.find(text => content.endsWith(`。${text}`));
  if (!ending) return null;
  let value: unknown;
  try { value = JSON.parse(content.slice(labelAt + targetLabel.length, -ending.length - 1)); }
  catch { return null; }
  if (Array.isArray(value) !== batch) return null;
  const targets: unknown[] = Array.isArray(value) ? value : [value];
  if (targets.length !== count) return null;

  const idField = kind === 'install' ? 'artifactId' : 'installId';
  let gameId: string | undefined;
  const ids = new Set<string>();
  for (const target of targets) {
    if (!target || typeof target !== 'object' || Object.keys(target).length !== 2 || !('gameId' in target)) return null;
    const id: unknown = Reflect.get(target, idField);
    if (typeof target.gameId !== 'string' || !target.gameId.trim() || typeof id !== 'string' || !id.trim()) return null;
    gameId ??= target.gameId;
    if (target.gameId !== gameId || ids.has(id)) return null;
    ids.add(id);
  }
  return gameId ? { kind, batch, gameId, ids: [...ids] } : null;
}
