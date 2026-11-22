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

type DiscardedRange = { first: number; last: number };
// Keep the holes independently of the stream: a later restart can remove an
// earlier marker, and a native replay already in flight can still return old rows.
const discarded = new Map<string, DiscardedRange[]>();

function historyThrough(event: AgentEvent) {
  const through = event.payload.historyThroughSeq;
  return event.kind === "turn-start" && typeof through === "number" && Number.isSafeInteger(through)
    && through >= 0 && through < event.seq ? through : undefined;
}

export function latestHistoryRewrite(events: AgentEvent[]) {
  for (let i = events.length - 1; i >= 0; i--) {
    if (historyThrough(events[i]) !== undefined) return events[i];
  }
}

function rememberDiscarded(ranges: DiscardedRange[], first: number, last: number) {
  let begin = 0;
  while (begin < ranges.length && ranges[begin].last < first - 1) begin++;
  const existing = ranges[begin];
  if (existing && existing.first <= first && existing.last >= last) return false;
  let end = begin;
  while (end < ranges.length && ranges[end].first <= last + 1) {
    first = Math.min(first, ranges[end].first);
    last = Math.max(last, ranges[end++].last);
  }
  ranges.splice(begin, end - begin, { first, last });
  return true;
}

function isDiscarded(ranges: DiscardedRange[], seq: number) {
  let lo = 0;
  let hi = ranges.length;
  while (lo < hi) {
    const mid = lo + Math.floor((hi - lo) / 2);
    if (ranges[mid].last < seq) lo = mid + 1;
    else hi = mid;
  }
  return lo < ranges.length && ranges[lo].first <= seq;
}

