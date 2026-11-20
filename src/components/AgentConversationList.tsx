import { useId, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ArchiveIcon, EllipsisIcon, PencilIcon, PinIcon, PinOffIcon, Trash2Icon } from "lucide-react";
import { api } from "@/lib/api";
import { activeTurn, useAgent } from "@/lib/agent-store";
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

export function AgentConversationList({ conversations, selectedId, disabled, onSelect }: Props) {
  const client = useQueryClient();
  const notify = useUI((state) => state.notify);
  const streams = useAgent((state) => state.streams);
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
          const running = !!activeTurn(streams[conversation.id] ?? []);
          const PinActionIcon = conversation.pinned ? PinOffIcon : PinIcon;
          return <li key={conversation.id}
            className={cn("flex max-w-full min-w-0 shrink-0 items-center rounded-md pr-1 md:w-full", current && "bg-secondary")}>
            <Button type="button" variant="ghost" size="sm" disabled={disabled || pending} aria-current={current || undefined}
              title={conversation.title} onClick={() => onSelect(conversation.id)}
              className="min-w-0 flex-1 justify-start text-left">
              {conversation.pinned && <PinIcon aria-hidden="true" className="text-muted-foreground" />}
              <span className="min-w-0 truncate">{conversation.title}</span>
            </Button>
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
                  <DropdownMenuItem disabled={running} onSelect={() => void apply("archive", conversation)}><ArchiveIcon />归档</DropdownMenuItem>
                </DropdownMenuGroup>
                <DropdownMenuSeparator />
                <DropdownMenuItem variant="destructive" disabled={running} onSelect={() => launch("delete", conversation)}><Trash2Icon />删除</DropdownMenuItem>
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
}
