import { Fragment, memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { openUrl } from "@tauri-apps/plugin-opener";
import Markdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { toast } from "sonner";
import {
  BotIcon,
  CheckIcon,
  CircleCheckIcon,
  CircleDotIcon,
  CopyIcon,
  FolderIcon,
  LoaderCircleIcon,
  MessageSquarePlusIcon,
  PencilIcon,
  RefreshCcwIcon,
  Settings2Icon,
  TriangleAlertIcon,
} from "lucide-react";
import { api, backendAvailable, useBackendQuery } from "@/lib/api";
import { activeTurn, isQuestionRequest, replayConversation, trackAgentTurn, useAgent } from "../lib/agent-store";
import { useUI } from "../lib/store";
import type { AgentEvent, AppSettings, ComposerPreferences, Conversation, Game, Message as ChatMessage, PermissionMode, ReasoningEffort } from "../lib/types";
import { Badge, ErrorState } from "./ui";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Bubble, BubbleContent } from "@/components/ui/bubble";
import { Button } from "@/components/ui/button";
import { CodeBlock, CodeBlockContent } from "@/components/ui/code-block";
import { Message, MessageAction, MessageContent, MessageFooter } from "@/components/ui/message";
import {
  Trace,
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceDescription,
  TraceIcon,
  TraceLine,
  TraceTitle,
} from "@/components/ui/trace";
import { cn } from "@/lib/utils";
import { AgentComposer } from "./AgentComposer";
import { AgentConversationList } from "./AgentConversationList";
import { AgentQuestionnaire, questionOutcome } from "./AgentQuestionnaire";

const EMPTY_EVENTS: AgentEvent[] = [];
const EMPTY_CONVERSATIONS: Conversation[] = [];

function groupByMessage<T>(items: T[], messageId: (item: T) => string | null) {
  const groups = new Map<string | null, T[]>();
  for (const item of items) {
    const id = messageId(item);
    const group = groups.get(id);
    if (group) group.push(item);
    else groups.set(id, [item]);
  }
  return groups;
}

