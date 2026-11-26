import { displayToolArguments, displayToolResult } from "@/lib/agent-path-presentation";
import { Fragment, memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { openUrl } from "@tauri-apps/plugin-opener";
import Markdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { toast } from "sonner";
import {
  BotIcon,
  CheckIcon,
  CircleCheckIcon,
  CircleDotIcon,
  CircleXIcon,
  CopyIcon,
  FolderIcon,
  LoaderCircleIcon,
  MessageSquarePlusIcon,
  PencilIcon,
  RefreshCcwIcon,
  Settings2Icon,
  TriangleAlertIcon,
} from "lucide-react";
import { api, backendAvailable, errorText, useBackendQuery } from "@/lib/api";
import { composerPreferencesQuery, rememberComposerPreferences, resolveComposerPermission, turnPermission } from "@/lib/composer-preferences";
import { activeTurnStart, describeAgentTools, isQuestionRequest, latestHistoryRewrite, pendingToolApproval, replayConversation, trackAgentTurn, useAgent, type AgentToolActivity } from "../lib/agent-store";
import { currentRoute } from "../router";
import { emptyDraft, useUI } from "../lib/store";
import type { AgentAttachment, AgentEvent, AppSettings, ComposerPreferences, Conversation, Game, GameWorkspace, Message as ChatMessage, PromptOrigin, ReasoningEffort } from "../lib/types";
import { readyAttachment, usePrepareAttachments, type DraftAttachment } from "@/lib/agent-attachments";
import { AttachmentGroup } from "@/components/ui/attachment";
import { MessageAttachment } from "./AgentAttachments";
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
import { tr, useLocale } from "@/lib/i18n";
import { ModRequestCard } from "./ModRequestCard";
import { parseBrowseInstallRequest, type BrowseInstallRequest } from "@/lib/browse-install-request";
import { parseModRequest, type ModRequest } from "@/lib/mod-request";
import { AgentConfirmation } from "./AgentConfirmation";
import { AgentWorkspaceDialog } from "./AgentWorkspaceDialog";
import { conversationTitle } from "@/lib/conversation-groups";
import { conversationWorkspace, workspaceLabel } from "@/lib/game-workspace";
import { AgentComposer } from "./AgentComposer";
import { AgentConversationList } from "./AgentConversationList";
import { AgentQuestionnaire, describeQuestionnaire, respondToQuestionnaire, QuestionnaireTrace, type Answer } from "./AgentQuestionnaire";
import { useDidChange } from "@/lib/motion";

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
  const locale = useLocale();
  const conversationId = useUI((s) => s.conversationId);
  const adapterChat = useUI((s) => s.adapterChat);
  const draftWorkspace = useUI((s) => s.draftWorkspace);
  const setDraftWorkspace = useUI((s) => s.setDraftWorkspace);
  const [picker, setPicker] = useState<{ id: string | null; adapter: typeof adapterChat; opener: HTMLButtonElement } | null>(null);
  const setConversation = useUI((s) => s.setConversation);
  const client = useQueryClient();
  const composer = useUI(s => s.composer);
  const setComposer = useUI(s => s.setComposer);
  const [submitting, setSubmitting] = useState<"start" | "restart" | null>(null);
  const submittingRef = useRef(false);
  const composerInput = useRef<HTMLTextAreaElement>(null);
  const edit = useUI(s => s.editDraft);
  const setEdit = useUI(s => s.setEditDraft);
  const [reanswerRequest, setReanswerRequest] = useState<AgentEvent | null>(null);
  const [submittedAnswers, setSubmittedAnswers] = useState(() => new Map<AgentEvent, Answer[]>());
  const [error, setError] = useState<unknown>(null);
  const permissionSelection = useUI((s) => s.composerPermission);
  const setPermission = useUI((s) => s.setComposerPermission);
  const [effortSelection, setEffortSelection] = useState<{ model: string; level: ReasoningEffort } | null>(null);
  const [modelSelection, setModelSelection] = useState("");
  const stream = useAgent((s) => conversationId ? s.streams[conversationId] || EMPTY_EVENTS : EMPTY_EVENTS);
  const milestones = useAgent((s) => conversationId ? s.milestones[conversationId] || EMPTY_EVENTS : EMPTY_EVENTS);
  const running = activeTurnStart(milestones);
  const turn = running?.turnId;
  const mode = running?.payload.permissionMode;
  const runningMode = mode === "request_approval" || mode === "auto_approve" || mode === "full_trust" ? mode : null;
  const conversations = useBackendQuery<Conversation[]>(["conversations", false], "list_conversations", { archived: false });
  const rewrite = useMemo(() => latestHistoryRewrite(milestones), [milestones]);
  const detail = useQuery({
    // A read issued before the rewrite committed must not repopulate the new snapshot.
    queryKey: ["conversation", conversationId, ...(rewrite ? [rewrite.seq] : [])],
    queryFn: () => api<{ conversation: Conversation; messages: ChatMessage[] }>("get_conversation", { id: conversationId }),
    enabled: backendAvailable && !!conversationId,
    retry: false,
    placeholderData: (previous) => {
      if (!rewrite || !previous || previous.conversation.id !== conversationId) return undefined;
      // The restart anchors the retained user message, whose text/attachments may
      // have been edited. Keep only the prefix that is known not to have changed.
      const target = previous.messages.findIndex(({ id }) => id === rewrite.messageId);
      return { ...previous, messages: previous.messages.slice(0, Math.max(0, target)) };
    },
  });
  const games = useBackendQuery<Game[]>(["games"], "list_games");
  const currentConversation = detail.data?.conversation ?? conversations.data?.find(row => row.id === conversationId);
  const workspace = conversationId ? (currentConversation ? conversationWorkspace(currentConversation) : undefined) : draftWorkspace;
  const gameName = workspace === undefined ? tr("正在读取工作区…") : workspaceLabel(workspace, games.data);
  const openPicker = (opener: HTMLButtonElement) => setPicker({ id: conversationId, adapter: adapterChat, opener });

  async function applyWorkspace(next: GameWorkspace) {
    if (!conversationId) { setDraftWorkspace(next); return; }
    const id = conversationId;
    const updated = await api<Conversation>("set_conversation_workspace", { id, gameIds: next });
    // Abandon older in-flight reads before publishing the committed native selection.
    const detailKey = ["conversation", id], listKey = ["conversations"];
    await Promise.all([client.cancelQueries({ queryKey: detailKey }), client.cancelQueries({ queryKey: listKey })]);
    client.setQueriesData<{conversation: Conversation; messages: ChatMessage[]}>({ queryKey: detailKey },
      previous => previous && { ...previous, conversation: updated });
    client.setQueriesData<Conversation[]>({ queryKey: listKey }, previous => previous?.map(row => row.id === id ? { ...row, ...updated } : row));
    void client.invalidateQueries({ queryKey: detailKey });
    void client.invalidateQueries({ queryKey: listKey });
  }

  const settings = useBackendQuery<AppSettings>(["settings"], "get_settings");
  const preferences = useQuery({ ...composerPreferencesQuery, enabled: backendAvailable });
  const permission = turnPermission(permissionSelection, preferences.data);
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
  const busy = !!submitting || !!turn;
  const workspaceDisabled = !backendAvailable || busy || workspace === undefined || !!currentConversation?.archived;
  const activeEdit = edit?.conversationId === conversationId ? edit : null;
  const draft = activeEdit ?? composer;
  function updateAttachments(apply: (items: DraftAttachment[]) => DraftAttachment[]) {
    if (activeEdit) setEdit((current) => current?.messageId === activeEdit.messageId ? { ...current, attachments: apply(current.attachments) } : current);
    else setComposer((current) => ({ ...current, attachments: apply(current.attachments) }));
  }
  usePrepareAttachments(draft.attachments, updateAttachments);
  const restoredPreferences = useRef(false);

  // Restore the last-used model and reasoning level once per
  // launch. Stale values fall back safely: `selectedModel` ignores unknown models
  // and `reasoningEffort` ignores levels the model no longer enables.
  useEffect(() => {
    const saved = preferences.data;
    if (!saved || restoredPreferences.current) return;
    restoredPreferences.current = true;
    if (saved.model) {
      setModelSelection((current) => current || (saved.model === model ? "" : saved.model!));
      if (saved.reasoning_effort) setEffortSelection((current) => current ?? { model: saved.model!, level: saved.reasoning_effort! });
    }
  }, [preferences.data, model]);

  useEffect(() => {
    end.current?.scrollIntoView({ block: "end", behavior: "auto" });
  }, [stream.length]);

  const modRequests = useMemo(() => {
    const requests = new Map<string, ModRequest | BrowseInstallRequest>();
    const history = detail.data?.messages ?? [];
    const opening = detail.data?.conversation.category === "install" ? history.find(message => message.role === "user") : undefined;
    for (const message of history) {
      const request = parseModRequest(message) ?? (message === opening ? parseBrowseInstallRequest(message) : null);
      if (request) requests.set(message.id, request);
    }
    return requests;
  }, [detail.data?.messages, detail.data?.conversation.category]);

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
    setReanswerRequest(null);
    setEdit(null);
    setComposer(emptyDraft());
    setError(null);
  }, [setConversation, setComposer, setEdit]);

  function editMessage(message: ChatMessage) {
    if (!conversationId || busy) return;
    setEdit({ conversationId, messageId: message.id, text: message.content, attachments: (message.attachments ?? []).map((value) => readyAttachment(value)) });
    composerInput.current?.focus();
  }

  async function send(content: string, options: { attachments?: AgentAttachment[]; restart?: { messageId: string; edit: boolean }; origin?: PromptOrigin } = {}) {
    const { attachments, restart, origin } = options;
    const questionnaire = origin?.kind === "questionnaire_answer";
    if (busy || submittingRef.current || (restart ? !conversationId : !content && !attachments?.length) || archivedConversation) return false;
    submittingRef.current = true;
    setSubmitting(restart ? "restart" : "start"); setError(null);
    // The native request may finish starting after the user opens a different chat.
    let selectedAtSubmit = conversationId;
    let started = false;
    const stillSelected = () => useUI.getState().conversationId === selectedAtSubmit &&
      useUI.getState().adapterChat === adapterChat;
    try {
      const { permissionMode: permission } = await resolveComposerPermission(client, permissionSelection);
      let id = conversationId;
      if (!id) {
        const conversation = await api<Conversation>("create_conversation", {
          title: "", gameId: draftWorkspace?.length === 1 ? draftWorkspace[0] : null,
          workspaceGameIds: draftWorkspace, ...(adapterChat && { category: "adapter" }),
        });
        id = conversation.id;
        client.setQueryData(["conversation", id], { conversation, messages: [] });
        if (stillSelected()) {
          setConversation(id, true);
          selectedAtSubmit = id;
        }
        void client.invalidateQueries({ queryKey: ["conversations"] });
      }
      const request = { conversationId: id, content, ...(attachments && { attachments }), permissionMode: permission, model: selectedModel && selectedModel !== model ? selectedModel : null, reasoningEffort };
      if (restart) {
        await api<string>("restart_turn", { request, ...restart });
        trackAgentTurn(id);
      } else {
        const turnId = await api<string>("start_turn", { request, ...(origin && { origin }) });
        trackAgentTurn(id, turnId);
      }
      started = true;
      const remembered: ComposerPreferences = {
        model: selectedModel || null, reasoning_effort: reasoningEffort, permission_mode: permission,
      };
      rememberComposerPreferences(client, remembered);
      if (stillSelected() && !questionnaire) {
        setEdit(null);
        // Restarting rewrites the current chat, not the unrelated unsent draft.
        if (!restart) {
          setConversation(id);
          setComposer(emptyDraft());
        }
      }
      await replayConversation(id, { afterPending: !!restart });
      await client.invalidateQueries({ queryKey: ["conversation", id] });
      await client.invalidateQueries({ queryKey: ["conversations"] });
    } catch (failure) {
      if (questionnaire && !started) throw failure; // Keep the form and its answers available for retry.
      if (currentRoute().view === "agent" && stillSelected()) setError(failure);
      else useUI.getState().notify(String(failure));
    }
    finally { submittingRef.current = false; setSubmitting(null); }
    return started; // A failed refresh cannot undo a successfully started turn.
  }

  async function cancel(turnId = turn) {
    if (!turnId || !conversationId) return;
    try { await api("cancel_turn", { conversationId, turnId }); }
    catch (failure) { setError(failure); }
  }

  const archivedConversation = !!detail.data?.conversation.archived;
  const { questions, pendingQuestion, tools, awaitingApproval, toolsByMessage, questionsByMessage } = useMemo(() => {
    const questions = milestones.filter(isQuestionRequest).map((request) =>
      describeQuestionnaire(request, milestones, archivedConversation, submittedAnswers.get(request)));
    const tools = describeAgentTools(milestones);
    return {
      questions, tools, pendingQuestion: questions.find(({ outcome, valid }) => !outcome && valid),
      awaitingApproval: tools.some(pendingToolApproval),
      toolsByMessage: groupByMessage(tools, ({ event }) => event.messageId),
      questionsByMessage: groupByMessage(questions, ({ request }) => request.messageId),
    };
  }, [milestones, archivedConversation, submittedAnswers, locale]);
  const writableChat = backendAvailable && detail.data?.conversation.id === conversationId && !archivedConversation;
  const activeQuestion = pendingQuestion ?? (writableChat ? questions.find(({ request, canReanswer }) => request === reanswerRequest && canReanswer) : undefined);
  const activeRequest = activeQuestion?.request;
  const previousRequest = useRef<AgentEvent | undefined>(undefined);
  useEffect(() => {
    if (!activeRequest && previousRequest.current?.conversationId === conversationId && document.activeElement === document.body) {
      composerInput.current?.focus({ preventScroll: true });
    }
    previousRequest.current = activeRequest;
  }, [activeRequest, conversationId]);

  async function submitAnswers(questionnaire: (typeof questions)[number], answers: Answer[]) {
    const { request, outcome, questions: steps, title } = questionnaire;
    if (outcome) {
      // Historical answers start a new turn in the same chat. The original approval is final.
      const lines = steps.map((item, index) => {
        const answer = answers[index];
        const reply = Array.isArray(answer) ? "\n" + answer.map((label) => `- ${label}`).join("\n") : answer ?? tr("已跳过");
        return tr("{index}. {question}\n回答：{reply}", { index: index + 1, question: item!.question, reply });
      });
      const started = await send(tr("补充回答：{title}\n\n{answers}", { title, answers: lines.join("\n\n") }), { origin: { kind: "questionnaire_answer" } });
      if (!started) throw new Error(tr("当前对话正在处理其他请求，请稍后提交。"));
      setReanswerRequest((current) => current === request ? null : current);
      return;
    }
    await respondToQuestionnaire(questionnaire, answers);
    // The approval is final even if its tool result arrives on a later replay.
    setSubmittedAnswers((current) => new Map(current).set(request, answers));
    void replayConversation(request.conversationId).catch((failure) => toast.error(errorText(failure)));
  }

  const renderTool = (tool: AgentToolActivity) => {
    const request = pendingToolApproval(tool);
    return request && !archivedConversation
      ? <AgentConfirmation key={request.seq} request={request} />
      : <ToolTrace key={tool.event.seq} tool={tool} />;
  };
  const renderQuestion = (questionnaire: (typeof questions)[number]) => <QuestionnaireTrace
    key={questionnaire.request.seq} questionnaire={questionnaire} disabled={busy || !!activeQuestion}
    onReanswer={writableChat ? () => setReanswerRequest(questionnaire.request) : undefined} />;
  const title = detail.data ? conversationTitle(detail.data.conversation) : tr("Agent 对话");
  const hasMessages = messages.length > 0 || tools.length > 0 || unlinkedErrors.length > 0 || questions.length > 0;

  return <div className="agent-chat-shell flex h-full min-h-0 flex-col overflow-hidden bg-background motion-safe:animate-soft-fade md:flex-row">
    <aside className="agent-chat-sidebar flex max-h-44 shrink-0 flex-col border-b bg-sidebar/70 md:max-h-none md:w-72 md:border-r md:border-b-0">
      <div className="flex items-center gap-2 border-b px-4 py-3">
        <div className="flex size-8 items-center justify-center rounded-lg bg-sidebar-primary text-sidebar-primary-foreground">
          <BotIcon aria-hidden="true" className="size-4" />
        </div>
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-semibold">Agent Mod Manager</p>
          <p className="truncate text-xs text-muted-foreground">{tr("本地工作区")}</p>
        </div>
      </div>
      <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-3">
        <Button className="w-full justify-start gap-2 rounded-lg" onClick={() => selectConversation(null)} disabled={submitting === "start"}>
          <MessageSquarePlusIcon data-icon="inline-start" />
          {tr("新建对话")}
        </Button>
        <AgentConversationList conversations={conversations.data ?? EMPTY_CONVERSATIONS} selectedId={conversationId}
          disabled={submitting === "start"} onSelect={selectConversation} />
      </div>
      <div className="hidden shrink-0 flex-col gap-2 border-t p-3 md:flex">
        <div className="flex items-center gap-2 px-2 text-xs font-medium text-muted-foreground"><FolderIcon className="size-3.5" />{tr("工作区")}</div>
        <Button variant="ghost" size="sm" aria-label={tr("切换工作区：{gameName}", { gameName })} aria-haspopup="dialog"
          className="w-full justify-start gap-2 bg-sidebar-accent/60" disabled={workspaceDisabled}
          onClick={event => openPicker(event.currentTarget)}>
          <span className="size-1.5 rounded-full bg-sidebar-primary" />
          <span className="min-w-0 flex-1 truncate text-left">{gameName}</span>
          <Settings2Icon aria-hidden="true" className="size-3.5 text-muted-foreground" />
        </Button>
      </div>
      <ErrorState error={conversations.error} className="m-3 mt-0" />
    </aside>

    <section className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header className="chat-header flex shrink-0 items-center justify-between gap-3 border-b px-4 py-3 sm:px-8">
        <div className="flex min-w-0 flex-col gap-0.5">
          <h1 className="truncate text-base font-semibold tracking-tight sm:text-lg">{title}</h1>
          <span className="hidden truncate text-xs text-muted-foreground md:block">{gameName}</span>
          <Button variant="ghost" size="xs" aria-label={tr("切换工作区：{gameName}", { gameName })} aria-haspopup="dialog"
            disabled={workspaceDisabled} className="min-w-0 max-w-full justify-start self-start md:hidden"
            onClick={event => openPicker(event.currentTarget)}>
            <Settings2Icon aria-hidden="true" /><span className="truncate">{gameName}</span>
          </Button>
        </div>
        <div className="flex max-w-[45%] flex-col items-end gap-0.5">
          <Badge className="min-w-0" tone={busy ? "warning" : "neutral"}>
            <span className="truncate">{pendingQuestion ? tr("等待你的回答") : awaitingApproval ? tr("等待授权") : busy ? tr("运行中") : selectedModelLabel || tr("未配置模型")}</span>
          </Badge>
          {totalTokens !== undefined && <span className="whitespace-nowrap text-xs text-muted-foreground tabular-nums">{totalTokens.toLocaleString(locale)} tokens</span>}
        </div>
      </header>

      <div className="relative min-h-0 flex-1 overflow-y-auto" aria-live="polite" aria-relevant="additions text">
        <div className={cn("mx-auto flex min-h-full w-full max-w-4xl flex-col px-4 py-6 sm:px-8", hasMessages ? "gap-2" : "justify-center")}>
          {!hasMessages && <div className="mx-auto flex max-w-xl flex-col items-center gap-4 pb-32 text-center motion-safe:animate-rise-in">
            <div className="relative flex size-14 items-center justify-center rounded-2xl border bg-card shadow-sm motion-safe:animate-float">
              <span aria-hidden="true" className="absolute inset-0 rounded-2xl border border-primary/40 motion-safe:animate-soft-ping" />
              <BotIcon aria-hidden="true" className="size-7 text-primary" />
            </div>
            <div className="flex flex-col gap-2">
              <h2 className="text-2xl font-semibold tracking-tight sm:text-3xl">{tr("Agent 对话")}</h2>
              <p className="text-sm leading-6 text-muted-foreground sm:text-base">{tr("与 Agent 协作，管理、分析并优化你的 Mod 工作流。")}</p>
            </div>
          </div>}

          {messages.map((message) => {
            const failure = errors.get(message.id);
            const isAssistant = message.role === "assistant";
            const hasContent = Boolean(message.content.trim());
            const attachments = message.attachments ?? [];
            const hasAttachments = attachments.length > 0;
            const modRequest = modRequests.get(message.id);
            const attachmentGroup = hasAttachments && <AttachmentGroup aria-label={tr("消息附件")} className="flex-wrap gap-1.5">
              {attachments.map((value, index) => <MessageAttachment key={index} value={value} />)}
            </AttachmentGroup>;
            const canAct = writableChat && !detail.isPlaceholderData && !busy && !activeQuestion;
            return <Fragment key={message.id}>
              <article className={cn("chat-message motion-safe:animate-rise-in", message.role)}>
                <Message align={isAssistant ? "start" : "end"}>
                  <MessageContent className={cn(isAssistant ? "max-w-3xl" : modRequest ? "max-w-sm" : "max-w-[min(80%,36rem)]")}>
                    {message.reasoning && <ReasoningTrace reasoning={message.reasoning} streaming={message.id === streamingReplyId} />}
                    {modRequest ? <>
                      {attachmentGroup}
                      <ModRequestCard request={modRequest} messageId={message.id} gameName={games.data?.find(game => game.id === modRequest.gameId)?.name} tools={tools} />
                    </> : <Bubble variant={isAssistant ? "ghost" : "default"} align={isAssistant ? "start" : "end"} className={isAssistant ? "max-w-full" : undefined}>
                      <BubbleContent className={cn("markdown min-w-0 break-words", !isAssistant && "rounded-2xl px-4 py-2.5")}>
                        <div data-message-id={message.id} className={cn(
                          "flex flex-col gap-3 [&_a]:wrap-anywhere [&_a]:underline [&_a]:underline-offset-4 [&_a_code]:bg-transparent",
                          "[&_code]:rounded [&_code]:bg-muted [&_code]:px-1 [&_code]:font-mono [&_code]:text-xs",
                          "[&_p]:leading-relaxed [&_ol]:list-decimal [&_ol]:pl-5 [&_ul]:list-disc [&_ul]:pl-5",
                          "[&_pre]:max-w-full [&_pre]:overflow-auto [&_pre]:rounded-md [&_pre]:border [&_pre]:bg-muted [&_pre]:p-3 [&_pre_code]:bg-transparent [&_pre_code]:p-0",
                          "[&_table]:w-full [&_table]:border-collapse [&_td]:border [&_td]:px-2 [&_td]:py-1 [&_th]:border [&_th]:px-2 [&_th]:py-1 [&_th]:text-left",
                          isAssistant && "[&_a]:text-accent-foreground",
                        )}>
                          {attachmentGroup}
                          {hasContent && <MessageMarkdown content={message.content} onError={setError} />}
                        </div>
                      </BubbleContent>
                    </Bubble>}
                    {toolsByMessage.get(message.id)?.map(renderTool)}
                    {questionsByMessage.get(message.id)?.map(renderQuestion)}
                    {isAssistant && <TurnError message={failure} />}
                    {(hasContent || hasAttachments || failure) && <MessageFooter className="mt-1" role="group" aria-label={tr("消息操作")}>
                      {hasContent && <CopyMessage content={message.content} />}
                      {!isAssistant && (hasContent || hasAttachments) && <MessageAction label={tr("编辑消息")} disabled={!canAct} onClick={() => editMessage(message)}>
                        <PencilIcon aria-hidden="true" />
                      </MessageAction>}
                      {(isAssistant || failure) && <MessageAction label={tr("重试")} disabled={!canAct} onClick={() => void send("", { restart: { messageId: message.id, edit: false } })}>
                        <RefreshCcwIcon aria-hidden="true" />
                      </MessageAction>}
                    </MessageFooter>}
                  </MessageContent>
                </Message>
              </article>
              {!isAssistant && failure && <div className="chat-message"><TurnError message={failure} /></div>}
            </Fragment>;
          })}

          {tools.filter(({ event }) => !event.messageId || !visibleIds.has(event.messageId)).map(renderTool)}
          {questions.filter(({ request }) => !request.messageId || !visibleIds.has(request.messageId)).map(renderQuestion)}
          {unlinkedErrors.map(({ seq, message }) => <div className="chat-message" key={seq}><TurnError message={message} /></div>)}
          {busy && !pendingQuestion && !awaitingApproval && <Trace className="max-w-3xl motion-safe:animate-soft-fade"><TraceLine status="running"><TraceIcon><LoaderCircleIcon className="animate-spin" /></TraceIcon><TraceTitle>{tr("正在处理请求")}</TraceTitle><TypingDots /></TraceLine></Trace>}
          <ErrorState error={error || detail.error} />
          <div ref={end} />
        </div>
      </div>

      <div className={cn("flex min-h-0 shrink-0 flex-col px-3 pt-2 pb-3 sm:px-8 sm:pb-5", activeQuestion && "max-h-[75%]")}>
        <div className="mx-auto flex min-h-0 w-full max-w-5xl flex-col">
          {activeQuestion ? <AgentQuestionnaire
            key={`${activeQuestion.request.conversationId}:${activeQuestion.request.seq}:${pendingQuestion ? "live" : "follow-up"}`}
            questionnaire={activeQuestion} disabled={!pendingQuestion && busy}
            onSubmit={(answers) => submitAnswers(activeQuestion, answers)}
            onCancel={pendingQuestion ? () => void cancel(pendingQuestion.request.turnId) : () => setReanswerRequest(null)}
          /> : <AgentComposer
            key={conversationId ?? "new"} games={games.data ?? []}
            libraryGameName={adapterChat?.gameId && workspace?.length === 1 && workspace[0] === adapterChat.gameId && !activeEdit ? adapterChat.label : undefined}
            input={draft.text}
            inputRef={composerInput}
            attachments={draft.attachments}
            updateAttachments={updateAttachments}
            onCancelEdit={activeEdit ? () => setEdit(null) : undefined}
            onInputChange={(text) => {
              if (activeEdit) setEdit((current) => current && { ...current, text });
              else setComposer((current) => ({ ...current, text }));
            }}
            onSend={(attachments, newGameName) => {
              const restart = activeEdit ? { messageId: activeEdit.messageId, edit: true } : undefined;
              const content = newGameName
                ? `${draft.text.trim()}\n\n${tr("请在绑定适配包时，将这款游戏在游戏库中的显示名称改为 {name}。", { name: JSON.stringify(newGameName) })}`
                : draft.text.trim();
              const origin: PromptOrigin | undefined = restart ? undefined : newGameName ? { kind: "game_rename" }
                : adapterChat && draft.text === adapterChat.prompt && !attachments.length ? { kind: "adapter_prompt" } : undefined;
              void send(content, { attachments: attachments.length || activeEdit ? attachments : undefined, restart, origin });
            }}
            onCancel={turn ? () => { void cancel(); } : undefined}
            disabled={!backendAvailable || archivedConversation || (permissionSelection === undefined && preferences.isLoading)}
            busy={busy}
            permission={(running ? runningMode : permission) ?? ""}
            inheritedPermission={running ? null : conversationId && !detail.data ? undefined : detail.data?.conversation.permission_mode ?? settings.data?.permission_mode}
            onPermissionChange={setPermission}
            model={selectedModel}
            models={models}
            onModelChange={(nextModel) => {
              setModelSelection(nextModel === model ? "" : nextModel);
              setEffortSelection(null);
            }}
            reasoningEffort={reasoningEffort}
            onReasoningChange={(level) => setEffortSelection(level ? { model: selectedModel, level } : null)}
          />}
        </div>
      </div>
    </section>
    {picker && picker.id === conversationId && picker.adapter === adapterChat && workspace !== undefined &&
      <AgentWorkspaceDialog workspace={workspace} games={games.data} gamesError={games.error}
        onRetry={() => void games.refetch()} disabled={workspaceDisabled} onApply={applyWorkspace}
        onClose={() => setPicker(null)} opener={picker.opener} />}
  </div>;
}

