import { useState } from "react";
import { ChevronDownIcon } from "lucide-react";
import { Popover } from "radix-ui";
import { sourceStatus, type BrowseSource } from "@/lib/browse";
import { getLocale, tr } from "@/lib/i18n";
import { Button } from "./ui/button";
import { Checkbox } from "./ui/checkbox";

export function BrowseSourcePicker({ sources, selected, busy, refresh, apply }: {
  sources: BrowseSource[];
  selected: BrowseSource[];
  busy: boolean;
  refresh: () => void;
  apply: (ids: string[]) => void;
}) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState<string[]>([]);
  const available = sources.filter((s) => s.status === "available");
  const included = available.filter((s) => draft.includes(s.id)).map((s) => s.id);
  const selectedCount = available.filter((s) => selected.some((entry) => entry.id === s.id)).length;
  return (
    <Popover.Root open={open} onOpenChange={(next) => {
      if (next) {
        setDraft(selected.map((s) => s.id));
        refresh();
      }
      setOpen(next);
    }}>
      <Popover.Trigger asChild>
        <Button variant="outline" size="sm" className="sm:ms-auto" disabled={busy}
          aria-label={tr("选择来源")} aria-description={tr("已选 {length} / {value2}", { length: selectedCount, value2: available.length })}>
          {tr("选择来源")}
          {selectedCount < available.length && <span className="text-xs tabular-nums text-muted-foreground">{selectedCount.toLocaleString(getLocale())}/{available.length.toLocaleString(getLocale())}</span>}
          <ChevronDownIcon aria-hidden="true" />
        </Button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content align="end" sideOffset={6} collisionPadding={8} aria-label={tr("选择来源")}
          className="z-50 flex max-h-[var(--radix-popover-content-available-height)] w-64 max-w-[calc(100vw-1rem)] flex-col rounded-lg border bg-popover text-popover-foreground shadow-md">
          <div className="flex items-center justify-between gap-2 px-3 py-2">
            <span className="text-xs text-muted-foreground">{tr("已选 {length} / {value2}", { length: included.length, value2: available.length })}</span>
            <Button variant="ghost" size="xs" disabled={included.length === available.length} onClick={() => setDraft(available.map((s) => s.id))}>{tr("全选")}</Button>
          </div>
          <div className="min-h-0 overflow-y-auto border-y p-1">
            {sources.map((s) => (
              <label key={s.id} className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm has-enabled:cursor-pointer has-enabled:hover:bg-accent has-disabled:text-muted-foreground">
                <Checkbox aria-label={s.name} checked={included.includes(s.id)} disabled={s.status !== "available"}
                  onCheckedChange={(checked) => setDraft((previous) => checked === true ? [...previous, s.id] : previous.filter((id) => id !== s.id))} />
                <span className="min-w-0 break-words">{s.name}</span>
                {s.status !== "available" && <span className="ms-auto text-xs">{sourceStatus(s.status)}</span>}
              </label>
            ))}
          </div>
          <div className="flex justify-end gap-2 p-2">
            <Popover.Close asChild><Button variant="ghost" size="sm">{tr("取消")}</Button></Popover.Close>
            <Button size="sm" disabled={!included.length} onClick={() => {
              setOpen(false);
              apply(included);
            }}>{tr("应用")}</Button>
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
