import { expect, test, type Page } from "@playwright/test";
import type { Conversation } from "../../src/lib/types";
import { installMockTauri } from "./mock-tauri";

type AgentStatus = NonNullable<Conversation["agent_status"]>;

function conversation(
  id: string,
  title: string,
  messageCount: number,
  checkpoint: AgentStatus,
) {
  return {
    id, title, game_id: "game-fixture", task_summary: null, archived: false, pinned: false,
    message_count: messageCount, created_at: "2026-11-20T00:00:00Z", updated_at: "2026-11-20T00:00:00Z",
    permission_mode: null, agent_status: checkpoint,
  };
}

const conversations = [
  conversation("conversation-empty", "空白会话", 0, { turn_id: null, turn_status: null, event_seq: 0, pending: null }),
  conversation("conversation-done", "成功结束", 2, { turn_id: "turn-done", turn_status: "done", event_seq: 2, pending: null }),
  conversation("conversation-running", "运行中会话", 2, { turn_id: "turn-running", turn_status: "running", event_seq: 3, pending: null }),
  conversation("conversation-structured", "结构化问卷", 2, { turn_id: "turn-structured", turn_status: "running", event_seq: 5, pending: { approval_id: "approval-structured", status: "waiting-answer" } }),
  conversation("conversation-legacy", "Legacy 问卷", 2, { turn_id: "turn-legacy", turn_status: "running", event_seq: 7, pending: { approval_id: "approval-legacy", status: "waiting-answer" } }),
  conversation("conversation-approval", "等待授权", 2, { turn_id: "turn-approval", turn_status: "running", event_seq: 10, pending: { approval_id: "approval-tool", status: "waiting-approval" } }),
  conversation("conversation-error", "失败后待输入", 2, { turn_id: "turn-error", turn_status: "error", event_seq: 12, pending: null }),
  conversation("conversation-cancelled", "取消后待输入", 2, { turn_id: "turn-cancelled", turn_status: "cancelled", event_seq: 16, pending: null }),
  conversation("conversation-interrupted", "中断后待输入", 2, { turn_id: "turn-interrupted", turn_status: "interrupted", event_seq: 18, pending: null }),
];

const at = (seq: number, conversationId: string, turnId: string, kind: string, payload: Record<string, unknown>, messageId: string | null = null) => ({
  seq, turnId, conversationId, messageId, kind, payload, createdAt: "2026-11-20T00:00:00Z",
});
const ask = (approvalId: string, preview: Record<string, unknown>) => ({
  approvalId, toolCallId: `tool-${approvalId}`, title: "ask_user", preview,
});
const structuredPreview = { title: "请补充游戏信息", questions: [{ question: "当前使用哪个平台？", options: ["Windows PC", "其他平台"] }] };
const legacyPreview = { question: "请输入安装目录。" };

const events = [
  at(1, "conversation-done", "turn-done", "turn-start", {}),
  at(2, "conversation-done", "turn-done", "done", { status: "done" }),
  at(3, "conversation-running", "turn-running", "turn-start", {}),
  at(4, "conversation-structured", "turn-structured", "turn-start", {}),
  at(5, "conversation-structured", "turn-structured", "approval-request", ask("approval-structured", structuredPreview)),
  at(6, "conversation-legacy", "turn-legacy", "turn-start", {}),
  at(7, "conversation-legacy", "turn-legacy", "approval-request", ask("approval-legacy", legacyPreview)),
  at(11, "conversation-error", "turn-error", "turn-start", {}),
  at(12, "conversation-error", "turn-error", "done", { status: "error" }),
  at(13, "conversation-cancelled", "turn-cancelled", "turn-start", {}),
  at(14, "conversation-cancelled", "turn-cancelled", "approval-request", ask("approval-cancelled", structuredPreview)),
  at(15, "conversation-cancelled", "turn-cancelled", "approval-resolved", { approvalId: "approval-cancelled", status: "cancelled" }),
  at(16, "conversation-cancelled", "turn-cancelled", "done", { status: "cancelled" }),
  at(17, "conversation-interrupted", "turn-interrupted", "turn-start", {}),
  at(18, "conversation-interrupted", "turn-interrupted", "done", { status: "interrupted" }),
];

async function installConversationStates(page: Page) {
  await installMockTauri(page);
  await page.addInitScript(({ conversations, events }) => {
    window.addEventListener("DOMContentLoaded", () => {
      const fixture = (window as any).__AMM_TEST__;
      fixture.conversations.splice(0, fixture.conversations.length, ...conversations);
      fixture.seedEvents(events);
    }, { once: true });
  }, { conversations, events });
}

