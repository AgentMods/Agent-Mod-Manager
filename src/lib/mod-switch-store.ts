import { dashboardStatsKey } from "./dashboard-stats";
import { tr } from "@/lib/i18n";
import { isCancelledError, type QueryClient } from "@tanstack/react-query";
import { create } from "zustand";
import { getReplayedAgentMilestones, subscribeAgentReplays, trackAgentTurn, useAgent } from "./agent-store";
import { api, backendAvailable, errorText } from "./api";
import type { AgentEvent } from "./types";

export type ModSwitchTarget = { gameId: string; artifactId?: string | null; installId?: string | null };
export type ModSwitchAction = ModSwitchTarget & { name: string } & (
    | { kind: "install"; artifactId: string; replaceInstallId?: string }
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
type Running = Extract<PendingModSwitch, { status: "running" }>;
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
    return bindModSwitches([id], conversationId, turnId);
}

/** Publish all reservations for the turn in a single store update. */
export function bindModSwitches(ids: string[], conversationId: string, turnId: string): boolean {
    if (!ids.length || !conversationId || !turnId || new Set(ids).size !== ids.length) return false;
    const { pending } = useModSwitch.getState();
    if (Object.values(pending).some(entry => entry.conversationId === conversationId && entry.turnId === turnId)) return false;
    const first = pending[ids[0]];
    if (first?.status !== "starting") return false;
    const next = { ...pending };
    for (const id of ids) {
        const entry = pending[id];
        if (entry?.status !== "starting" || entry.gameId !== first.gameId || entry.kind !== first.kind) return false;
        next[id] = { ...entry, status: "running", conversationId, turnId };
    }
    trackAgentTurn(conversationId, turnId);
    useModSwitch.setState({ pending: next });
    return true;
}

/** Release only if no turn started; once bound, its durable events own completion. */
export function releaseModSwitch(id: string): void {
    if (useModSwitch.getState().pending[id]?.status === "starting") forget(id);
}

/** Re-read native state after an IPC error. Never sends or re-runs an Agent turn. */
export function retryModSwitchReadback(id: string): void {
    const { pending } = useModSwitch.getState();
    const entry = pending[id];
    if (entry?.status !== "readback_failed") return;
    const next = { ...pending };
    // One retry can verify all failed rows of this turn against the same list.
    for (const other of Object.values(pending)) {
        if (other.status === "readback_failed" && other.conversationId === entry.conversationId && other.turnId === entry.turnId) {
            next[other.id] = { ...other, status: "verifying", error: undefined };
        }
    }
    useModSwitch.setState({ pending: next });
}

function record(value: unknown): Record<string, unknown> {
    return value && typeof value === "object" ? value as Record<string, unknown> : {};
}

function text(value: unknown): string | undefined {
    return typeof value === "string" && value.length > 0 ? value : undefined;
}

function turnKey({ conversationId, turnId }: { conversationId: string; turnId: string }) {
    return JSON.stringify([conversationId, turnId]);
}

type MatchingCall = { name: string; mutation: boolean; transactionId?: string };

function matchingCall(entry: ModSwitchAction, event: AgentEvent): MatchingCall | undefined {
    const { name, arguments: value } = event.payload;
    const args = record(value);
    if (entry.kind === "install") {
        if (name === "install_library_mod") {
            const request = record(args.request);
            if (request.gameId === entry.gameId && (request.artifactId ?? request.libraryEntryId) === entry.artifactId &&
                (request.replaceInstallId ?? undefined) === entry.replaceInstallId) return { name, mutation: true };
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
            else calls.delete(callId);
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
                    failure = { outcome: "failed", detail: tr("Agent 未返回有效的删除回执。") };
                } else if (receipt.gameId !== entry.gameId || receipt.installId !== entry.installId) {
                    failure = { outcome: "failed", detail: tr("Agent 返回的删除目标与请求不符。") };
                } else {
                    committed = { outcome: "committed", rollback: false };
                }
            } else if (receipt.status !== (call.name === "rollback_transaction" ? "rolled_back" : "committed")) {
                failure = { outcome: "failed", detail: tr("Agent 未返回已提交的操作结果。") };
            } else if ((receipt.gameId !== undefined && receipt.gameId !== entry.gameId) ||
                (receipt.artifactId !== undefined && receipt.artifactId !== entry.artifactId) ||
                (entry.kind === "uninstall" && receipt.installId !== undefined && receipt.installId !== entry.installId) ||
                (call.transactionId && receipt.transactionId !== undefined && receipt.transactionId !== call.transactionId)) {
                failure = { outcome: "failed", detail: tr("Agent 返回的操作目标与请求不符。") };
            } else {
                committed = {
                    outcome: "committed", installId: text(receipt.installId),
                    transactionId: call.transactionId ?? text(receipt.transactionId), rollback: call.name === "rollback_transaction"
                };
            }
        }
    }
    // A turn can be cancelled or fail after the tool committed. Report the real
    // state in that case, not the outcome of the model's final reply.
    if (committed) return committed;
    if (rejected) return { outcome: "rejected" };
    if (done.payload.status === "interrupted") return { outcome: "failed", detail: text(done.payload.reason) ?? tr("任务已中断。") };
    if (done.payload.status === "cancelled") return { outcome: "cancelled" };
    if (cancelled && !failure?.detail) return { outcome: "cancelled" };
    if (failure) return failure;
    if (done.payload.status === "error" || turnError) return { outcome: "failed", detail: turnError };
    if (entry.kind === "remove" && [...calls.values()].some(call => call.name === "forget_uninstalled_mod")) {
        return { outcome: "failed", detail: tr("Agent 未返回有效的删除回执。") };
    }
    // This is a read-only outcome, not a successful uninstall or removal.
    return { outcome: filesMissing && ![...calls.values()].some(call => call.mutation) ? "files_missing" : "no_action" };
}