export function AgentChat() {
  const conversationId = useUI((s) => s.conversationId);
  const adapterChat = useUI((s) => s.adapterChat);
  const selectedGameId = useUI((s) => s.gameId);
  const gameId = adapterChat ? adapterChat.gameId : selectedGameId;
  const setConversation = useUI((s) => s.setConversation);
  const client = useQueryClient();
  const [composer, setComposer] = useState({ seed: adapterChat, text: adapterChat?.prompt ?? "" });
  const [submitting, setSubmitting] = useState(false);
  const submittingRef = useRef(false);
  const composerInput = useRef<HTMLTextAreaElement>(null);
  const [edit, setEdit] = useState<{ conversationId: string; messageId: string; text: string } | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [permission, setPermission] = useState<PermissionMode | "">("");
  const [effortSelection, setEffortSelection] = useState<{ model: string; level: ReasoningEffort } | null>(null);
  const [modelSelection, setModelSelection] = useState("");
  const stream = useAgent((s) => conversationId ? s.streams[conversationId] || EMPTY_EVENTS : EMPTY_EVENTS);
  const milestones = useAgent((s) => conversationId ? s.milestones[conversationId] || EMPTY_EVENTS : EMPTY_EVENTS);
  const turn = activeTurn(milestones);
  const conversations = useBackendQuery<Conversation[]>(["conversations", false], "list_conversations", { archived: false });
  const detail = useBackendQuery<{ conversation: Conversation; messages: ChatMessage[] }>(["conversation", conversationId], "get_conversation", { id: conversationId }, !!conversationId);
  const games = useBackendQuery<Game[]>(["games"], "list_games");
  const settings = useBackendQuery<AppSettings>(["settings"], "get_settings");
  const preferences = useBackendQuery<ComposerPreferences>(["composer-preferences"], "get_composer_preferences");
  const model = settings.data?.model ?? "";
  const models = settings.data?.models ?? [];
  const selectedModel = models.some((entry) => entry.id === modelSelection) ? modelSelection : model;
  const levels = models.find((entry) => entry.id === selectedModel)?.reasoning_levels ?? [];
  const selectedModelLabel = models.find((entry) => entry.id === selectedModel)?.display_name || selectedModel;
  const latestUsage = useMemo(() => [...milestones].reverse().find((event) => event.kind === "usage")?.payload, [milestones]);
  const totalTokens = typeof latestUsage?.total_tokens === "number" ? latestUsage.total_tokens : undefined;
  const reasoningEffort = effortSelection && effortSelection.model === selectedModel && levels.includes(effortSelection.level)
    ? effortSelection.level : null;
  const end = useRef<HTMLDivElement>(null);
  const busy = submitting || !!turn;
  const activeEdit = edit?.conversationId === conversationId ? edit : null;
  const input = activeEdit?.text ?? composer.text;
  const restoredPreferences = useRef(false);

  // Restore the last-used model, reasoning level and permission policy once per
  // launch. Stale values fall back safely: `selectedModel` ignores unknown models
  // and `reasoningEffort` ignores levels the model no longer enables.
  useEffect(() => {
    const saved = preferences.data;
    if (!saved || restoredPreferences.current) return;
    restoredPreferences.current = true;
    if (saved.permission_mode) setPermission((current) => current || saved.permission_mode!);
    if (saved.model) {
      setModelSelection((current) => current || (saved.model === model ? "" : saved.model!));
      if (saved.reasoning_effort) setEffortSelection((current) => current ?? { model: saved.model!, level: saved.reasoning_effort! });
    }
  }, [preferences.data, model]);

  useEffect(() => {
    setComposer({ seed: adapterChat, text: adapterChat?.prompt ?? "" });
  }, [adapterChat]);

  useEffect(() => {
    end.current?.scrollIntoView({ block: "end", behavior: "auto" });
  }, [stream.length]);

  const { messages, visibleIds, errors, unlinkedErrors, streamingReplyId } = useMemo(() => {
    const all = (detail.data?.messages || []).map((message) => ({ ...message }));
    const byId = new Map(all.map((message) => [message.id, message]));
    const text = new Map<string, { content: string; reasoning: string }>();
    const lastReplyByTurn = new Map<string, string>();
    for (const event of stream) {
      if (!event.messageId) continue;
      if (event.kind === "reply-reset") {
        text.set(event.messageId, { content: "", reasoning: "" });
        continue;
      }
      if (event.kind !== "delta") continue;
      lastReplyByTurn.set(event.turnId, event.messageId);
      const accumulated = text.get(event.messageId) || { content: "", reasoning: "" };
      accumulated.content += typeof event.payload.content === "string" ? event.payload.content : "";
      accumulated.reasoning += typeof event.payload.reasoning === "string" ? event.payload.reasoning : "";
      text.set(event.messageId, accumulated);
      if (!byId.has(event.messageId)) {
        const message = { id: event.messageId, role: "assistant", content: "", reasoning: null, tool_calls: null, tool_call_id: null };
        all.push(message); byId.set(message.id, message);
      }
    }
    for (const [id, value] of text) {
      const message = byId.get(id);
      if (message) Object.assign(message, value);
    }
    const messages = all.filter((message) => message.role === "user" || message.role === "assistant");
    const visibleIds = new Set(messages.map((message) => message.id));
    const errors = new Map<string, string>();
    const unlinkedErrors: { seq: number; message: string }[] = [];
    for (const event of milestones) {
      if (event.kind !== "error" || typeof event.payload.message !== "string") continue;
      const id = event.messageId ?? lastReplyByTurn.get(event.turnId);
      if (id && visibleIds.has(id)) errors.set(id, event.payload.message);
      else unlinkedErrors.push({ seq: event.seq, message: event.payload.message });
    }
    return { messages, visibleIds, errors, unlinkedErrors, streamingReplyId: turn ? lastReplyByTurn.get(turn) : undefined };
  }, [detail.data, stream, milestones, turn]);

  const selectConversation = useCallback((id: string | null) => {
    setConversation(id);
    setEdit(null);
    setComposer({ seed: null, text: "" });
    setError(null);
  }, [setConversation]);

  function editMessage(message: ChatMessage) {
    if (!conversationId || busy) return;
    setEdit({ conversationId, messageId: message.id, text: message.content });
    composerInput.current?.focus();
  }

  async function send(content: string, options: { restart?: { messageId: string; edit: boolean }; questionnaire?: boolean } = {}) {
    const { restart, questionnaire } = options;
    if (busy || submittingRef.current || (!restart && !content) || archivedConversation) return false;
    submittingRef.current = true;
    setSubmitting(true); setError(null);
    // The native request may finish starting after the user opens a different chat.
    let selectedAtSubmit = conversationId;
    let started = false;
    const stillSelected = () => useUI.getState().conversationId === selectedAtSubmit &&
      useUI.getState().adapterChat === adapterChat;
    try {
      let id = conversationId;
      if (!id) {
        const conversation = await api<Conversation>("create_conversation", { title: "", gameId });
        id = conversation.id;
        if (stillSelected()) {
          setConversation(id, true);
          selectedAtSubmit = id;
        }
        void client.invalidateQueries({ queryKey: ["conversations"] });
      }
      const request = { conversationId: id, content, permissionMode: permission || null, model: selectedModel && selectedModel !== model ? selectedModel : null, reasoningEffort };
      if (restart) {
        id = await api<string>("restart_turn", { request, ...restart });
        trackAgentTurn(id);
      } else {
        const turnId = await api<string>("start_turn", { request });
        trackAgentTurn(id, turnId);
      }
      started = true;
      const remembered: ComposerPreferences = {
        model: selectedModel || null, reasoning_effort: reasoningEffort, permission_mode: permission || null,
      };
      client.setQueryData(["composer-preferences"], remembered);
      // Remembering the choice must never fail an already-started turn.
      api("set_composer_preferences", { preferences: remembered }).catch(() => undefined);
      if (stillSelected() && !questionnaire) {
        setConversation(id);
        setEdit(null);
        if (!restart || restart.edit) setComposer({ seed: adapterChat, text: "" });
      }
      await replayConversation(id);
      await client.invalidateQueries({ queryKey: ["conversation", id] });
      await client.invalidateQueries({ queryKey: ["conversations"] });
    } catch (failure) {
      if (questionnaire && !started) throw failure; // Keep the form and its answers available for retry.
      if (useUI.getState().view === "agent" && stillSelected()) setError(failure);
      else useUI.getState().notify(String(failure));
    }
    finally { submittingRef.current = false; setSubmitting(false); }
    return started; // A failed refresh cannot undo a successfully started turn.
  }

  async function cancel() {
    if (!turn || !conversationId) return;
    try { await api("cancel_turn", { conversationId, turnId: turn }); }
    catch (failure) { setError(failure); }
  }

  const archivedConversation = !!detail.data?.conversation.archived;
  const { questions, answering, tools, toolsByMessage, questionsByMessage } = useMemo(() => {
    const questions = milestones.filter(isQuestionRequest).map((request) => ({
      request,
      outcome: questionOutcome(request, milestones) ?? (archivedConversation ? { kind: "closed" as const, label: "对话已归档" } : null),
    }));
    const questionTools = new Set(questions.map(({ request }) => request.payload.toolCallId));
    const tools = milestones.filter((event) => event.kind === "tool-start" && !questionTools.has(event.payload.toolCallId));
    return {
      questions, tools, answering: questions.some(({ outcome }) => !outcome),
      toolsByMessage: groupByMessage(tools, (event) => event.messageId),
      questionsByMessage: groupByMessage(questions, ({ request }) => request.messageId),
    };
  }, [milestones, archivedConversation]);
  const renderQuestion = ({ request, outcome }: (typeof questions)[number]) => <AgentQuestionnaire
    key={String(request.payload.approvalId)} request={request} outcome={outcome} turnBusy={busy}
    onReanswer={backendAvailable && detail.data?.conversation.id === request.conversationId && !archivedConversation
      ? (content) => send(content, { questionnaire: true }) : undefined} />;
  const title = detail.data?.conversation.title || "Agent 对话";
  const conversationGame = detail.data ? detail.data.conversation.game_id : gameId;
  const gameName = adapterChat?.label ?? (games.data?.find((game) => game.id === conversationGame)?.name || "全部游戏");
  const hasMessages = messages.length > 0 || unlinkedErrors.length > 0 || questions.length > 0;

  return <div className="agent-chat-shell flex h-full min-h-0 flex-col overflow-hidden bg-background md:flex-row">
    <aside className="agent-chat-sidebar flex max-h-44 shrink-0 flex-col border-b bg-sidebar/70 md:max-h-none md:w-72 md:border-r md:border-b-0">
      <div className="flex items-center gap-2 border-b px-4 py-3">
        <div className="flex size-8 items-center justify-center rounded-lg bg-sidebar-primary text-sidebar-primary-foreground">
          <BotIcon aria-hidden="true" className="size-4" />
        </div>
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-semibold">Agent Mod Manager</p>
          <p className="truncate text-xs text-muted-foreground">本地工作区</p>
        </div>
      </div>
      <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-3">
        <Button className="w-full justify-start gap-2 rounded-lg" onClick={() => selectConversation(null)} disabled={submitting}>
          <MessageSquarePlusIcon data-icon="inline-start" />
          新建对话
        </Button>
        <AgentConversationList conversations={conversations.data ?? EMPTY_CONVERSATIONS} selectedId={conversationId}
          disabled={submitting} onSelect={selectConversation} />
        <div className="mt-auto hidden flex-col gap-2 border-t pt-3 md:flex">
          <div className="flex items-center gap-2 px-2 text-xs font-medium text-muted-foreground"><FolderIcon className="size-3.5" />工作区</div>
          <div className="flex items-center gap-2 rounded-md bg-sidebar-accent/60 px-2.5 py-2 text-sm">
            <span className="size-1.5 rounded-full bg-sidebar-primary" />
            <span className="min-w-0 flex-1 truncate">{gameName}</span>
            <Settings2Icon className="size-3.5 text-muted-foreground" />
          </div>
        </div>
      </div>
      <ErrorState error={conversations.error} className="m-3 mt-0" />
    </aside>

    <section className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header className="chat-header flex shrink-0 items-center justify-between gap-3 border-b px-4 py-3 sm:px-8">
        <div className="flex min-w-0 flex-col gap-0.5">
          <h1 className="truncate text-base font-semibold tracking-tight sm:text-lg">{title}</h1>
          <span className="truncate text-xs text-muted-foreground">{gameName}</span>
        </div>
        <div className="flex max-w-[45%] flex-col items-end gap-0.5">
          <Badge className="min-w-0" tone={busy ? "warning" : "neutral"}>
            <span className="truncate">{answering ? "等待你的回答" : busy ? "运行中" : selectedModelLabel || "未配置模型"}</span>
          </Badge>
          {totalTokens !== undefined && <span className="whitespace-nowrap text-xs text-muted-foreground tabular-nums">{totalTokens.toLocaleString()} tokens</span>}
        </div>
      </header>

      <div className="relative min-h-0 flex-1 overflow-y-auto" aria-live="polite" aria-relevant="additions text">
        <div className={cn("mx-auto flex min-h-full w-full max-w-4xl flex-col px-4 py-6 sm:px-8", hasMessages ? "gap-2" : "justify-center")}>
          {!hasMessages && <div className="mx-auto flex max-w-xl flex-col items-center gap-4 pb-32 text-center">
            <div className="flex size-14 items-center justify-center rounded-2xl border bg-card shadow-sm">
              <BotIcon aria-hidden="true" className="size-7 text-primary" />
            </div>
            <div className="flex flex-col gap-2">
              <h2 className="text-2xl font-semibold tracking-tight sm:text-3xl">Agent 对话</h2>
              <p className="text-sm leading-6 text-muted-foreground sm:text-base">与 Agent 协作，管理、分析并优化你的 Mod 工作流。</p>
            </div>
          </div>}

          {messages.map((message) => {
            const failure = errors.get(message.id);
            const isAssistant = message.role === "assistant";
            const hasContent = Boolean(message.content.trim());
            const canAct = backendAvailable && !!conversationId && !busy && !archivedConversation;
            return <Fragment key={message.id}>
              <article className={cn("chat-message", message.role)}>
                <Message align={isAssistant ? "start" : "end"}>
                  <MessageContent className={cn(isAssistant ? "max-w-3xl" : "max-w-[min(80%,36rem)]")}>
                    {message.reasoning && <ReasoningTrace reasoning={message.reasoning} streaming={message.id === streamingReplyId} />}
                    <Bubble variant={isAssistant ? "ghost" : "default"} align={isAssistant ? "start" : "end"} className={isAssistant ? "max-w-full" : undefined}>
                      <BubbleContent className={cn("markdown min-w-0 break-words", !isAssistant && "rounded-2xl px-4 py-2.5")}>
                        <div data-message-id={message.id} className="flex flex-col gap-3 [&_a]:text-primary [&_a]:underline [&_a]:underline-offset-4 [&_code]:rounded [&_code]:bg-muted [&_code]:px-1 [&_code]:font-mono [&_code]:text-xs [&_ol]:list-decimal [&_ol]:pl-5 [&_p]:leading-relaxed [&_pre]:max-w-full [&_pre]:overflow-auto [&_pre]:rounded-md [&_pre]:border [&_pre]:bg-muted [&_pre]:p-3 [&_pre_code]:bg-transparent [&_pre_code]:p-0 [&_table]:w-full [&_table]:border-collapse [&_td]:border [&_td]:px-2 [&_td]:py-1 [&_th]:border [&_th]:px-2 [&_th]:py-1 [&_th]:text-left [&_ul]:list-disc [&_ul]:pl-5">
                          <MessageMarkdown content={message.content} onError={setError} />
                        </div>
                      </BubbleContent>
                    </Bubble>
                    {toolsByMessage.get(message.id)?.map((event) => <ToolTrace key={event.seq} event={event} events={milestones} />)}
                    {questionsByMessage.get(message.id)?.map(renderQuestion)}
                    {isAssistant && <TurnError message={failure} />}
                    {(hasContent || failure) && <MessageFooter className="mt-1" role="group" aria-label="消息操作">
                      {hasContent && <CopyMessage content={message.content} />}
                      {!isAssistant && hasContent && <MessageAction label="编辑消息" disabled={!canAct} onClick={() => editMessage(message)}>
                        <PencilIcon aria-hidden="true" />
                      </MessageAction>}
                      {(isAssistant || failure) && <MessageAction label="重试" disabled={!canAct} onClick={() => void send("", { restart: { messageId: message.id, edit: false } })}>
                        <RefreshCcwIcon aria-hidden="true" />
                      </MessageAction>}
                    </MessageFooter>}
                  </MessageContent>
                </Message>
              </article>
              {!isAssistant && failure && <div className="chat-message"><TurnError message={failure} /></div>}
            </Fragment>;
          })}

          {tools.filter((event) => !event.messageId || !visibleIds.has(event.messageId)).map((event) => <ToolTrace key={event.seq} event={event} events={milestones} />)}
          {questions.filter(({ request }) => !request.messageId || !visibleIds.has(request.messageId)).map(renderQuestion)}
          {unlinkedErrors.map(({ seq, message }) => <div className="chat-message" key={seq}><TurnError message={message} /></div>)}
          {busy && !answering && <Trace className="max-w-3xl"><TraceLine status="running"><TraceIcon><LoaderCircleIcon className="animate-spin" /></TraceIcon><TraceTitle>正在处理请求</TraceTitle></TraceLine></Trace>}
          <ErrorState error={error || detail.error} />
          <div ref={end} />
        </div>
      </div>

      <AgentComposer
        input={input}
        inputRef={composerInput}
        onCancelEdit={activeEdit ? () => setEdit(null) : undefined}
        onInputChange={(text) => {
          if (activeEdit) setEdit({ ...activeEdit, text });
          else setComposer({ seed: adapterChat, text });
        }}
        onSend={() => {
          const text = input.trim();
          if (text) void send(text, activeEdit ? { restart: { messageId: activeEdit.messageId, edit: true } } : undefined);
        }}
        onCancel={turn ? () => { void cancel(); } : undefined}
        disabled={!backendAvailable || archivedConversation}
        busy={busy}
        permission={permission}
        inheritedPermission={detail.data?.conversation.permission_mode ?? settings.data?.permission_mode ?? "request_approval"}
        onPermissionChange={setPermission}
        model={selectedModel}
        models={models}
        onModelChange={(nextModel) => {
          setModelSelection(nextModel === model ? "" : nextModel);
          setEffortSelection(null);
        }}
        reasoningEffort={reasoningEffort}
        onReasoningChange={(level) => setEffortSelection(level ? { model: selectedModel, level } : null)}
      />
    </section>
  </div>;
}

