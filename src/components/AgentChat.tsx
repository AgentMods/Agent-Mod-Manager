import { useEffect, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { openUrl } from "@tauri-apps/plugin-opener";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Bot, Check, ChevronDown, LoaderCircle, MessageSquarePlus, Send, Square, Wrench } from "lucide-react";
import { api, backendAvailable, useBackendQuery } from "../lib/api";
import { activeTurn, ensureAgentListeners, replayConversation, useAgent } from "../lib/agent-store";
import { useUI } from "../lib/store";
import type { AgentEvent, AppSettings, Conversation, Game, Message, PermissionMode } from "../lib/types";
import { Badge, EmptyState, ErrorState, IconButton } from "./ui";

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
  const stream = useAgent((s) => conversationId ? s.streams[conversationId] || EMPTY_EVENTS : EMPTY_EVENTS);
  const turn = activeTurn(stream);
  const conversations = useBackendQuery<Conversation[]>(["conversations", false], "list_conversations", {archived: false});
  const detail = useBackendQuery<{conversation: Conversation; messages: Message[]}>(["conversation", conversationId], "get_conversation", {id: conversationId}, !!conversationId);
  const games = useBackendQuery<Game[]>(["games"], "list_games");
  const settings = useBackendQuery<AppSettings>(["settings"], "get_settings");
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

  const messages = useMemo(() => {
    const all = (detail.data?.messages || []).map((message) => ({...message}));
    const byId = new Map(all.map((message) => [message.id, message]));
    const text = new Map<string, {content: string; reasoning: string}>();
    for (const event of stream) {
      if (event.kind !== "delta" || !event.messageId) continue;
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
    return all.filter((message) => message.role === "user" || message.role === "assistant");
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
      await api<string>("start_turn", {request: {conversationId: id, content, permissionMode: permission || null}});
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
  return <div className="chat-layout">
    <aside className="conversation-list"><div className="section-heading"><h2>对话</h2><IconButton title="新建对话" onClick={() => {setConversation(null); setError(null);}}><MessageSquarePlus size={18}/></IconButton></div>
      {(conversations.data || []).map((conversation) => <button key={conversation.id} className={`conversation-item ${conversation.id === conversationId ? "active" : ""}`} onClick={() => {setConversation(conversation.id); setError(null);}}>{conversation.title}</button>)}
      <ErrorState error={conversations.error}/>
    </aside>
    <section className="chat-main"><header className="chat-header"><div><h1>{title}</h1><span className="muted">{games.data?.find((game) => game.id === (detail.data?.conversation.game_id || gameId))?.name || "全部游戏"}</span></div><Badge tone={busy ? "warning" : "neutral"}>{busy ? "运行中" : settings.data?.model || "未配置模型"}</Badge></header>
      <div className="chat-scroll" aria-live="polite" aria-relevant="additions text">
        {!messages.length && <EmptyState title="今天想为游戏做些什么？"/>}
        {messages.map((message) => <article className={`chat-message ${message.role}`} key={message.id}><div className="message-role">{message.role === "assistant" ? <Bot size={18}/> : null}{message.role === "user" ? "你" : "Agent"}</div>
          {message.reasoning && <details className="reasoning"><summary>思考过程 <ChevronDown size={14}/></summary><p>{message.reasoning}</p></details>}
          <div className="markdown" data-message-id={message.id}><Markdown remarkPlugins={[remarkGfm]} components={{img: ({alt}) => <span>{alt || "远程图片"}</span>, a: ({children, href}) => href && /^https?:\/\//i.test(href) ? <a href={href} target="_blank" rel="noreferrer" onClick={(event) => {if (backendAvailable) {event.preventDefault(); void openUrl(href).catch(setError);}}}>{children}</a> : <span>{children}</span>}}>{message.content}</Markdown></div>
          {tools.filter((event) => event.messageId === message.id).map((event) => <ToolCard key={event.seq} event={event} events={stream}/>)}
        </article>)}
        {tools.filter((event) => !messages.some((message) => message.id === event.messageId)).map((event) => <ToolCard key={event.seq} event={event} events={stream}/>)}
        {busy && <div className="stream-status"><LoaderCircle size={15} className="spin"/>正在处理</div>}
        <ErrorState error={error || detail.error}/><div ref={end}/>
      </div>
      <form className="chat-composer" onSubmit={(event) => {event.preventDefault(); void send();}}>
        <textarea aria-label="告诉 Agent 你想做什么" placeholder="帮我安装 Mod，或检查游戏为什么打不开…" value={input} onChange={(event) => setInput(event.target.value)} rows={3} disabled={!backendAvailable || !!detail.data?.conversation.archived} onKeyDown={(event) => {if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {event.preventDefault(); void send();}}}/>
        <div className="composer-toolbar"><select aria-label="本轮权限策略" value={permission} onChange={(event) => setPermission(event.target.value as PermissionMode | "")} disabled={busy}><option value="">继承会话设置</option><option value="request_approval">逐次审批</option><option value="auto_approve">游戏内自动批准</option><option value="full_trust">完全信任</option></select>
          <span className="muted usage">{usage ? `${Number(usage.payload.total_tokens || 0).toLocaleString()} tokens` : ""}</span>
          {turn ? <IconButton title="停止生成" onClick={() => {void cancel();}}><Square size={17}/></IconButton> : <IconButton title="发送消息" className="primary" type="submit" disabled={!backendAvailable || !input.trim() || busy}><Send size={18}/></IconButton>}
        </div>
      </form>
    </section>
  </div>;
}

function ToolCard({event, events}: {event: AgentEvent; events: AgentEvent[]}) {
  const result = events.find((item) => item.kind === "tool-result" && item.payload.toolCallId === event.payload.toolCallId);
  const approval = events.find((item) => item.kind === "approval-request" && item.payload.toolCallId === event.payload.toolCallId);
  const resolved = approval && events.find((item) => item.kind === "approval-resolved" && item.payload.approvalId === approval.payload.approvalId);
  return <details className="tool-card"><summary><Wrench size={15}/><span>{String(event.payload.name)}</span><Badge tone={result ? result.payload.status === "done" ? "success" : "danger" : "warning"}>{result ? <>{result.payload.status === "done" && <Check size={12}/>} {String(result.payload.status)}</> : approval && !resolved ? "等待审批" : "执行中"}</Badge></summary><pre className="json-preview">{JSON.stringify({arguments: event.payload.arguments, ...(result ? {result: result.payload.result} : {})}, null, 2)}</pre></details>;
}

export default AgentChat;
