import { conversationTitle } from "@/lib/conversation-groups";
import { tr } from "@/lib/i18n";
import { conversationCountSources } from "@/locales/plurals";
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
    [conversationTitle(conversation), conversation.task_summary, conversation.game_id, conversation.game_id && gameNames.get(conversation.game_id)]
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
      notify(next ? tr("对话已归档") : tr("对话已取消归档"));
    } catch (failure) {
      setError(failure);
    } finally {
      operationPending.current = false;
      setBusyId(null);
    }
  }

  const tab = archived ? "archived" : "active";
  return <Page data-testid="history-page" className="min-w-0">
    <PageHeader title={tr("历史对话")} subtitle={!backendAvailable ? tr("桌面服务未连接") : conversations.data ? tr(conversationCountSources[tab], { length: conversations.data.length }) : undefined}
      actions={<IconButton title={tr("刷新历史对话")} variant="outline" size="icon" disabled={!backendAvailable || !!busyId || conversations.isFetching} onClick={() => { void conversations.refetch(); }}><RefreshCwIcon className={conversations.isFetching ? "animate-spin" : undefined}/></IconButton>}/>

    <Tabs value={tab} onValueChange={(value) => { setArchived(value === "archived"); setError(null); }} className="min-w-0 gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <TabsList aria-label={tr("对话状态")}>
          <TabsTrigger value="active"><Clock3Icon/>{tr("活动对话")}</TabsTrigger>
          <TabsTrigger value="archived"><ArchiveIcon/>{tr("已归档")}</TabsTrigger>
        </TabsList>
        <InputGroup className="max-w-full flex-1 basis-60 sm:max-w-sm">
          <InputGroupAddon><SearchIcon/></InputGroupAddon>
          <InputGroupInput type="search" aria-label={tr("搜索历史对话")} placeholder={tr("搜索标题、摘要或游戏")} value={search} disabled={!backendAvailable} onChange={(event) => setSearch(event.target.value)}/>
        </InputGroup>
      </div>

      <ErrorState error={conversations.error}/>
      <ErrorState error={games.error}/>
      <ErrorState error={error}/>
      <TabsContent value={tab} id="history-panel" aria-busy={conversations.isFetching || !!busyId} className="min-w-0">
        {!backendAvailable ? <EmptyState title={tr("桌面服务未连接")}/> : conversations.isLoading ?
          <div role="status" className="flex items-center gap-2 text-sm text-muted-foreground"><Spinner/>{tr("正在读取历史对话…")}</div> :
          rows.length > 0 ? <ItemGroup className="gap-2" aria-label={archived ? tr("已归档对话") : tr("活动对话")}>
            {rows.map((conversation) => <Item key={conversation.id} variant="outline" role="listitem" className="min-w-0">
              <button type="button" aria-label={tr("打开对话：{value1}", { value1: conversationTitle(conversation) })} disabled={!backendAvailable} onClick={() => openConversation(conversation.id)}
                className="flex min-w-0 flex-[1_1_14rem] items-start gap-3 rounded-md text-left outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:opacity-50">
                <ItemMedia variant="icon"><MessageSquareIcon/></ItemMedia>
                <ItemContent className="min-w-0">
                  <ItemTitle className="flex-wrap break-words">
                    {conversationTitle(conversation)}
                    {conversation.pinned && <span title={tr("已置顶")} aria-label={tr("已置顶")}><PinIcon className="size-3.5 text-muted-foreground" aria-hidden="true"/></span>}
                    {conversation.archived && <Badge>{tr("已归档")}</Badge>}
                  </ItemTitle>
                  {conversation.task_summary && <ItemDescription className="line-clamp-none break-words">{conversation.task_summary}</ItemDescription>}
                  <span className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
                    <span>{conversation.game_id ? gameNames.get(conversation.game_id) ?? conversation.game_id : tr("未关联游戏")}</span>
                    <span>{tr("{message_count} 条消息", { message_count: conversation.message_count })}</span>
                    <time dateTime={conversation.updated_at}>{date(conversation.updated_at)}</time>
                  </span>
                </ItemContent>
              </button>
              <ItemActions>
                <Button variant="outline" size="sm" title={`${conversation.archived ? tr("取消归档") : tr("归档")}：${conversationTitle(conversation)}`}
                  disabled={!backendAvailable || !!busyId || conversations.isFetching || conversations.isError} onClick={() => { void toggleArchive(conversation); }}>
                  {busyId === conversation.id ? <Spinner data-icon="inline-start"/> : conversation.archived ? <ArchiveRestoreIcon data-icon="inline-start"/> : <ArchiveIcon data-icon="inline-start"/>}
                  {conversation.archived ? tr("取消归档") : tr("归档")}
                </Button>
              </ItemActions>
            </Item>)}
          </ItemGroup> : !conversations.isError && <EmptyState title={term ? tr("没有匹配的对话") : archived ? tr("暂无归档对话") : tr("暂无历史对话")}
            action={term ? <Button variant="outline" onClick={() => setSearch("")}><XIcon data-icon="inline-start"/>{tr("清除搜索")}</Button> : undefined}/>}
      </TabsContent>
    </Tabs>
  </Page>;
}