// React Markdown parses synchronously; skip unchanged history when the turn or draft changes.
const MessageMarkdown = memo(function MessageMarkdown({ content, onError }: { content: string; onError: (error: unknown) => void }) {
  const components = useMemo<Components>(() => ({
    img: ({ alt }) => <span>{alt || "远程图片"}</span>,
    a: ({ children, href }) => href && /^https?:\/\//i.test(href) ? <a href={href} target="_blank" rel="noreferrer" onClick={(event) => {
      if (backendAvailable) {
        event.preventDefault();
        void openUrl(href).catch(onError);
      }
    }}>{children}</a> : <span>{children}</span>,
  }), [onError]);
  return <Markdown remarkPlugins={[remarkGfm]} components={components}>{content}</Markdown>;
});

function CopyMessage({ content }: { content: string }) {
  const [copied, setCopied] = useState(false);
  const reset = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(reset.current), []);

  async function copy() {
    try {
      await navigator.clipboard.writeText(content);
      setCopied(true);
      clearTimeout(reset.current);
      reset.current = setTimeout(() => setCopied(false), 2000);
    } catch {
      toast.error("复制失败，请重试");
    }
  }

  return <MessageAction label={copied ? "已复制" : "复制消息"} onClick={() => void copy()}>
    {copied ? <CheckIcon aria-hidden="true" /> : <CopyIcon aria-hidden="true" />}
  </MessageAction>;
}

