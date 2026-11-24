import { tr } from "@/lib/i18n";
import { displayPath } from "@/lib/paths";
import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangleIcon, CheckCircle2Icon } from "lucide-react";
import { api, backendAvailable } from "@/lib/api";
import { useUI } from "@/lib/store";
import type { GamePlanConflict } from "@/lib/types";
import { Badge, ErrorState, PathText } from "@/components/ui";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";

type GameChange = "remove" | "select_adapter";

export function useGameConflicts(gameId: string, change: GameChange) {
  const client = useQueryClient();
  const notify = useUI((state) => state.notify);
  const [resolving, setResolving] = useState<string | null>(null);
  const [error, setError] = useState<unknown>(null);
  const query = useQuery({
    queryKey: ["game-plan-conflicts", gameId, change],
    queryFn: () => api<GamePlanConflict[]>("list_game_conflicts", {gameId, change}),
    enabled: backendAvailable, staleTime: 0, retry: false,
  });

  async function refresh() {
    setError(null);
    await query.refetch();
  }

  async function resolve(conflict: GamePlanConflict, confirmApproved: boolean) {
    setResolving(conflict.id);
    setError(null);
    try {
      await api("resolve_game_conflict", {gameId, change, conflictId: conflict.id, confirmApproved});
      notify(tr("已解除待处理任务"));
    } catch (failure) {
      setError(failure);
    } finally {
      // The same task can be visible in both dialogs. Recheck even on failure in case it already finished.
      await client.invalidateQueries({queryKey: ["game-plan-conflicts", gameId]});
      setResolving(null);
    }
  }

  return {change, query, resolving, error, refresh, resolve,
    canContinue: query.isSuccess && !query.isFetching && query.data.length === 0 && !resolving};
}

const statusLabels: Record<string, string> = {
  get planned() { return tr("待执行"); }, get pending() { return tr("待审批"); }, get approved() { return tr("已批准，待执行"); }, get applying() { return tr("执行中"); }, get rolling_back() { return tr("回滚中"); }, get recovery_failed() { return tr("需恢复"); },
};

function ConflictRow({conflict, disabled, resolving, onResolve}: {
  conflict: GamePlanConflict; disabled: boolean; resolving: boolean; onResolve: (confirmApproved: boolean) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const needsConfirmation = conflict.status === "approved";
  function release() {
    if (needsConfirmation && !confirming) { setConfirming(true); return; }
    setConfirming(false);
    onResolve(needsConfirmation);
  }
  return <li className="flex min-w-0 flex-col gap-2 rounded-md border bg-muted/20 p-3">
    <div className="flex flex-wrap items-center justify-between gap-2">
      <span className="min-w-0 wrap-anywhere text-sm font-medium">{conflict.title}</span>
      <Badge tone={conflict.kind === "recovery" ? "danger" : "warning"}>{statusLabels[conflict.status] ?? conflict.status}</Badge>
    </div>
    {conflict.summary && conflict.summary !== conflict.title && <p className="wrap-anywhere text-sm">{conflict.summary}</p>}
    {conflict.paths[0] && <PathText>{displayPath(conflict.paths[0])}</PathText>}
    <div className="flex flex-wrap items-center gap-2">
      <Button variant="ghost" size="sm" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>{expanded ? tr("收起") : tr("查看")}</Button>
      {conflict.canResolve && <Button variant="outline" size="sm" disabled={disabled} onClick={release}>
        {resolving && <Spinner data-icon="inline-start" />}{confirming ? tr("确认解除") : tr("解除")}
      </Button>}
      {confirming && <Button variant="ghost" size="sm" onClick={() => setConfirming(false)}>{tr("保留计划")}</Button>}
    </div>
    {conflict.resolutionHint && (!conflict.canResolve || confirming || expanded) && <p className="text-xs text-muted-foreground">{conflict.resolutionHint}</p>}
    {expanded && <div className="flex flex-col gap-1.5 border-t pt-2 text-sm">
      {conflict.detail && <p className="whitespace-pre-wrap wrap-anywhere">{conflict.detail}</p>}
      {conflict.paths.slice(1).map((path) => <PathText key={path}>{displayPath(path)}</PathText>)}
      <PathText>{tr("任务 ID：{id}", { id: conflict.id })}</PathText>
      {conflict.planId && conflict.planId !== conflict.id && <PathText>{tr("关联计划：{planId}", { planId: conflict.planId })}</PathText>}
    </div>}
  </li>;
}

export function GameConflictPanel({state, busy}: {state: ReturnType<typeof useGameConflicts>; busy: boolean}) {
  const {query, change, resolving} = state;
  const items = query.data ?? [];
  return <>
    <ErrorState error={state.error || query.error} />
    {query.isError && <Button variant="outline" size="sm" disabled={query.isFetching} onClick={() => void state.refresh()}>{tr("重新检查任务")}</Button>}
    {query.isLoading && <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground"><Spinner />{tr("正在检查待处理任务…")}</p>}
    {items.length > 0 && <section className="flex min-w-0 flex-col gap-2" aria-label={tr("待处理任务")}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="flex items-center gap-2 text-sm font-medium"><AlertTriangleIcon className="size-4 text-warning" />{change === "remove" ? tr("移除前需处理的任务") : tr("适配包选择已阻止")}（{items.length}）</h3>
        <Button variant="ghost" size="sm" disabled={busy || !!resolving || query.isFetching} onClick={() => void state.refresh()}>{tr("刷新")}</Button>
      </div>
      {items.some((task) => task.canResolve) && <p className="text-xs text-muted-foreground">{tr("解除将取消所选任务，保留磁盘上的文件。")}</p>}
      <ul className="flex max-h-80 min-w-0 flex-col gap-2 overflow-y-auto">
        {items.map((conflict) => <ConflictRow key={`${conflict.kind}-${conflict.id}`} conflict={conflict} disabled={busy || !!resolving || query.isFetching} resolving={resolving === conflict.id} onResolve={(confirm) => void state.resolve(conflict, confirm)} />)}
      </ul>
    </section>}
    {change === "select_adapter" && state.canContinue && <p className="flex items-center gap-2 text-sm text-muted-foreground"><CheckCircle2Icon className="size-4 text-success" />{tr("没有发现会阻止适配包选择的待处理任务。")}</p>}
  </>;
}
