import { expect, test, type Page } from "@playwright/test";
import type { AgentEvent } from "../../src/lib/types";
import { installMockTauri } from "./mock-tauri";

const conversationId = "conversation-fixture";
const title = "检查 Minecraft 模组";
const otherTitle = "另一个会话";
const createdAt = "2026-11-21T00:00:00Z";
const event = (seq: number, turnId: string, kind: string, messageId: string | null, payload: AgentEvent["payload"] = {}): AgentEvent =>
  ({ seq, turnId, kind, messageId, payload, conversationId, createdAt });
const kept = [
  event(1, "kept", "turn-start", "kept-user"),
  event(2, "kept", "delta", "kept-reply", { content: "保留的回复" }),
  event(3, "kept", "approval-request", "kept-reply", { title: "ask_user", approvalId: "kept-question", toolCallId: "kept-tool", preview: { question: "保留的安装问卷？" } }),
  event(4, "kept", "approval-resolved", null, { approvalId: "kept-question", status: "approved" }),
  event(5, "kept", "tool-result", "kept-reply", { toolCallId: "kept-tool", status: "done", result: { answer: "保留的回答" } }),
  event(6, "kept", "done", null, { status: "done" }),
];
const replaced = [
  event(7, "replaced", "turn-start", "target-user"),
  event(8, "replaced", "delta", "removed-reply", { content: "已删除的流式回复", reasoning: "已删除的思考" }),
  event(9, "replaced", "approval-request", "removed-reply", { title: "ask_user", approvalId: "removed-question", toolCallId: "removed-tool", preview: { question: "已删除的问卷？" } }),
  event(10, "replaced", "approval-resolved", null, { approvalId: "removed-question", status: "cancelled" }),
  event(11, "replaced", "error", "removed-reply", { message: "已删除的错误" }),
  event(12, "replaced", "done", null, { status: "error" }),
];
const newStart = event(13, "replacement", "turn-start", "target-user", { historyThroughSeq: 6 });
const newDelta = event(14, "replacement", "delta", "new-reply", { content: "新的流式回复" });
const newDone = event(15, "replacement", "done", null, { status: "done" });

async function openChat(page: Page, replayOnly: boolean) {
  await installMockTauri(page);
  await page.addInitScript(({ kept, replaced, newStart, newDelta, newDone, conversationId, otherTitle, replayOnly }) => {
    window.addEventListener("DOMContentLoaded", () => {
      const { __AMM_TEST__: fixture, __TAURI_INTERNALS__: tauri } = window as any;
      const invoke = tauri.invoke;
      const emit = (row: AgentEvent) => fixture.emitEvent(`agent://${row.kind}`, row);
      const message = (id: string, role: string, content: string) => ({ id, role, content, reasoning: null, tool_calls: null, tool_call_id: null });
      let messages = [
        ...(!replayOnly ? [message("kept-user", "user", "保留的用户请求"), message("kept-reply", "assistant", "保留的回复")] : []),
        message("target-user", "user", "原始用户请求"),
        message("removed-reply", "assistant", "已删除的持久化回复"),
        message("removed-user", "user", "已删除的后续请求"),
      ];
      fixture.conversations.push({ ...fixture.conversations[0], id: "other", title: otherTitle });
      if (!replayOnly) fixture.seedEvents(kept);
      fixture.emitReplaced = () => replaced.forEach(emit);
      fixture.emitDone = () => { fixture.seedEvents([newDone]); emit(newDone); };
      tauri.invoke = async (command: string, args: Record<string, any> = {}) => {
        if (command === "restart_turn") {
          fixture.calls.push({ command, args });
          if (args.request.conversationId !== conversationId) throw new Error("unexpected restart conversation");
          const through = replayOnly ? 0 : Number(newStart.payload.historyThroughSeq);
          for (let i = fixture.events.length - 1; i >= 0; i--) {
            if (fixture.events[i].conversationId === conversationId && fixture.events[i].seq > through) fixture.events.splice(i, 1);
          }
          const start = { ...newStart, payload: { historyThroughSeq: through } };
          fixture.seedEvents([start, newDelta]);
          const targetIndex = messages.findIndex(({ id }) => id === start.messageId);
          messages = [
            ...messages.slice(0, targetIndex),
            { ...messages[targetIndex], content: args.edit ? args.request.content : messages[targetIndex].content },
          ];
          if (replayOnly) {
            await new Promise<void>((resolve) => { fixture.releaseRestart = resolve; });
          } else {
            // A new delta may arrive before its start on another live channel.
            emit(newDelta);
            emit(start);
          }
          return conversationId;
        }
        if (command === "get_conversation" && args.id === conversationId) {
          const detail = await invoke(command, args);
          const snapshot = structuredClone(messages);
          if (fixture.holdDetail && fixture.events.some(({ seq }: AgentEvent) => seq === newStart.seq)) {
            await new Promise<void>((resolve) => { fixture.releaseDetail = resolve; });
          }
          return { ...detail, messages: snapshot };
        }
        if (command === "replay_agent_events" && args.conversationId === conversationId && fixture.holdReplay) {
          fixture.holdReplay = false;
          const snapshot = await invoke(command, args);
          await new Promise<void>((resolve) => { fixture.releaseReplay = resolve; });
          return snapshot;
        }
        return invoke(command, args);
      };
    }, { once: true });
  }, { kept, replaced, newStart, newDelta, newDone, conversationId, otherTitle, replayOnly });
  await page.goto("/");
  await page.getByRole("button", { name: title, exact: true }).click();
  await expect(page.getByText("原始用户请求", { exact: true })).toBeVisible();
}

