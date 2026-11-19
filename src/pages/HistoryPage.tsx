import { useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ArchiveIcon, ArchiveRestoreIcon, Clock3Icon, MessageSquareIcon, PinIcon, RefreshCwIcon, SearchIcon, XIcon } from "lucide-react";
import { api, backendAvailable, useBackendQuery } from "@/lib/api";
import { useUI } from "@/lib/store";
import type { Conversation, Game } from "@/lib/types";
import { Badge, EmptyState, ErrorState, IconButton, Page, PageHeader, date } from "@/components/ui";
import { Button } from "@/components/ui/button";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemMedia, ItemTitle } from "@/components/ui/item";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

export default function HistoryPage() {
  const [archived, setArchived] = useState(false);
  const [search, setSearch] = useState("");
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<unknown>(null);
  const operationPending = useRef(false);
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

  const tab = archived ? "archived" : "active";
  return <Page data-testid="history-page" className="min-w-0">
    <PageHeader title="历史对话" subtitle={!backendAvailable ? "桌面服务未连接" : conversations.data ? `${conversations.data.length} 个${archived ? "已归档" : "活动"}对话` : undefined}
      actions={<IconButton title="刷新历史对话" variant="outline" size="icon" disabled={!backendAvailable || !!busyId || conversations.isFetching} onClick={() => { void conversations.refetch(); }}><RefreshCwIcon className={conversations.isFetching ? "animate-spin" : undefined}/></IconButton>}/>

    <Tabs value={tab} onValueChange={(value) => { setArchived(value === "archived"); setError(null); }} className="min-w-0 gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <TabsList aria-label="对话状态">
          <TabsTrigger value="active"><Clock3Icon/>活动对话</TabsTrigger>
          <TabsTrigger value="archived"><ArchiveIcon/>已归档</TabsTrigger>
        </TabsList>
        <InputGroup className="max-w-full flex-1 basis-60 sm:max-w-sm">
          <InputGroupAddon><SearchIcon/></InputGroupAddon>
          <InputGroupInput type="search" aria-label="搜索历史对话" placeholder="搜索标题、摘要或游戏" value={search} disabled={!backendAvailable} onChange={(event) => setSearch(event.target.value)}/>
        </InputGroup>
      </div>

      <ErrorState error={conversations.error}/>
      <ErrorState error={games.error}/>
      <ErrorState error={error}/>
      <TabsContent value={tab} id="history-panel" aria-busy={conversations.isFetching || !!busyId} className="min-w-0">
        {!backendAvailable ? <EmptyState title="桌面服务未连接"/> : conversations.isLoading ?
          <div role="status" className="flex items-center gap-2 text-sm text-muted-foreground"><Spinner/>正在读取历史对话…</div> :
          rows.length > 0 ? <ItemGroup className="gap-2" aria-label={archived ? "已归档对话" : "活动对话"}>
            {rows.map((conversation) => <Item key={conversation.id} variant="outline" role="listitem" className="min-w-0">
              <button type="button" aria-label={`打开对话：${conversation.title || "未命名对话"}`} disabled={!backendAvailable} onClick={() => openConversation(conversation.id)}
                className="flex min-w-0 flex-[1_1_14rem] items-start gap-3 rounded-md text-left outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:opacity-50">
                <ItemMedia variant="icon"><MessageSquareIcon/></ItemMedia>
                <ItemContent className="min-w-0">
                  <ItemTitle className="flex-wrap break-words">
                    {conversation.title || "未命名对话"}
                    {conversation.pinned && <span title="已置顶" aria-label="已置顶"><PinIcon className="size-3.5 text-muted-foreground" aria-hidden="true"/></span>}
                    {conversation.archived && <Badge>已归档</Badge>}
                  </ItemTitle>
                  {conversation.task_summary && <ItemDescription className="line-clamp-none break-words">{conversation.task_summary}</ItemDescription>}
                  <span className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
                    <span>{conversation.game_id ? gameNames.get(conversation.game_id) ?? conversation.game_id : "未关联游戏"}</span>
                    <span>{conversation.message_count} 条消息</span>
                    <time dateTime={conversation.updated_at}>{date(conversation.updated_at)}</time>
                  </span>
                </ItemContent>
              </button>
              <ItemActions>
                <Button variant="outline" size="sm" title={`${conversation.archived ? "取消归档" : "归档"}：${conversation.title || "未命名对话"}`}
                  disabled={!backendAvailable || !!busyId || conversations.isFetching || conversations.isError} onClick={() => { void toggleArchive(conversation); }}>
                  {busyId === conversation.id ? <Spinner data-icon="inline-start"/> : conversation.archived ? <ArchiveRestoreIcon data-icon="inline-start"/> : <ArchiveIcon data-icon="inline-start"/>}
                  {conversation.archived ? "取消归档" : "归档"}
                </Button>
              </ItemActions>
            </Item>)}
          </ItemGroup> : !conversations.isError && <EmptyState title={term ? "没有匹配的对话" : archived ? "暂无归档对话" : "暂无历史对话"}
            action={term ? <Button variant="outline" onClick={() => setSearch("")}><XIcon data-icon="inline-start"/>清除搜索</Button> : undefined}/>}
      </TabsContent>
    </Tabs>
  </Page>;
}