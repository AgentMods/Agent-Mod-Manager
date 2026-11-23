import { isCancelledError, type QueryClient } from "@tanstack/react-query";
import { create } from "zustand";
import { getReplayedAgentMilestones, subscribeAgentReplays, trackAgentTurn, useAgent } from "./agent-store";
import { api, backendAvailable, errorText } from "./api";
import type { AgentEvent } from "./types";

export type ModSwitchTarget = { gameId: string; artifactId?: string | null; installId?: string | null };
export type ModSwitchAction = ModSwitchTarget & { name: string } & (
  | { kind: "install"; artifactId: string }
  | { kind: "uninstall"; installId: string }
  | { kind: "remove"; installId: string }
);

type Uncommitted = { outcome: "no_action" | "files_missing" | "failed" | "rejected" | "cancelled"; detail?: string };
type Committed = { outcome: "committed"; installId?: string; transactionId?: string; rollback: boolean };
type Settlement = Committed | Uncommitted;

export type PendingModSwitch = ModSwitchAction & { id: string } & (
  | { status: "starting"; conversationId?: undefined; turnId?: undefined; error?: undefined }
  | { status: "running"; conversationId: string; turnId: string; error?: undefined }
  | { status: "verifying" | "readback_failed"; conversationId: string; turnId: string; settlement: Settlement; error?: string }
);

type Pending = Record<string, PendingModSwitch>;
type Readback = Extract<PendingModSwitch, { settlement: Settlement }>;
export const useModSwitch = create<{ pending: Pending }>(() => ({ pending: {} }));
let nextId = 0;

function put(entry: PendingModSwitch) {
  useModSwitch.setState(({ pending }) => ({ pending: { ...pending, [entry.id]: entry } }));
}

function forget(id: string) {
  useModSwitch.setState(({ pending }) => {
    const next = { ...pending };
    delete next[id];
    return { pending: next };
  });
}

function matchesInstall(entry: PendingModSwitch, installId: string) {
  return entry.installId === installId ||
    ("settlement" in entry && entry.settlement.outcome === "committed" && entry.settlement.installId === installId);
}

/** Match installed rows first, then follow the same archive across refreshes. */
export function findPendingModSwitch(pending: Pending, row: ModSwitchTarget): PendingModSwitch | undefined {
  const inGame = Object.values(pending).filter(entry => entry.gameId === row.gameId);
  if (row.installId) {
    const { installId } = row;
    const exact = inGame.find(entry => matchesInstall(entry, installId));
    if (exact) return exact;
  }
  if (!row.artifactId) return;
  return inGame.find(entry => entry.artifactId === row.artifactId && entry.kind === "install") ??
    (!row.installId ? inGame.find(entry => entry.artifactId === row.artifactId) : undefined);
}

/** Reserve synchronously before any IPC. Different install IDs can be tracked independently. */
export function reserveModSwitch(action: ModSwitchAction): string | null {
  if (!action.gameId || !(action.kind === "install" ? action.artifactId : action.installId)) return null;
  if (Object.values(useModSwitch.getState().pending).some(entry => entry.gameId === action.gameId && (
    (action.installId && matchesInstall(entry, action.installId)) ||
    ((action.kind === "install" || entry.kind === "install") && action.artifactId && action.artifactId === entry.artifactId)
  ))) return null;
  const id = `mod-switch-${++nextId}`;
  put({ ...action, id, status: "starting" });
  return id;
}

export function bindModSwitch(id: string, conversationId: string, turnId: string): boolean {
  const { pending } = useModSwitch.getState();
  const entry = pending[id];
  if (entry?.status !== "starting" || !conversationId || !turnId ||
      Object.values(pending).some(other => other.conversationId === conversationId && other.turnId === turnId)) return false;
  trackAgentTurn(conversationId, turnId);
  put({ ...entry, status: "running", conversationId, turnId });
  return true;
}

/** Release only if no turn started; once bound, its durable events own completion. */
export function releaseModSwitch(id: string): void {
  if (useModSwitch.getState().pending[id]?.status === "starting") forget(id);
}

