import type { Conversation, ConversationCategory } from "./types";

export const conversationCategoryLabels = {
  install: "安装 Mod",
  uninstall: "卸载 Mod",
  remove: "删除 Mod",
  adapter: "适配包",
} satisfies Record<ConversationCategory, string>;

export interface ConversationGroup {
  category: ConversationCategory;
  rows: Conversation[];
}

export function groupConversations(conversations: Conversation[]) {
  const pinned: Conversation[] = [];
  const recent: (Conversation | ConversationGroup)[] = [];
  const groups = new Map<ConversationCategory, ConversationGroup>();

  // The backend orders rows by recency. A group takes its newest member's place.
  for (const conversation of conversations) {
    if (conversation.pinned) {
      pinned.push(conversation);
    } else if (conversation.category) {
      let group = groups.get(conversation.category);
      if (!group) {
        group = { category: conversation.category, rows: [] };
        groups.set(group.category, group);
        recent.push(group);
      }
      group.rows.push(conversation);
    } else {
      recent.push(conversation);
    }
  }
  return { pinned, recent };
}
