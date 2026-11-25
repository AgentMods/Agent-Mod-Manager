import { ListTodoIcon } from "lucide-react";
import { tr } from "@/lib/i18n";
import { useBackendQuery } from "@/lib/api";
import { Modal, EmptyState, ErrorState } from "@/components/ui";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";

interface Entry {
  searchId: string;
  itemId: string;
  gameId: string;
  title: string;
}
export function BrowseTaskList({ close, select }: { close: () => void; select: (task: Entry) => void }) {
  const tasks = useBackendQuery<Entry[]>(["browse-install-tasks"], "list_browse_install_tasks");
  return (
    <Modal
      title={tr("安装任务")}
      description={tr("打开任务可查看安装进度、回答问题或确认文件操作。")}
      onClose={close}
      className="sm:max-w-lg"
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
          {tasks.data.map((task) => (
            <li key={JSON.stringify([task.searchId, task.itemId, task.gameId])}>
              <button
                type="button"
                className="flex w-full items-center gap-3 p-3 text-start text-sm hover:bg-accent"
                onClick={() => select(task)}
              >
                <ListTodoIcon className="size-4 shrink-0" />
                <span className="break-words">{task.title}</span>
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <EmptyState title={tr("暂无安装任务")} />
      )}
    </Modal>
  );
}
