import { useState } from "react";
import { Check, ShieldCheck, X } from "lucide-react";
import { api } from "../lib/api";
import { pendingApprovals, replayConversation, useAgent } from "../lib/agent-store";
import type { AgentEvent, FilePlan } from "../lib/types";
import { PlanPreview } from "./PlanDialog";
import { Button, ErrorState, Modal } from "./ui";

function findPlan(value: unknown): FilePlan | null {
  if (!value || typeof value !== "object") return null;
  const object = value as Record<string, unknown>;
  if (typeof object.id === "string" && Array.isArray(object.entries)) return object as unknown as FilePlan;
  return findPlan(object.plan) || findPlan(object.deployment);
}

export function ApprovalDialog() {
  const streams = useAgent((s) => s.streams);
  const pending = pendingApprovals(streams)[0];
  return pending ? <Approval key={String(pending.payload.approvalId)} event={pending}/> : null;
}
function Approval({event}: {event: AgentEvent}) {
  const [answer, setAnswer] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const preview = event.payload.preview;
  const question = preview && typeof preview === "object" ? (preview as Record<string, unknown>).question : null;
  const plan = findPlan(preview);
  async function respond(approved: boolean) {
    if (busy) return;
    setBusy(true); setError(null);
    try {
      await api("respond_approval", {conversationId: event.conversationId, approvalId: event.payload.approvalId, response: {approved, answer: answer || null}});
      await replayConversation(event.conversationId);
    } catch (failure) { setError(failure); }
    finally { setBusy(false); }
  }
  return <Modal title={question ? "Agent 等待你的回答" : "操作审批"} onClose={() => {void respond(false);}} actions={<><Button disabled={busy} onClick={() => {void respond(false);}}><X size={16}/>拒绝</Button><Button className="primary" disabled={busy || (!!question && !answer.trim())} onClick={() => {void respond(true);}}><Check size={16}/>{question ? "提交回答" : "批准并执行"}</Button></>}>
    <p className="approval-title"><ShieldCheck size={18}/>{String(event.payload.title || "待确认操作")}</p>
    {question ? <label className="field">{String(question)}<textarea value={answer} onChange={(e) => setAnswer(e.target.value)} rows={4}/></label> : plan ? <PlanPreview plan={plan}/> : <pre className="json-preview">{JSON.stringify(preview, null, 2)}</pre>}
    {plan && <details><summary>完整请求</summary><pre className="json-preview">{JSON.stringify(preview, null, 2)}</pre></details>}
    <ErrorState error={error}/>
  </Modal>;
}
