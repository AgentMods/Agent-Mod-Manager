import { useState } from "react";
import { AgentConfirmation } from "@/components/AgentConfirmation";
import { AgentQuestionnaire, describeQuestionnaire, respondToQuestionnaire } from "@/components/AgentQuestionnaire";
import { ErrorState } from "@/components/ui";
import { api } from "@/lib/api";
import { describeAgentTools, isQuestionRequest, pendingToolApproval, replayConversation, useAgent } from "@/lib/agent-store";
import type { AgentEvent } from "@/lib/types";

const noEvents: AgentEvent[] = [];

// Surface just the inputs this exact Mod operation needs, not chat history or other turns.
export function ModActionReview({ operationId, name, conversationId, turnId }: {
  operationId: string; name: string; conversationId: string; turnId: string;
}) {
  const milestones = useAgent(state => state.milestones[conversationId] ?? noEvents);
  const events = milestones.filter(event => event.turnId === turnId);
  const approvals = describeAgentTools(events).flatMap(tool => pendingToolApproval(tool) ?? []);
  const question = events.filter(isQuestionRequest).map(request => describeQuestionnaire(request, events, false))
    .find(item => item.valid && !item.outcome);
  const [submitted, setSubmitted] = useState<number | null>(null);
  const [error, setError] = useState<unknown>(null);
  if (!approvals.length && !question) return null;
  return <section id={operationId} tabIndex={-1} className="flex min-w-0 flex-col gap-2" aria-label={`处理 ${name}`}>
    <h3 className="wrap-anywhere text-sm font-medium">{name} · 需要你的确认</h3>
    {approvals.map(request => <AgentConfirmation key={request.seq} request={request} />)}
    {question && <AgentQuestionnaire key={question.request.seq} questionnaire={question}
      disabled={submitted === question.request.seq} onSubmit={async answers => {
        await respondToQuestionnaire(question, answers);
        setSubmitted(question.request.seq);
        void replayConversation(conversationId, { afterPending: true }).catch(setError);
      }} onCancel={() => { void api("cancel_turn", { conversationId, turnId }).catch(setError); }} />}
    <ErrorState error={error} />
  </section>;
}
