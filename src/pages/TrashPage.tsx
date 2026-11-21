import { displayPath } from "@/lib/paths";
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { FileIcon, FolderIcon, RefreshCwIcon, RotateCcwIcon, SearchIcon, Trash2Icon, TriangleAlertIcon } from "lucide-react";
import { api, backendAvailable, useBackendQuery } from "@/lib/api";
import { useUI } from "@/lib/store";
import type { FilePlan, Game, TrashItem } from "@/lib/types";
import { Badge, EmptyState, ErrorState, IconButton, Modal, Page, PageHeader, PathText, bytes, date } from "@/components/ui";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { Label } from "@/components/ui/label";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Spinner } from "@/components/ui/spinner";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";

export default function TrashPage() {
  const trash = useBackendQuery<TrashItem[]>(["trash"], "list_trash", {includeRestored: false});
  const games = useBackendQuery<Game[]>(["games"], "list_games");
  const client = useQueryClient();
  const reviewPlan = useUI((state) => state.reviewPlan);
  const notify = useUI((state) => state.notify);
  const [search, setSearch] = useState("");
  const [gameId, setGameId] = useState("");
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [busy, setBusy] = useState<"restore" | "purge" | null>(null);
  const operationPending = useRef(false);
  const [error, setError] = useState<unknown>(null);
  const [purgeItems, setPurgeItems] = useState<TrashItem[] | null>(null);
  const [purgeAcknowledged, setPurgeAcknowledged] = useState(false);
  const [purgeText, setPurgeText] = useState("");
  const [purgeError, setPurgeError] = useState<unknown>(null);

  const items = trash.data ?? [];
  const gameNames = new Map((games.data ?? []).map((game) => [game.id, game.name]));
  const gameOptions = [...new Set(items.flatMap((item) => item.game_id ? [item.game_id] : []))];
  const term = search.trim().toLocaleLowerCase();
  const rows = items.filter((item) => (!gameId || item.game_id === gameId) &&
    [item.label, item.original_path, displayPath(item.original_path), item.reason, item.game_id && gameNames.get(item.game_id)]
      .some((value) => value?.toLocaleLowerCase().includes(term)));
  const selectedItems = items.filter((item) => selected.has(item.id));
  const selectedBytes = selectedItems.reduce((sum, item) => sum + item.size, 0);
  const allVisibleSelected = rows.length > 0 && rows.every((item) => selected.has(item.id));
  const disabled = !backendAvailable || !!busy || trash.isFetching || trash.isError;
  const restoreRequiresRollback = selectedItems.some((item) => item.restore_requires_rollback);
  const canRestore = !restoreRequiresRollback && selectedItems.length > 0 && selectedItems.every((item) => item.exists);

  useEffect(() => {
    if (!trash.data) return;
    const ids = new Set(trash.data.map((item) => item.id));
    setSelected((current) => {
      const next = new Set([...current].filter((id) => ids.has(id)));
      return next.size === current.size ? current : next;
    });
  }, [trash.data]);


  function toggleItem(id: string) {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  }

  async function restore() {
    if (disabled || !canRestore || operationPending.current) return;
    const ids = selectedItems.map((item) => item.id);
    operationPending.current = true;
    setBusy("restore");
    setError(null);
    try {
      const plan = await api<FilePlan>("restore_trash", {ids, overwrite: false});
      // The shared review dialog is the only place that may apply this plan.
      reviewPlan(plan, () => {
        setSelected((current) => new Set([...current].filter((id) => !ids.includes(id))));
        void client.invalidateQueries({queryKey: ["trash"]});
      });
    } catch (failure) {
      setError(failure);
    } finally {
      operationPending.current = false;
      setBusy(null);
    }
  }

  function requestPurge() {
    if (disabled || !selectedItems.length || operationPending.current) return;
    // Freeze the targets so a refresh cannot change what the user confirms.
    setPurgeItems([...selectedItems]);
    setPurgeAcknowledged(false);
    setPurgeText("");
    setPurgeError(null);
  }

  function closePurge() {
    if (!operationPending.current) setPurgeItems(null);
  }

  async function purge() {
    if (!backendAvailable || !purgeItems?.length || !purgeAcknowledged ||
      purgeText !== "PURGE_TRASH" || operationPending.current) return;
    const ids = purgeItems.map((item) => item.id);
    operationPending.current = true;
    setBusy("purge");
    setPurgeError(null);
    try {
      const freed = await api<number>("purge_trash", {ids, confirmation: "PURGE_TRASH"});
      setSelected((current) => new Set([...current].filter((id) => !ids.includes(id))));
      setPurgeItems(null);
      notify(`已永久删除 ${ids.length} 项，释放 ${bytes(freed)}`);
    } catch (failure) {
      setPurgeError(failure);
    } finally {
      // Purge may have completed some entries before reporting a filesystem error.
      await client.invalidateQueries({queryKey: ["trash"]});
      operationPending.current = false;
      setBusy(null);
    }
  }

  const someVisibleSelected = !allVisibleSelected && rows.some((item) => selected.has(item.id));
  const subtitle = !backendAvailable ? "桌面服务未连接" : trash.data ? `${items.length} 项 · ${bytes(items.reduce((sum, item) => sum + item.size, 0))}` : undefined;
  return <Page data-testid="trash-page" className="min-w-0">
    <PageHeader title="回收站" subtitle={subtitle}
      actions={<IconButton title="刷新回收站" variant="outline" size="icon" disabled={!backendAvailable || !!busy || trash.isFetching} onClick={() => {void trash.refetch();}}><RefreshCwIcon className={trash.isFetching ? "animate-spin" : undefined}/></IconButton>}/>

    <div className="flex flex-wrap items-center gap-3">
      <InputGroup className="max-w-full flex-1 basis-56 sm:max-w-sm">
        <InputGroupAddon><SearchIcon/></InputGroupAddon>
        <InputGroupInput type="search" aria-label="搜索回收站" placeholder="搜索文件或原始路径" value={search} disabled={!backendAvailable} onChange={(event) => setSearch(event.target.value)}/>
      </InputGroup>
      <NativeSelect aria-label="按游戏筛选回收站" className="max-w-full sm:w-48" value={gameId} disabled={!backendAvailable} onChange={(event) => setGameId(event.target.value)}>
        <NativeSelectOption value="">全部游戏</NativeSelectOption>
        {gameOptions.map((id) => <NativeSelectOption key={id} value={id}>{gameNames.get(id) ?? id}</NativeSelectOption>)}
      </NativeSelect>
    </div>

    <div className="flex flex-wrap items-center gap-2 rounded-lg border bg-muted/40 px-4 py-3">
      <span className="text-sm text-muted-foreground" role="status">已选择 {selectedItems.length} 项{selectedItems.length > 0 ? ` · ${bytes(selectedBytes)}` : ""}</span>
      <span className="flex-1"/>
      <Button disabled={disabled || !canRestore} aria-describedby={restoreRequiresRollback ? "trash-restore-restriction" : undefined} onClick={() => {void restore();}}>
        {busy === "restore" ? <Spinner data-icon="inline-start"/> : <RotateCcwIcon data-icon="inline-start"/>}恢复所选
      </Button>
      <Button variant="destructive" disabled={disabled || !selectedItems.length} onClick={requestPurge}><Trash2Icon data-icon="inline-start"/>永久删除所选</Button>
      {restoreRequiresRollback && <p id="trash-restore-restriction" className="basis-full text-xs text-warning" role="status">
        所选项目包含被替换的 Mod 文件，请通过回滚替换事务恢复。
      </p>}
    </div>

    <ErrorState error={trash.error}/>
    <ErrorState error={games.error}/>
    <ErrorState error={error}/>
    {!backendAvailable ? <EmptyState title="桌面服务未连接"/> : trash.isLoading ?
      <div role="status" className="flex items-center gap-2 text-sm text-muted-foreground"><Spinner/>正在读取回收站…</div> :
      rows.length > 0 ? <div className="min-w-0 rounded-lg border">
        <Table aria-label="回收站文件" aria-busy={trash.isFetching || !!busy} className="min-w-[800px] table-fixed">
          <colgroup><col className="w-11"/><col/><col className="w-36"/><col className="w-40"/><col className="w-24"/><col className="w-28"/></colgroup>
          <TableHeader><TableRow>
            <TableHead><Checkbox aria-label="选择当前列表中的全部文件" checked={allVisibleSelected ? true : someVisibleSelected ? "indeterminate" : false} disabled={disabled} onCheckedChange={() => setSelected((current) => {
              const next = new Set(current);
              for (const item of rows) {if (allVisibleSelected) next.delete(item.id); else next.add(item.id);}
              return next;
            })}/></TableHead>
            <TableHead scope="col">名称 / 原始位置</TableHead><TableHead scope="col">所属游戏</TableHead><TableHead scope="col">移入时间</TableHead><TableHead scope="col">大小</TableHead><TableHead scope="col">状态</TableHead>
          </TableRow></TableHeader>
          <TableBody>{rows.map((item) => <TableRow key={item.id} data-state={selected.has(item.id) ? "selected" : undefined}>
            <TableCell><Checkbox aria-label={`选择 ${item.label || displayPath(item.original_path)}`} checked={selected.has(item.id)} disabled={disabled} onCheckedChange={() => toggleItem(item.id)}/></TableCell>
            <TableCell className="break-all whitespace-normal">
              <div className="flex min-w-0 items-center gap-2 font-medium">
                {item.is_dir ? <FolderIcon className="size-4 shrink-0 text-muted-foreground" aria-hidden="true"/> : <FileIcon className="size-4 shrink-0 text-muted-foreground" aria-hidden="true"/>}
                <span>{item.label || displayPath(item.original_path)}</span>
              </div>
              <PathText className="block" title={displayPath(item.original_path)}>{displayPath(item.original_path)}</PathText>
              {item.reason && <span className="block text-xs text-muted-foreground">{item.reason}</span>}
            </TableCell>
            <TableCell className="break-all whitespace-normal">{item.game_id ? gameNames.get(item.game_id) ?? item.game_id : "未关联游戏"}</TableCell>
            <TableCell><time dateTime={item.created_at}>{date(item.created_at)}</time></TableCell>
            <TableCell>{bytes(item.size)}</TableCell>
            <TableCell><Badge tone={item.exists ? "neutral" : "warning"}>{item.exists ? "在回收站" : "文件缺失"}</Badge></TableCell>
          </TableRow>)}</TableBody>
        </Table>
      </div> : !trash.isError && <EmptyState title={term || gameId ? "没有匹配的文件" : "回收站为空"}
        action={term || gameId ? <Button variant="outline" onClick={() => {setSearch(""); setGameId("");}}><RotateCcwIcon data-icon="inline-start"/>重置筛选</Button> : undefined}/>}

    {purgeItems && <Modal title="永久删除回收站文件" onClose={closePurge} className="sm:max-w-xl" actions={<>
      <Button variant="outline" disabled={!!busy} onClick={closePurge}>取消</Button>
      <Button variant="destructive" disabled={!backendAvailable || !!busy || !purgeAcknowledged || purgeText !== "PURGE_TRASH"} onClick={() => {void purge();}}>
        {busy === "purge" ? <Spinner data-icon="inline-start"/> : <Trash2Icon data-icon="inline-start"/>}永久删除
      </Button>
    </>}>
      <Alert variant="warning">
        <TriangleAlertIcon aria-hidden="true"/>
        <AlertDescription>
          将永久删除 {purgeItems.length} 项回收站文件。此操作不可撤销。
          {purgeItems.some((item) => item.restore_requires_rollback) && "删除被替换的 Mod 文件后，相关替换事务也将无法回滚。"}
        </AlertDescription>
      </Alert>
      <ul className="flex max-h-44 list-disc flex-col gap-2 overflow-y-auto pl-5 text-sm break-all">
        {purgeItems.map((item) => <li key={item.id}>{item.label || displayPath(item.original_path)}<PathText className="block">{displayPath(item.original_path)}</PathText></li>)}
      </ul>
      <Label className="items-start font-normal">
        <Checkbox checked={purgeAcknowledged} disabled={!!busy} onCheckedChange={(checked) => setPurgeAcknowledged(checked === true)}/>
        <span>我确认永久删除以上文件，且无法再从回收站恢复。</span>
      </Label>
      <Field>
        <FieldLabel htmlFor="purge-confirm">输入 PURGE_TRASH 确认</FieldLabel>
        <Input id="purge-confirm" aria-label="永久删除确认文本" value={purgeText} disabled={!!busy} autoComplete="off" spellCheck={false} onChange={(event) => setPurgeText(event.target.value)}/>
      </Field>
      <ErrorState error={purgeError}/>
    </Modal>}
  </Page>;
}