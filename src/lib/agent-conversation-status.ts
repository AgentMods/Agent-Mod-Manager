import { useEffect, useMemo } from "react";
import { isQuestionRequest, trackAgentTurn, useAgent } from "./agent-store";
import { backendAvailable } from "./api";
import type { AgentEvent, Conversation } from "./types";

type Checkpoint = NonNullable<Conversation["agent_status"]>;
type WaitingStatus = NonNullable<Checkpoint["pending"]>["status"];
export type AgentConversationStatus = "running" | "completed" | "waiting-input" | WaitingStatus;

export interface AgentConversationState {
  status: AgentConversationStatus;
  /** True for a nonterminal turn, including when it is waiting. */
  active: boolean;
}

type ConversationStatusSource = Pick<Conversation, "id" | "message_count" | "agent_status">;

function settledStatus(status: unknown): AgentConversationState {
  return { status: status === "done" ? "completed" : "waiting-input", active: false };
}

/** Project the latest turn, not the latest event overall (an older turn's done can arrive late). */
export function getAgentConversationStatus(
  conversation: ConversationStatusSource,
  milestones: readonly AgentEvent[] = [],
): AgentConversationState | undefined {
  const checkpoint = conversation.agent_status;
  const after = checkpoint?.event_seq ?? 0;
  let start: AgentEvent | undefined;
  for (const event of milestones) {
    if (event.conversationId === conversation.id && event.kind === "turn-start" && event.seq > (start?.seq ?? after)) {
      start = event;
    }
  }

  if (!start && checkpoint?.turn_status !== "running") {
    // Without a checkpoint or turn history, a non-empty row's status is unknown.
    return checkpoint || conversation.message_count === 0 ? settledStatus(checkpoint?.turn_status) : undefined;
  }

  const turnId = start?.turnId ?? checkpoint?.turn_id;
  const pending = new Map<string, { seq: number; status: WaitingStatus }>();
  const resolved = new Map<string, number>();
  if (!start && checkpoint?.pending) {
    pending.set(checkpoint.pending.approval_id, { seq: after, status: checkpoint.pending.status });
  }
  for (const event of milestones) {
    if (event.conversationId !== conversation.id || event.turnId !== turnId || event.seq <= (start?.seq ?? after)) continue;
    if (event.kind === "done") return settledStatus(event.payload.status);
    const { approvalId } = event.payload;
    if (typeof approvalId !== "string" || !approvalId) continue;
    if (event.kind === "approval-resolved") {
      resolved.set(approvalId, Math.max(event.seq, resolved.get(approvalId) ?? 0));
    } else if (event.kind === "approval-request" && event.seq > (pending.get(approvalId)?.seq ?? 0)) {
      pending.set(approvalId, { seq: event.seq, status: isQuestionRequest(event) ? "waiting-answer" : "waiting-approval" });
    }
  }

  let latest: { seq: number; status: WaitingStatus } | undefined;
  for (const [id, request] of pending) {
    if ((resolved.get(id) ?? -1) > request.seq) continue;
    if (!latest || request.seq > latest.seq) latest = request;
  }
  return { status: latest?.status ?? "running", active: true };
}

export function useAgentConversationStatuses(
  conversations: readonly ConversationStatusSource[],
): Readonly<Partial<Record<string, AgentConversationState>>> {
  const milestones = useAgent((state) => state.milestones);
  const statuses = useMemo(() => Object.fromEntries(conversations.map((conversation) => [
    conversation.id,
    getAgentConversationStatus(conversation, milestones[conversation.id]),
  ])), [conversations, milestones]);

  // A webview reload can miss the live turn-start for a background turn. Add
  // those turns to the shell's existing replay loop; it invalidates the list
  // after the terminal event has been replayed, without polling the list itself.
  useEffect(() => {
    if (!backendAvailable) return;
    for (const { id, agent_status: checkpoint } of conversations) {
      if (checkpoint?.turn_status === "running" && checkpoint.turn_id) {
        trackAgentTurn(id, checkpoint.turn_id, { ifUntracked: true });
      }
    }
  }, [conversations]);

  return statuses;
}
