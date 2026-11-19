import { useRef, useState, type KeyboardEvent } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Archive, ArchiveRestore, Clock3, LoaderCircle, MessageSquare, Pin, RefreshCw, Search, X } from "lucide-react";
import { api, backendAvailable, useBackendQuery } from "../lib/api";
import { useUI } from "../lib/store";
import type { Conversation, Game } from "../lib/types";
import { Badge, Button, EmptyState, ErrorState, IconButton, PageHeader, date } from "../components/ui";

export default function HistoryPage() {
  const [archived, setArchived] = useState(false);
  const [search, setSearch] = useState("");
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<unknown>(null);
  const operationPending = useRef(false);
  const activeTab = useRef<HTMLButtonElement>(null);
  const archivedTab = useRef<HTMLButtonElement>(null);
  const conversations = useBackendQuery<Conversation[]>(["conversations", archived], "list_conversations", {archived});
  const games = useBackendQuery<Game[]>(["games"], "list_games");
  const client = useQueryClient();
  const openConversation = useUI((state) => state.openConversation);
  const notify = useUI((state) => state.notify);
  const gameNames = new Map((games.data ?? []).map((game) => [game.id, game.name]));
  const term = search.trim().toLocaleLowerCase();
  const rows = (conversations.data ?? []).filter((conversation) =>
    [conversation.title, conversation.task_summary, conversation.game_id, conversation.game_id && gameNames.get(conversation.game_id)]
      .some((value) => value?.toLocaleLowerCase().includes(term)));

  function selectTab(next: boolean) {
    setArchived(next);
    setError(null);
  }

  function tabKeyDown(event: KeyboardEvent<HTMLButtonElement>) {
    let next: boolean;
    if (event.key === "ArrowRight" || event.key === "ArrowLeft") next = !archived;
    else if (event.key === "Home") next = false;
    else if (event.key === "End") next = true;
    else return;
    event.preventDefault();
    selectTab(next);
    (next ? archivedTab : activeTab).current?.focus();
  }

  async function toggleArchive(conversation: Conversation) {
    if (!backendAvailable || operationPending.current || conversations.isFetching || conversations.isError) return;
    const next = !conversation.archived;
    operationPending.current = true;
    setBusyId(conversation.id);
    setError(null);
    try {
      await api<void>("archive_conversation", {id: conversation.id, archived: next});
      await client.invalidateQueries({queryKey: ["conversations"]});
      await client.invalidateQueries({queryKey: ["conversation", conversation.id]});
      notify(next ? "对话已归档" : "对话已取消归档");
    } catch (failure) {
      setError(failure);
    } finally {
      operationPending.current = false;
      setBusyId(null);
    }
  }

  return <div className="page history-page" data-testid="history-page" style={{minWidth: 0, maxWidth: "100%"}}>
    <PageHeader title="历史对话" subtitle={!backendAvailable ? "桌面服务未连接" : conversations.data ? `${conversations.data.length} 个${archived ? "已归档" : "活动"}对话` : undefined}
      actions={<IconButton title="刷新历史对话" disabled={!backendAvailable || !!busyId || conversations.isFetching} onClick={() => {void conversations.refetch();}}><RefreshCw size={17} className={conversations.isFetching ? "spin" : ""}/></IconButton>}/>

    <div className="toolbar" style={{flexWrap: "wrap"}}>
      <div className="tabs" role="tablist" aria-label="对话状态" style={{display: "flex", maxWidth: "100%", margin: 0}}>
        <button ref={activeTab} type="button" role="tab" id="history-active-tab" aria-controls="history-panel" aria-selected={!archived} tabIndex={!archived ? 0 : -1}
          className={!archived ? "active" : ""} onClick={() => selectTab(false)} onKeyDown={tabKeyDown} style={{display: "inline-flex", alignItems: "center", gap: 7}}>
          <Clock3 size={16} aria-hidden="true"/>活动对话
        </button>
        <button ref={archivedTab} type="button" role="tab" id="history-archived-tab" aria-controls="history-panel" aria-selected={archived} tabIndex={archived ? 0 : -1}
          className={archived ? "active" : ""} onClick={() => selectTab(true)} onKeyDown={tabKeyDown} style={{display: "inline-flex", alignItems: "center", gap: 7}}>
          <Archive size={16} aria-hidden="true"/>已归档
        </button>
      </div>
      <span className="toolbar-spacer"/>
      <label className="search-field" style={{minWidth: 0, flex: "1 1 240px", maxWidth: "100%"}}>
        <Search size={17} aria-hidden="true"/>
        <input type="search" aria-label="搜索历史对话" placeholder="搜索标题、摘要或游戏" value={search} disabled={!backendAvailable} onChange={(event) => setSearch(event.target.value)} style={{minWidth: 0, width: "100%"}}/>
      </label>
    </div>

    <ErrorState error={conversations.error}/>
    <ErrorState error={games.error}/>
    <ErrorState error={error}/>
    <section id="history-panel" role="tabpanel" aria-labelledby={archived ? "history-archived-tab" : "history-active-tab"} aria-busy={conversations.isFetching || !!busyId} style={{minWidth: 0}}>
      {!backendAvailable ? <EmptyState title="桌面服务未连接"/> : conversations.isLoading ?
        <div className="loading-state" role="status" style={{display: "flex", alignItems: "center", gap: 8}}><LoaderCircle size={18} className="spin"/>正在读取历史对话…</div> :
        rows.length > 0 ? <div className="history-list" role="list" aria-label={archived ? "已归档对话" : "活动对话"}>
          {rows.map((conversation) => <article className="list-row history-row" role="listitem" key={conversation.id} style={{minWidth: 0, flexWrap: "wrap"}}>
            <button type="button" className="conversation-open grow" aria-label={`打开对话：${conversation.title || "未命名对话"}`} disabled={!backendAvailable}
              onClick={() => openConversation(conversation.id)}
              style={{display: "flex", alignItems: "flex-start", gap: 12, flex: "1 1 220px", minWidth: 0, maxWidth: "100%", padding: 0, border: 0, background: "transparent", color: "inherit", textAlign: "left", cursor: "pointer"}}>
              <MessageSquare size={19} aria-hidden="true" style={{flexShrink: 0, marginTop: 3}}/>
              <span style={{display: "block", minWidth: 0, overflowWrap: "anywhere"}}>
                <span className="conversation-title" style={{display: "flex", alignItems: "center", flexWrap: "wrap", gap: 8}}>
                  <strong>{conversation.title || "未命名对话"}</strong>
                  {conversation.pinned && <span title="已置顶" aria-label="已置顶"><Pin size={13} aria-hidden="true"/></span>}
                  {conversation.archived && <Badge>已归档</Badge>}
                </span>
                {conversation.task_summary && <span className="muted conversation-summary" style={{display: "block", marginTop: 4, whiteSpace: "normal", overflowWrap: "anywhere"}}>{conversation.task_summary}</span>}
                <span className="muted conversation-meta" style={{display: "flex", flexWrap: "wrap", gap: "4px 14px", marginTop: 6}}>
                  <span>{conversation.game_id ? gameNames.get(conversation.game_id) ?? conversation.game_id : "未关联游戏"}</span>
                  <span>{conversation.message_count} 条消息</span>
                  <time dateTime={conversation.updated_at}>{date(conversation.updated_at)}</time>
                </span>
              </span>
            </button>
            <Button title={`${conversation.archived ? "取消归档" : "归档"}：${conversation.title || "未命名对话"}`}
              disabled={!backendAvailable || !!busyId || conversations.isFetching || conversations.isError} onClick={() => {void toggleArchive(conversation);}}>
              {busyId === conversation.id ? <LoaderCircle size={16} className="spin"/> : conversation.archived ? <ArchiveRestore size={16}/> : <Archive size={16}/>}
              {conversation.archived ? "取消归档" : "归档"}
            </Button>
          </article>)}
        </div> : !conversations.isError && <EmptyState title={term ? "没有匹配的对话" : archived ? "暂无归档对话" : "暂无历史对话"}
          action={term ? <Button onClick={() => setSearch("")}><X size={16}/>清除搜索</Button> : undefined}/>}
    </section>
  </div>;
}
