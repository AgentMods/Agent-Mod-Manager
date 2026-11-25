import { useEffect, useId, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { CheckIcon, PackagePlusIcon, RotateCwIcon } from "lucide-react";
import { AgentConfirmation } from "./AgentConfirmation";
import { AgentQuestionnaire, describeQuestionnaire, respondToQuestionnaire } from "./AgentQuestionnaire";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Item, ItemActions, ItemContent, ItemMedia, ItemTitle } from "./ui/item";
import { Spinner } from "./ui/spinner";
import { ErrorState } from "./ui";
import { isQuestionRequest, syncAgentEvents, useAgent } from "@/lib/agent-store";
import { backendAvailable } from "@/lib/api";
import {
  browseInstallKey, browseInstallStatus, cancelBrowseInstall, readBrowseInstall, startBrowseInstall,
  type BrowseInstallStatus, type BrowseInstallTask, type BrowseInstallWork,
} from "@/lib/browse-install";
import { tr } from "@/lib/i18n";
import type { AgentEvent } from "@/lib/types";

export interface BrowseInstallPanelProps {
  work: BrowseInstallWork;
  gameId: string;
  /** A historical handle can resume its task, but cannot start a new attempt. */
  canStart?: boolean;
}

const EMPTY_EVENTS: AgentEvent[] = [];

function statusLabel(status: BrowseInstallStatus) {
  switch (status) {
    case "running": return tr("安装进行中");
    case "installed": return tr("安装已验证");
    case "unverified": return tr("未验证到安装结果");
    case "rejected": return tr("安装操作已拒绝");
    case "cancelled": return tr("安装已取消");
    case "interrupted": return tr("安装已中断");
    case "failed": return tr("安装失败");
  }
}

function activeApproval(events: AgentEvent[], turnId: string | null) {
  if (!turnId) return undefined;
  const turn = events.filter((event) => event.turnId === turnId);
  if (turn.some((event) => event.kind === "done")) return undefined;
  const resolved = new Set(turn.filter((event) => event.kind === "approval-resolved").map((event) => event.payload.approvalId));
  const completed = new Set(turn.filter((event) => event.kind === "tool-result").map((event) => event.payload.toolCallId));
  return [...turn].reverse().find((event) => event.kind === "approval-request" &&
    typeof event.payload.approvalId === "string" && !resolved.has(event.payload.approvalId) && !completed.has(event.payload.toolCallId));
}

/** The parent selects an allowed local game. Identity and evidence remain native. */
export function BrowseInstallPanel(props: BrowseInstallPanelProps) {
  return <InstallPanel key={JSON.stringify(browseInstallKey(props.work, props.gameId))} {...props} />;
}

