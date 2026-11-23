import { ChevronRightIcon, FileIcon, FolderIcon } from "lucide-react";
import { Badge, PathText, bytes, date } from "@/components/ui";
import { Checkbox } from "@/components/ui/checkbox";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { displayPath } from "@/lib/paths";
import type { TrashItem } from "@/lib/types";

export function TrashFileTable({rows, gameNames, selected, disabled, onSelect, onSelectPage}: {
  rows: TrashItem[]; gameNames: Map<string, string>; selected: Set<string>; disabled: boolean;
  onSelect: (id: string) => void; onSelectPage: (checked: boolean) => void;
}) {
  const checked = rows.every((item) => selected.has(item.id)) ? true : rows.some((item) => selected.has(item.id)) ? "indeterminate" : false;
  return <Table aria-label="回收站文件" className="table-fixed">
    <colgroup>
      <col className="w-9"/><col/><col className="hidden w-28 md:table-column lg:w-32"/><col className="hidden w-36 lg:table-column"/>
      <col className="hidden w-20 sm:table-column"/><col className="w-[5.5rem]"/>
    </colgroup>
    <TableHeader className="bg-muted/30 [&_th]:h-8 [&_th]:text-xs [&_th]:text-muted-foreground"><TableRow>
      <TableHead className="pl-3"><Checkbox aria-label="选择本页全部文件" checked={checked} disabled={disabled} onCheckedChange={(value) => onSelectPage(value === true)}/></TableHead>
      <TableHead scope="col">名称 / 原始位置</TableHead><TableHead scope="col" className="hidden md:table-cell">所属游戏</TableHead>
      <TableHead scope="col" className="hidden lg:table-cell">移入时间</TableHead><TableHead scope="col" className="hidden sm:table-cell">大小</TableHead><TableHead scope="col">状态</TableHead>
    </TableRow></TableHeader>
    <TableBody className="[&_td]:py-1">{rows.map((item) => {
      const path = displayPath(item.original_path);
      const name = item.label || path;
      const game = item.game_id ? gameNames.get(item.game_id) ?? item.game_id : "未关联游戏";
      const ItemIcon = item.is_dir ? FolderIcon : FileIcon;
      return <TableRow key={item.id} data-state={selected.has(item.id) ? "selected" : undefined}>
        <TableCell className="pl-3"><Checkbox aria-label={`选择 ${name}`} checked={selected.has(item.id)} disabled={disabled} onCheckedChange={() => onSelect(item.id)}/></TableCell>
        <TableCell><details className="group/file">
          <summary className="flex cursor-pointer list-none items-center gap-2 rounded-sm focus-visible:outline-ring [&::-webkit-details-marker]:hidden" title="展开或收起文件详情">
            <ItemIcon aria-hidden="true" className="size-4 shrink-0 text-muted-foreground"/>
            <span className="min-w-0 flex-1">
              <span className="block truncate font-medium leading-[1.125rem]">{name}</span>
              <span className="flex items-baseline gap-2 leading-4">
                <PathText className="min-w-0 truncate" title={path}>{path}</PathText>
                {item.reason && <span className="max-w-[40%] shrink-0 truncate border-l pl-2 text-xs text-muted-foreground" title={item.reason}>{item.reason}</span>}
              </span>
            </span>
            <ChevronRightIcon aria-hidden="true" className="size-3 shrink-0 text-muted-foreground group-open/file:rotate-90"/>
          </summary>
          <div className="my-1.5 space-y-1 border-l pl-3 text-xs whitespace-normal text-muted-foreground">
            <p className="break-all font-medium text-foreground">名称：{name}</p>
            <p>原始位置：<PathText>{path}</PathText></p>
            {item.reason && <p className="break-all">移入原因：{item.reason}</p>}
            <p className="break-all lg:hidden">{game} · {date(item.created_at)} · {bytes(item.size)}</p>
          </div>
        </details></TableCell>
        <TableCell className="hidden truncate text-xs md:table-cell" title={game}>{game}</TableCell>
        <TableCell className="hidden text-xs tabular-nums lg:table-cell"><time dateTime={item.created_at}>{date(item.created_at)}</time></TableCell>
        <TableCell className="hidden text-xs tabular-nums sm:table-cell">{bytes(item.size)}</TableCell>
        <TableCell><Badge className="whitespace-nowrap" tone={item.exists ? "neutral" : "warning"}>{item.exists ? "在回收站" : "文件缺失"}</Badge></TableCell>
      </TableRow>;
    })}</TableBody>
  </Table>;
}