function ReasoningTrace({ reasoning, streaming }: { reasoning: string; streaming: boolean }) {
  const [open, setOpen] = useState(streaming);

  useEffect(() => {
    if (!streaming) setOpen(false);
  }, [streaming]);

  return <Trace>
    <TraceCollapsible open={open} onOpenChange={setOpen}>
      <TraceCollapsibleTrigger>
        <TraceLine status={streaming ? "running" : undefined}>
          <TraceIcon>{streaming ? <LoaderCircleIcon className="animate-spin" /> : <CircleCheckIcon />}</TraceIcon>
          <TraceTitle>{streaming ? "思考中" : "思考过程"}</TraceTitle>
          <TraceDescription>{streaming ? "进行中" : "已完成"}</TraceDescription>
        </TraceLine>
      </TraceCollapsibleTrigger>
      <TraceCollapsibleContent className="text-sm whitespace-pre-wrap">{reasoning}</TraceCollapsibleContent>
    </TraceCollapsible>
  </Trace>;
}

function TurnError({ message }: { message?: string }) {
  if (!message) return null;
  return <Alert variant="destructive" className="agent-turn-error">
    <TriangleAlertIcon aria-hidden="true" />
    <AlertTitle>本轮回复未完成</AlertTitle>
    <AlertDescription className="min-w-0">
      <pre className="max-w-full overflow-auto font-mono text-xs break-all whitespace-pre-wrap"><code>{message}</code></pre>
    </AlertDescription>
  </Alert>;
}