function InstallPanel({ work, gameId, canStart = true }: BrowseInstallPanelProps) {
  const client = useQueryClient();
  const heading = useId();
  const key = browseInstallKey(work, gameId);
  const taskQuery = useQuery({
    queryKey: key,
    queryFn: () => readBrowseInstall(work, gameId),
    enabled: backendAvailable,
    retry: false,
    refetchInterval: (query) => query.state.data?.status === "running" ? 1500 : false,
  });
  const [action, setAction] = useState<"start" | "cancel" | null>(null);
  const actionInFlight = useRef(false);
  const [actionError, setActionError] = useState<unknown>(null);
  const [replayError, setReplayError] = useState<unknown>(null);
  const task = taskQuery.data;
  const status = task ? browseInstallStatus(task) : null;
  const conversationId = task?.conversationId ?? null;
  const turnId = task?.turnId ?? null;
  const events = useAgent((state) => conversationId ? state.milestones[conversationId] ?? EMPTY_EVENTS : EMPTY_EVENTS);
  const done = status === "running" && events.some((event) => event.kind === "done" && event.turnId === turnId);
  useEffect(() => {
    if (status !== "running" || !conversationId) return;
    return syncAgentEvents(conversationId, () => setReplayError(null), setReplayError);
  }, [status, conversationId]);
  useEffect(() => {
    if (done) void client.invalidateQueries({ queryKey: ["browse-install", work.searchId, work.itemId, gameId], exact: true });
  }, [done, client, work.searchId, work.itemId, gameId]);
  const installId = task?.installId ?? null;
  useEffect(() => {
    if (status !== "installed" || !installId) return;
    void client.invalidateQueries({ queryKey: ["installs", gameId] });
    void client.invalidateQueries({ queryKey: ["mod-library", gameId] });
    void client.invalidateQueries({ queryKey: ["games"] });
  }, [client, gameId, status, installId]);

  async function start() {
    if (!canStart || actionInFlight.current || status === "running" || status === "installed") return;
    actionInFlight.current = true;
    setAction("start");
    setActionError(null);
    setReplayError(null);
    try {
      await client.cancelQueries({ queryKey: key, exact: true });
      const task = await startBrowseInstall(work, gameId);
      client.setQueryData<BrowseInstallTask>(key, task);
      void client.invalidateQueries({ queryKey: ["browse-install-tasks"] });
    } catch (error) {
      setActionError(error);
    } finally {
      actionInFlight.current = false;
      setAction(null);
    }
  }

  async function cancel() {
    if (!task || !turnId || actionInFlight.current) return;
    actionInFlight.current = true;
    setAction("cancel");
    setActionError(null);
    try {
      await cancelBrowseInstall(task);
      await taskQuery.refetch();
    } catch (error) {
      // A turn can finish while Cancel is in flight; consult its durable status first.
      const latest = await taskQuery.refetch();
      if (!latest.data || latest.data.status === "running") setActionError(error);
    } finally {
      actionInFlight.current = false;
      setAction(null);
    }
  }

  const request = status === "running" && !done ? activeApproval(events, turnId) : undefined;
  const questionnaire = request && isQuestionRequest(request) ? describeQuestionnaire(request, events, false) : null;
  const label = request ? (questionnaire ? tr("等待你回答") : tr("等待你审批")) : status ? statusLabel(status) : null;
  const error = actionError ?? (status === "running" ? replayError : null) ?? taskQuery.error;

  return <section aria-labelledby={heading} className="flex min-w-0 flex-col gap-2">
    <Item variant="outline" size="sm" className="min-w-0">
      <ItemMedia variant="icon">{status === "installed" ? <CheckIcon aria-hidden="true" /> : <PackagePlusIcon aria-hidden="true" />}</ItemMedia>
      <ItemContent className="min-w-0">
        <ItemTitle id={heading}>{tr("Agent 安装")}</ItemTitle>
        {label && <span role="status">
          <Badge variant={status === "installed" ? "success" : request ? "warning" : status === "failed" ? "danger" : "secondary"}>
            {status === "running" && !request && <Spinner aria-hidden="true" />}{label}
          </Badge>
        </span>}
      </ItemContent>
      <ItemActions className="ms-auto flex-wrap justify-end">
        {status === "running" && !questionnaire && <Button variant="outline" size="sm" type="button" disabled={!!action || !turnId || done} onClick={() => void cancel()}>{tr("取消")}</Button>}
        {canStart && status !== "running" && status !== "installed" && <Button size="sm" type="button" disabled={!backendAvailable || !!action || taskQuery.isPending || taskQuery.isError} onClick={() => void start()}>
          {action === "start" && <Spinner data-icon="inline-start" />}{status ? tr("重新尝试") : tr("开始安装")}
        </Button>}
        {taskQuery.isError && <Button size="sm" variant="ghost" type="button" aria-label={tr("刷新安装状态")} disabled={taskQuery.isFetching} onClick={() => void taskQuery.refetch()}><RotateCwIcon aria-hidden="true" /></Button>}
      </ItemActions>
      {!!error && <ErrorState className="basis-full" error={error} />}
    </Item>
    {(status === "unverified" || status === "failed") && <p className="text-sm text-muted-foreground">{tr("无法验证安装时，可在原作品页选择文件并手动导入。")}</p>}
    {request && (questionnaire ? questionnaire.valid && !questionnaire.outcome && <AgentQuestionnaire
      key={String(request.payload.approvalId)} questionnaire={questionnaire} disabled={!!action}
      onSubmit={async (answers) => { await respondToQuestionnaire(questionnaire, answers); }} onCancel={() => void cancel()}
    /> : <AgentConfirmation key={String(request.payload.approvalId)} request={request} />)}
  </section>;
}
