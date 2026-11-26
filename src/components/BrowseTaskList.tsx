import { useId, useState } from "react";
import { ChevronRightIcon, ListTodoIcon } from "lucide-react";
import { tr } from "@/lib/i18n";
import { useBackendQuery } from "@/lib/api";
import {
  browseInstallIndexKey, browseInstallKey, browseInstallStatusLabel, browseInstallWaitingOn, useBrowseInstallTask,
  type BrowseInstallIndexItem,
} from "@/lib/browse-install";
import { Modal, EmptyState, ErrorState } from "@/components/ui";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import { BrowseInstallPanel } from "./BrowseInstallPanel";

interface TaskListProps {
  close: () => void;
  retrySearchId: string | null;
  retry: (task: BrowseInstallIndexItem) => Promise<unknown>;
  gameName: (id: string) => string;
}

export function BrowseTaskList({ close, retrySearchId, retry, gameName }: TaskListProps) {
  const [expandedTask, setExpandedTask] = useState<string | null>(null);
  const tasks = useBackendQuery<BrowseInstallIndexItem[]>([...browseInstallIndexKey], "list_browse_install_tasks");
  return (
    <Modal
      title={tr("安装任务")}
      description={tr("打开任务可查看安装进度、回答问题或确认文件操作。")}
      onClose={close}
      className="sm:max-w-xl"
    >
      <ErrorState error={tasks.error} />
      {tasks.isPending ? (
        <Spinner />
      ) : tasks.error ? (
        <Button variant="outline" onClick={() => void tasks.refetch()}>
          {tr("重试")}
        </Button>
      ) : tasks.data?.length ? (
        <ul className="flex flex-col divide-y rounded-md border">
          {tasks.data.map((task) => {
            const key = JSON.stringify(browseInstallKey(task, task.gameId));
            return <BrowseTaskRow
              key={key}
              task={task}
              game={gameName(task.gameId)}
              open={key === expandedTask}
              onOpenChange={(open) => setExpandedTask(open ? key : null)}
              onRetry={task.searchId === retrySearchId ? () => retry(task) : undefined}
            />;
          })}
        </ul>
      ) : (
        <EmptyState title={tr("暂无安装任务")} />
      )}
    </Modal>
  );
}

interface TaskRowProps {
  task: BrowseInstallIndexItem;
  game: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onRetry?: () => Promise<unknown>;
}

function BrowseTaskRow({ task, game, open, onOpenChange, onRetry }: TaskRowProps) {
  const descriptionId = useId();
  const state = useBrowseInstallTask(task, task.gameId);
  const { status, request } = state;
  const waiting = browseInstallWaitingOn(request);
  return <Collapsible asChild open={open} onOpenChange={onOpenChange}>
    <li className="min-w-0">
      <CollapsibleTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          className="h-auto w-full items-start justify-start gap-3 p-3 text-start whitespace-normal"
          aria-label={task.title}
          aria-describedby={descriptionId}
        >
          <ListTodoIcon className="mt-0.5 size-4 shrink-0" />
          <span className="flex min-w-0 flex-col gap-1">
            <span className="wrap-anywhere">{task.title}</span>
            <span id={descriptionId} className="flex flex-wrap items-center gap-2 text-xs wrap-anywhere text-muted-foreground">
              <span>{game}</span>
              {status && <Badge variant={waiting ? "warning" : status === "installed" ? "success" : "secondary"}>
                {browseInstallStatusLabel(status, waiting)}
              </Badge>}
            </span>
          </span>
          <ChevronRightIcon aria-hidden="true" className={cn("ms-auto mt-0.5 shrink-0 text-muted-foreground", open && "rotate-90")} />
        </Button>
      </CollapsibleTrigger>
      <CollapsibleContent className="px-3 pb-3">
        {open && <BrowseInstallPanel gameId={task.gameId} state={state} onRetry={onRetry} />}
      </CollapsibleContent>
    </li>
  </Collapsible>;
}
