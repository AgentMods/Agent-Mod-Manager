import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { api, backendAvailable } from "./api";
import type { AgentEvent } from "./types";

interface AgentState {
  streams: Record<string, AgentEvent[]>;
  // Turn status, approvals and tool traces don't change with each text delta.
  milestones: Record<string, AgentEvent[]>;
  merge: (events: AgentEvent[]) => void;
}

function lowerBound(events: AgentEvent[], seq: number) {
  let lo = 0;
  let hi = events.length;
  while (lo < hi) {
    const mid = lo + Math.floor((hi - lo) / 2);
    if (events[mid].seq < seq) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

function mergeOrdered(previous: AgentEvent[], additions: AgentEvent[]) {
  // Live delivery and durable replay carry the same immutable event per seq.
  // Don't rebuild or publish the snapshot when replay only confirms live events.
  const tail = previous.at(-1)?.seq ?? -Infinity;
  const fresh = additions.filter((event) => event.seq > tail || previous[lowerBound(previous, event.seq)]?.seq !== event.seq);
  if (!fresh.length) return previous;
  if (fresh[0].seq > tail) return previous.concat(fresh);

  // Late replay can fill a gap before already-rendered live events.
  let index = lowerBound(previous, fresh[0].seq);
  const next = previous.slice(0, index);
  for (const event of fresh) {
    while (index < previous.length && previous[index].seq < event.seq) next.push(previous[index++]);
    next.push(event);
  }
  while (index < previous.length) next.push(previous[index++]);
  return next;
}

export const useAgent = create<AgentState>((set) => ({
  streams: {},
  milestones: {},
  merge: (events) => set((state) => {
    const groups = new Map<string, Map<number, AgentEvent>>();
    for (const event of events) {
      if (!Number.isSafeInteger(event.seq) || !event.conversationId) continue;
      let group = groups.get(event.conversationId);
      if (!group) groups.set(event.conversationId, group = new Map());
      group.set(event.seq, event);
    }
    let streams: AgentState["streams"] | undefined;
    let milestones = state.milestones;
    for (const [id, group] of groups) {
      const additions = [...group.values()].sort((a, b) => a.seq - b.seq);
      const previous = state.streams[id] ?? [];
      const merged = mergeOrdered(previous, additions);
      if (merged === previous) continue;
      streams ??= { ...state.streams };
      streams[id] = merged;

      const activity = additions.filter(({ kind }) => kind !== "delta" && kind !== "reply-reset" && kind !== "tool-progress");
      if (!activity.length) continue;
      const previousMilestones = state.milestones[id] ?? [];
      const mergedMilestones = mergeOrdered(previousMilestones, activity);
      if (mergedMilestones !== previousMilestones) {
        if (milestones === state.milestones) milestones = { ...milestones };
        milestones[id] = mergedMilestones;
      }
    }
    return streams ? { streams, milestones } : state;
  }),
}));

const LIVE_DELTA_INTERVAL_MS = 32;
let liveDeltas: AgentEvent[] = [];
let deltaTimer: ReturnType<typeof setTimeout> | undefined;
function publishLive(event?: AgentEvent) {
  clearTimeout(deltaTimer);
  deltaTimer = undefined;
  const events = liveDeltas;
  liveDeltas = [];
  if (event) events.push(event);
  useAgent.getState().merge(events);
}

function enqueueLive(event: AgentEvent) {
  // Don't delay controls/approvals/done, or publish them ahead of buffered text.
  if (event.kind !== "delta") {
    publishLive(event);
    return;
  }
  liveDeltas.push(event);
  deltaTimer ??= setTimeout(publishLive, LIVE_DELTA_INTERVAL_MS);
}

let listening: Promise<void> | null = null;
// A live done event may arrive before missing deltas have been replayed.
// Release the watch only after the durable cursor reaches the end of the turn.
const watched = new Map<string, string | undefined>();
export function trackAgentTurn(id: string, turnId?: string, { ifUntracked = false } = {}) {
  if (ifUntracked) {
    // A cached list checkpoint must not replace a newer live watch or rearm a
    // turn that this webview already replayed to completion.
    if (watched.has(id) || useAgent.getState().milestones[id]?.some((event) =>
      event.turnId === turnId && event.kind === "done" && event.seq <= (cursors.get(id) ?? 0))) return;
  }
  watched.set(id, turnId ?? watched.get(id));
}
export function ensureAgentListeners() {
  if (!backendAvailable) return Promise.resolve();
  if (listening) return listening;
  listening = (async () => {
    const unlisten: Array<() => void> = [];
    try {
      for (const kind of ["turn-start", "delta", "reply-reset", "tool-start", "tool-result", "tool-progress", "approval-request", "approval-resolved", "usage", "done", "error"]) {
        unlisten.push(await listen<AgentEvent>(`agent://${kind}`, ({payload}) => {
          if (payload.kind === "turn-start") trackAgentTurn(payload.conversationId, payload.turnId);
          enqueueLive(payload);
        }));
      }
    } catch (error) {
      for (const stop of unlisten) stop();
      listening = null;
      throw error;
    }
  })();
  return listening;
}

// Replay cursors advance only from durable fetches, never from a live event.
// A later live sequence must not hide an earlier event lost during reconnect.
const cursors = new Map<string, number>();
const replaying = new Map<string, Promise<void>>();
export function replayConversation(id: string) {
  const existing = replaying.get(id);
  if (existing) return existing;
  const replay = (async () => {
    for (let page = 0; page < 64; page++) {
      const after = cursors.get(id) || 0;
      const events = await api<AgentEvent[]>("replay_agent_events", {conversationId: id, after, limit: 2000});
      if (!events.length) return;
      useAgent.getState().merge(events);
      const next = Math.max(...events.map((event) => event.seq));
      if (next <= after) return;
      cursors.set(id, next);
      if (events.length < 2000) return;
    }
  })().finally(() => replaying.delete(id));
  replaying.set(id, replay);
  return replay;
}

export function syncAgentEvents(
  selected: string | null,
  onSettled: (id: string, events: AgentEvent[]) => void,
  onError: (error: unknown) => void,
) {
  if (!backendAvailable) return;
  let disposed = false;
  const failed = new Set<string>();
  const report = (key: string, error: unknown) => {
    if (disposed || failed.has(key)) return;
    failed.add(key);
    onError(error);
  };
  const refresh = () => {
    void ensureAgentListeners().then(() => failed.delete("listeners"), (error) => report("listeners", error));
    const ids = new Set(watched.keys());
    if (selected) ids.add(selected);
    for (const id of ids) {
      void replayConversation(id).then(() => {
        if (disposed) return;
        failed.delete(id);
        const { milestones, streams } = useAgent.getState();
        const events = milestones[id] ?? [];
        const latest = latestTurnStart(events);
        if (!latest) return;
        const done = events.find((event) => event.turnId === latest.turnId && event.kind === "done");
        if (!done) {
          if (!watched.has(id)) trackAgentTurn(id, latest.turnId);
          return;
        }
        const expected = watched.get(id);
        if (!watched.has(id) || done.seq > (cursors.get(id) ?? 0)) return;
        // If the expected start hasn't been observed, this might be an older
        // turn's done. After replay sees both starts, the latest turn wins.
        if (expected && expected !== latest.turnId && !events.some((event) =>
          event.kind === "turn-start" && event.turnId === expected && event.seq < latest.seq)) return;
        watched.delete(id);
        onSettled(id, streams[id].filter((event) => event.turnId === latest.turnId));
      }, (error) => report(id, error));
    }
  };
  refresh();
  const timer = setInterval(refresh, 2000);
  return () => { disposed = true; clearInterval(timer); };
}

function latestTurnStart(events: AgentEvent[]) {
  for (let i = events.length - 1; i >= 0; i--) {
    if (events[i].kind === "turn-start") return events[i];
  }
}

export function activeTurn(events: AgentEvent[]) {
  const latest = latestTurnStart(events);
  if (!latest || events.some((event) => event.turnId === latest.turnId && event.kind === "done")) return null;
  return latest.turnId;
}

export function isQuestionRequest(event: AgentEvent) {
  return event.kind === "approval-request" && event.payload.title === "ask_user";
}

export function pendingApproval(milestones: Record<string, AgentEvent[]>) {
  for (const events of Object.values(milestones)) {
    const resolved = new Set(events.filter((event) => event.kind === "approval-resolved").map((event) => event.payload.approvalId));
    const pending = events.find((event) => event.kind === "approval-request" && !isQuestionRequest(event) && !resolved.has(event.payload.approvalId));
    if (pending) return pending;
  }
}