function dropInterval(events: AgentEvent[], through: number, start: number) {
  const begin = lowerBound(events, through + 1);
  const end = lowerBound(events, start);
  return begin === end ? events : events.slice(0, begin).concat(events.slice(end));
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
      let additions = [...group.values()].sort((a, b) => a.seq - b.seq);
      const cached = state.streams[id] ?? [];
      const cachedMilestones = state.milestones[id] ?? [];
      let previous = cached;
      let previousMilestones = cachedMilestones;
      let ranges = discarded.get(id);
      // Apply cutoffs before ingesting any rows; the same batch may contain a
      // buffered delta or replay events from both sides of a restart.
      for (const event of additions) {
        if (event.kind !== "turn-start") continue;
        const through = historyThrough(event);
        if (through === undefined || through + 1 === event.seq) continue;
        if (!ranges) discarded.set(id, ranges = []);
        if (!rememberDiscarded(ranges, through + 1, event.seq - 1)) continue;
        previous = dropInterval(previous, through, event.seq);
        previousMilestones = dropInterval(previousMilestones, through, event.seq);
      }
      if (ranges && additions[0].seq <= ranges[ranges.length - 1].last) {
        additions = additions.filter((event) => !isDiscarded(ranges, event.seq));
      }
      const merged = mergeOrdered(previous, additions);
      if (merged === cached) continue;
      streams ??= { ...state.streams };
      streams[id] = merged;

      const activity = additions.filter(({ kind }) => kind !== "delta" && kind !== "reply-reset" && kind !== "tool-progress");
      const mergedMilestones = mergeOrdered(previousMilestones, activity);
      if (mergedMilestones !== cachedMilestones) {
        if (milestones === state.milestones) milestones = { ...milestones };
        milestones[id] = mergedMilestones;
        if (activity.some((event) => historyThrough(event) !== undefined &&
          cachedMilestones[lowerBound(cachedMilestones, event.seq)]?.seq !== event.seq)) {
          // A replay-only restart can erase the watched start and its done.
          // Don't replace a newer IPC watch whose start we haven't received yet.
          const latest = latestTurnStart(mergedMilestones);
          const expected = watched.get(id);
          if (latest && (!expected || cachedMilestones.some((event) => event.turnId === expected && event.seq < latest.seq))) {
            trackAgentTurn(id, latest.turnId);
          }
        }
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
  // Restart IPC returns the conversation ID, not the replacement turn ID.
  // An unspecified turn follows the latest observed start, never the old watch.
  watched.set(id, turnId);
}
export function subscribeAgentToolResults(onResults: (results: AgentEvent[]) => void) {
  return useAgent.subscribe(({ milestones }, previous) => {
    if (milestones === previous.milestones) return;
    const results: AgentEvent[] = [];
    for (const [id, events] of Object.entries(milestones)) {
      const before = previous.milestones[id] ?? [];
      if (before === events) continue;
      const start = latestTurnStart(events);
      // Opening a finished conversation replays history, not new tool results.
      if (!start || (!watched.has(id) && !activeTurn(events))) continue;
      for (let i = lowerBound(events, start.seq); i < events.length; i++) {
        const event = events[i];
        if (event.kind === "tool-result" && event.turnId === start.turnId &&
          before[lowerBound(before, event.seq)]?.seq !== event.seq) results.push(event);
      }
    }
    if (results.length) onResults(results);
  });
}

export function ensureAgentListeners() {
  if (!backendAvailable) return Promise.resolve();
  if (listening) return listening;
  listening = (async () => {
    const unlisten: Array<() => void> = [];
    try {
      for (const kind of ["turn-start", "delta", "reply-reset", "tool-start", "tool-result", "tool-progress", "approval-request", "approval-resolved", "usage", "done", "error"]) {
        unlisten.push(await listen<AgentEvent>(`agent://${kind}`, ({payload}) => {
          enqueueLive(payload);
          if (payload.kind === "turn-start" && payload.seq > (cursors.get(payload.conversationId) ?? 0) &&
            latestTurnStart(useAgent.getState().milestones[payload.conversationId] ?? [])?.seq === payload.seq) {
            trackAgentTurn(payload.conversationId, payload.turnId);
          }
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
export function replayConversation(id: string, { afterPending = false } = {}): Promise<void> {
  const existing = replaying.get(id);
  if (existing) {
    // A request issued before restart committed may still return its old snapshot.
    if (afterPending) return existing.then(() => replayConversation(id), () => replayConversation(id));
    return existing;
  }
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

export interface AgentToolActivity {
  event: AgentEvent;
  request?: AgentEvent;
  resolution?: AgentEvent;
  result?: AgentEvent;
  completion?: AgentEvent;
}

// The native toolCallId identifies one invocation across live events and replay.
// A request may be visible before the corresponding tool-start has been replayed.
export function describeAgentTools(events: AgentEvent[]): AgentToolActivity[] {
  const calls = new Map<string, AgentToolActivity>();
  const resolutions = new Map<unknown, AgentEvent>();
  const completions = new Map<string, AgentEvent>();
  for (const event of events) {
    if (event.kind === "done") completions.set(event.turnId, event);
    if (event.kind === "approval-resolved") resolutions.set(event.payload.approvalId, event);
    if (event.kind !== "tool-start" && event.kind !== "approval-request" && event.kind !== "tool-result") continue;
    const id = event.payload.toolCallId;
    if (typeof id !== "string") continue;
    let call = calls.get(id);
    if (!call) {
      call = { event };
      calls.set(id, call);
    }
    if (event.kind === "tool-start") call.event = event;
    if (event.kind === "approval-request") call.request = event;
    if (event.kind === "tool-result") call.result = event;
  }
  const tools = [];
  for (const call of calls.values()) {
    if (call.request && isQuestionRequest(call.request)) continue;
    if (call.event.kind === "tool-result" && !call.request) continue;
    call.resolution = call.request && resolutions.get(call.request.payload.approvalId);
    call.completion = completions.get(call.event.turnId);
    tools.push(call);
  }
  return tools;
}

export function pendingToolApproval(tool: AgentToolActivity) {
  return !tool.resolution && !tool.result && !tool.completion ? tool.request : undefined;
}
