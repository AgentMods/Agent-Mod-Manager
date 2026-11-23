import type { Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

import type { ConversationCategory } from "../../src/lib/types";

export interface CategoryConversationFixture {
  id: string;
  title: string;
  category?: ConversationCategory | null;
  pinned?: boolean;
  archived?: boolean;
}

const defaults = [
  {id: "conversation-ordinary", title: "普通对话", category: null},
  {id: "conversation-install", title: "安装任务", category: "install"},
  {id: "conversation-install-2", title: "安装 Mod：性能补丁", category: "install"},
  {id: "conversation-uninstall", title: "卸载任务", category: "uninstall"},
  {id: "conversation-remove", title: "删除任务", category: "remove"},
  {id: "conversation-adapter", title: "适配包任务", category: "adapter"},
  {id: "conversation-pinned", title: "置顶对话", category: "install", pinned: true},
  {id: "conversation-legacy", title: "旧版普通对话"},
] satisfies CategoryConversationFixture[];

export const categoryConversationFixtures = defaults;

export async function installConversationCategoryFixture(
  page: Page,
  conversations: CategoryConversationFixture[] = categoryConversationFixtures,
) {
  await installMockTauri(page);
  await page.addInitScript((items) => {
    window.addEventListener("DOMContentLoaded", () => {
      const fixture = (window as any).__AMM_TEST__;
      const now = "2026-11-23T00:00:00Z";
      fixture.conversations.splice(0, fixture.conversations.length, ...items.map((item: CategoryConversationFixture) => ({
        task_summary: null,
        game_id: null,
        archived: false,
        pinned: false,
        message_count: 0,
        created_at: now,
        updated_at: now,
        permission_mode: null,
        ...item,
      })));
    }, {once: true});
  }, conversations);
}
