import { useMemo, useRef, useState } from "react";
import { ShieldCheckIcon } from "lucide-react";
import { toast } from "sonner";
import { api, errorText } from "@/lib/api";
import { describeAgentTools, pendingToolApproval, replayConversation, useAgent } from "@/lib/agent-store";
import { displayApprovalPreview, isFilePlanApproval } from "@/lib/agent-path-presentation";
import { useUI } from "@/lib/store";
import type { AgentEvent, FilePlan } from "@/lib/types";
import { Button } from "@/components/ui/button";
import { CodeBlock, CodeBlockContent } from "@/components/ui/code-block";
import { Item, ItemActions, ItemContent, ItemDescription, ItemMedia, ItemTitle } from "@/components/ui/item";
import { PlanPreview } from "./PlanDialog";
import { ErrorState } from "./ui";

function findPlan(value: unknown): FilePlan | null {
  if (!value || typeof value !== "object") return null;
  const object = value as Record<string, unknown>;
  if (typeof object.id === "string" && Array.isArray(object.entries)) return object as unknown as FilePlan;
  return findPlan(object.plan) || findPlan(object.deployment);
}

export function AgentConfirmation({ request }: { request: AgentEvent }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const submitting = useRef(false);
  const { approvalId, title, preview } = request.payload;
  const plan = isFilePlanApproval(title) ? findPlan(preview) : null;

  async function respond(approved: boolean) {
    if (submitting.current) return;
    submitting.current = true;
    setBusy(true);
    setError(null);
    try {
      await api("respond_approval", { conversationId: request.conversationId, approvalId, response: { approved, answer: null } });
    } catch (failure) {
      submitting.current = false;
      setBusy(false);
      setError(failure);
      return;
    }
    // A successful response consumes the approval even if replay has to retry.
    void replayConversation(request.conversationId, { afterPending: true }).catch((failure) => toast.error(errorText(failure)));
  }

  const jsonPreview = <CodeBlock size="sm"><CodeBlockContent wrap className="max-h-64 break-all">
    {JSON.stringify(displayApprovalPreview(title, preview), null, 2)}
  </CodeBlockContent></CodeBlock>;

  return <Item role="group" aria-label="操作审批" aria-busy={busy} variant="outline" size="sm" className="w-full min-w-0 max-w-3xl bg-card">
    <ItemMedia variant="icon"><ShieldCheckIcon aria-hidden="true" /></ItemMedia>
    <ItemContent className="min-w-0">
      <ItemTitle>是否允许执行此操作？</ItemTitle>
      <ItemDescription className="line-clamp-none break-all font-mono text-xs">{String(title || "待确认操作")}</ItemDescription>
    </ItemContent>
    <div className="flex min-w-0 basis-full flex-col gap-2">
      {plan ? <>
        <PlanPreview plan={plan} />
        <details><summary className="cursor-pointer text-xs text-muted-foreground">完整请求</summary><div className="mt-2">{jsonPreview}</div></details>
      </> : jsonPreview}
      <ErrorState error={error} />
    </div>
    <ItemActions className="ms-auto">
      <Button type="button" size="sm" variant="outline" disabled={busy} onClick={() => void respond(false)}>拒绝</Button>
      <Button type="button" size="sm" disabled={busy} onClick={() => void respond(true)}>批准并执行</Button>
    </ItemActions>
  </Item>;
}

export function BackgroundApprovalLink({ visibleConversation }: { visibleConversation: string | null }) {
  const milestones = useAgent((state) => state.milestones);
  const openConversation = useUI((state) => state.openConversation);
  const pending = useMemo(() => {
    for (const [id, events] of Object.entries(milestones)) {
      if (id !== visibleConversation && describeAgentTools(events).some(pendingToolApproval)) return id;
    }
  }, [milestones, visibleConversation]);
  return pending ? <Button type="button" size="sm" variant="outline" onClick={() => openConversation(pending)}>
    <ShieldCheckIcon aria-hidden="true" data-icon="inline-start" />查看待授权操作
  </Button> : null;
}
