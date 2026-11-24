import { getLocale, useLocale, tr } from "@/lib/i18n";
import { conversationCountSources } from "@/locales/plurals";
import { memo, useEffect, useId, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ArchiveIcon, ChevronRightIcon, CircleCheckIcon, CircleHelpIcon, EllipsisIcon, LoaderCircleIcon, SquarePenIcon, PencilIcon, PinIcon, PinOffIcon, ShieldIcon, Trash2Icon } from "lucide-react";
import { api } from "@/lib/api";
import { conversationCategoryLabels, conversationTitle, groupConversations, type ConversationGroup } from "@/lib/conversation-groups";
import { type AgentConversationStatus, useAgentConversationStatuses } from "@/lib/agent-conversation-status";
import { useUI } from "@/lib/store";
import type { Conversation } from "@/lib/types";
import { cn } from "@/lib/utils";
import { ErrorState } from "./ui";
import {
  AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter,
  AlertDialogHeader, AlertDialogTitle,
} from "./ui/alert-dialog";
import { Button } from "./ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "./ui/collapsible";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "./ui/dialog";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger,
} from "./ui/dropdown-menu";
import { Field, FieldLabel } from "./ui/field";
import { Input } from "./ui/input";
import { Tooltip, TooltipContent, TooltipTrigger } from "./ui/tooltip";
import { stagger } from "@/lib/motion";

interface Props {
  conversations: Conversation[];
  selectedId: string | null;
  disabled: boolean;
  onSelect: (id: string | null) => void;
}

type Action = "rename" | "pin" | "archive" | "delete";
const successMessages = {
  get rename() { return tr("对话已重命名"); },
  get pin() { return tr("对话已置顶"); },
  get archive() { return tr("对话已归档"); },
  get delete() { return tr("对话已删除"); },
};

const statusIndicators = {
  running: { get text() { return tr("进行中"); }, Icon: LoaderCircleIcon, color: "text-agent-running" },
  completed: { get text() { return tr("已完成"); }, Icon: CircleCheckIcon, color: "text-success" },
  "waiting-answer": { get text() { return tr("等待回答"); }, Icon: CircleHelpIcon, color: "text-agent-question" },
  "waiting-input": { get text() { return tr("等待输入"); }, Icon: SquarePenIcon, color: "text-muted-foreground" },
  "waiting-approval": { get text() { return tr("等待授权"); }, Icon: ShieldIcon, color: "text-warning" },
} satisfies Record<AgentConversationStatus, { text: string; Icon: typeof CircleCheckIcon; color: string }>;

function ConversationStatus({ id, status }: { id: string; status: AgentConversationStatus }) {
  const { text, Icon, color } = statusIndicators[status];
  return <span id={id} role="img" aria-label={text} className={cn("relative ms-auto inline-flex size-4 shrink-0 items-center justify-center", color)}>
    {status === "running" && <span aria-hidden="true" className="absolute size-3 rounded-full border-[1.5px] border-current opacity-25 motion-safe:animate-soft-ping" />}
    <Icon aria-hidden="true" className={cn("size-3.5", status === "running" && "motion-safe:animate-spin")} />
  </span>;
}

function CategoryDisclosure({ group, selectedId, statuses, renderRow }: {
  group: ConversationGroup;
  selectedId: string | null;
  statuses: ReturnType<typeof useAgentConversationStatuses>;
  renderRow: (conversation: Conversation, index: number) => React.ReactNode;
}) {
  const { category, rows } = group;
  const label = conversationCategoryLabels[category];
  const descriptionId = useId();
  const selectedMember = rows.some(row => row.id === selectedId) ? selectedId : null;
  const storageKey = `amm.chat.group.${category}.expanded`;
  const [open, setOpen] = useState(() => {
    if (selectedMember) return true;
    try { return localStorage.getItem(storageKey) === "true"; }
    catch { return false; }
  });
  // Reveal a chat opened from another page, including when the list arrives later.
  useEffect(() => { if (selectedMember) setOpen(true); }, [selectedMember]);
  function toggle(next: boolean) {
    setOpen(next);
    try { localStorage.setItem(storageKey, String(next)); }
    catch { /* Disclosure still works when WebView storage is unavailable. */ }
  }
  const activeStatus = !open && (["waiting-approval", "waiting-answer", "running"] as const)
    .find(status => rows.some(row => statuses[row.id]?.status === status));

  return <Collapsible asChild open={open} onOpenChange={toggle}>
    <li className="min-w-0">
      <CollapsibleTrigger asChild>
        <Button type="button" variant="ghost" size="sm" aria-label={label} aria-describedby={descriptionId}
          className={cn("w-full justify-start gap-2 bg-sidebar-accent/35", !open && selectedMember && "bg-secondary")}>
          <ChevronRightIcon aria-hidden="true" className={cn("size-3.5 text-muted-foreground motion-safe:transition-transform", open && "rotate-90")} />
          <span>{label}</span>
          <span id={descriptionId} className="ms-auto flex items-center gap-2 text-xs text-muted-foreground tabular-nums">
            {activeStatus && <ConversationStatus id={`${descriptionId}-status`} status={activeStatus} />}
            <span aria-hidden="true">{rows.length.toLocaleString(getLocale())}</span>
            <span className="sr-only">{tr(conversationCountSources.group, { length: rows.length })}{!open && selectedMember ? tr("，含当前对话") : ""}</span>
          </span>
        </Button>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <ul aria-label={label} className="my-1 ms-4 flex gap-1 overflow-x-auto border-s ps-1.5 md:flex-col md:overflow-visible">
          {open && rows.map(renderRow)}
        </ul>
      </CollapsibleContent>
    </li>
  </Collapsible>;
}

