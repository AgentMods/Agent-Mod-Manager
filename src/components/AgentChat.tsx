import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { openUrl } from "@tauri-apps/plugin-opener";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { BotIcon, CheckIcon, ChevronDownIcon, MessageSquarePlusIcon, SendIcon, SquareIcon, TriangleAlertIcon, WrenchIcon } from "lucide-react";
import { api, backendAvailable, useBackendQuery } from "@/lib/api";
import { activeTurn, ensureAgentListeners, replayConversation, useAgent } from "../lib/agent-store";
import { reasoningEffortLabel } from "../lib/reasoning-effort";
import { useUI } from "../lib/store";
import type { AgentEvent, AppSettings, Conversation, Game, Message as ChatMessage, PermissionMode, ReasoningEffort } from "../lib/types";
import { Badge, EmptyState, ErrorState, IconButton } from "./ui";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Bubble, BubbleContent } from "@/components/ui/bubble";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Message, MessageAvatar, MessageContent, MessageHeader } from "@/components/ui/message";
import { InputGroup, InputGroupAddon, InputGroupTextarea } from "@/components/ui/input-group";
import { Label } from "@/components/ui/label";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Separator } from "@/components/ui/separator";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";

const EMPTY_EVENTS: AgentEvent[] = [];
export function AgentChat() {
  const conversationId = useUI((s) => s.conversationId);
  const gameId = useUI((s) => s.gameId);
  const setConversation = useUI((s) => s.setConversation);
  const client = useQueryClient();
  const [input, setInput] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [permission, setPermission] = useState<PermissionMode | "">("");
  const [effortSelection, setEffortSelection] = useState<{model: string; level: ReasoningEffort} | null>(null);
  const stream = useAgent((s) => conversationId ? s.streams[conversationId] || EMPTY_EVENTS : EMPTY_EVENTS);
  const turn = activeTurn(stream);
  const conversations = useBackendQuery<Conversation[]>(["conversations", false], "list_conversations", {archived: false});
  const detail = useBackendQuery<{conversation: Conversation; messages: ChatMessage[]}>(["conversation", conversationId], "get_conversation", {id: conversationId}, !!conversationId);
  const games = useBackendQuery<Game[]>(["games"], "list_games");
  const settings = useBackendQuery<AppSettings>(["settings"], "get_settings");
  const model = settings.data?.model ?? "";
  const levels = settings.data?.models?.find((entry) => entry.id === model)?.reasoning_levels ?? [];
  const reasoningEffort = effortSelection && effortSelection.model === model && levels.includes(effortSelection.level)
    ? effortSelection.level : null;
  const end = useRef<HTMLDivElement>(null);
  const busy = submitting || !!turn;

  useEffect(() => {
    if (!conversationId || !backendAvailable) return;
    let disposed = false;
    const refresh = () => replayConversation(conversationId).catch((failure) => {if (!disposed) setError(failure);});
    void refresh();
    const timer = setInterval(() => {void refresh();}, 2000);
    return () => { disposed = true; clearInterval(timer); };
  }, [conversationId]);
  useEffect(() => {
    if (!turn && conversationId && stream.length) {
      void client.invalidateQueries({queryKey: ["conversation", conversationId]});
      void client.invalidateQueries({queryKey: ["conversations"]});
    }
  }, [turn, conversationId, stream.length, client]);
  useEffect(() => {end.current?.scrollIntoView({block: "end", behavior: "auto"});}, [stream.length]);

  const {messages, errors, unlinkedErrors} = useMemo(() => {
    const all = (detail.data?.messages || []).map((message) => ({...message}));
    const byId = new Map(all.map((message) => [message.id, message]));
    const text = new Map<string, {content: string; reasoning: string}>();
    const lastReplyByTurn = new Map<string, string>();
    for (const event of stream) {
      if (event.kind !== "delta" || !event.messageId) continue;
      lastReplyByTurn.set(event.turnId, event.messageId);
      const accumulated = text.get(event.messageId) || {content: "", reasoning: ""};
      accumulated.content += typeof event.payload.content === "string" ? event.payload.content : "";
      accumulated.reasoning += typeof event.payload.reasoning === "string" ? event.payload.reasoning : "";
      text.set(event.messageId, accumulated);
      if (!byId.has(event.messageId)) {
        const message = {id: event.messageId, role: "assistant", content: "", reasoning: null, tool_calls: null, tool_call_id: null};
        all.push(message); byId.set(message.id, message);
      }
    }
    for (const [id, value] of text) Object.assign(byId.get(id)!, value);
    const messages = all.filter((message) => message.role === "user" || message.role === "assistant");
    const visibleIds = new Set(messages.map((message) => message.id));
    const errors = new Map<string, string>();
    const unlinkedErrors: {seq: number; message: string}[] = [];
    for (const event of stream) {
      if (event.kind !== "error" || typeof event.payload.message !== "string") continue;
      const id = event.messageId ?? lastReplyByTurn.get(event.turnId);
      if (id && visibleIds.has(id)) errors.set(id, event.payload.message);
      else unlinkedErrors.push({seq: event.seq, message: event.payload.message});
    }
    return {messages, errors, unlinkedErrors};
  }, [detail.data, stream]);

  async function send() {
    if (!input.trim() || busy) return;
    const content = input.trim();
    setSubmitting(true); setError(null);
    try {
      await ensureAgentListeners();
      let id = conversationId;
      if (!id) {
        const conversation = await api<Conversation>("create_conversation", {title: content.slice(0, 60), gameId});
        id = conversation.id; setConversation(id);
      }
      await replayConversation(id);
      await api<string>("start_turn", {request: {conversationId: id, content, permissionMode: permission || null, reasoningEffort}});
      setInput("");
      await replayConversation(id);
      await client.invalidateQueries({queryKey: ["conversation", id]});
      await client.invalidateQueries({queryKey: ["conversations"]});
    } catch (failure) {setError(failure);}
    finally {setSubmitting(false);}
  }
  async function cancel() {
    if (!turn || !conversationId) return;
    try { await api("cancel_turn", {conversationId, turnId: turn}); }
    catch (failure) {setError(failure);}
  }
  const usage = [...stream].reverse().find((event) => event.kind === "usage");
  const tools = stream.filter((event) => event.kind === "tool-start");
  const title = detail.data?.conversation.title || "Agent 对话";
  const gameName = games.data?.find((game) => game.id === (detail.data?.conversation.game_id || gameId))?.name || "全部游戏";
  const archivedConversation = !!detail.data?.conversation.archived;
  return <div className="flex h-full min-h-0 flex-col md:flex-row">
    <aside className="flex max-h-40 shrink-0 flex-col gap-2 overflow-y-auto border-b p-3 md:max-h-none md:w-60 md:border-r md:border-b-0">
      <div className="flex items-center justify-between gap-2 px-1">
        <h2 className="text-sm font-semibold">对话</h2>
        <IconButton title="新建对话" onClick={() => {setConversation(null); setError(null);}}><MessageSquarePlusIcon/></IconButton>
      </div>
      <div className="flex gap-1 md:flex-col">
        {(conversations.data || []).map((conversation) => <Button key={conversation.id} variant={conversation.id === conversationId ? "secondary" : "ghost"} size="sm"
          className="max-w-full shrink-0 justify-start md:w-full" onClick={() => {setConversation(conversation.id); setError(null);}}><span className="truncate">{conversation.title}</span></Button>)}
      </div>
      <ErrorState error={conversations.error}/>
    </aside>
    <section className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header className="chat-header flex items-center justify-between gap-3 border-b px-4 py-3 sm:px-6">
        <div className="flex min-w-0 flex-col">
          <h1 className="truncate text-lg font-semibold tracking-tight">{title}</h1>
          <span className="truncate text-xs text-muted-foreground">{gameName}</span>
        </div>
        <Badge tone={busy ? "warning" : "neutral"}>{busy ? "运行中" : model || "未配置模型"}</Badge>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto" aria-live="polite" aria-relevant="additions text">
        <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 px-4 py-6 sm:px-6">
          {!messages.length && !unlinkedErrors.length && <EmptyState icon={<BotIcon/>} title="今天想为游戏做些什么？"/>}
          {messages.map((message) => {
            const failure = errors.get(message.id);
            const isAssistant = message.role === "assistant";
            return <Fragment key={message.id}>
              <article className={`chat-message ${message.role}`}>
                <Message align={isAssistant ? "start" : "end"}>
                  {isAssistant && <MessageAvatar><Avatar className="size-8"><AvatarFallback><BotIcon className="size-4"/></AvatarFallback></Avatar></MessageAvatar>}
                  <MessageContent>
                    <MessageHeader className="px-1">{isAssistant ? "Agent" : "你"}</MessageHeader>
                    {message.reasoning && <Collapsible className="rounded-md border bg-muted/40 px-3 py-2 text-sm">
                      <CollapsibleTrigger className="flex w-full items-center justify-between gap-2 text-muted-foreground">思考过程<ChevronDownIcon className="size-3.5"/></CollapsibleTrigger>
                      <CollapsibleContent><p className="pt-2 text-muted-foreground break-words whitespace-pre-wrap">{message.reasoning}</p></CollapsibleContent>
                    </Collapsible>}
                    <Bubble variant={isAssistant ? "ghost" : "default"} align={isAssistant ? "start" : "end"} className={isAssistant ? "max-w-full" : undefined}>
                      <BubbleContent className={cn("markdown min-w-0 break-words", !isAssistant && "rounded-2xl px-4 py-2.5")}>
                        <div data-message-id={message.id} className="flex flex-col gap-3 [&_a]:text-primary [&_a]:underline [&_a]:underline-offset-4 [&_code]:rounded [&_code]:bg-muted [&_code]:px-1 [&_code]:font-mono [&_code]:text-xs [&_ol]:list-decimal [&_ol]:pl-5 [&_p]:leading-relaxed [&_pre]:max-w-full [&_pre]:overflow-auto [&_pre]:rounded-md [&_pre]:border [&_pre]:bg-muted [&_pre]:p-3 [&_pre_code]:bg-transparent [&_pre_code]:p-0 [&_table]:w-full [&_table]:border-collapse [&_td]:border [&_td]:px-2 [&_td]:py-1 [&_th]:border [&_th]:px-2 [&_th]:py-1 [&_th]:text-left [&_ul]:list-disc [&_ul]:pl-5">
                          <Markdown remarkPlugins={[remarkGfm]} components={{img: ({alt}) => <span>{alt || "远程图片"}</span>, a: ({children, href}) => href && /^https?:\/\//i.test(href) ? <a href={href} target="_blank" rel="noreferrer" onClick={(event) => {if (backendAvailable) {event.preventDefault(); void openUrl(href).catch(setError);}}}>{children}</a> : <span>{children}</span>}}>{message.content}</Markdown>
                        </div>
                      </BubbleContent>
                    </Bubble>
                    {tools.filter((event) => event.messageId === message.id).map((event) => <ToolCard key={event.seq} event={event} events={stream}/>)}
                    {isAssistant && <TurnError message={failure}/>}
                  </MessageContent>
                </Message>
              </article>
              {!isAssistant && failure && <div className="chat-message"><TurnError message={failure}/></div>}
            </Fragment>;
          })}
          {tools.filter((event) => !messages.some((message) => message.id === event.messageId)).map((event) => <ToolCard key={event.seq} event={event} events={stream}/>)}
          {unlinkedErrors.map(({seq, message}) => <div className="chat-message" key={seq}><TurnError message={message}/></div>)}
          {busy && <div className="flex items-center gap-2 text-sm text-muted-foreground"><Spinner/>正在处理</div>}
          <ErrorState error={error || detail.error}/><div ref={end}/>
        </div>
      </div>
      <form className="border-t p-3 sm:px-6" onSubmit={(event) => {event.preventDefault(); void send();}}>
        <div className="mx-auto flex w-full max-w-3xl flex-col gap-2">
          <InputGroup>
            <InputGroupTextarea aria-label="告诉 Agent 你想做什么" placeholder="帮我安装 Mod，或检查游戏为什么打不开…" value={input} onChange={(event) => setInput(event.target.value)} rows={3}
              disabled={!backendAvailable || archivedConversation}
              onKeyDown={(event) => {if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {event.preventDefault(); void send();}}}/>
            <InputGroupAddon align="block-end" className="flex-wrap gap-2">
              <NativeSelect size="sm" aria-label="本轮权限策略" value={permission} onChange={(event) => setPermission(event.target.value as PermissionMode | "")} disabled={busy}>
                <NativeSelectOption value="">继承会话设置</NativeSelectOption>
                <NativeSelectOption value="request_approval">逐次审批</NativeSelectOption>
                <NativeSelectOption value="auto_approve">游戏内自动批准</NativeSelectOption>
                <NativeSelectOption value="full_trust">完全信任</NativeSelectOption>
              </NativeSelect>
              {!!levels.length && <Label className="text-xs font-normal text-muted-foreground">
                <span>思考等级</span>
                <NativeSelect size="sm" aria-label="本轮思考等级" value={reasoningEffort ?? ""} disabled={busy || archivedConversation}
                  onChange={(event) => {
                    const level = levels.find((option) => option === event.target.value);
                    setEffortSelection(level ? {model, level} : null);
                  }}>
                  <NativeSelectOption value="">服务默认</NativeSelectOption>
                  {levels.map((level) => <NativeSelectOption key={level} value={level}>{reasoningEffortLabel(level)}</NativeSelectOption>)}
                </NativeSelect>
              </Label>}
              <span className="ml-auto text-xs text-muted-foreground">{usage ? `${Number(usage.payload.total_tokens || 0).toLocaleString()} tokens` : ""}</span>
              {turn ? <IconButton title="停止生成" variant="outline" onClick={() => {void cancel();}}><SquareIcon/></IconButton>
                : <IconButton title="发送消息" variant="default" type="submit" disabled={!backendAvailable || !input.trim() || busy}><SendIcon/></IconButton>}
            </InputGroupAddon>
          </InputGroup>
        </div>
      </form>
    </section>
  </div>;
}

function TurnError({message}: {message?: string}) {
  if (!message) return null;
  return <Alert variant="destructive" className="agent-turn-error">
    <TriangleAlertIcon aria-hidden="true"/>
    <AlertTitle>本轮回复未完成</AlertTitle>
    <AlertDescription className="min-w-0">
      <pre className="max-w-full overflow-auto font-mono text-xs break-all whitespace-pre-wrap"><code>{message}</code></pre>
    </AlertDescription>
  </Alert>;
}

function ToolCard({event, events}: {event: AgentEvent; events: AgentEvent[]}) {
  const result = events.find((item) => item.kind === "tool-result" && item.payload.toolCallId === event.payload.toolCallId);
  const approval = events.find((item) => item.kind === "approval-request" && item.payload.toolCallId === event.payload.toolCallId);
  const resolved = approval && events.find((item) => item.kind === "approval-resolved" && item.payload.approvalId === approval.payload.approvalId);
  return <details className="group rounded-md border bg-muted/40 text-sm">
    <summary className="flex cursor-pointer list-none items-center gap-2 px-3 py-2 [&::-webkit-details-marker]:hidden">
      <WrenchIcon className="size-3.5 text-muted-foreground"/><span className="min-w-0 flex-1 truncate font-medium">{String(event.payload.name)}</span>
      <Badge tone={result ? result.payload.status === "done" ? "success" : "danger" : "warning"}>{result ? <>{result.payload.status === "done" && <CheckIcon className="size-3"/>} {String(result.payload.status)}</> : approval && !resolved ? "等待审批" : "执行中"}</Badge>
      <ChevronDownIcon className="size-3.5 text-muted-foreground transition-transform group-open:rotate-180"/>
    </summary>
    <Separator/>
    <pre className="max-h-72 overflow-auto p-3 font-mono text-xs break-all whitespace-pre-wrap">{JSON.stringify({arguments: event.payload.arguments, ...(result ? {result: result.payload.result} : {})}, null, 2)}</pre>
  </details>;
}

export default AgentChat;