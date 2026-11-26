import { expect, test, type Page } from "@playwright/test";
import type { AgentEvent } from "../../src/lib/types";
import { installMockTauri } from "./mock-tauri";

const conversationId = "conversation-fixture";
const storePath = "/src/lib/agent-store.ts";
const draft = '[data-message-id="assistant-fixture"]';

async function openConversation(page: Page) {
  await page.goto("/");
  await page.getByRole("button", { name: /^Agent( 对话)?$/ }).click();
  await page.getByRole("button", { name: "检查 Minecraft 模组", exact: true }).click();
  await page.waitForFunction((id) => (window as any).__AMM_TEST__.calls.some((call: any) =>
    call.command === "replay_agent_events" && call.args.conversationId === id), conversationId);
}

async function submitTurn(page: Page) {
  await page.getByRole("button", { name: "发送消息", exact: true }).click();
  await expect(page.getByRole("button", { name: "停止生成", exact: true })).toBeVisible();
  await expect(page.locator(draft)).toHaveText("正在检查模组文件。");
}

test("keeps historical Markdown intact when a new reply starts and streams", async ({ page }) => {
  await installMockTauri(page, { holdAgentTurn: true });
  await page.addInitScript((id) => {
    const fixture = (window as any).__AMM_TEST__;
    const history = Array.from({ length: 16 }, (_, index) => ({
      seq: index + 1, turnId: `historical-${index}`, conversationId: id,
      messageId: `historical-reply-${index}`, kind: "delta", createdAt: "",
      payload: { reasoning: `历史推理 ${index}`, content: Array.from({ length: 12 }, (_, link) =>
        `- [历史资料 ${index}-${link}](https://example.com/${index}/${link})`).join("\n") },
    } satisfies AgentEvent));
    fixture.seedEvents(history);
  }, conversationId);
  await openConversation(page);
  const links = page.locator('[data-message-id^="historical-reply-"] a');
  await expect(links).toHaveCount(192);
  await page.getByRole("textbox", { name: "告诉 Agent 你想做什么" }).fill("检查模组");
  const previous = await links.elementHandles();
  const attachedLinks = () => page.evaluate((nodes) => nodes.filter((node) => node.isConnected).length, previous);

  await submitTurn(page);
  await expect(page.getByRole("button", { name: /思考中.*进行中/ })).toHaveCount(0);
  await expect(page.getByRole("button", { name: /思考过程.*已完成/ })).toHaveCount(16);
  expect(await attachedLinks(), "starting a turn must not replace links in saved replies").toBe(192);
  await page.evaluate(() => (window as any).__AMM_TEST__.emit("delta", { content: "新回复已到达。" }));
  await expect(page.locator(draft)).toContainText("新回复已到达。");
  expect(await attachedLinks(), "streaming must not replace links in saved replies").toBe(192);
});

test("coalesces a live burst and only publishes durable replay when it fills a gap", async ({ page }) => {
  await installMockTauri(page, { holdAgentTurn: true });
  await openConversation(page);
  await page.getByRole("textbox", { name: "告诉 Agent 你想做什么" }).fill("检查模组");
  await submitTurn(page);

  const counts = await page.evaluate(async ({ id, storePath }) => {
    const { useAgent, replayConversation } = await import(storePath) as typeof import("../../src/lib/agent-store");
    const fixture = (window as any).__AMM_TEST__;
    await replayConversation(id);
    let updates = 0;
    const unsubscribe = useAgent.subscribe((state, previous) => {
      if (state.streams[id] !== previous.streams[id]) updates++;
    });
    try {
      const gap = fixture.persistOnly("delta", { content: "[仅持久化的片段]" }) as AgentEvent;
      let last: AgentEvent = gap;
      for (let i = 0; i < 128; i++) last = fixture.emit("delta", { content: `[${i}]` });
      // A non-delta event must publish the queued text before the terminal state.
      fixture.emit("done", { status: "done" }, id, null);
      const live = updates;
      await replayConversation(id);
      const replay = updates - live;
      const stream = useAgent.getState().streams[id];
      return {
        live, replay,
        gapCount: stream.filter((event) => event.seq === gap.seq).length,
        ordered: stream.every((event, index) => index === 0 || stream[index - 1].seq < event.seq),
        hasLast: stream.some((event) => event.seq === last.seq),
      };
    } finally {
      unsubscribe();
    }
  }, { id: conversationId, storePath });
  expect(counts).toEqual({ live: 1, replay: 1, gapCount: 1, ordered: true, hasLast: true });
  await expect(page.locator(draft)).toHaveText(`正在检查模组文件。[仅持久化的片段]${Array.from({ length: 128 }, (_, i) => `[${i}]`).join("")}`);
  await expect(page.getByRole("button", { name: "发送消息", exact: true })).toBeVisible();

  const republished = await page.evaluate(async ({ id, storePath }) => {
    const { useAgent, replayConversation } = await import(storePath) as typeof import("../../src/lib/agent-store");
    (window as any).__AMM_TEST__.emit("usage", { total_tokens: 20 }, id, null);
    const live = useAgent.getState().streams[id];
    await replayConversation(id);
    return useAgent.getState().streams[id] !== live;
  }, { id: conversationId, storePath });
  expect(republished, "replaying already-delivered events must preserve the snapshot").toBe(false);
});
