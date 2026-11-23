import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";
import {
  categoryConversationFixtures,
  installConversationCategoryFixture,
} from "./conversation-category-fixture";

const categories = [
  "安装 Mod",
  "卸载 Mod",
  "删除 Mod",
  "适配包",
] as const;

const groupButton = (page: Page, name: string) => page.getByRole("button", {name, exact: true});

test("operation groups start collapsed, support keyboard control, and preserve a selected conversation when closed", async ({page}) => {
  await installConversationCategoryFixture(page);
  await page.goto("/");

  for (const category of categories) {
    const button = groupButton(page, category);
    await expect(button).toBeVisible();
    await expect(button).toHaveAttribute("aria-expanded", "false");
  }

  const install = groupButton(page, "安装 Mod");
  await install.focus();
  await page.keyboard.press("Enter");
  await expect(install).toHaveAttribute("aria-expanded", "true");
  await expect(page.locator('ul[aria-label="安装 Mod"] > li')).toHaveCount(2);
  await expect(page.getByRole("button", {name: "安装任务", exact: true})).toBeVisible();
  const secondRow = page.locator('ul[aria-label="安装 Mod"] > li')
    .filter({has: page.getByRole("button", {name: "安装 Mod：性能补丁", exact: true})});
  await expect(secondRow.getByRole("button", {name: "安装 Mod：性能补丁", exact: true})).toHaveText("性能补丁");
  await secondRow.getByRole("button", {name: "对话操作：安装 Mod：性能补丁", exact: true}).click();
  await expect(page.getByRole("menuitem", {name: "重命名", exact: true})).toBeVisible();
  await page.keyboard.press("Escape");
  await install.focus();
  await page.keyboard.press("Space");
  await expect(install).toHaveAttribute("aria-expanded", "false");

  await install.click();
  const selected = page.getByRole("button", {name: "安装任务", exact: true});
  await selected.click();
  await expect(selected).toHaveAttribute("aria-current", "true");
  await expect(page.locator(".chat-header h1")).toHaveText("安装任务");

  await install.click();
  await expect(install).toHaveAttribute("aria-expanded", "false");
  await expect(page.locator(".chat-header h1")).toHaveText("安装任务");
  await install.click();
  await expect(page.locator('ul[aria-label="安装 Mod"] button[aria-current="true"]')).toHaveCount(1);
  await page.getByRole("button", {name: "安装 Mod：性能补丁", exact: true}).click();
  await expect(page.locator(".chat-header h1")).toHaveText("安装 Mod：性能补丁");
  await expect(page.locator('ul[aria-label="安装 Mod"] button[aria-current="true"]')).toHaveAccessibleName("安装 Mod：性能补丁");
});

test("category expansion preferences persist and empty categories stay hidden", async ({page}) => {
  await installConversationCategoryFixture(page, [
    {id: "conversation-adapter-only", title: "适配包任务", category: "adapter"},
  ]);
  await page.goto("/");

  for (const category of categories.filter(name => name !== "适配包")) {
    await expect(groupButton(page, category)).toHaveCount(0);
  }

  const adapter = groupButton(page, "适配包");
  await expect(adapter).toHaveAttribute("aria-expanded", "false");
  await adapter.click();
  await expect(adapter).toHaveAttribute("aria-expanded", "true");
  await page.reload();
  await expect(groupButton(page, "适配包")).toHaveAttribute("aria-expanded", "true");

  await groupButton(page, "适配包").click();
  await expect(groupButton(page, "适配包")).toHaveAttribute("aria-expanded", "false");
  await page.reload();
  await expect(groupButton(page, "适配包")).toHaveAttribute("aria-expanded", "false");
});

test("ordinary and pinned conversations remain directly available in their existing lists", async ({page}) => {
  await installConversationCategoryFixture(page, categoryConversationFixtures);
  await page.goto("/");

  const recent = page.getByRole("list", {name: "最近对话", exact: true});
  const pinned = page.getByRole("list", {name: "置顶对话", exact: true});
  await expect(recent.getByRole("button", {name: "普通对话", exact: true})).toBeVisible();
  await expect(recent.getByRole("button", {name: "旧版普通对话", exact: true})).toBeVisible();
  await expect(pinned.getByRole("button", {name: "置顶对话", exact: true})).toBeVisible();

  await recent.getByRole("button", {name: "普通对话", exact: true}).click();
  await expect(page.locator(".chat-header h1")).toHaveText("普通对话");
  await pinned.getByRole("button", {name: "置顶对话", exact: true}).click();
  await expect(page.locator(".chat-header h1")).toHaveText("置顶对话");
});

test("ordinary conversation creation defaults to all games in the workspace", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");

  await page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true}).fill("普通对话创建检查");
  await page.getByRole("button", {name: "发送消息", exact: true}).click();
  await expect.poll(async () => page.evaluate(() => (
    (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "create_conversation").length
  ))).toBe(1);

  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls
    .filter((call: any) => call.command === "create_conversation"));
  expect(calls.map((call: any) => call.args)).toEqual([{title: "", gameId: null, workspaceGameIds: null}]);
});
