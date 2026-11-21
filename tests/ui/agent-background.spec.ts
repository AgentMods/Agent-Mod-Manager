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

test("background pending approval shortcut returns to its inline authorization", async ({page}) => {
  await installMockTauri(page, {holdAgentTurn: true});
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: /^Agent( 对话)?$/}).click();
  await page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true}).fill("检查文件");
  await page.getByRole("button", {name: "发送消息", exact: true}).click();
  await expect(page.getByRole("button", {name: "停止生成", exact: true})).toBeVisible();

  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.evaluate(() => {
    const fixture = (window as any).__AMM_TEST__;
    const preview = {
      id: "txn-background-fixture",
      summary: "检查后台文件计划",
      total_bytes: 1024,
      created_at: "2026-11-21T00:00:00Z",
      entries: [{operation: {op: "write", dst: "/sandbox/minecraft/mods/Example.jar"}, impact: "write", before: null, source: null}],
    };
    fixture.emit("tool-start", {toolCallId: "tool-fixture", name: "apply_file_ops", arguments: {plan_id: preview.id}});
    fixture.emit("approval-request", {approvalId: "approval-fixture", toolCallId: "tool-fixture", title: "apply_file_ops", preview});
  });

  const shortcut = page.getByRole("button", {name: "查看待授权操作", exact: true});
  await expect(shortcut).toBeVisible();
  await shortcut.click();
  const approval = page.getByRole("group", {name: "操作审批", exact: true});
  await expect(page.getByRole("dialog", {name: "操作审批", exact: true})).toHaveCount(0);
  await expect(approval).toContainText("/sandbox/minecraft/mods/Example.jar");
  await approval.getByRole("button", {name: "批准并执行", exact: true}).click();
  await expect(page.getByRole("button", {name: "apply_file_ops 已执行", exact: true})).toBeVisible();
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "respond_approval").at(-1)?.args.response))
    .toEqual({approved: true, answer: null});
});