test("侧栏用有色状态图标展示未打开会话，Tooltip 与标题按钮名称保持独立", async ({ page }) => {
  await installConversationStates(page);
  await page.goto("/");
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await page.getByRole("button", { name: /^Agent( 对话)?$/ }).click();

  const rows = [
    { title: "空白会话", id: "conversation-empty", status: "等待输入" },
    { title: "成功结束", id: "conversation-done", status: "已完成" },
    { title: "运行中会话", id: "conversation-running", status: "进行中" },
    { title: "结构化问卷", id: "conversation-structured", status: "等待回答" },
    { title: "Legacy 问卷", id: "conversation-legacy", status: "等待回答" },
    { title: "等待授权", id: "conversation-approval", status: "等待授权" },
    { title: "失败后待输入", id: "conversation-error", status: "等待输入" },
    { title: "取消后待输入", id: "conversation-cancelled", status: "等待输入" },
    { title: "中断后待输入", id: "conversation-interrupted", status: "等待输入" },
  ];

  // Check every row before opening any transcript: list checkpoints must carry
  // enough information to render unopened conversations without replaying them.
  for (const row of rows) {
    const button = page.getByRole("button", { name: row.title, exact: true });
    const listItem = page.getByRole("listitem").filter({ has: button });
    const icon = listItem.getByRole("img", { name: row.status, exact: true });
    await expect(icon).toBeVisible();
    await expect(icon).toHaveText("");
    await expect(button).toHaveText(row.title);
    await expect(button).toHaveAccessibleName(row.title);
    expect(await button.getAttribute("aria-describedby")).toBe(await icon.getAttribute("id"));
  }

  const uniqueStatuses = [...new Set(rows.map(({ status }) => status))];
  const colors = await Promise.all(uniqueStatuses.map((status) => page.getByRole("img", { name: status, exact: true }).first()
    .evaluate((element) => getComputedStyle(element).color)));
  expect(new Set(colors).size).toBe(5);

  const hoverIcon = page.getByRole("img", { name: "等待回答", exact: true }).first();
  await hoverIcon.hover();
  const tooltip = page.getByRole("tooltip");
  await expect(tooltip).toHaveText("等待回答");
  await page.keyboard.press("Escape");
  await expect(tooltip).toHaveCount(0);

  const keyboardButton = page.getByRole("button", { name: "运行中会话", exact: true });
  await keyboardButton.focus();
  await expect(keyboardButton).toBeFocused();
  await expect(tooltip).toHaveText("进行中");
  await page.keyboard.press("Tab");
  await expect(page.getByRole("button", { name: "对话操作：运行中会话", exact: true })).toBeFocused();
  await expect(tooltip).toHaveCount(0);

  const select = async (row: typeof rows[number]) => {
    const button = page.getByRole("button", { name: row.title, exact: true });
    await button.click();
    await expect(page.locator(".chat-header h1")).toHaveText(row.title);
    await expect(button).toHaveAttribute("aria-current", "true");
    for (const candidate of rows) {
      const candidateButton = page.getByRole("button", { name: candidate.title, exact: true });
      const candidateRow = page.getByRole("listitem").filter({ has: candidateButton });
      const candidateIcon = candidateRow.getByRole("img", { name: candidate.status, exact: true });
      await expect(candidateIcon).toBeVisible();
      await expect(candidateButton).toHaveText(candidate.title);
      await expect(candidateButton).toHaveAccessibleName(candidate.title);
      await expect(candidateIcon).toHaveText("");
    }
  };

  await select(rows[2]);
  await expect(page.getByRole("button", { name: "停止生成", exact: true })).toBeVisible();
  await select(rows[3]);
  await expect(page.getByRole("region", { name: structuredPreview.title })).toBeVisible();
  await select(rows[4]);
  await expect(page.getByRole("region", { name: "Agent 等待你的回答" })).toBeVisible();
  await select(rows[1]);
  await expect(page.getByRole("button", { name: "停止生成", exact: true })).toHaveCount(0);
  await select(rows[0]);
  await select(rows[2]);
  await expect(page.getByRole("button", { name: "停止生成", exact: true })).toBeVisible();

  const unopenedIds = rows.filter((row) => ["conversation-approval", "conversation-error", "conversation-cancelled", "conversation-interrupted"].includes(row.id)).map(({ id }) => id);
  const transcriptReads = await page.evaluate(() => (window as any).__AMM_TEST__.calls
    .filter((call: any) => call.command === "get_conversation").map((call: any) => call.args.id));
  expect(transcriptReads).not.toEqual(expect.arrayContaining(unopenedIds));
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls
    .filter((call: any) => call.command === "cancel_turn"))).toEqual([]);
});
