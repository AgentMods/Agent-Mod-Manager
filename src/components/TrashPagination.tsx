import { ChevronLeftIcon, ChevronRightIcon } from "lucide-react";
import { IconButton } from "@/components/ui";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";

export const DEFAULT_TRASH_PAGE_SIZE = 10;

export function TrashPagination({page, pageSize, total, onPageChange, onPageSizeChange}: {
  page: number; pageSize: number; total: number; onPageChange: (page: number) => void;
  onPageSizeChange?: (size: number) => void;
}) {
  const pages = Math.ceil(total / pageSize);
  return <nav aria-label="文件列表分页" className="flex flex-wrap items-center justify-between gap-2 border-t px-3 py-2 text-xs text-muted-foreground">
    <span role="status" className="tabular-nums">显示 {(page - 1) * pageSize + 1}–{Math.min(page * pageSize, total)} 项，共 {total} 项</span>
    <div className="ml-auto flex items-center gap-2">
      {onPageSizeChange && <NativeSelect size="sm" aria-label="每页显示数量" value={pageSize} onChange={(event) => onPageSizeChange(Number(event.target.value))}>
        {[DEFAULT_TRASH_PAGE_SIZE, 25, 50].map((size) => <NativeSelectOption key={size} value={size}>{size} 项 / 页</NativeSelectOption>)}
      </NativeSelect>}
      <IconButton title="上一页" variant="outline" disabled={page <= 1} onClick={() => onPageChange(page - 1)}><ChevronLeftIcon/></IconButton>
      <span className="tabular-nums" aria-label={`第 ${page} 页，共 ${pages} 页`}>{page} / {pages}</span>
      <IconButton title="下一页" variant="outline" disabled={page >= pages} onClick={() => onPageChange(page + 1)}><ChevronRightIcon/></IconButton>
    </div>
  </nav>;
}