/** The switch processor owns the authoritative list read for these exact writes. */
export function withoutModSwitchWrites(events: AgentEvent[]) {
    if (!events.length) return events;
    const byTurn = new Map<string, ModSwitchAction[]>();
    for (const entry of Object.values(useModSwitch.getState().pending)) {
        if (entry.status === "starting") continue;
        const key = turnKey(entry);
        const entries = byTurn.get(key);
        if (entries) entries.push(entry);
        else byTurn.set(key, [entry]);
    }
    if (!byTurn.size) return events;
    const trackedCalls = new Map<string, string>();
    return events.filter(event => {
        if (event.kind !== "tool-start" && event.kind !== "tool-result") return true;
        const callId = text(event.payload.toolCallId);
        const entries = byTurn.get(turnKey(event));
        if (!callId || !entries) return true;
        const key = JSON.stringify([event.conversationId, event.turnId, callId]);
        if (event.kind === "tool-start") {
            const name = text(event.payload.name);
            if (name && entries.some(entry => matchingCall(entry, event)?.mutation)) trackedCalls.set(key, name);
            else trackedCalls.delete(key);
            return true;
        }
        const name = trackedCalls.get(key);
        return !name || (event.payload.name !== undefined && event.payload.name !== name);
    });
}

type InstallRecord = {
    id: string; gameId: string; status: string;
    downloadOrigin?: { artifactId?: string } | null;
    metadata?: { libraryEntryId?: string; replacesInstallId?: string | null; transactionId?: string | null } | null;
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
    if (matches.length !== 1) return false;
    if (!entry.replaceInstallId) return true;
    return matches[0].metadata?.replacesInstallId === entry.replaceInstallId &&
        rows.some(old => old.gameId === entry.gameId && old.id === entry.replaceInstallId && old.status === 'superseded');
}

function removalFeedback(entry: Extract<ModSwitchAction, { kind: "remove" }>, settlement: Settlement, rows: InstallRecord[]) {
    const target = `「${entry.name}」`;
    const row = rows.find(row => row.gameId === entry.gameId && row.id === entry.installId);
    if (settlement.outcome === "committed") {
        return row ? tr("未能确认{target}的删除结果：收到删除回执，但该记录仍在管理列表中。", { target })
            : tr("{target}已从管理列表删除。", { target });
    }

    const state = !row ? tr("管理列表中已无该记录，但没有有效删除回执，无法确认是否由本次任务删除。")
        : row.status === "uninstalled" ? tr("该记录当前为已卸载，仍在管理列表中。")
            : row.status === "rolled_back" ? tr("该记录当前为已回滚，仍在管理列表中。")
                : tr("该记录仍在管理列表中。");
    switch (settlement.outcome) {
        case "rejected": return tr("已拒绝删除{target}。{state}", { target, state });
        case "cancelled": return tr("已取消删除{target}。{state}", { target, state });
        case "failed": return tr("未能确认{target}的删除结果。{value2}{state}", { target, value2: settlement.detail ? ` ${settlement.detail} ` : "", state });
        case "files_missing": return tr("{target}的安装文件已不在，Agent 未执行删除。{state}", { target, state });
        case "no_action": return tr("Agent 已结束，未执行{target}的删除。{state}", { target, state });
    }
}