export const AgentConversationList = memo(function AgentConversationList({ conversations, selectedId, disabled, onSelect }: Props) {
  useLocale();
  const client = useQueryClient();
  const notify = useUI((state) => state.notify);
  const statuses = useAgentConversationStatuses(conversations);
  const titleId = useId();
  const [dialog, setDialog] = useState<{ action: "rename" | "delete"; conversation: Conversation } | null>(null);
  const [title, setTitle] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const saving = useRef(false);
  const opener = useRef<HTMLButtonElement | null>(null);
  const list = useRef<HTMLDivElement>(null);
  const validTitle = title.trim().length > 0 && Array.from(title.trim()).length <= 60;

  function launch(action: "rename" | "delete", conversation: Conversation) {
    setError(null);
    setTitle(conversationTitle(conversation));
    setDialog({ action, conversation });
  }

  function closeDialog(open: boolean) {
    if (!open && !saving.current) {
      setDialog(null);
      setError(null);
    }
  }

  function restoreFocus(event: Event) {
    event.preventDefault();
    if (opener.current?.isConnected) opener.current.focus();
    else list.current?.focus();
  }

  async function apply(action: Action, conversation: Conversation) {
    if (saving.current) return;
    const { id } = conversation;
    saving.current = true;
    setPending(true);
    setError(null);
    try {
      switch (action) {
        case "rename": await api("rename_conversation", { id, title: title.trim() }); break;
        case "pin": await api("set_conversation_pinned", { id, pinned: !conversation.pinned }); break;
        case "archive": await api("archive_conversation", { id, archived: true }); break;
        case "delete": await api("delete_conversation", { id }); break;
      }
      if ((action === "archive" || action === "delete") && useUI.getState().conversationId === id) onSelect(null);
      if (action === "delete") client.removeQueries({ queryKey: ["conversation", id] });
      else void client.invalidateQueries({ queryKey: ["conversation", id] });
      await client.invalidateQueries({ queryKey: ["conversations"] });
      setDialog(null);
      notify(action === "pin" && conversation.pinned ? tr("已取消置顶") : successMessages[action]);
    } catch (failure) {
      setError(failure);
    } finally {
      saving.current = false;
      setPending(false);
    }
  }

  const { pinned, recent } = useMemo(() => groupConversations(conversations), [conversations]);

  function renderConversation(conversation: Conversation, index: number) {
    // The group already names the operation; leave more space for the Mod name.
    const label = !conversation.pinned && conversation.category ? conversationCategoryLabels[conversation.category] : null;
    const title = conversationTitle(conversation);
    const visibleTitle = label && (title.startsWith(label + "：") || title.startsWith(label + ":"))
      ? title.slice(label.length + 1).trim() || title : title;
    const current = conversation.id === selectedId;
    const state = statuses[conversation.id];
    const statusId = `${titleId}-status-${conversation.id}`;
    const PinActionIcon = conversation.pinned ? PinOffIcon : PinIcon;
    return <li key={conversation.id}
      className={cn("flex max-w-full min-w-0 shrink-0 items-center rounded-md pr-1 transition-colors duration-200 motion-safe:animate-slide-in-left md:w-full", current && "bg-secondary")} style={stagger(index, 30)}>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button type="button" variant="ghost" size="sm" disabled={disabled || pending} aria-current={current || undefined}
            aria-label={title} aria-describedby={state ? statusId : undefined} onClick={() => onSelect(conversation.id)}
            className="min-w-0 flex-1 justify-start text-left">
            {conversation.pinned && <PinIcon aria-hidden="true" className="text-muted-foreground" />}
            <span className="min-w-0 truncate" title={title}>{visibleTitle}</span>
            {state && <ConversationStatus id={statusId} status={state.status} />}
          </Button>
        </TooltipTrigger>
        {state && <TooltipContent side="top" align="end" sideOffset={4}>{statusIndicators[state.status].text}</TooltipContent>}
      </Tooltip>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button type="button" size="icon-xs" variant="ghost" className="shrink-0 text-muted-foreground"
            aria-label={tr("对话操作：{title}", { title })} disabled={disabled || pending}
            onPointerDown={(event) => { opener.current = event.currentTarget; }}
            onFocus={(event) => { opener.current = event.currentTarget; }}>
            <EllipsisIcon aria-hidden="true" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" side="right" collisionPadding={8} onCloseAutoFocus={(event) => {
          if (dialog) event.preventDefault();
        }}>
          <DropdownMenuGroup>
            <DropdownMenuItem onSelect={() => launch("rename", conversation)}><PencilIcon />{tr("重命名")}</DropdownMenuItem>
            <DropdownMenuItem onSelect={() => void apply("pin", conversation)}>
              <PinActionIcon />{conversation.pinned ? tr("取消置顶") : tr("置顶")}
            </DropdownMenuItem>
            <DropdownMenuItem disabled={state?.active} onSelect={() => void apply("archive", conversation)}><ArchiveIcon />{tr("归档")}</DropdownMenuItem>
          </DropdownMenuGroup>
          <DropdownMenuSeparator />
          <DropdownMenuItem variant="destructive" disabled={state?.active} onSelect={() => launch("delete", conversation)}><Trash2Icon />{tr("删除")}</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </li>;
  }

  function renderSection(label: string, entries: (Conversation | ConversationGroup)[], count: number) {
    const grouped = entries.some(entry => "rows" in entry);
    return <div className="flex min-w-0 flex-col gap-2">
      <div className="flex items-center justify-between px-2 text-xs font-medium text-muted-foreground">
        <span>{label}</span><span className="tabular-nums">{count.toLocaleString(getLocale())}</span>
      </div>
      <ul aria-label={label} className={cn("flex gap-1 md:flex-col", grouped ? "flex-col" : "overflow-x-auto md:overflow-visible")}>
        {entries.map((entry, index) => "rows" in entry
          ? <CategoryDisclosure key={entry.category} group={entry} selectedId={selectedId} statuses={statuses} renderRow={renderConversation} />
          : renderConversation(entry, index))}
      </ul>
    </div>;
  }

  return <div ref={list} tabIndex={-1} className="flex flex-col gap-3 outline-none">
    {pinned.length > 0 && renderSection(tr("置顶对话"), pinned, pinned.length)}
    {(recent.length > 0 || !pinned.length) && renderSection(tr("最近对话"), recent, conversations.length - pinned.length)}
    {!conversations.length && <p className="px-2 text-xs text-muted-foreground">{tr("还没有已保存的对话")}</p>}
    {!dialog && <ErrorState error={error} />}
    <Dialog open={dialog?.action === "rename"} onOpenChange={closeDialog}>
      <DialogContent onCloseAutoFocus={restoreFocus} showCloseButton={!pending}>
        <DialogHeader>
          <DialogTitle>{tr("重命名对话")}</DialogTitle>
          <DialogDescription>{tr("输入 1–60 个字符的对话名称。")}</DialogDescription>
        </DialogHeader>
        <form className="flex flex-col gap-4" onSubmit={(event) => {
          event.preventDefault();
          if (validTitle && dialog?.action === "rename") void apply("rename", dialog.conversation);
        }}>
          <Field>
            <FieldLabel htmlFor={titleId}>{tr("对话名称")}</FieldLabel>
            <Input id={titleId} value={title} onChange={(event) => setTitle(event.target.value)} disabled={pending}
              onFocus={(event) => event.currentTarget.select()} />
          </Field>
          <ErrorState error={error} />
          <DialogFooter>
            <Button variant="outline" type="button" disabled={pending} onClick={() => closeDialog(false)}>{tr("取消")}</Button>
            <Button type="submit" disabled={!validTitle || pending}>{pending ? tr("保存中…") : tr("保存")}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
    <AlertDialog open={dialog?.action === "delete"} onOpenChange={closeDialog}>
      <AlertDialogContent onCloseAutoFocus={restoreFocus}>
        <AlertDialogHeader>
          <AlertDialogTitle>{tr("删除对话")}</AlertDialogTitle>
          <AlertDialogDescription className="wrap-anywhere">{tr("将永久删除「{title}」及其消息。已执行的游戏文件操作仍可在事务记录中查看。", { title: dialog && conversationTitle(dialog.conversation) })}</AlertDialogDescription>
        </AlertDialogHeader>
        <ErrorState error={error} />
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>{tr("取消")}</AlertDialogCancel>
          <Button variant="destructive" disabled={pending} onClick={() => {
            if (dialog?.action === "delete") void apply("delete", dialog.conversation);
          }}>{pending ? tr("删除中…") : tr("删除对话")}</Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  </div>;
});
