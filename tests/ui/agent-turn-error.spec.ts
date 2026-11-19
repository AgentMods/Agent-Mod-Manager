import { expect, test } from "@playwright/test";
import type { AgentEvent } from "../../src/lib/types";
import { installMockTauri } from "./mock-tauri";

test("renders a provider turn error live and after durable replay", async ({page}) => {
  const partialReply = "已完成部分分析。";
  const providerError = JSON.stringify({
    status: 502,
    code: "provider_bad_gateway",
    message: "上游模型服务返回错误",
    detail: "gateway timeout while contacting provider",
  }, null, 2);

  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "Agent 对话", exact: true}).click();
  await page.getByRole("button", {name: "检查 Minecraft 模组", exact: true}).click();

  await expect.poll(async () => page.evaluate(() => (
    (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "replay_agent_events").length
  ))).toBeGreaterThan(0);

  await page.evaluate(({partialReply, providerError}) => {
    const test = (window as any).__AMM_TEST__;
    test.emit("turn-start", {}, "conversation-fixture", null);
    test.emit("delta", {content: partialReply, reasoning: ""}, "conversation-fixture", "assistant-error-fixture");
    test.emit("error", {message: providerError}, "conversation-fixture", "assistant-error-fixture");
    test.emit("done", {status: "error"}, "conversation-fixture", null);
  }, {partialReply, providerError});

  const assistant = page.locator("article.chat-message.assistant").filter({
    has: page.locator('[data-message-id="assistant-error-fixture"]'),
  });
  const error = assistant.locator(".agent-turn-error");
  await expect(assistant).toContainText(partialReply);
  await expect(error).toHaveCount(1);
  await expect(error).toHaveAttribute("role", "alert");
  await expect(error.locator("pre code")).toHaveText(providerError);
  await expect(error).toContainText('"status": 502');
  await expect(error).toContainText('"code": "provider_bad_gateway"');
  await expect(error).toContainText('"message": "上游模型服务返回错误"');
  await expect(error).toContainText('"detail": "gateway timeout while contacting provider"');

  const persisted = await page.evaluate<AgentEvent[]>(() => (window as any).__AMM_TEST__.events);
  await page.addInitScript((events) => {
    window.addEventListener("DOMContentLoaded", () => {
      (window as any).__AMM_TEST__.events.push(...events);
    }, {once: true});
  }, persisted);
  await page.reload();
  await page.getByRole("button", {name: "Agent 对话", exact: true}).click();
  await page.getByRole("button", {name: "检查 Minecraft 模组", exact: true}).click();

  await expect(error).toHaveCount(1);
  await expect(error.locator("pre code")).toHaveText(providerError);
  await expect(assistant).toContainText(partialReply);
});