function feedback(entry: PendingModSwitch, settlement: Settlement, rows: InstallRecord[]) {
    if (entry.kind === "remove") return removalFeedback(entry, settlement, rows);
    const action = entry.kind === "install" ? (entry.replaceInstallId ? tr("更新") : tr("安装")) : tr("卸载");
    const target = `「${entry.name}」`;
    switch (settlement.outcome) {
        case "committed":
            return isVerified(entry, settlement, rows) ? tr("{target}已{action}。", { target, action }) : tr("未能确认{target}的{action}结果：管理列表状态与操作回执不符。", { target, action });
        case "rejected": return tr("已拒绝{action}{target}。", { action, target });
        case "cancelled": return tr("已取消{action}{target}。", { action, target });
        case "failed": return tr("{action}{target}失败。{value3}", { action, target, value3: settlement.detail ? ` ${settlement.detail}` : "" });
        case "files_missing":
            return rows.some(row => row.gameId === entry.gameId && row.id === entry.installId)
                ? tr("{target}的安装文件已不在，管理记录已保留。", { target })
                : tr("{target}的安装文件已不在，但未能确认管理记录仍在列表中。", { target });
        case "no_action": return tr("Agent 已结束，未执行{target}的{action}。", { target, action });
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

    function observe(entries: Running[]): Readback[] {
        const [first] = entries;
        const { conversationId, turnId } = first;
        const key = turnKey(first);
        const current = useAgent.getState().milestones[conversationId] ?? [];
        const start = current.find(event => event.kind === "turn-start" && event.turnId === turnId);
        if (start) startSeqs.set(key, start.seq);
        const durable = getReplayedAgentMilestones(conversationId);
        const events = durable.filter(event => event.turnId === turnId);
        const done = events.find(event => event.kind === "done");
        if (done) {
            startSeqs.delete(key);
            return entries.map(entry => ({ ...entry, status: "verifying", settlement: turnSettlement(entry, events, done) }));
        }
        const startSeq = startSeqs.get(key);
        if (startSeq === undefined || !durable.some(event => event.kind === "turn-start" &&
            event.seq > startSeq && typeof event.payload.historyThroughSeq === "number" && event.payload.historyThroughSeq < startSeq)) return [];
        startSeqs.delete(key);
        return entries.map(entry => ({
            ...entry, status: "verifying",
            settlement: { outcome: "failed", detail: tr("原任务已被重试或编辑；请核对当前状态。") }
        }));
    }

    function completeReadback(gameId: string, entries: Readback[], result: { rows: InstallRecord[] } | { error: unknown }) {
        const { pending } = useModSwitch.getState();
        const next = { ...pending };
        const notices: { entry: Readback; message: string }[] = [];
        for (const entry of entries) {
            if (pending[entry.id] !== entry) continue;
            if ("rows" in result) {
                delete next[entry.id];
                notices.push({ entry, message: feedback(entry, entry.settlement, result.rows) });
            } else {
                const state = entry.kind === "remove" ? tr("管理列表") : tr("安装状态");
                const message = tr("无法读取「{name}」的最新{state}：{value3}。可重试读取。", { name: entry.name, state, value3: errorText(result.error) });
                next[entry.id] = { ...entry, status: "readback_failed", error: message };
                notices.push({ entry, message });
            }
        }
        if (!notices.length) return;
        useModSwitch.setState({ pending: next });
        notifyReadbacks(gameId, notices);
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
                    throw new Error(tr("安装列表返回无效数据。"));
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
            for (const key of [dashboardStatsKey, ["game", gameId], ["mod-library", gameId], ["mod-tags", gameId], ["games"], ["trash"]]) {
                void client.invalidateQueries({ queryKey: key });
            }
            completeReadback(gameId, entries, { rows });
        } catch (error) {
            if (!disposed) completeReadback(gameId, entries, { error });
        } finally {
            readingGames.delete(gameId);
            update();
        }
    }

    let settling = false;
    function update(conversationId?: string) {
        if (disposed || settling) return;
        for (const gameId of removalNotices.keys()) {
            if (!readingGames.has(gameId) && !hasPendingRemoval(gameId)) removalNotices.delete(gameId);
        }
        const { pending } = useModSwitch.getState();
        const byTurn = new Map<string, Running[]>();
        for (const entry of Object.values(pending)) {
            if (entry.status !== "running" || (conversationId && entry.conversationId !== conversationId)) continue;
            const key = turnKey(entry);
            const entries = byTurn.get(key);
            if (entries) entries.push(entry);
            else byTurn.set(key, [entry]);
        }
        const settled = Array.from(byTurn.values()).flatMap(observe);
        if (settled.length) {
            const next = { ...pending };
            for (const entry of settled) next[entry.id] = entry;
            // Publish every settlement before a store subscriber can start a read.
            settling = true;
            try {
                useModSwitch.setState({ pending: next });
            } finally {
                settling = false;
            }
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
            if (entry.status !== "running") continue;
            const key = turnKey(entry);
            if (startSeqs.has(key)) continue;
            const start = milestones[entry.conversationId]?.find(event => event.turnId === entry.turnId && event.kind === "turn-start");
            if (start) startSeqs.set(key, start.seq);
        }
    });
    update();
    return () => { disposed = true; stopReplay(); stopStore(); stopLive(); };
}
