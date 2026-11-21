import { expect, test } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

test("replays a completed turn while Agent is in the background", async ({page}) => {
  const completion = "已完成后台检查。";

  await installMockTauri(page, {holdAgentTurn: true});
  await page.goto("/");
  await page.getByRole("button", {name: "检查 Minecraft 模组", exact: true}).click();

  await page.getByRole("textbox", {name: "告诉 Agent 你想做什么"}).fill("检查 Minecraft 模组");
  await page.getByRole("button", {name: "发送消息", exact: true}).click();
  await expect(page.getByRole("button", {name: "停止生成", exact: true})).toBeVisible();

  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await expect(page.getByRole("button", {name: "游戏库", exact: true})).toHaveAttribute("aria-current", "page");

  await page.evaluate(() => {
    (window as any).__AMM_TEST__.persistOnly("delta", {content: "第一步已完成", reasoning: ""});
  });
  await page.getByRole("button", {name: /^Agent( 对话)?$/}).click();
  await expect(page.locator('[data-message-id="assistant-fixture"]')).toContainText("第一步已完成");
  await expect(page.getByRole("button", {name: "停止生成", exact: true})).toBeVisible();
  await page.getByRole("button", {name: "新建对话", exact: true}).click();
  await page.getByRole("button", {name: "游戏库", exact: true}).click();

  const done = await page.evaluate((completion) => {
    const test = (window as any).__AMM_TEST__;
    test.persistOnly("reply-reset", {});
    test.persistOnly("delta", {content: completion, reasoning: ""});
    return test.persistOnly("done", {status: "done"}, "conversation-fixture", null);
  }, completion);

  await page.waitForFunction((doneSeq) => (
    (window as any).__AMM_TEST__.replayedEvents.some((event: any) => event.seq === doneSeq && event.kind === "done")
  ), done.seq);
  await expect(page.getByRole("button", {name: "游戏库", exact: true})).toHaveAttribute("aria-current", "page");
  await page.getByRole("button", {name: /^Agent( 对话)?$/}).click();
  await page.getByRole("button", {name: "检查 Minecraft 模组", exact: true}).click();
  const reply = page.locator('[data-message-id="assistant-fixture"]');
  await expect(reply).toContainText(completion);
  await expect(reply).not.toContainText("正在检查模组文件。");
  await expect(page.getByRole("button", {name: "发送消息", exact: true})).toBeVisible();
  expect(await page.evaluate(() => (
    (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "cancel_turn")
  ))).toHaveLength(0);
});
