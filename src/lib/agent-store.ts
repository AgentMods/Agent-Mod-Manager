import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { api, backendAvailable } from "./api";
import type { AgentEvent } from "./types";

interface AgentState {
  streams: Record<string, AgentEvent[]>;
  merge: (events: AgentEvent[]) => void;
}
export const useAgent = create<AgentState>((set) => ({
  streams: {},
  merge: (events) => set((state) => {
    const streams = {...state.streams};
    const groups = new Map<string, AgentEvent[]>();
    for (const event of events) {
      if (!Number.isSafeInteger(event.seq) || !event.conversationId) continue;
      const group = groups.get(event.conversationId) || [];
      group.push(event); groups.set(event.conversationId, group);
    }
    for (const [id, additions] of groups) {
      const unique = new Map((streams[id] || []).map((event) => [event.seq, event]));
      for (const event of additions) unique.set(event.seq, event);
      streams[id] = [...unique.values()].sort((a, b) => a.seq - b.seq);
    }
    return {streams};
  }),
}));

let listening: Promise<void> | null = null;
export function ensureAgentListeners() {
  if (!backendAvailable) return Promise.resolve();
  if (listening) return listening;
  listening = (async () => {
    const unlisten: Array<() => void> = [];
    try {
      for (const kind of ["turn-start", "delta", "tool-start", "tool-result", "tool-progress", "approval-request", "approval-resolved", "usage", "done", "error"]) {
        unlisten.push(await listen<AgentEvent>(`agent://${kind}`, ({payload}) => useAgent.getState().merge([payload])));
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
    await ensureAgentListeners();
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

export function activeTurn(events: AgentEvent[]) {
  const latest = [...events].reverse().find((event) => event.kind === "turn-start");
  if (!latest || events.some((event) => event.turnId === latest.turnId && event.kind === "done")) return null;
  return latest.turnId;
}

export function pendingApprovals(streams: Record<string, AgentEvent[]>) {
  return Object.values(streams).flatMap((events) => events.filter((event) => event.kind === "approval-request" && !events.some((next) => next.kind === "approval-resolved" && next.payload.approvalId === event.payload.approvalId)));
}
