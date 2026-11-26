import { useEffect } from "react";
import { queryOptions, useMutation, useMutationState, useQueries, useQuery, type QueryClient } from "@tanstack/react-query";
import { activeTurnStart, isQuestionRequest, trackAgentTurn, useAgent } from "./agent-store";
import { api, backendAvailable } from "./api";
import { tr } from "./i18n";
import type { AgentEvent } from "./types";

/** Opaque capabilities issued by the native browse session; never include links or provider IDs. */
export interface BrowseInstallWork {
  searchId: string;
  itemId: string;
}

export interface BrowseInstallIndexItem extends BrowseInstallWork {
  gameId: string;
  title: string;
}

export const browseInstallIndexKey = ["browse-install-tasks"] as const;
const browseInstallTaskKey = ["browse-install"] as const;

export type BrowseInstallStatus = "running" | "installed" | "unverified" | "rejected" | "cancelled" | "interrupted" | "failed";

export interface BrowseInstallTask {
  id: string;
  conversationId: string;
  turnId: string | null;
  status: BrowseInstallStatus;
  installId: string | null;
  transactionId: string | null;
}

export function browseInstallKey(work: BrowseInstallWork, gameId: string) {
  return [...browseInstallTaskKey, work.searchId, work.itemId, gameId] as const;
}

export function readBrowseInstall(work: BrowseInstallWork, gameId: string) {
  return api<BrowseInstallTask | null>("browse_install_task", { searchId: work.searchId, itemId: work.itemId, gameId });
}

const startingInstalls = new Map<string, Promise<BrowseInstallTask>>();

export function startBrowseInstall(client: QueryClient, work: BrowseInstallWork, gameId: string) {
  const key = browseInstallKey(work, gameId);
  const id = JSON.stringify(key);
  const inFlight = startingInstalls.get(id);
  if (inFlight) return inFlight;
  const start = api<BrowseInstallTask>("start_browse_install", { searchId: work.searchId, itemId: work.itemId, gameId }).then(async (task) => {
    // A status read started before this task existed may still be in flight after paging back.
    await client.cancelQueries({ queryKey: key, exact: true });
    client.setQueryData(key, task);
    trackBrowseInstall(task);
    void client.invalidateQueries({ queryKey: browseInstallIndexKey });
    return task;
  }).finally(() => startingInstalls.delete(id));
  startingInstalls.set(id, start);
  return start;
}

function trackBrowseInstall(task: BrowseInstallTask | null | undefined) {
  if (task?.status === "running" && task.turnId) {
    trackAgentTurn(task.conversationId, task.turnId, { ifUntracked: true });
  }
}

export function refreshBrowseInstallTasks(client: QueryClient, conversationId: string) {
  void client.invalidateQueries({
    queryKey: browseInstallTaskKey,
    predicate: ({ state }) => (state.data as BrowseInstallTask | null | undefined)?.conversationId === conversationId,
  });
}

export async function cancelBrowseInstall({ conversationId, turnId, status }: BrowseInstallTask) {
  if (status === "running" && turnId) await api("cancel_turn", { conversationId, turnId });
}

export function browseInstallStatus(task: BrowseInstallTask): BrowseInstallStatus {
  // A terminal Agent or download event is never a receipt. Only native task
  // reconciliation may assert success, with both linked evidence identifiers.
  return task.status === "installed" && (!task.installId || !task.transactionId) ? "unverified" : task.status;
}

const EMPTY_EVENTS: AgentEvent[] = [];

type Waiting = "answer" | "approval";

function activeApproval(events: readonly AgentEvent[], turnId: string | null | undefined) {
  if (!turnId) return undefined;
  const turn = events.filter((event) => event.turnId === turnId);
  if (turn.some((event) => event.kind === "done")) return undefined;
  const resolved = new Set(turn.filter((event) => event.kind === "approval-resolved").map((event) => event.payload.approvalId));
  const completed = new Set(turn.filter((event) => event.kind === "tool-result").map((event) => event.payload.toolCallId));
  return turn.reverse().find((event) => event.kind === "approval-request" &&
    typeof event.payload.approvalId === "string" && !resolved.has(event.payload.approvalId) && !completed.has(event.payload.toolCallId));
}

export function browseInstallWaitingOn(request: AgentEvent | undefined): Waiting | undefined {
  return request ? (isQuestionRequest(request) ? "answer" : "approval") : undefined;
}

export function browseInstallStatusLabel(status: BrowseInstallStatus, waiting?: Waiting) {
  if (waiting === "answer") return tr("等待你回答");
  if (waiting === "approval") return tr("等待你审批");
  switch (status) {
    case "running": return tr("安装进行中");
    case "installed": return tr("安装已验证");
    case "unverified": return tr("未验证到安装结果");
    case "rejected": return tr("安装操作已拒绝");
    case "cancelled": return tr("安装已取消");
    case "interrupted": return tr("安装已中断");
    case "failed": return tr("安装失败");
  }
}

export function usePendingBrowseInstall() {
  return useAgent(({ milestones }) => {
    let waiting: Waiting | undefined;
    for (const events of Object.values(milestones)) {
      const request = activeApproval(events, activeTurnStart(events)?.turnId);
      if (request?.payload.reviewSurface !== "browseInstall") continue;
      waiting = browseInstallWaitingOn(request);
      if (waiting === "answer") return waiting;
    }
    return waiting;
  });
}

function browseInstallTaskQuery(work: BrowseInstallWork, gameId: string) {
  return queryOptions({
    queryKey: browseInstallKey(work, gameId),
    queryFn: () => readBrowseInstall(work, gameId),
    enabled: backendAvailable && !!work.searchId,
    retry: false,
    refetchOnMount: "always",
    refetchInterval: ({ state }) => state.data?.status === "running" ? 1500 : false,
  });
}

export function useBrowseCardInstall(work: BrowseInstallWork, gameIds: string[], onInstall: (gameId: string) => Promise<unknown>) {
  const tasks = useQueries({ queries: gameIds.map((id) => browseInstallTaskQuery(work, id)) });
  const mutationKey = ["start-browse-install", work.searchId, work.itemId];
  const start = useMutation({ mutationKey, mutationFn: onInstall, networkMode: "always" });
  // The card can remount while its native start request is still pending.
  const submitting = useMutationState({
    filters: { mutationKey, exact: true, status: "pending" },
    select: (mutation) => mutation.state.variables,
  });
  const states = new Map<string, "running" | "installed">();
  for (const [index, gameId] of gameIds.entries()) {
    const task = tasks[index].data;
    const status = submitting.includes(gameId) ? "running" : task && browseInstallStatus(task);
    if (status === "running" || status === "installed") states.set(gameId, status);
  }
  return { start, states };
}

export function useBrowseInstallTask(work: BrowseInstallWork, gameId: string) {
  const query = useQuery(browseInstallTaskQuery(work, gameId));
  const task = query.data;
  useEffect(() => trackBrowseInstall(task), [task]);
  const events = useAgent((state) => task ? state.milestones[task.conversationId] ?? EMPTY_EVENTS : EMPTY_EVENTS);
  const status = task ? browseInstallStatus(task) : null;
  const done = status === "running" && events.some((event) => event.kind === "done" && event.turnId === task?.turnId);
  const request = status === "running" ? activeApproval(events, task?.turnId) : undefined;
  return { query, task, status, done, events, request };
}
