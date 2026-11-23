import { useState } from "react";
import { Trash2Icon, TriangleAlertIcon } from "lucide-react";
import { Modal, PathText, bytes } from "@/components/ui";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { DEFAULT_TRASH_PAGE_SIZE, TrashPagination } from "@/components/TrashPagination";
import { displayPath } from "@/lib/paths";
import type { TrashItem } from "@/lib/types";

export type PurgeRequest = {scope: "selected" | "all"; items: TrashItem[]};

export function TrashPurgeDialog({request, busy, onClose, onConfirm}: {
  request: PurgeRequest; busy: boolean; onClose: () => void; onConfirm: () => void;
}) {
  const [acknowledged, setAcknowledged] = useState(false);
  const [page, setPage] = useState(1);
  const {items, scope} = request;
  const all = scope === "all";
  const size = DEFAULT_TRASH_PAGE_SIZE;
  return <Modal title={all ? "清空回收站" : "永久删除回收站文件"} onClose={onClose} className="sm:max-w-xl" actions={<>
    <Button variant="outline" disabled={busy} onClick={onClose}>取消</Button>
    <Button variant="destructive" disabled={busy || !acknowledged} onClick={() => {if (acknowledged && !busy) onConfirm();}}>
      {busy ? <Spinner data-icon="inline-start"/> : <Trash2Icon data-icon="inline-start"/>}{all ? "确认清空" : "永久删除"}
    </Button>
  </>}>
    <Alert variant="warning"><TriangleAlertIcon aria-hidden="true"/><AlertDescription>
      <p>将永久删除{all ? "回收站内全部" : "所选的"} {items.length} 项（{bytes(items.reduce((sum, item) => sum + item.size, 0))}）。此操作不可撤销。</p>
      {all && <p>包括所有游戏和所有分页中的文件，不受当前筛选或勾选范围限制。</p>}
      {items.some((item) => item.restore_requires_rollback) && <p>删除被替换的 Mod 文件后，相关替换事务也将无法回滚。</p>}
    </AlertDescription></Alert>
    <div className="min-w-0 rounded-md border">
      <ul className="max-h-44 divide-y overflow-y-auto text-sm break-all">
        {items.slice((page - 1) * size, page * size).map((item) => <li key={item.id} className="px-3 py-1.5">{item.label || displayPath(item.original_path)}<PathText className="block">{displayPath(item.original_path)}</PathText></li>)}
      </ul>
      {items.length > size && <TrashPagination page={page} pageSize={size} total={items.length} onPageChange={setPage}/>}
    </div>
    <Label className="font-normal leading-snug"><Switch checked={acknowledged} disabled={busy} onCheckedChange={setAcknowledged}/>
      <span>我确认永久删除以上文件，且无法再从回收站恢复。</span>
    </Label>
  </Modal>;
}
