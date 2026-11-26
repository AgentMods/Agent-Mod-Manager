import { useEffect, useId, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { CheckIcon, PackagePlusIcon, RotateCwIcon } from "lucide-react";
import { AgentConfirmation } from "./AgentConfirmation";
import { AgentQuestionnaire, describeQuestionnaire, respondToQuestionnaire } from "./AgentQuestionnaire";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Item, ItemActions, ItemContent, ItemMedia, ItemTitle } from "./ui/item";
import { Spinner } from "./ui/spinner";
import { ErrorState } from "./ui";
import { isQuestionRequest } from "@/lib/agent-store";
import {
  browseInstallStatusLabel, browseInstallWaitingOn, cancelBrowseInstall, type useBrowseInstallTask,
} from "@/lib/browse-install";
import { tr } from "@/lib/i18n";

interface BrowseInstallPanelProps {
  gameId: string;
  state: ReturnType<typeof useBrowseInstallTask>;
  /** A historical handle can resume its task, but cannot start a new attempt. */
  onRetry?: () => Promise<unknown>;
}

export function BrowseInstallPanel({ gameId, onRetry, state }: BrowseInstallPanelProps) {
  const client = useQueryClient();
  const heading = useId();
  const { query: taskQuery, task, status, done, events, request } = state;
  const [cancelling, setCancelling] = useState(false);
  const cancelInFlight = useRef(false);
  const [retrying, setRetrying] = useState(false);
  const [actionError, setActionError] = useState<unknown>(null);
  const turnId = task?.turnId ?? null;
  const installId = task?.installId ?? null;
  useEffect(() => {
    if (status !== "installed" || !installId) return;
    void client.invalidateQueries({ queryKey: ["installs", gameId] });
    void client.invalidateQueries({ queryKey: ["mod-library", gameId] });
    void client.invalidateQueries({ queryKey: ["games"] });
  }, [client, gameId, status, installId]);

  async function cancel() {
    if (!task || !turnId || cancelInFlight.current) return;
    cancelInFlight.current = true;
    setCancelling(true);
    setActionError(null);
    try {
      await cancelBrowseInstall(task);
      await taskQuery.refetch();
    } catch (error) {
      // A turn can finish while Cancel is in flight; consult its durable status first.
      const latest = await taskQuery.refetch();
      if (!latest.data || latest.data.status === "running") setActionError(error);
    } finally {
      cancelInFlight.current = false;
      setCancelling(false);
    }
  }

  async function retry() {
    if (!onRetry || retrying) return;
    setRetrying(true);
    setActionError(null);
    try {
      await onRetry();
    } catch (error) {
      setActionError(error);
    } finally {
      setRetrying(false);
    }
  }

  const questionnaire = request && isQuestionRequest(request) ? describeQuestionnaire(request, events, false) : null;
  const label = retrying ? tr("安装中…") : status ? browseInstallStatusLabel(status, browseInstallWaitingOn(request)) : null;
  const error = actionError ?? taskQuery.error;

  return <section aria-labelledby={heading} className="flex min-w-0 flex-col gap-2">
    <Item variant="outline" size="sm" className="min-w-0">
      <ItemMedia variant="icon">{status === "installed" ? <CheckIcon aria-hidden="true" /> : <PackagePlusIcon aria-hidden="true" />}</ItemMedia>
      <ItemContent className="min-w-0">
        <ItemTitle id={heading}>{tr("Agent 安装")}</ItemTitle>
        {label && <span role="status">
          <Badge variant={status === "installed" ? "success" : request ? "warning" : status === "failed" ? "danger" : "secondary"}>
            {(retrying || (status === "running" && !request)) && <Spinner aria-hidden="true" />}{label}
          </Badge>
        </span>}
      </ItemContent>
      <ItemActions className="ms-auto flex-wrap justify-end">
        {status === "running" && !questionnaire && <Button variant="outline" size="sm" type="button" disabled={cancelling || !turnId || done} onClick={() => void cancel()}>{tr("取消")}</Button>}
        {onRetry && status && status !== "running" && status !== "installed" && <Button size="sm" type="button" disabled={retrying || cancelling || taskQuery.isError} onClick={() => void retry()}>
          {tr("重新尝试")}
        </Button>}
        {taskQuery.isError && <Button size="sm" variant="ghost" type="button" aria-label={tr("刷新安装状态")} disabled={taskQuery.isFetching} onClick={() => void taskQuery.refetch()}><RotateCwIcon aria-hidden="true" /></Button>}
      </ItemActions>
      {!!error && <ErrorState className="basis-full" error={error} />}
    </Item>
    {(status === "unverified" || status === "failed") && <p className="text-sm text-muted-foreground">{tr("无法验证安装时，可在原作品页选择文件并手动导入。")}</p>}
    {request && (questionnaire ? questionnaire.valid && !questionnaire.outcome && <AgentQuestionnaire
      key={String(request.payload.approvalId)} questionnaire={questionnaire} disabled={cancelling}
      onSubmit={async (answers) => { await respondToQuestionnaire(questionnaire, answers); }} onCancel={() => void cancel()}
    /> : <AgentConfirmation key={String(request.payload.approvalId)} request={request} />)}
  </section>;
}