async function expectDiscarded(page: Page, seqs: number[]) {
  expect(await page.evaluate(async ({ conversationId, seqs }) => {
    const { useAgent } = await import("/src/lib/agent-store.ts");
    const { streams, milestones } = useAgent.getState();
    return [streams[conversationId], milestones[conversationId]].flatMap((rows) => rows?.filter(({ seq }) => seqs.includes(seq)) ?? []);
  }, { conversationId, seqs })).toEqual([]);
}

const messageAction = (page: Page, id: string, name: string) => page.locator("article.chat-message")
  .filter({ has: page.locator(`[data-message-id="${id}"]`) }).getByRole("button", { name, exact: true });
const composer = (page: Page) => page.getByRole("textbox", { name: "告诉 Agent 你想做什么" });
const send = (page: Page) => page.getByRole("button", { name: "发送消息", exact: true });

test("retry prunes buffered and late events and stale replay without losing the retained turn or draft", async ({ page }) => {
  await openChat(page, false);
  await expect(page.getByText("保留的回复", { exact: true })).toBeVisible();
  await page.evaluate(async ({ conversationId, replaced }) => {
    const { replayConversation } = await import("/src/lib/agent-store.ts");
    const fixture = (window as any).__AMM_TEST__;
    fixture.seedEvents(replaced);
    fixture.holdReplay = true;
    fixture.holdDetail = true;
    void replayConversation(conversationId);
    fixture.emitReplaced();
  }, { conversationId, replaced });
  await expect.poll(() => page.evaluate(() => typeof (window as any).__AMM_TEST__.releaseReplay)).toBe("function");
  await expect(page.getByText("已删除的流式回复", { exact: true })).toBeVisible();
  await composer(page).fill("未发送的草稿");
  await page.getByLabel("选择附件").setInputFiles({ name: "draft.png", mimeType: "image/png", buffer: Buffer.from([137, 80, 78, 71]) });
  await expect(send(page)).toBeEnabled();
  await messageAction(page, "removed-reply", "重试").click();
  await expect(page.getByText("新的流式回复", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "已询问 1 个问题", exact: true }).click();
  await expect(page.getByText("保留的安装问卷？", { exact: true })).toBeVisible();
  await expect(page.locator('[data-message-id="removed-reply"], [data-message-id="removed-user"]')).toHaveCount(0);
  await page.evaluate(() => {
    const fixture = (window as any).__AMM_TEST__;
    fixture.emitReplaced();
    fixture.releaseReplay();
    fixture.holdDetail = false;
    fixture.releaseDetail?.();
    fixture.emitDone();
  });
  await expect(send(page)).toBeEnabled();
  await expectDiscarded(page, replaced.map(({ seq }) => seq));
  await expect(composer(page)).toHaveValue("未发送的草稿");
  await expect(page.getByText("draft.png", { exact: true })).toBeVisible();
  await expect(page.getByText("新的流式回复", { exact: true })).toHaveCount(1);
  await expect(page.getByText("已删除的问卷？", { exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: title, exact: true })).toHaveAttribute("aria-current", "true");
});

