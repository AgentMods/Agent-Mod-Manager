import { useLayoutEffect } from "react";
import { CircleHelpIcon, MessageSquareIcon, ShieldCheckIcon } from "lucide-react";
import { SidebarMenuButton } from "@/components/ui/sidebar";
import { useAgentConversationStatuses } from "@/lib/agent-conversation-status";
import { useAgent } from "@/lib/agent-store";
import { useBackendQuery } from "@/lib/api";
import { formatList, getLocale, tr } from "@/lib/i18n";
import { useUI } from "@/lib/store";
import type { Conversation } from "@/lib/types";
import { cn } from "@/lib/utils";
import { navigatePage } from "@/router";

const emptyConversations: Conversation[] = [];
// Match the event store lifetime across shell remounts.
const seenReplies = new Map<string, number>();

export function AgentNavigationButton({ active, visibleConversation }: { active: boolean; visibleConversation: string | null }) {
  const { data: conversations = emptyConversations } = useBackendQuery<Conversation[]>(["conversations", false], "list_conversations", { archived: false });
  const statuses = useAgentConversationStatuses(conversations);
  const replies = useAgent(state => state.replyActivity);
  const visibleReply = visibleConversation ? replies[visibleConversation] : undefined;
  useLayoutEffect(() => {
    if (visibleReply) seenReplies.set(visibleReply.conversationId, visibleReply.seq);
  }, [visibleReply]);

  const approvals: string[] = [], questions: string[] = [], messages: string[] = [];
  for (const { id } of conversations) {
    const status = statuses[id]?.status;
    if (status === "waiting-approval") approvals.push(id);
    else if (status === "waiting-answer") questions.push(id);
    else if (id !== visibleConversation && replies[id]?.seq > (seenReplies.get(id) ?? 0)) messages.push(id);
  }
  const actions = [...approvals, ...questions];
  const target = actions.find(id => id !== visibleConversation) ?? actions[0] ?? messages[0];
  const count = actions.length + messages.length;
  const groups = [
    { label: tr("等待授权"), count: approvals.length },
    { label: tr("等待回答"), count: questions.length },
    { label: tr("新消息"), count: messages.length },
  ];
  const description = count ? formatList(groups.filter(group => group.count > 0)
    .map(group => `${group.label} (${group.count.toLocaleString(getLocale())})`)) : undefined;
  const StatusIcon = approvals.length ? ShieldCheckIcon : questions.length ? CircleHelpIcon : MessageSquareIcon;
  const hint = description && `Agent · ${description}`;

  return <SidebarMenuButton className="relative" aria-label="Agent" aria-description={description}
    tooltip={hint ?? "Agent"} title={hint} isActive={active} aria-current={active ? "page" : undefined}
    onClick={() => target ? useUI.getState().openConversation(target) : navigatePage("agent")}>
    <MessageSquareIcon /><span>Agent</span>
    {count > 0 && <span aria-hidden="true" className={cn(
      "ms-auto inline-flex shrink-0 items-center gap-1 rounded-full px-1.5 text-xs font-semibold tabular-nums group-data-[collapsible=icon]:absolute group-data-[collapsible=icon]:end-1 group-data-[collapsible=icon]:top-1 group-data-[collapsible=icon]:size-2 group-data-[collapsible=icon]:p-0 group-data-[collapsible=icon]:ring-1 group-data-[collapsible=icon]:ring-sidebar",
      actions.length ? "bg-warning/15 text-warning group-data-[collapsible=icon]:bg-warning" : "bg-primary/15 text-primary group-data-[collapsible=icon]:bg-primary",
    )}>
      <StatusIcon className="size-3 group-data-[collapsible=icon]:hidden" />
      <span className="group-data-[collapsible=icon]:hidden">{count.toLocaleString(getLocale())}</span>
    </span>}
  </SidebarMenuButton>;
}
