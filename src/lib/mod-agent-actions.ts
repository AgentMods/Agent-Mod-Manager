import type { QueryClient } from '@tanstack/react-query';
import { api, errorText } from './api';
import { trackAgentTurn } from './agent-store';
import { rememberComposerPreferences, resolveComposerPermission } from './composer-preferences';
import { conversationCategoryLabels } from './conversation-groups';
import { tr } from '@/lib/i18n';
import type { BatchAction, LibraryMod, ModAction } from './mod-management';
import { batchLabels, createModRequestMessage } from './mod-request';
import { bindModSwitches, findPendingModSwitch, releaseModSwitch, reserveModSwitch, useModSwitch, type ModSwitchTarget } from './mod-switch-store';
import { useUI } from './store';
import type { AppSettings, Conversation } from './types';

export { batchLabels };
export type AgentModItem = { key: string; action: ModAction; target: ModSwitchTarget };
export type BatchResult = { submitted: string[]; removed: string[]; skipped: number; failures: string[] };

function exactTarget(gameId: string, action: ModAction) {
  return 'artifactId' in action ? { gameId, artifactId: action.artifactId } : { gameId, installId: action.installId };
}

async function bindArchive(client: QueryClient, gameId: string, action: ModAction) {
  if (!('artifactId' in action) || action.gameId === gameId) return;
  if (action.gameId !== null) throw new Error(tr('归档已绑定到其他游戏，请刷新 Mod 管理列表后重试。'));
  await api('bind_mod_library_entry', { gameId, artifactId: action.artifactId });
  client.setQueryData<LibraryMod[]>(['mod-library', gameId], entries =>
    entries?.map(entry => entry.artifactId === action.artifactId ? { ...entry, gameId } : entry));
  void client.invalidateQueries({ queryKey: ['mod-library'] });
}

// Reserve the selected rows before the first IPC. One launch owns all its reservations,
// while the switch store verifies the exact tool receipt and native state for each row.
export async function startModTurn(client: QueryClient, drafts: Map<string, string>, gameId: string, kind: BatchAction,
  items: AgentModItem[], batch: boolean) {
  const submitted: string[] = [], failures: string[] = [];
  let skipped = 0;
  const reserved: { item: AgentModItem; id: string }[] = [];
  for (const item of items) {
    const { action, target } = item;
    const id = !findPendingModSwitch(useModSwitch.getState().pending, target) && reserveModSwitch('artifactId' in action
      ? { ...target, name: action.name, kind: 'install', artifactId: action.artifactId }
      : { ...target, name: action.name, kind: action.remove ? 'remove' : 'uninstall', installId: action.installId });
    if (id) reserved.push({ item, id }); else skipped++;
  }
  const ready: typeof reserved = [];
  try {
    if (!reserved.length) return { submitted, failures, skipped };
    const [settings, { permissionMode, preferences }] = await Promise.all([
      api<AppSettings>('get_settings'), resolveComposerPermission(client, useUI.getState().composerPermission),
    ]);
    if (!settings.model.trim() || !settings.base_url.trim()) throw new Error(tr('请先在「设置 → 模型服务」中配置 Agent 模型。'));
    for (const entry of reserved) {
      try { await bindArchive(client, gameId, entry.item.action); ready.push(entry); }
      catch (error) { failures.push(tr('{name}：无法启动 Agent：{error}', { name: entry.item.action.name, error: errorText(error) })); }
    }
    if (!ready.length) return { submitted, failures, skipped };
    const accepted = ready.map(entry => entry.item);
    const targets = accepted.map(item => exactTarget(gameId, item.action));
    const key = JSON.stringify([gameId, kind, batch, targets.map(target => JSON.stringify(target)).sort()]);
    let conversationId = drafts.get(key);
    if (!conversationId) {
      const title = batch ? tr('{value1} {count} 个 Mod', { value1: batchLabels[kind], count: accepted.length }) : accepted[0].action.name.slice(0, 80);
      const conversation = await api<Conversation>('create_conversation', { gameId, category: kind, title: tr('{category}：{title}', { category: conversationCategoryLabels[kind], title }) });
      conversationId = conversation.id;
      drafts.set(key, conversationId);
      void client.invalidateQueries({ queryKey: ['conversations'] });
    }
    const { content, origin } = createModRequestMessage(kind, targets, batch);
    const turnId = await api<string>('start_turn', { request: { conversationId, content, model: null, permissionMode, reasoningEffort: null }, origin });
    drafts.delete(key);
    if (preferences && permissionMode !== preferences.permission_mode) rememberComposerPreferences(client, { ...preferences, permission_mode: permissionMode });
    if (!batch && kind === 'remove') {
      trackAgentTurn(conversationId, turnId);
      useUI.getState().openConversation(conversationId, gameId);
    } else if (!bindModSwitches(ready.map(entry => entry.id), conversationId, turnId)) {
      // The native turn already started. Keep it accessible and never invite a second submission.
      trackAgentTurn(conversationId, turnId);
      useUI.getState().notify(tr('任务已启动，请在最近对话中查看处理进度。'));
    }
    submitted.push(...accepted.map(item => item.key));
  } catch (error) {
    const affected = ready.length ? ready : reserved;
    for (const { item } of affected) failures.push(tr('{name}：无法启动 Agent：{error}', { name: item.action.name, error: errorText(error) }));
  } finally {
    // Bound reservations stay with their turn until readback; releaseModSwitch only frees unbound ones.
    for (const { id } of reserved) releaseModSwitch(id);
  }
  return { submitted, failures, skipped };
}
