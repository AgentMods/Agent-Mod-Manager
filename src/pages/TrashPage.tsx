import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, File, Folder, LoaderCircle, RefreshCw, RotateCcw, Search, Trash2 } from "lucide-react";
import { api, backendAvailable, useBackendQuery } from "../lib/api";
import { useUI } from "../lib/store";
import type { FilePlan, Game, TrashItem } from "../lib/types";
import { Badge, Button, EmptyState, ErrorState, IconButton, Modal, PageHeader, bytes, date } from "../components/ui";

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
  const selectAll = useRef<HTMLInputElement>(null);

  const items = trash.data ?? [];
  const gameNames = new Map((games.data ?? []).map((game) => [game.id, game.name]));
  const gameOptions = [...new Set(items.flatMap((item) => item.game_id ? [item.game_id] : []))];
  const term = search.trim().toLocaleLowerCase();
  const rows = items.filter((item) => (!gameId || item.game_id === gameId) &&
    [item.label, item.original_path, item.reason, item.game_id && gameNames.get(item.game_id)]
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

  useEffect(() => {
    if (selectAll.current) {
      selectAll.current.indeterminate = !allVisibleSelected && rows.some((item) => selected.has(item.id));
    }
  }, [allVisibleSelected, rows, selected]);

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

  return <div className="page trash-page" data-testid="trash-page" style={{minWidth: 0, maxWidth: "100%"}}>
    <PageHeader title="回收站" subtitle={!backendAvailable ? "桌面服务未连接" : trash.data ? `${items.length} 项 · ${bytes(items.reduce((sum, item) => sum + item.size, 0))}` : undefined}
      actions={<IconButton title="刷新回收站" disabled={!backendAvailable || !!busy || trash.isFetching} onClick={() => {void trash.refetch();}}><RefreshCw size={17} className={trash.isFetching ? "spin" : ""}/></IconButton>}/>

    <div className="toolbar" style={{flexWrap: "wrap"}}>
      <label className="search-field" style={{minWidth: 0, flex: "1 1 220px", maxWidth: "100%"}}>
        <Search size={17} aria-hidden="true"/>
        <input type="search" aria-label="搜索回收站" placeholder="搜索文件或原始路径" value={search} disabled={!backendAvailable} onChange={(event) => setSearch(event.target.value)} style={{minWidth: 0, width: "100%"}}/>
      </label>
      <select aria-label="按游戏筛选回收站" value={gameId} disabled={!backendAvailable} onChange={(event) => setGameId(event.target.value)} style={{minWidth: 0, maxWidth: "100%", flex: "0 1 180px"}}>
        <option value="">全部游戏</option>
        {gameOptions.map((id) => <option key={id} value={id}>{gameNames.get(id) ?? id}</option>)}
      </select>
    </div>

    <div className="toolbar selection-toolbar" style={{flexWrap: "wrap"}}>
      <span className="muted" role="status">已选择 {selectedItems.length} 项{selectedItems.length > 0 ? ` · ${bytes(selectedBytes)}` : ""}</span>
      <span className="toolbar-spacer"/>
      <Button className="primary" disabled={disabled || !canRestore}
        aria-describedby={restoreRequiresRollback ? "trash-restore-restriction" : undefined} onClick={() => {void restore();}}>
        {busy === "restore" ? <LoaderCircle size={16} className="spin"/> : <RotateCcw size={16}/>}恢复所选
      </Button>
      <Button className="danger" disabled={disabled || !selectedItems.length} onClick={requestPurge}><Trash2 size={16}/>永久删除所选</Button>
      {restoreRequiresRollback && <small id="trash-restore-restriction" className="warning-text" role="status" style={{flexBasis: "100%"}}>
        所选项目包含被替换的 Mod 文件，请通过回滚替换事务恢复。
      </small>}
    </div>

    <ErrorState error={trash.error}/>
    <ErrorState error={games.error}/>
    <ErrorState error={error}/>
    {!backendAvailable ? <EmptyState title="桌面服务未连接"/> : trash.isLoading ?
      <div className="loading-state" role="status" style={{display: "flex", alignItems: "center", gap: 8}}><LoaderCircle size={18} className="spin"/>正在读取回收站…</div> :
      rows.length > 0 ? <div className="table-wrap" style={{minWidth: 0, maxWidth: "100%", overflowX: "auto"}}>
        <table aria-label="回收站文件" aria-busy={trash.isFetching || !!busy} style={{width: "100%", minWidth: 800, tableLayout: "fixed"}}>
          <colgroup><col style={{width: 42}}/><col/><col style={{width: 140}}/><col style={{width: 155}}/><col style={{width: 90}}/><col style={{width: 105}}/></colgroup>
          <thead><tr>
            <th><input ref={selectAll} type="checkbox" aria-label="选择当前列表中的全部文件" checked={allVisibleSelected} disabled={disabled} onChange={() => setSelected((current) => {
              const next = new Set(current);
              for (const item of rows) {if (allVisibleSelected) next.delete(item.id); else next.add(item.id);}
              return next;
            })}/></th>
            <th scope="col">名称 / 原始位置</th><th scope="col">所属游戏</th><th scope="col">移入时间</th><th scope="col">大小</th><th scope="col">状态</th>
          </tr></thead>
          <tbody>{rows.map((item) => <tr key={item.id} className={selected.has(item.id) ? "selected" : ""}>
            <td><input type="checkbox" aria-label={`选择 ${item.label || item.original_path}`} checked={selected.has(item.id)} disabled={disabled} onChange={() => toggleItem(item.id)}/></td>
            <td style={{overflowWrap: "anywhere"}}>
              <div className="file-name" style={{display: "flex", alignItems: "center", gap: 8, minWidth: 0}}>
                {item.is_dir ? <Folder size={17} style={{flexShrink: 0}} aria-hidden="true"/> : <File size={17} style={{flexShrink: 0}} aria-hidden="true"/>}
                <strong>{item.label || item.original_path}</strong>
              </div>
              <small className="path-text" title={item.original_path} style={{display: "block", whiteSpace: "normal", overflowWrap: "anywhere"}}>{item.original_path}</small>
              {item.reason && <small className="muted" style={{display: "block", whiteSpace: "normal", overflowWrap: "anywhere"}}>{item.reason}</small>}
            </td>
            <td style={{overflowWrap: "anywhere"}}>{item.game_id ? gameNames.get(item.game_id) ?? item.game_id : "未关联游戏"}</td>
            <td><time dateTime={item.created_at}>{date(item.created_at)}</time></td>
            <td>{bytes(item.size)}</td>
            <td><Badge tone={item.exists ? "neutral" : "warning"}>{item.exists ? "在回收站" : "文件缺失"}</Badge></td>
          </tr>)}</tbody>
        </table>
      </div> : !trash.isError && <EmptyState title={term || gameId ? "没有匹配的文件" : "回收站为空"}
        action={term || gameId ? <Button onClick={() => {setSearch(""); setGameId("");}}><RotateCcw size={16}/>重置筛选</Button> : undefined}/>}

    {purgeItems && <Modal title="永久删除回收站文件" onClose={closePurge} actions={<>
      <Button disabled={!!busy} onClick={closePurge}>取消</Button>
      <Button className="danger" disabled={!backendAvailable || !!busy || !purgeAcknowledged || purgeText !== "PURGE_TRASH"} onClick={() => {void purge();}}>
        {busy === "purge" ? <LoaderCircle size={16} className="spin"/> : <Trash2 size={16}/>}永久删除
      </Button>
    </>}>
      <p className="warning-text">
        <AlertTriangle size={18} aria-hidden="true"/>将永久删除 {purgeItems.length} 项回收站文件。此操作不可撤销。
        {purgeItems.some((item) => item.restore_requires_rollback) && "删除被替换的 Mod 文件后，相关替换事务也将无法回滚。"}
      </p>
      <ul style={{paddingInlineStart: 20, maxHeight: 180, overflowY: "auto", overflowWrap: "anywhere"}}>
        {purgeItems.map((item) => <li key={item.id}>{item.label || item.original_path}<small className="path-text" style={{display: "block", whiteSpace: "normal", overflowWrap: "anywhere"}}>{item.original_path}</small></li>)}
      </ul>
      <label className="checkbox-field" style={{display: "flex", alignItems: "flex-start", gap: 8}}>
        <input type="checkbox" checked={purgeAcknowledged} disabled={!!busy} onChange={(event) => setPurgeAcknowledged(event.target.checked)} style={{flexShrink: 0}}/>
        <span>我确认永久删除以上文件，且无法再从回收站恢复。</span>
      </label>
      <label className="field" style={{minWidth: 0}}>输入 PURGE_TRASH 确认
        <input aria-label="永久删除确认文本" value={purgeText} disabled={!!busy} autoComplete="off" spellCheck={false} onChange={(event) => setPurgeText(event.target.value)} style={{width: "100%", minWidth: 0, boxSizing: "border-box"}}/>
      </label>
      <ErrorState error={purgeError}/>
    </Modal>}
  </div>;
}
