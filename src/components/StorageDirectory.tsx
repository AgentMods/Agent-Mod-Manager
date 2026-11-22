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
  onBusyChange?: (busy: boolean) => void;
  /** Record the raw bootstrap path before requesting restart, or clear it on failure. */
  onMigrationRequest?: (currentRoot: string | null) => void;
  renderExtraActions?: (disabled: boolean) => ReactNode;
};

export function StorageDirectory({ onBusyChange, onMigrationRequest, renderExtraActions }: StorageDirectoryProps) {
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
      const selected = await open({ directory: true, multiple: false, title: "选择存储位置" });
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

  return <div className="flex min-w-0 flex-col items-start gap-4">
    <div className="grid min-w-0 gap-1.5 self-stretch rounded-lg border bg-muted/30 px-4 py-3">
      <span className="text-sm font-medium">当前存储目录</span>
      <PathText>{currentRoot ? displayPath(currentRoot) : "未连接桌面服务"}</PathText>
    </div>
    <div className="flex flex-wrap gap-2">
      <Button variant="outline" disabled={disabled} onClick={chooseStorage}><FolderOpenIcon data-icon="inline-start" />更改存储目录</Button>
      {renderExtraActions?.(disabled)}
    </div>
    <ErrorState error={bootstrap.error || (previousError && `迁移失败，仍在使用原目录。${previousError}`) || (!parent && storageError)} />
    <AlertDialog open={parent !== null} onOpenChange={(open) => { if (!open && !restarting) setParent(null); }}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>迁移存储目录</AlertDialogTitle>
          <AlertDialogDescription>应用将重启并把管理器数据复制到下方的新目录。原目录会保留为备份。</AlertDialogDescription>
        </AlertDialogHeader>
        {parent && <PathText>{displayPath(appendPathLeaf(parent, "Agent Mod Manager"))}</PathText>}
        <ErrorState error={storageError} />
        <AlertDialogFooter>
          <AlertDialogCancel disabled={restarting}>取消</AlertDialogCancel>
          <Button disabled={restarting || !currentRoot} onClick={migrateStorage}>
            {restarting && <Spinner data-icon="inline-start" />}
            {restarting ? "正在重启…" : "迁移并重启"}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  </div>;
}
