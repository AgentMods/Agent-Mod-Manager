import { tr } from "@/lib/i18n";
import { useEffect, useState, type ReactNode } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpenIcon } from "lucide-react";
import { api, backendAvailable, useBackendQuery } from "@/lib/api";
import type { BootstrapState } from "@/lib/types";
import { appendPathLeaf, displayPath } from "@/lib/paths";
import { ErrorState, PathText } from "@/components/ui";
import { AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";

type StorageDirectoryProps = {
  compact?: boolean;
  onBusyChange?: (busy: boolean) => void;
  /** Record the raw bootstrap path before requesting restart, or clear it on failure. */
  onMigrationRequest?: (currentRoot: string | null) => void;
  renderExtraActions?: (disabled: boolean) => ReactNode;
};

export function StorageDirectory({ compact = false, onBusyChange, onMigrationRequest, renderExtraActions }: StorageDirectoryProps) {
  const bootstrap = useBackendQuery<BootstrapState>(["bootstrap"], "get_bootstrap_state");
  const currentRoot = bootstrap.data?.dataRoot;
  const previousError = bootstrap.data?.storageMigrationError;
  const [parent, setParent] = useState<string | null>(null);
  const [choosing, setChoosing] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const [storageError, setStorageError] = useState<unknown>(null);
  const busy = choosing || restarting;
  const disabled = !backendAvailable || !currentRoot || busy;

  useEffect(() => { onBusyChange?.(busy); }, [busy, onBusyChange]);

  async function chooseStorage() {
    if (disabled) return;
    setChoosing(true);
    setStorageError(null);
    try {
      const selected = await open({ directory: true, multiple: false, title: tr("选择存储位置") });
      if (typeof selected === "string" && selected) setParent(selected);
    } catch (error) {
      setStorageError(error);
    } finally {
      setChoosing(false);
    }
  }

  async function migrateStorage() {
    if (!parent || !currentRoot || restarting) return;
    setRestarting(true);
    setStorageError(null);
    try {
      onMigrationRequest?.(currentRoot);
      await api("migrate_storage", { parent });
      // A scheduled migration runs after restart. Keep the flow locked until then.
    } catch (error) {
      setStorageError(error);
      setRestarting(false);
      onMigrationRequest?.(null);
    }
  }

  const actions = <>
    <Button variant="outline" size={compact ? "sm" : "default"} className="group/btn bg-background/65 hover:bg-muted/60" disabled={disabled} onClick={chooseStorage}>
      <FolderOpenIcon data-icon="inline-start" className="motion-safe:transition-transform motion-safe:duration-200 group-hover/btn:-translate-y-0.5" />
      {tr("更改存储目录")}
    </Button>
    {renderExtraActions?.(disabled)}
  </>;

  return <div className={compact ? "flex min-w-0 flex-col items-start gap-2.5" : "flex min-w-0 flex-col items-start gap-4"}>
    {compact ? (
      <div className="group/dir flex min-w-0 flex-col gap-2.5 self-stretch rounded-xl border bg-muted/25 px-3.5 py-2.5 transition-colors duration-200 hover:border-foreground/20 hover:bg-muted/40 md:flex-row md:items-center md:justify-between">
        <div className="flex min-w-0 items-center gap-3">
          <span aria-hidden="true" className="flex size-8 shrink-0 items-center justify-center rounded-lg border bg-background/80 text-foreground/85 motion-safe:transition-transform motion-safe:duration-300 group-hover/dir:scale-110">
            <FolderOpenIcon className="size-4" />
          </span>
          <div className="grid min-w-0 gap-0.5">
            <span className="text-sm font-medium">{tr("当前存储目录")}</span>
            <PathText>{currentRoot ? displayPath(currentRoot) : tr("未连接桌面服务")}</PathText>
          </div>
        </div>
        <div className="flex shrink-0 flex-wrap items-center gap-2">
          {actions}
        </div>
      </div>
    ) : <>
      <div className="group/dir flex min-w-0 items-center gap-3.5 self-stretch rounded-xl border bg-muted/25 px-4 py-3.5 transition-colors duration-200 hover:border-foreground/20 hover:bg-muted/40">
        <span aria-hidden="true" className="flex size-9 shrink-0 items-center justify-center rounded-lg border bg-background/80 text-foreground/85 motion-safe:transition-transform motion-safe:duration-300 group-hover/dir:scale-110">
          <FolderOpenIcon className="size-4" />
        </span>
        <div className="grid min-w-0 gap-1">
          <span className="text-sm font-medium">{tr("当前存储目录")}</span>
          <PathText>{currentRoot ? displayPath(currentRoot) : tr("未连接桌面服务")}</PathText>
        </div>
      </div>
      <div className="flex flex-wrap gap-2.5">
        {actions}
      </div>
    </>}
    <ErrorState error={bootstrap.error || (previousError && tr("迁移失败，仍在使用原目录。{previousError}", { previousError })) || (!parent && storageError)} />
    <AlertDialog open={parent !== null} onOpenChange={(open) => { if (!open && !restarting) setParent(null); }}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{tr("迁移存储目录")}</AlertDialogTitle>
          <AlertDialogDescription>{tr("应用将重启并把管理器数据复制到下方的新目录。原目录会保留为备份。")}</AlertDialogDescription>
        </AlertDialogHeader>
        {parent && <div className="rounded-xl border bg-muted/40 px-3.5 py-2.5"><PathText>{displayPath(appendPathLeaf(parent, "Agent Mod Manager"))}</PathText></div>}
        <ErrorState error={storageError} />
        <AlertDialogFooter>
          <AlertDialogCancel disabled={restarting}>{tr("取消")}</AlertDialogCancel>
          <Button disabled={restarting || !currentRoot} onClick={migrateStorage}>
            {restarting && <Spinner data-icon="inline-start" />}
            {restarting ? tr("正在重启…") : tr("迁移并重启")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  </div>;
}