// React Markdown parses synchronously; skip unchanged history when the turn or draft changes.
const MessageMarkdown = memo(function MessageMarkdown({ content, onError }: { content: string; onError: (error: unknown) => void }) {
  const components = useMemo<Components>(() => ({
    img: ({ alt }) => <span>{alt || tr("远程图片")}</span>,
    a: ({ children, href }) => href && /^https?:\/\//i.test(href) ? <a href={href} target="_blank" rel="noreferrer" onClick={(event) => {
      if (backendAvailable) {
        event.preventDefault();
        void openUrl(href).catch(onError);
      }
    }}>{children}</a> : <span>{children}</span>,
  }), [onError]);
  return <Markdown remarkPlugins={[remarkGfm]} components={components}>{content}</Markdown>;
});

/** Three dots that bob in turn while the Agent works. Decorative: the status text already says what is happening. */
function TypingDots() {
  return <span aria-hidden="true" className="flex items-center gap-0.5">
    {[0, 1, 2].map((dot) => <span key={dot} className="size-1 rounded-full bg-current motion-safe:animate-dot-wave" style={{ animationDelay: `${dot * 150}ms` }} />)}
  </span>;
}

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
      toast.error(tr("复制失败，请重试"));
    }
  }

  return <MessageAction label={copied ? tr("已复制") : tr("复制消息")} onClick={() => void copy()}>
    {copied ? <CheckIcon aria-hidden="true" className="motion-safe:animate-pop-in" /> : <CopyIcon aria-hidden="true" />}
  </MessageAction>;
}

