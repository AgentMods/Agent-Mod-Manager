import { useState } from "react";
import { CheckIcon, ShieldCheckIcon, XIcon } from "lucide-react";
import { api } from "../lib/api";
import { isQuestionRequest, pendingApprovals, replayConversation, useAgent } from "../lib/agent-store";
import type { AgentEvent, FilePlan } from "../lib/types";
import { PlanPreview } from "./PlanDialog";
import { ErrorState, Modal } from "./ui";
import { Button } from "@/components/ui/button";

function findPlan(value: unknown): FilePlan | null {
  if (!value || typeof value !== "object") return null;
  const object = value as Record<string, unknown>;
  if (typeof object.id === "string" && Array.isArray(object.entries)) return object as unknown as FilePlan;
  return findPlan(object.plan) || findPlan(object.deployment);
}

export function ApprovalDialog() {
  const streams = useAgent((s) => s.streams);
  const pending = pendingApprovals(streams).find((event) => !isQuestionRequest(event));
  return pending ? <Approval key={String(pending.payload.approvalId)} event={pending}/> : null;
}
function Approval({event}: {event: AgentEvent}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const preview = event.payload.preview;
  const plan = findPlan(preview);
  async function respond(approved: boolean) {
    if (busy) return;
    setBusy(true); setError(null);
    try {
      await api("respond_approval", {conversationId: event.conversationId, approvalId: event.payload.approvalId, response: {approved, answer: null}});
      await replayConversation(event.conversationId);
    } catch (failure) { setError(failure); }
    finally { setBusy(false); }
  }
  const jsonPreview = <pre className="max-h-72 overflow-auto rounded-md border bg-muted p-3 font-mono text-xs break-all whitespace-pre-wrap">{JSON.stringify(preview, null, 2)}</pre>;
  return <Modal title="操作审批" onClose={() => {void respond(false);}} actions={<>
    <Button variant="outline" disabled={busy} onClick={() => {void respond(false);}}><XIcon data-icon="inline-start"/>拒绝</Button>
    <Button disabled={busy} onClick={() => {void respond(true);}}><CheckIcon data-icon="inline-start"/>批准并执行</Button>
  </>}>
    <p className="flex items-center gap-2 text-sm font-medium"><ShieldCheckIcon className="size-4 text-primary"/>{String(event.payload.title || "待确认操作")}</p>
    {plan ? <PlanPreview plan={plan}/> : jsonPreview}
    {plan && <details className="text-sm"><summary className="cursor-pointer text-muted-foreground">完整请求</summary><div className="mt-2">{jsonPreview}</div></details>}
    <ErrorState error={error}/>
  </Modal>;
}
