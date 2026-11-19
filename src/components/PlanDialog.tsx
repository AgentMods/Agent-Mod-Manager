import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Check, LoaderCircle } from "lucide-react";
import type { FilePlan } from "../lib/types";
import { api } from "../lib/api";
import { useUI } from "../lib/store";
import { Badge, Button, bytes, ErrorState, Modal } from "./ui";

export function PlanPreview({plan}: {plan: FilePlan}) {
  const counts = plan.entries.reduce((sum, entry) => ({...sum, [entry.impact]: (sum[entry.impact] || 0) + 1}), {} as Record<string, number>);
  return <div className="plan-preview">
    <p>{plan.summary}</p><div className="toolbar">{Object.entries(counts).map(([kind, count]) => <Badge key={kind}>{kind}: {count}</Badge>)}<span>{bytes(plan.total_bytes)}</span></div>
    <div className="table-wrap"><table><thead><tr><th>操作</th><th>源文件 / 目标文件</th><th>原文件</th></tr></thead><tbody>
      {plan.entries.map((entry, index) => <tr key={`${index}-${entry.operation.dst}`}><td><Badge tone={entry.before ? "warning" : "success"}>{entry.operation.op}</Badge></td><td className="file-path">{entry.operation.src && <small>{entry.operation.src}</small>}<div>{entry.operation.dst}</div></td><td>{entry.before ? `${bytes(entry.before.size)} / 自动备份` : "新增"}</td></tr>)}
    </tbody></table></div><small className="muted">事务 {plan.id}</small>
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
  return <Modal title="确认文件变更" onClose={() => {if (!busy) close();}} actions={<><Button disabled={busy} onClick={close}>取消</Button><Button className="primary" disabled={busy} onClick={apply}>{busy ? <LoaderCircle size={16} className="spin"/> : <Check size={16}/>}批准并执行</Button></>}>
    <PlanPreview plan={plan}/><ErrorState error={error}/>
  </Modal>;
}