function ReasoningTrace({ reasoning, streaming }: { reasoning: string; streaming: boolean }) {
  const [open, setOpen] = useState(streaming);
  const finished = useDidChange(streaming);

  useEffect(() => {
    if (!streaming) setOpen(false);
  }, [streaming]);

  return <Trace>
    <TraceCollapsible open={open} onOpenChange={setOpen}>
      <TraceCollapsibleTrigger>
        <TraceLine status={streaming ? "running" : undefined}>
          <TraceIcon>{streaming ? <LoaderCircleIcon className="animate-spin" /> : <CircleCheckIcon className={cn(finished && "motion-safe:animate-pop-in")} />}</TraceIcon>
          <TraceTitle>{streaming ? tr("思考中") : tr("思考过程")}</TraceTitle>
          <TraceDescription>{streaming ? tr("进行中") : tr("已完成")}</TraceDescription>
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
    <AlertTitle>{tr("本轮回复未完成")}</AlertTitle>
    <AlertDescription className="min-w-0">
      <pre className="max-w-full overflow-auto font-mono text-xs break-all whitespace-pre-wrap"><code>{message}</code></pre>
    </AlertDescription>
  </Alert>;
}

const ToolTrace = memo(function ToolTrace({ tool }: { tool: AgentToolActivity }) {
  useLocale();
  const { event, request, result, resolution, completion } = tool;
  const name = event.payload.name ?? request?.payload.title;
  const settled = useDidChange(!!result);
  const rejected = resolution?.payload.status === "rejected" || result?.payload.status === "denied";
  const cancelled = resolution?.payload.status === "cancelled" || result?.payload.status === "cancelled" ||
    (!result && completion?.payload.status === "cancelled");
  const failed = !rejected && !cancelled && result?.payload.status === "error";
  const running = !result && !rejected && !cancelled && !completion;
  const label = rejected ? tr("已拒绝") : cancelled ? tr("已取消") : failed ? tr("执行失败") : result ? tr("已执行")
    : completion ? (completion.payload.status === "interrupted" ? tr("已中断") : tr("已结束"))
    : resolution?.payload.status === "approved" ? tr("已批准，正在执行") : tr("正在执行");
  const Icon = failed ? TriangleAlertIcon : rejected || cancelled ? CircleXIcon : result ? CheckIcon : CircleDotIcon;
  const payload = useMemo(() => JSON.stringify({
    arguments: displayToolArguments(name, event.payload.arguments),
    ...(result ? { result: displayToolResult(name, result.payload.result) } : {}),
  }, null, 2), [event, name, result]);
  return <Trace>
    <TraceCollapsible defaultOpen={failed}>
      <TraceCollapsibleTrigger>
        <TraceLine status={failed ? "error" : running ? "running" : undefined}>
          <TraceIcon><Icon className={cn(settled && "motion-safe:animate-pop-in")} /></TraceIcon>
          <TraceTitle>{String(name)}</TraceTitle>
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
