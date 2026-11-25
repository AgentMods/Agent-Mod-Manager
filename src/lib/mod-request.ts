import { getLocale, tr } from '@/lib/i18n';
import type { BatchAction } from './mod-management';
import type { Message } from './types';

export const batchLabels = {
    get install() { return tr('批量安装'); },
    get uninstall() { return tr('批量卸载'); },
    get remove() { return tr('批量删除'); },
};

export type ModRequestTarget = { gameId: string } & ({ artifactId: string } | { installId: string });
export type ModRequest = { kind: BatchAction; batch: boolean; gameId: string; ids: string[] };

export type ModRequestOrigin = {
    kind: 'mod_request'; version: 1; action: BatchAction; batch: boolean; targets: ModRequestTarget[];
};

export function createModRequestMessage(kind: BatchAction, targets: ModRequestTarget[], batch: boolean) {
    const targetJson = JSON.stringify(batch ? targets : targets[0]);
    const intent = batch ? {
        install: tr('批量安装以下 {count} 个 Mod', { count: targets.length }),
        uninstall: tr('批量卸载以下 {count} 个 Mod', { count: targets.length }),
        remove: tr('批量删除以下 {count} 个 Mod', { count: targets.length }),
    }[kind] : {
        install: tr('为此游戏安装这一项 Mod'),
        uninstall: tr('卸载并保留在管理中这一条安装记录'),
        remove: tr('从管理列表删除这一条安装记录'),
    }[kind];
    const locale = getLocale();
    const separator = locale === 'zh-CN' || locale === 'ja' ? '' : ' ';
    const workflow = kind === 'install'
        ? tr('请按系统安装 Skill，先用 {matchModInstall} 核对本地归档和适配规则。仅在本地证据不足时，先通过来源 MCP 查阅作者说明；若 MCP 无法提供足够说明，再用 Playwright 查阅对应发布页。只有本地证据和作者说明都不足以确定安装方式，或需要我决定可选偏好时，才向我提问。确定后用 {installLibraryMod} 申请安装，不要替换成其他游戏或归档。', {
            matchModInstall: 'match_mod_install', installLibraryMod: 'install_library_mod',
        })
        : [tr('请先读取 {skill} Skill，再用 {inspectInstall} 核对每条记录，按 {recommendedAction} 和 Skill 处理。不要按名称匹配或操作清单外的安装记录。', {
            skill: 'system-mod-uninstall', inspectInstall: 'inspect_install', recommendedAction: 'reconciliation.recommendedAction',
        }), tr('如果报告为 {forgetMissingFiles}（清单文件全部缺失，且无覆盖、共享或替换关系），{nextStep}', {
            forgetMissingFiles: 'forget_missing_files',
            nextStep: kind === 'remove'
                ? tr('直接调用 {forgetCall}，仅移除这条管理记录，无需卸载或回滚。', { forgetCall: 'forget_uninstalled_mod({game_id, install_id})' })
                : tr('只说明清单文件已不在，保留管理记录和库条目，不执行写操作，不报告卸载成功。'),
        }), tr('仅在 {safeUninstall} 时用 {uninstallMod}；在 {rollbackInstall} 或 {rollbackReplacement} 时，用报告中的事务 ID（{transactionId}）和相同的 {gameId}、{installId} 申请 {rollbackTransaction}。', {
            safeUninstall: 'safe_uninstall', uninstallMod: 'uninstall_mod', rollbackInstall: 'rollback_install', rollbackReplacement: 'rollback_replacement',
            transactionId: 'transaction_id', gameId: 'game_id', installId: 'install_id', rollbackTransaction: 'rollback_transaction',
        }), tr('GTA RPF 例外：若报告建议 {prepareJob}，按游戏适配包 Skill 重建其余贡献，然后用 {uninstallJob} 提交。', {
            prepareJob: 'prepare_gta_rpf_job', uninstallJob: 'uninstall_mod({game_id, install_id, rpf_job_id})',
        }), kind === 'remove'
            ? tr('在这些卸载或回滚分支中，成功后才对同一记录调用 {forgetUninstalledMod}。', { forgetUninstalledMod: 'forget_uninstalled_mod' }) : '',
        tr('其他异常先说明原因和冲突并提交审核，未解决前保留管理记录。')].filter(Boolean).join(separator);
    const batchWorkflow = batch
        ? tr('请在本次会话逐项处理上述目标，每次工具调用仅使用当前项的精确 ID。某项无法处理时记录原因，继续处理其余独立项。最终按项汇总实际结果。') : '';
    const order = batch && kind !== 'install'
        ? tr('批量卸载或删除遇到共享路径时，先处理本批中可回滚的记录。每成功一项，重新检查受影响的已选记录，再继续处理。') : '';
    const content = tr('我已在游戏详情中明确选择{intent}。目标（精确 ID）：{targets}。{workflow}', {
        intent, targets: targetJson, workflow: [workflow, batchWorkflow, order].filter(Boolean).join(separator),
    });
    const origin: ModRequestOrigin = { kind: 'mod_request', version: 1, action: kind, batch, targets };
    return { content, origin };
}