/** Re-read native state after an IPC error. Never sends or re-runs an Agent turn. */
export function retryModSwitchReadback(id: string): void {
  const entry = useModSwitch.getState().pending[id];
  if (entry?.status === "readback_failed") put({ ...entry, status: "verifying", error: undefined });
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" ? value as Record<string, unknown> : {};
}

function text(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

type MatchingCall = { name: string; mutation: boolean; transactionId?: string };

function matchingCall(entry: ModSwitchAction, event: AgentEvent): MatchingCall | undefined {
  const { name, arguments: value } = event.payload;
  const args = record(value);
  if (entry.kind === "install") {
    if (name === "install_library_mod") {
      const request = record(args.request);
      if (request.gameId === entry.gameId && (request.artifactId ?? request.libraryEntryId) === entry.artifactId) return { name, mutation: true };
    }
    if (name === "match_mod_install" && args.game_id === entry.gameId && args.artifact_id === entry.artifactId) {
      return { name, mutation: false };
    }
  } else if (args.game_id === entry.gameId && args.install_id === entry.installId) {
    if (entry.kind === "remove" && name === "forget_uninstalled_mod") return { name, mutation: true };
    if (name === "uninstall_mod" || name === "inspect_install") return { name, mutation: name === "uninstall_mod" };
    // An unscoped transaction rollback can target anything. Only the native tool's scoped
    // uninstall path enforces that this transaction belongs to the inspected install.
    if (name === "rollback_transaction" && text(args.transaction_id)) {
      return { name, mutation: true, transactionId: text(args.transaction_id) };
    }
  }
}

function turnSettlement(entry: ModSwitchAction, events: AgentEvent[], done: AgentEvent): Settlement {
  const calls = new Map<string, MatchingCall>();
  const approvals = new Set<string>();
  let committed: Committed | undefined;
  let failure: Uncommitted | undefined;
  let rejected = false;
  let cancelled = false;
  let turnError: string | undefined;
  let filesMissing = false;
  for (const event of events) {
    const { kind, payload } = event;
    const callId = text(payload.toolCallId);
    const approvalId = text(payload.approvalId);
    if (kind === "tool-start" && callId) {
      const call = matchingCall(entry, event);
      if (call) calls.set(callId, call);
    }
    const call = callId ? calls.get(callId) : undefined;
    if (kind === "approval-request" && call?.mutation && approvalId) approvals.add(approvalId);
    if (kind === "approval-resolved" && approvalId && approvals.has(approvalId)) {
      if (payload.status === "rejected") rejected = true;
      if (payload.status === "cancelled") cancelled = true;
    }
    if (kind === "error") turnError = text(payload.message);
    if (kind !== "tool-result" || !call || (payload.name !== undefined && payload.name !== call.name)) continue;
    const receipt = record(payload.result);
    if (call.name === "inspect_install") {
      filesMissing = payload.status === "done" && receipt.gameId === entry.gameId && receipt.installId === entry.installId &&
        record(receipt.reconciliation).recommendedAction === "forget_missing_files";
    }
    if (payload.status === "denied" || payload.status === "rejected") {
      rejected = true;
    } else if (payload.status === "cancelled") {
      cancelled = true;
    } else if (payload.status === "error") {
      failure = { outcome: "failed", detail: text(receipt.error) ?? text(payload.error) };
    } else if (call.mutation && payload.status === "done") {
      if (entry.kind === "remove") {
        // Uninstall and rollback only prepare the record for removal. A removal
        // receipt must itself identify both the game and the install exactly.
        if (call.name !== "forget_uninstalled_mod") continue;
        if (payload.name !== call.name || receipt.status !== "removed") {
          failure = { outcome: "failed", detail: "Agent 未返回有效的删除回执。" };
        } else if (receipt.gameId !== entry.gameId || receipt.installId !== entry.installId) {
          failure = { outcome: "failed", detail: "Agent 返回的删除目标与请求不符。" };
        } else {
          committed = { outcome: "committed", rollback: false };
        }
      } else if (receipt.status !== (call.name === "rollback_transaction" ? "rolled_back" : "committed")) {
        failure = { outcome: "failed", detail: "Agent 未返回已提交的操作结果。" };
      } else if ((receipt.gameId !== undefined && receipt.gameId !== entry.gameId) ||
          (receipt.artifactId !== undefined && receipt.artifactId !== entry.artifactId) ||
          (entry.kind === "uninstall" && receipt.installId !== undefined && receipt.installId !== entry.installId) ||
          (call.transactionId && receipt.transactionId !== undefined && receipt.transactionId !== call.transactionId)) {
        failure = { outcome: "failed", detail: "Agent 返回的操作目标与请求不符。" };
      } else {
        committed = { outcome: "committed", installId: text(receipt.installId),
          transactionId: call.transactionId ?? text(receipt.transactionId), rollback: call.name === "rollback_transaction" };
      }
    }
  }
  // A turn can be cancelled or fail after the tool committed. Report the real
  // state in that case, not the outcome of the model's final reply.
  if (committed) return committed;
  if (done.payload.status === "interrupted") return { outcome: "failed", detail: text(done.payload.reason) ?? "任务已中断。" };
  if (done.payload.status === "cancelled") return { outcome: "cancelled" };
  if (rejected) return { outcome: "rejected" };
  if (cancelled && !failure?.detail) return { outcome: "cancelled" };
  if (done.payload.status === "error" || turnError) return { outcome: "failed", detail: turnError ?? failure?.detail };
  if (failure) return failure;
  if (entry.kind === "remove" && [...calls.values()].some(call => call.name === "forget_uninstalled_mod")) {
    return { outcome: "failed", detail: "Agent 未返回有效的删除回执。" };
  }
  // This is a read-only outcome, not a successful uninstall or removal.
  return { outcome: filesMissing && ![...calls.values()].some(call => call.mutation) ? "files_missing" : "no_action" };
}

/** The switch processor owns the authoritative list read for these exact writes. */
export function withoutModSwitchWrites(events: AgentEvent[]) {
  const first = events[0];
  if (!first) return events;
  const entry = Object.values(useModSwitch.getState().pending).find(action =>
    action.conversationId === first.conversationId && action.turnId === first.turnId);
  if (!entry) return events;
  const callIds = new Set(events.flatMap(event => {
    const callId = text(event.payload.toolCallId);
    return event.kind === "tool-start" && callId && matchingCall(entry, event)?.mutation ? [callId] : [];
  }));
  return events.filter(event => event.kind !== "tool-result" || !callIds.has(text(event.payload.toolCallId) ?? ""));
}

type InstallRecord = {
  id: string; gameId: string; status: string;
  downloadOrigin?: { artifactId?: string } | null;
  metadata?: { libraryEntryId?: string; transactionId?: string | null } | null;
};

function isVerified(entry: Exclude<PendingModSwitch, { kind: "remove" }>, settlement: Committed, rows: InstallRecord[]) {
  if (entry.kind === "uninstall") {
    return rows.some(row => row.gameId === entry.gameId && row.id === entry.installId &&
      row.status === (settlement.rollback ? "rolled_back" : "uninstalled") &&
      (!settlement.rollback || !row.metadata?.transactionId || row.metadata.transactionId === settlement.transactionId));
  }
  if (!settlement.installId || !settlement.transactionId) return false;
  const matches = rows.filter(row => row.gameId === entry.gameId && ["installed", "enabled"].includes(row.status) &&
    (row.metadata?.libraryEntryId ?? row.downloadOrigin?.artifactId) === entry.artifactId &&
    row.id === settlement.installId && row.metadata?.transactionId === settlement.transactionId);
  return matches.length === 1;
}

function removalFeedback(entry: Extract<ModSwitchAction, { kind: "remove" }>, settlement: Settlement, rows: InstallRecord[]) {
  const target = `「${entry.name}」`;
  const row = rows.find(row => row.gameId === entry.gameId && row.id === entry.installId);
  if (settlement.outcome === "committed") {
    return row ? `未能确认${target}的删除结果：收到删除回执，但该记录仍在管理列表中。`
      : `${target}已从管理列表删除。`;
  }

  const state = !row ? "管理列表中已无该记录，但没有有效删除回执，无法确认是否由本次任务删除。"
    : row.status === "uninstalled" ? "该记录当前为已卸载，仍在管理列表中。"
    : row.status === "rolled_back" ? "该记录当前为已回滚，仍在管理列表中。"
    : "该记录仍在管理列表中。";
  switch (settlement.outcome) {
    case "rejected": return `已拒绝删除${target}。${state}`;
    case "cancelled": return `已取消删除${target}。${state}`;
    case "failed": return `未能确认${target}的删除结果。${settlement.detail ? ` ${settlement.detail} ` : ""}${state}`;
    case "files_missing": return `${target}的安装文件已不在，Agent 未执行删除。${state}`;
    case "no_action": return `Agent 已结束，未执行${target}的删除。${state}`;
  }
}

function feedback(entry: PendingModSwitch, settlement: Settlement, rows: InstallRecord[]) {
  if (entry.kind === "remove") return removalFeedback(entry, settlement, rows);
  const action = entry.kind === "install" ? "安装" : "卸载";
  const target = `「${entry.name}」`;
  switch (settlement.outcome) {
    case "committed":
      return isVerified(entry, settlement, rows) ? `${target}已${action}。` : `未能确认${target}的${action}结果：管理列表状态与操作回执不符。`;
    case "rejected": return `已拒绝${action}${target}。`;
    case "cancelled": return `已取消${action}${target}。`;
    case "failed": return `${action}${target}失败。${settlement.detail ? ` ${settlement.detail}` : ""}`;
    case "files_missing":
      return rows.some(row => row.gameId === entry.gameId && row.id === entry.installId)
        ? `${target}的安装文件已不在，管理记录已保留。`
        : `${target}的安装文件已不在，但未能确认管理记录仍在列表中。`;
    case "no_action": return `Agent 已结束，未执行${target}的${action}。`;
  }
}

/** Install one App-level processor: it does not depend on the chat or detail page being mounted. */
export function syncModSwitches(client: QueryClient, notify: (message: string) => void) {
  if (!backendAvailable) return;
  const startSeqs = new Map<string, number>();
  // Use one native read in flight per game, not per row. Rows whose turn
  // finishes after it starts are picked up by the next authoritative read.
  const readingGames = new Set<string>();
  const removalNotices = new Map<string, Map<string, string>>();
  let disposed = false;

  const hasPendingRemoval = (gameId: string) => Object.values(useModSwitch.getState().pending)
    .some(entry => entry.gameId === gameId && entry.kind === "remove");

  function notifyReadbacks(gameId: string, notices: { entry: Readback; message: string }[]) {
    const messages: string[] = [];
    const batch = removalNotices.get(gameId) ?? new Map<string, string>();
    let hasRemovalNotice = false;
    for (const { entry, message } of notices) {
      if (entry.kind === "remove") {
        batch.set(entry.id, message);
        hasRemovalNotice = true;
      } else {
        messages.push(message);
      }
    }
    if (hasRemovalNotice) {
      // The app replaces its current notice, so keep earlier results visible
      // when other removals for the same game finish in separate readbacks.
      messages.push(...batch.values());
      if (hasPendingRemoval(gameId)) removalNotices.set(gameId, batch);
      else removalNotices.delete(gameId);
    }
    if (messages.length) notify(messages.join(" "));
  }

  function observe(entry: Extract<PendingModSwitch, { status: "running" }>) {
    const { id, conversationId, turnId } = entry;
    if (useModSwitch.getState().pending[id] !== entry) return;
    const current = useAgent.getState().milestones[conversationId] ?? [];
    const start = current.find(event => event.kind === "turn-start" && event.turnId === turnId);
    if (start) startSeqs.set(id, start.seq);
    const durable = getReplayedAgentMilestones(conversationId);
    const events = durable.filter(event => event.turnId === turnId);
    const done = events.find(event => event.kind === "done");
    if (done) {
      startSeqs.delete(id);
      put({ ...entry, status: "verifying", settlement: turnSettlement(entry, events, done) });
      return;
    }
    const startSeq = startSeqs.get(id);
    if (startSeq !== undefined && durable.some(event => event.kind === "turn-start" &&
      event.seq > startSeq && typeof event.payload.historyThroughSeq === "number" && event.payload.historyThroughSeq < startSeq)) {
      startSeqs.delete(id);
      put({ ...entry, status: "verifying", settlement: { outcome: "failed", detail: "原任务已被重试或编辑；请核对当前状态。" } });
    }
  }

  async function readback(gameId: string, entries: Readback[]) {
    readingGames.add(gameId);
    const queryKey = ["installs", gameId];
    const fetchLatest = () => client.fetchQuery({
      queryKey, staleTime: 0, retry: false, networkMode: "always",
      queryFn: async () => {
        const rows = await api<InstallRecord[]>("list_installs", { gameId });
        if (!Array.isArray(rows) || rows.some(row => !row || row.gameId !== gameId ||
            typeof row.id !== "string" || typeof row.status !== "string")) {
          throw new Error("安装列表返回无效数据。");
        }
        return rows;
      },
    });
    try {
      // Don't dedupe onto a list query that might predate the terminal event.
      // React Query owns the cache write and ignores results of cancelled reads.
      await client.cancelQueries({ queryKey, exact: true });
      if (disposed) return;
      const rows = await fetchLatest().catch(error => {
        // An unrelated general invalidation can supersede this refetch. Join
        // the replacement if available; it was started after our readback.
        if (isCancelledError(error) && !disposed) return fetchLatest();
        throw error;
      });
      if (disposed) return;
      for (const key of [["game", gameId], ["mod-library", gameId], ["games"], ["trash"]]) {
        void client.invalidateQueries({ queryKey: key });
      }
      const notices: { entry: Readback; message: string }[] = [];
      for (const entry of entries) {
        if (useModSwitch.getState().pending[entry.id] !== entry) continue;
        forget(entry.id);
        notices.push({ entry, message: feedback(entry, entry.settlement, rows) });
      }
      notifyReadbacks(gameId, notices);
    } catch (error) {
      if (disposed) return;
      const notices: { entry: Readback; message: string }[] = [];
      for (const entry of entries) {
        if (useModSwitch.getState().pending[entry.id] !== entry) continue;
        const state = entry.kind === "remove" ? "管理列表" : "安装状态";
        const message = `无法读取「${entry.name}」的最新${state}：${errorText(error)}。可重试读取。`;
        put({ ...entry, status: "readback_failed", error: message });
        notices.push({ entry, message });
      }
      notifyReadbacks(gameId, notices);
    } finally {
      readingGames.delete(gameId);
      update();
    }
  }

  function update(conversationId?: string) {
    if (disposed) return;
    for (const gameId of removalNotices.keys()) {
      if (!readingGames.has(gameId) && !hasPendingRemoval(gameId)) removalNotices.delete(gameId);
    }
    for (const entry of Object.values(useModSwitch.getState().pending)) {
      if (entry.status === "running" && (!conversationId || entry.conversationId === conversationId)) observe(entry);
    }
    const byGame = new Map<string, Readback[]>();
    for (const entry of Object.values(useModSwitch.getState().pending)) {
      if (entry.status !== "verifying" || readingGames.has(entry.gameId)) continue;
      const entries = byGame.get(entry.gameId);
      if (entries) entries.push(entry);
      else byGame.set(entry.gameId, [entry]);
    }
    for (const [gameId, entries] of byGame) void readback(gameId, entries);
  }

  const stopReplay = subscribeAgentReplays(update);
  const stopStore = useModSwitch.subscribe(() => update());
  const stopLive = useAgent.subscribe(({ milestones }, before) => {
    if (milestones === before.milestones) return;
    // Remember a live start if a rewrite later removes it before another replay.
    for (const entry of Object.values(useModSwitch.getState().pending)) {
      if (entry.status !== "running" || startSeqs.has(entry.id)) continue;
      const start = milestones[entry.conversationId]?.find(event => event.turnId === entry.turnId && event.kind === "turn-start");
      if (start) startSeqs.set(entry.id, start.seq);
    }
  });
  update();
  return () => { disposed = true; stopReplay(); stopStore(); stopLive(); };
}
