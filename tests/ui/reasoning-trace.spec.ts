import { expect, test } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

test("collapses completed reasoning and allows reopening", async ({ page }) => {
  const reasoning = "先读取适配包编写技能，并检查现有适配包清单。";

  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: /^Agent( 对话)?$/}).click();
  await page.getByRole("button", { name: "检查 Minecraft 模组", exact: true }).click();

  await expect.poll(async () => page.evaluate(() => (
    (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "replay_agent_events").length
  ))).toBeGreaterThan(0);

  await page.evaluate((reasoning) => {
    const test = (window as any).__AMM_TEST__;
    test.emit("turn-start", {}, "conversation-fixture", null);
    test.emit("delta", { content: "", reasoning }, "conversation-fixture", "assistant-fixture");
  }, reasoning);

  const reasoningText = page.getByText(reasoning, { exact: true });
  const runningTrigger = page.getByRole("button", { name: /思考中.*进行中/ });
  await expect(reasoningText).toBeVisible();
  await expect(runningTrigger).toBeVisible();

  await page.evaluate(() => {
    const test = (window as any).__AMM_TEST__;
    test.emit("done", { status: "done" }, "conversation-fixture", null);
  });

  const completedTrigger = page.getByRole("button", { name: /思考过程.*已完成/ });
  await expect(completedTrigger).toBeVisible();
  await expect(reasoningText).toBeHidden();

  await completedTrigger.click();
  await expect(reasoningText).toBeVisible();
});