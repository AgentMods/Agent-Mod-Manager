import { displayPath } from "@/lib/paths";
import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { CheckIcon } from "lucide-react";
import type { FilePlan } from "@/lib/types";
import { api } from "@/lib/api";
import { useUI } from "@/lib/store";
import { Badge, bytes, ErrorState, Modal, PathText } from "@/components/ui";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";

export function PlanPreview({plan}: {plan: FilePlan}) {
  const counts = plan.entries.reduce((sum, entry) => ({...sum, [entry.impact]: (sum[entry.impact] || 0) + 1}), {} as Record<string, number>);
  return <div className="plan-preview flex min-w-0 flex-col gap-3">
    <p className="text-sm break-words">{plan.summary}</p>
    <div className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
      {Object.entries(counts).map(([kind, count]) => <Badge key={kind}>{kind}: {count}</Badge>)}
      <span>{bytes(plan.total_bytes)}</span>
    </div>
    <div className="rounded-md border">
      <Table>
        <TableHeader><TableRow><TableHead>操作</TableHead><TableHead>源文件 / 目标文件</TableHead><TableHead>原文件</TableHead></TableRow></TableHeader>
        <TableBody>
          {plan.entries.map((entry, index) => <TableRow key={`${index}-${entry.operation.dst}`}>
            <TableCell className="align-top"><Badge tone={entry.before ? "warning" : "success"}>{entry.operation.op}</Badge></TableCell>
            <TableCell className="align-top whitespace-normal">
              {entry.operation.src && <PathText className="block">{displayPath(entry.operation.src)}</PathText>}
              <PathText className="block text-foreground">{displayPath(entry.operation.dst)}</PathText>
            </TableCell>
            <TableCell className="align-top whitespace-normal text-muted-foreground">{entry.before ? `${bytes(entry.before.size)} / 自动备份` : "新增"}</TableCell>
          </TableRow>)}
        </TableBody>
      </Table>
    </div>
    <small className="text-xs text-muted-foreground">事务 {plan.id}</small>
  </div>;
}

export function PlanDialog() {
  const review = useUI((s) => s.planReview);
  return review ? <PlanConfirmation key={review.plan.id} plan={review.plan} onApplied={review.onApplied}/> : null;
}
function PlanConfirmation({plan, onApplied}: {plan: FilePlan; onApplied?: () => void}) {
  const close = useUI((s) => s.closePlan);
  const notify = useUI((s) => s.notify);
  const client = useQueryClient();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  async function apply() {
    setBusy(true); setError(null);
    try {
      await api("apply_plan", {planId: plan.id});
      await client.invalidateQueries();
      onApplied?.(); close(); notify("文件事务已提交");
    } catch (failure) { setError(failure); }
    finally { setBusy(false); }
  }
  return <Modal title="确认文件变更" onClose={() => {if (!busy) close();}} actions={<>
    <Button variant="outline" disabled={busy} onClick={close}>取消</Button>
    <Button disabled={busy} onClick={() => { void apply(); }}>{busy ? <Spinner data-icon="inline-start"/> : <CheckIcon data-icon="inline-start"/>}批准并执行</Button>
  </>}>
    <PlanPreview plan={plan}/><ErrorState error={error}/>
  </Modal>;
}