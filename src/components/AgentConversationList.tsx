import { memo, useId, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ArchiveIcon, CircleCheckIcon, CircleHelpIcon, EllipsisIcon, LoaderCircleIcon, SquarePenIcon, PencilIcon, PinIcon, PinOffIcon, ShieldIcon, Trash2Icon } from "lucide-react";
import { api } from "@/lib/api";
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
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "./ui/dialog";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger,
} from "./ui/dropdown-menu";
import { Field, FieldLabel } from "./ui/field";
import { Input } from "./ui/input";
import { Tooltip, TooltipContent, TooltipTrigger } from "./ui/tooltip";

interface Props {
  conversations: Conversation[];
  selectedId: string | null;
  disabled: boolean;
  onSelect: (id: string | null) => void;
}

type Action = "rename" | "pin" | "archive" | "delete";
const successMessages = {
  rename: "对话已重命名",
  pin: "对话已置顶",
  archive: "对话已归档",
  delete: "对话已删除",
};

const statusIndicators = {
  running: { text: "进行中", Icon: LoaderCircleIcon, color: "text-agent-running" },
  completed: { text: "已完成", Icon: CircleCheckIcon, color: "text-success" },
  "waiting-answer": { text: "等待回答", Icon: CircleHelpIcon, color: "text-agent-question" },
  "waiting-input": { text: "等待输入", Icon: SquarePenIcon, color: "text-muted-foreground" },
  "waiting-approval": { text: "等待授权", Icon: ShieldIcon, color: "text-warning" },
} satisfies Record<AgentConversationStatus, { text: string; Icon: typeof CircleCheckIcon; color: string }>;

function ConversationStatus({ id, status }: { id: string; status: AgentConversationStatus }) {
  const { text, Icon, color } = statusIndicators[status];
  return <span id={id} role="img" aria-label={text} className={cn("relative ms-auto inline-flex size-4 shrink-0 items-center justify-center", color)}>
    {status === "running" && <span aria-hidden="true" className="absolute size-3 rounded-full border-[1.5px] border-current opacity-25" />}
    <Icon aria-hidden="true" className={cn("size-3.5", status === "running" && "motion-safe:animate-spin")} />
  </span>;
}

