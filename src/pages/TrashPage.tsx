import { tr } from "@/lib/i18n";
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { RefreshCwIcon, RotateCcwIcon, SearchIcon, Trash2Icon } from "lucide-react";
import { api, backendAvailable, useBackendQuery } from "@/lib/api";
import { displayPath } from "@/lib/paths";
import { useUI } from "@/lib/store";
import type { FilePlan, Game, TrashItem } from "@/lib/types";
import { EmptyState, ErrorState, IconButton, Page, PageHeader, bytes } from "@/components/ui";
import { TrashFileTable } from "@/components/TrashFileTable";
import { DEFAULT_TRASH_PAGE_SIZE, TrashPagination } from "@/components/TrashPagination";
import { TrashPurgeDialog, type PurgeRequest } from "@/components/TrashPurgeDialog";
import { Button } from "@/components/ui/button";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Spinner } from "@/components/ui/spinner";

// Desktop purge_trash validates at most 5,000 explicit IDs per call.
const PURGE_BATCH_SIZE = 5000;

export default function TrashPage() {
  const trash = useBackendQuery<TrashItem[]>(["trash"], "list_trash", {includeRestored: false});
  const games = useBackendQuery<Game[]>(["games"], "list_games");
  const client = useQueryClient();
  const reviewPlan = useUI((state) => state.reviewPlan);
  const notify = useUI((state) => state.notify);
  const [search, setSearch] = useState("");
  const [gameId, setGameId] = useState("");
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(DEFAULT_TRASH_PAGE_SIZE);
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [busy, setBusy] = useState<"restore" | "purge" | null>(null);
  const operationPending = useRef(false);
  const [error, setError] = useState<unknown>(null);
  const [purgeRequest, setPurgeRequest] = useState<PurgeRequest | null>(null);

  const items = trash.data ?? [];
  const gameNames = new Map((games.data ?? []).map((game) => [game.id, game.name]));
  const gameOptions = [...new Set(items.flatMap((item) => item.game_id ? [item.game_id] : []))];
  if (gameId && !gameOptions.includes(gameId)) gameOptions.push(gameId);
  const term = search.trim().toLocaleLowerCase();
  const filtered = items.filter((item) => (!gameId || item.game_id === gameId) && (!term ||
    [item.label, item.original_path, displayPath(item.original_path), item.reason, item.game_id && gameNames.get(item.game_id)]
      .some((value) => value?.toLocaleLowerCase().includes(term))));
  const pageCount = Math.max(1, Math.ceil(filtered.length / pageSize));
  const currentPage = Math.min(page, pageCount);
  const pageRows = filtered.slice((currentPage - 1) * pageSize, currentPage * pageSize);
  const selectedItems = items.filter((item) => selected.has(item.id));
  const selectedBytes = selectedItems.reduce((sum, item) => sum + item.size, 0);
  const disabled = !backendAvailable || !!busy || trash.isFetching || trash.isError;
  const restoreRequiresRollback = selectedItems.some((item) => item.restore_requires_rollback);
  const canRestore = !restoreRequiresRollback && selectedItems.length > 0 && selectedItems.every((item) => item.exists);

  useEffect(() => {
    setPage((previous) => Math.min(previous, pageCount));
  }, [pageCount]);

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

  function selectPage(checked: boolean) {
    setSelected((current) => {
      const next = new Set(current);
      for (const {id} of pageRows) {if (checked) next.add(id); else next.delete(id);}
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

  function requestPurge(scope: PurgeRequest["scope"]) {
    const targets = scope === "all" ? items : selectedItems;
    if (disabled || !targets.length || operationPending.current) return;
    // A later refresh must never add targets to an open confirmation.
    setPurgeRequest({scope, items: [...targets]});
  }

  async function purge() {
    if (!backendAvailable || !purgeRequest || operationPending.current) return;
    const ids = purgeRequest.items.map((item) => item.id);
    operationPending.current = true;
    setBusy("purge");
    setError(null);
    try {
      let freed = 0;
      for (let start = 0; start < ids.length; start += PURGE_BATCH_SIZE) {
        freed += await api<number>("purge_trash", {ids: ids.slice(start, start + PURGE_BATCH_SIZE), confirmation: "PURGE_TRASH"});
      }
      notify(tr("已永久删除 {length} 项，释放 {value2}", { length: ids.length, value2: bytes(freed) }));
    } catch (failure) {
      // Some entries may already be gone. Retrying requires reviewing a fresh list.
      setError(failure);
    } finally {
      await client.invalidateQueries({queryKey: ["trash"]});
      operationPending.current = false;
      setBusy(null);
      setPurgeRequest(null);
    }
  }

  const subtitle = !backendAvailable ? tr("桌面服务未连接") : trash.data ? tr("{length} 项 · {value2}", { length: items.length, value2: bytes(items.reduce((sum, item) => sum + item.size, 0)) }) : undefined;
  return <Page data-testid="trash-page" className="min-w-0 gap-4">
    <PageHeader title={tr("回收站")} subtitle={subtitle} actions={<>
      <IconButton title={tr("刷新回收站")} variant="outline" disabled={!backendAvailable || !!busy || trash.isFetching} onClick={() => {void trash.refetch();}}><RefreshCwIcon className={trash.isFetching ? "animate-spin" : undefined}/></IconButton>
      <Button variant="outline" size="sm" className="text-destructive hover:text-destructive" disabled={disabled || !items.length} onClick={() => requestPurge("all")}><Trash2Icon data-icon="inline-start"/>{tr("清空回收站")}</Button>
    </>}/>

    <div className="flex flex-wrap items-center gap-2">
      <InputGroup className="max-w-full flex-1 basis-56 sm:max-w-sm">
        <InputGroupAddon><SearchIcon/></InputGroupAddon>
        <InputGroupInput type="search" aria-label={tr("搜索回收站")} placeholder={tr("搜索文件或原始路径")} value={search} disabled={!backendAvailable} onChange={(event) => {setSearch(event.target.value); setPage(1);}}/>
      </InputGroup>
      <NativeSelect aria-label={tr("按游戏筛选回收站")} className="max-w-full sm:w-48" value={gameId} disabled={!backendAvailable} onChange={(event) => {setGameId(event.target.value); setPage(1);}}>
        <NativeSelectOption value="">{tr("全部游戏")}</NativeSelectOption>
        {gameOptions.map((id) => <NativeSelectOption key={id} value={id}>{gameNames.get(id) ?? id}</NativeSelectOption>)}
      </NativeSelect>
    </div>

    <ErrorState error={trash.error}/><ErrorState error={games.error}/><ErrorState error={error}/>
    {!backendAvailable ? <EmptyState title={tr("桌面服务未连接")}/> : trash.isLoading ?
      <div role="status" className="flex items-center gap-2 text-sm text-muted-foreground"><Spinner/>{tr("正在读取回收站…")}</div> :
      items.length > 0 ? <section aria-label={tr("回收站列表")} aria-busy={trash.isFetching || !!busy} className="min-w-0 overflow-hidden rounded-lg border">
        <div className="flex flex-wrap items-center gap-2 border-b bg-muted/40 px-3 py-2">
          <span className="mr-auto basis-full text-xs text-muted-foreground sm:basis-auto" role="status">{tr("已选择 {length} 项", { length: selectedItems.length })}{selectedItems.length > 0 ? ` · ${bytes(selectedBytes)}` : ""}</span>
          <Button size="sm" variant="outline" disabled={disabled || !canRestore} aria-describedby={restoreRequiresRollback ? "trash-restore-restriction" : undefined} onClick={() => {void restore();}}>
            {busy === "restore" ? <Spinner data-icon="inline-start"/> : <RotateCcwIcon data-icon="inline-start"/>}{tr("恢复所选")}
          </Button>
          <Button size="sm" variant="outline" disabled={disabled || !selectedItems.length} onClick={() => requestPurge("selected")}><Trash2Icon data-icon="inline-start"/>{tr("永久删除所选")}</Button>
          {restoreRequiresRollback && <p id="trash-restore-restriction" className="basis-full text-xs text-warning" role="status">{tr("所选项目包含被替换的 Mod 文件，请通过回滚替换事务恢复。")}</p>}
        </div>
        {filtered.length > 0 ? <>
          <TrashFileTable rows={pageRows} gameNames={gameNames} selected={selected} disabled={disabled} onSelect={toggleItem} onSelectPage={selectPage}/>
          <TrashPagination page={currentPage} pageSize={pageSize} total={filtered.length} onPageChange={setPage} onPageSizeChange={(size) => {setPageSize(size); setPage(1);}}/>
        </> : <EmptyState title={tr("没有匹配的文件")} action={<Button variant="outline" onClick={() => {setSearch(""); setGameId(""); setPage(1);}}><RotateCcwIcon data-icon="inline-start"/>{tr("重置筛选")}</Button>}/>}
      </section> : !trash.isError && <EmptyState title={tr("回收站为空")}/>}

    {purgeRequest && <TrashPurgeDialog request={purgeRequest} busy={!!busy} onClose={() => {if (!operationPending.current) setPurgeRequest(null);}} onConfirm={() => {void purge();}}/>}
  </Page>;
}