// i18n-ignore-start: frozen Chinese wire templates from releases before message origin.
// Do not translate or reuse UI label tables here. These are exact history bytes.
const prefix = '我已在游戏详情中明确选择';
const targetLabel = '。目标（精确 ID）：';
const legacyBatchLabels = { install: '批量安装', uninstall: '批量卸载', remove: '批量删除' };

function intentLabel(kind: BatchAction, count: number, batch: boolean) {
    if (batch) return `${legacyBatchLabels[kind]}以下 ${count} 个 Mod`;
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

function parseLegacyModRequest(content: string): ModRequest | null {
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
    const request = readTargets(kind, batch, targets);
    if (!request || request.ids.length !== count) return null;
    // JSON.parse also accepts whitespace, duplicate keys, a different order and
    // alternate escapes. None were emitted by the old formatters.
    const canonicalTargets = request.ids.map(id => kind === 'install'
        ? { gameId: request.gameId, artifactId: id } : { gameId: request.gameId, installId: id });
    const canonicalJson = JSON.stringify(batch ? canonicalTargets : canonicalTargets[0]);
    return content === `${prefix}${intent}${targetLabel}${canonicalJson}。${ending}` ? request : null;
}
// i18n-ignore-end

const actions = ['install', 'uninstall', 'remove'] as const;

function readTargets(kind: BatchAction, batch: boolean, targets: unknown): ModRequest | null {
    if (!Array.isArray(targets) || targets.length === 0 || (!batch && targets.length !== 1)) return null;
    const idField = kind === 'install' ? 'artifactId' : 'installId';
    let gameId: string | undefined;
    const ids = new Set<string>();
    for (const target of targets) {
        if (!target || typeof target !== 'object' || Object.keys(target).length !== 2 ||
            !Object.hasOwn(target, 'gameId') || !Object.hasOwn(target, idField)) return null;
        const id: unknown = target[idField];
        if (typeof target.gameId !== 'string' || !target.gameId.trim() || typeof id !== 'string' || !id.trim()) return null;
        gameId ??= target.gameId;
        if (target.gameId !== gameId || ids.has(id)) return null;
        ids.add(id);
    }
    return gameId === undefined ? null : { kind, batch, gameId, ids: [...ids] };
}

export function parseModRequest({ role, content, origin, attachments }: Pick<Message, 'role' | 'content' | 'origin' | 'attachments'>): ModRequest | null {
    if (role !== 'user' || attachments?.length) return null;
    // Only old rows have no provenance. Explicit user/edited and unknown future
    // origins must never fall back to looking like an app-generated Mod action.
    if (origin == null) return parseLegacyModRequest(content);
    if (origin.kind !== 'mod_request' || origin.version !== 1 || origin.originalContent !== content ||
        !actions.includes(origin.action) || typeof origin.batch !== 'boolean') return null;
    return readTargets(origin.action, origin.batch, origin.targets);
}