test("a replay-only empty-prefix restart replaces the old watch and does not steal selection", async ({ page }) => {
  await openChat(page, true);
  await page.evaluate(() => (window as any).__AMM_TEST__.emitReplaced());
  await expect(page.getByText("已删除的流式回复", { exact: true })).toBeVisible();
  await messageAction(page, "target-user", "编辑消息").click();
  await expect(page.getByRole("button", { name: "取消编辑", exact: true })).toBeVisible();
  await composer(page).fill("编辑后的用户请求");
  await send(page).click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).__AMM_TEST__.releaseRestart)).toBe("function");
  await expect(page.getByText("新的流式回复", { exact: true })).toBeVisible();
  await expect(page.locator('[data-message-id="target-user"]')).toHaveText("编辑后的用户请求");

  const detailReads = () => page.evaluate((id) => (window as any).__AMM_TEST__.calls
    .filter(({ command, args }: any) => command === "get_conversation" && args.id === id).length, conversationId);
  const before = await detailReads();
  await page.evaluate(() => {
    const fixture = (window as any).__AMM_TEST__;
    fixture.emitReplaced();
    fixture.emitDone();
  });
  // The restart IPC has not resolved. Only settlement of the replacement watch
  // can refresh the authoritative message snapshot here.
  await expect.poll(detailReads).toBeGreaterThan(before);
  await expectDiscarded(page, replaced.map(({ seq }) => seq));
  await page.getByRole("button", { name: otherTitle, exact: true }).click();
  await composer(page).fill("另一会话的草稿");
  await page.evaluate(() => (window as any).__AMM_TEST__.releaseRestart());
  await expect(send(page)).toBeEnabled();
  await expect(page.getByRole("button", { name: otherTitle, exact: true })).toHaveAttribute("aria-current", "true");
  await expect(composer(page)).toHaveValue("另一会话的草稿");
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter(({ command }: any) => command === "create_conversation"))).toEqual([]);
});

test("multiple rewrite intervals do not resurrect older rows or rebuild milestones on ordinary deltas", async ({ page }) => {
  await installMockTauri(page);
  await page.goto("/");
  const normalStart = event(16, "second-old", "turn-start", "second-target");
  const secondStart = event(19, "second", "turn-start", "second-target", { historyThroughSeq: 15 });
  const finalStart = event(21, "third", "turn-start", "target-user", { historyThroughSeq: 6 });
  const finalDelta = event(22, "third", "delta", "final-reply", { content: "最终回复" });
  const result = await page.evaluate(async ({ conversationId, batches, finalDelta }) => {
    const { useAgent } = await import("/src/lib/agent-store.ts");
    const { merge } = useAgent.getState();
    for (const batch of batches) merge(batch);
    const before = useAgent.getState();
    merge([finalDelta]);
    const after = useAgent.getState();
    return {
      stableMilestones: before.milestones === after.milestones,
      otherStreamUnchanged: before.streams.other === after.streams.other,
      otherSeqs: after.streams.other.map(({ seq }) => seq),
      seqs: after.streams[conversationId].map(({ seq }) => seq),
    };
  }, {
    conversationId, finalDelta,
    batches: [
      [...kept, ...replaced, { ...replaced[3], conversationId: "other" }],
      [newStart, newDelta, newDone, normalStart],
      [secondStart, normalStart, ...replaced],
      [finalStart],
      [secondStart, normalStart, newStart, newDelta, ...replaced],
    ],
  });
  expect(result).toEqual({ stableMilestones: true, otherStreamUnchanged: true, otherSeqs: [10], seqs: [1, 2, 3, 4, 5, 6, 21, 22] });
});