const ToolTrace = memo(function ToolTrace({ event, events }: { event: AgentEvent; events: AgentEvent[] }) {
  const result = events.find((item) => item.kind === "tool-result" && item.payload.toolCallId === event.payload.toolCallId);
  const approval = events.find((item) => item.kind === "approval-request" && item.payload.toolCallId === event.payload.toolCallId);
  const resolved = approval && events.find((item) => item.kind === "approval-resolved" && item.payload.approvalId === approval.payload.approvalId);
  const resultStatus = String(result?.payload.status || "");
  const approvalStatus = String(resolved?.payload.status || "");
  const failed = resultStatus === "error";
  const denied = approvalStatus === "rejected" || approvalStatus === "cancelled";
  const pending = !!approval && !resolved && !result;
  const status = failed ? "error" : pending ? "pending" : !result && !denied ? "running" : undefined;
  const label = failed ? "执行失败" : denied ? "已拒绝" : pending ? "等待审批" : result ? "已执行" : "正在执行";
  const payload = useMemo(() => JSON.stringify({ arguments: event.payload.arguments, ...(result ? { result: result.payload.result } : {}) }, null, 2), [event, result]);
  return <Trace>
    <TraceCollapsible defaultOpen={pending || failed}>
      <TraceCollapsibleTrigger>
        <TraceLine status={status}>
          <TraceIcon>{failed ? <TriangleAlertIcon /> : pending || !result ? <CircleDotIcon /> : <CheckIcon />}</TraceIcon>
          <TraceTitle>{String(event.payload.name)}</TraceTitle>
          <TraceDescription>{label}</TraceDescription>
        </TraceLine>
      </TraceCollapsibleTrigger>
      <TraceCollapsibleContent>
        <CodeBlock size="sm" variant={failed ? "destructive" : "default"}>
          <CodeBlockContent wrap className="max-h-72">{payload}</CodeBlockContent>
        </CodeBlock>
      </TraceCollapsibleContent>
    </TraceCollapsible>
  </Trace>;
});

export default AgentChat;