export const AgentConversationList = memo(function AgentConversationList({ conversations, selectedId, disabled, onSelect }: Props) {
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
    setTitle(conversation.title);
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
      if (action === "delete") client.removeQueries({ queryKey: ["conversation", id], exact: true });
      else void client.invalidateQueries({ queryKey: ["conversation", id] });
      await client.invalidateQueries({ queryKey: ["conversations"] });
      setDialog(null);
      notify(action === "pin" && conversation.pinned ? "已取消置顶" : successMessages[action]);
    } catch (failure) {
      setError(failure);
    } finally {
      saving.current = false;
      setPending(false);
    }
  }

  const pinned = conversations.filter((conversation) => conversation.pinned);
  const recent = conversations.filter((conversation) => !conversation.pinned);

  function renderGroup(label: string, rows: Conversation[]) {
    return <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between px-2 text-xs font-medium text-muted-foreground">
        <span>{label}</span><span className="tabular-nums">{rows.length}</span>
      </div>
      <ul aria-label={label} className="flex gap-1 overflow-x-auto md:flex-col md:overflow-visible">
        {rows.map((conversation) => {
          const current = conversation.id === selectedId;
          const state = statuses[conversation.id];
          const statusId = `${titleId}-status-${conversation.id}`;
          const PinActionIcon = conversation.pinned ? PinOffIcon : PinIcon;
          return <li key={conversation.id}
            className={cn("flex max-w-full min-w-0 shrink-0 items-center rounded-md pr-1 md:w-full", current && "bg-secondary")}>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button type="button" variant="ghost" size="sm" disabled={disabled || pending} aria-current={current || undefined}
                  aria-label={conversation.title} aria-describedby={state ? statusId : undefined} onClick={() => onSelect(conversation.id)}
                  className="min-w-0 flex-1 justify-start text-left">
                  {conversation.pinned && <PinIcon aria-hidden="true" className="text-muted-foreground" />}
                  <span className="min-w-0 truncate" title={conversation.title}>{conversation.title}</span>
                  {state && <ConversationStatus id={statusId} status={state.status} />}
                </Button>
              </TooltipTrigger>
              {state && <TooltipContent side="top" align="end" sideOffset={4}>{statusIndicators[state.status].text}</TooltipContent>}
            </Tooltip>
            <DropdownMenu modal={false}>
              <DropdownMenuTrigger asChild>
                <Button type="button" size="icon-xs" variant="ghost" className="shrink-0 text-muted-foreground"
                  aria-label={`对话操作：${conversation.title}`} disabled={disabled || pending}
                  onPointerDown={(event) => { opener.current = event.currentTarget; }}
                  onFocus={(event) => { opener.current = event.currentTarget; }}>
                  <EllipsisIcon aria-hidden="true" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" side="right" collisionPadding={8} onCloseAutoFocus={(event) => {
                if (dialog) event.preventDefault();
              }}>
                <DropdownMenuGroup>
                  <DropdownMenuItem onSelect={() => launch("rename", conversation)}><PencilIcon />重命名</DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => void apply("pin", conversation)}>
                    <PinActionIcon />{conversation.pinned ? "取消置顶" : "置顶"}
                  </DropdownMenuItem>
                  <DropdownMenuItem disabled={state?.active} onSelect={() => void apply("archive", conversation)}><ArchiveIcon />归档</DropdownMenuItem>
                </DropdownMenuGroup>
                <DropdownMenuSeparator />
                <DropdownMenuItem variant="destructive" disabled={state?.active} onSelect={() => launch("delete", conversation)}><Trash2Icon />删除</DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </li>;
        })}
      </ul>
    </div>;
  }

  return <div ref={list} tabIndex={-1} className="flex flex-col gap-3 outline-none">
    {pinned.length > 0 && renderGroup("置顶对话", pinned)}
    {(recent.length > 0 || !pinned.length) && renderGroup("最近对话", recent)}
    {!conversations.length && <p className="px-2 text-xs text-muted-foreground">还没有已保存的对话</p>}
    {!dialog && <ErrorState error={error} />}
    <Dialog open={dialog?.action === "rename"} onOpenChange={closeDialog}>
      <DialogContent onCloseAutoFocus={restoreFocus} showCloseButton={!pending}>
        <DialogHeader>
          <DialogTitle>重命名对话</DialogTitle>
          <DialogDescription>输入 1–60 个字符的对话名称。</DialogDescription>
        </DialogHeader>
        <form className="flex flex-col gap-4" onSubmit={(event) => {
          event.preventDefault();
          if (validTitle && dialog?.action === "rename") void apply("rename", dialog.conversation);
        }}>
          <Field>
            <FieldLabel htmlFor={titleId}>对话名称</FieldLabel>
            <Input id={titleId} value={title} onChange={(event) => setTitle(event.target.value)} disabled={pending}
              onFocus={(event) => event.currentTarget.select()} />
          </Field>
          <ErrorState error={error} />
          <DialogFooter>
            <Button variant="outline" type="button" disabled={pending} onClick={() => closeDialog(false)}>取消</Button>
            <Button type="submit" disabled={!validTitle || pending}>{pending ? "保存中…" : "保存"}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
    <AlertDialog open={dialog?.action === "delete"} onOpenChange={closeDialog}>
      <AlertDialogContent onCloseAutoFocus={restoreFocus}>
        <AlertDialogHeader>
          <AlertDialogTitle>删除对话</AlertDialogTitle>
          <AlertDialogDescription className="wrap-anywhere">
            将永久删除「{dialog?.conversation.title}」及其消息。已执行的游戏文件操作仍可在事务记录中查看。
          </AlertDialogDescription>
        </AlertDialogHeader>
        <ErrorState error={error} />
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>取消</AlertDialogCancel>
          <Button variant="destructive" disabled={pending} onClick={() => {
            if (dialog?.action === "delete") void apply("delete", dialog.conversation);
          }}>{pending ? "删除中…" : "删除对话"}</Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  </div>;
});
