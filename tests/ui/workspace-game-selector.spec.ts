import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const workspaceGames = [
  {id: "game-stardew", name: "星露谷物语"},
  {id: "game-cyberpunk", name: "赛博朋克 2077"},
];

const now = "2026-11-23T00:00:00Z";
const conversation = (id: string, title: string, gameId: string | null, workspaceGameIds?: string[]) => ({
  id,
  title,
  game_id: gameId,
  ...(workspaceGameIds === undefined ? {} : {workspace_game_ids: workspaceGameIds}),
  category: null,
  task_summary: null,
  archived: false,
  pinned: false,
  message_count: 0,
  created_at: now,
  updated_at: now,
  permission_mode: null,
});

const workspaceButton = (page: Page) =>
  page.getByRole("button", {name: /^切换工作区/});
const workspaceDialog = (page: Page) =>
  page.getByRole("dialog", {name: "选择工作区游戏", exact: true});
const gameCheckbox = (page: Page, name: string) =>
  workspaceDialog(page).getByRole("checkbox", {name, exact: true});

async function openConversation(page: Page, title: string) {
  await page.getByRole("button", {name: title, exact: true}).click();
  await expect(page.locator(".chat-header h1")).toHaveText(title);
}

async function recordedCalls(page: Page, command: string) {
  return page.evaluate((name) => (window as any).__AMM_TEST__.calls
    .filter((call: any) => call.command === name)
    .map((call: any) => call.args), command);
}

test("new conversations start with every game selected and send an explicit multi-game workspace", async ({page}) => {
  await installMockTauri(page, {workspaceGames});
  await page.goto("/");
  await page.getByRole("button", {name: "新建对话", exact: true}).click();

  const button = workspaceButton(page);
  await expect(button).toBeVisible();
  await button.click();
  await expect(workspaceDialog(page)).toBeVisible();
  await expect(gameCheckbox(page, "Minecraft Java Edition")).toBeChecked();
  await expect(gameCheckbox(page, "星露谷物语")).toBeChecked();
  await expect(gameCheckbox(page, "赛博朋克 2077")).toBeChecked();

  await gameCheckbox(page, "赛博朋克 2077").uncheck();
  expect(await recordedCalls(page, "create_conversation")).toEqual([]);
  expect(await recordedCalls(page, "set_conversation_workspace")).toEqual([]);
  await page.getByRole("button", {name: "应用", exact: true}).click();
  await expect(button).toHaveAccessibleName(/已选 2 款游戏/);

  await page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true}).fill("多游戏工作区创建检查");
  await page.getByRole("button", {name: "发送消息", exact: true}).click();
  await expect.poll(() => recordedCalls(page, "create_conversation")).toHaveLength(1);
  expect(await recordedCalls(page, "create_conversation")).toEqual([{
    title: "",
    gameId: null,
    workspaceGameIds: ["game-fixture", "game-stardew"],
  }]);
});

test("workspace changes apply per conversation and legacy game_id fills a missing workspace field", async ({page}) => {
  const conversationFixtures = [
    conversation("conversation-fixture", "工作区 A", null, ["game-fixture", "game-stardew"]),
    // Old records have no workspace_game_ids column value; keep their legacy game_id binding.
    conversation("conversation-legacy", "旧版工作区 B", "game-cyberpunk"),
  ];
  await installMockTauri(page, {workspaceGames, conversationFixtures});
  await page.goto("/");
  await openConversation(page, "工作区 A");

  const button = workspaceButton(page);
  await button.click();
  await expect(gameCheckbox(page, "Minecraft Java Edition")).toBeChecked();
  await expect(gameCheckbox(page, "星露谷物语")).toBeChecked();
  await expect(gameCheckbox(page, "赛博朋克 2077")).not.toBeChecked();
  await gameCheckbox(page, "星露谷物语").uncheck();
  await gameCheckbox(page, "赛博朋克 2077").check();
  await page.getByRole("button", {name: "应用", exact: true}).click();
  await expect(button).toHaveAccessibleName(/已选 2 款游戏/);
  expect(await recordedCalls(page, "set_conversation_workspace")).toEqual([{
    id: "conversation-fixture",
    gameIds: ["game-fixture", "game-cyberpunk"],
  }]);

  await openConversation(page, "旧版工作区 B");
  await workspaceButton(page).click();
  await expect(gameCheckbox(page, "Minecraft Java Edition")).not.toBeChecked();
  await expect(gameCheckbox(page, "星露谷物语")).not.toBeChecked();
  await expect(gameCheckbox(page, "赛博朋克 2077")).toBeChecked();
  await page.getByRole("button", {name: "取消", exact: true}).click();

  await openConversation(page, "工作区 A");
  await workspaceButton(page).click();
  await expect(gameCheckbox(page, "Minecraft Java Edition")).toBeChecked();
  await expect(gameCheckbox(page, "星露谷物语")).not.toBeChecked();
  await expect(gameCheckbox(page, "赛博朋克 2077")).toBeChecked();
});

test("select-all is staged, cancel discards it, and applying clear persists an empty workspace", async ({page}) => {
  const conversationFixtures = [conversation("conversation-fixture", "旧版工作区", "game-fixture")];
  await installMockTauri(page, {workspaceGames, conversationFixtures});
  await page.goto("/");
  await openConversation(page, "旧版工作区");

  const button = workspaceButton(page);
  await button.click();
  await expect(gameCheckbox(page, "Minecraft Java Edition")).toBeChecked();
  await expect(gameCheckbox(page, "星露谷物语")).not.toBeChecked();
  await expect(gameCheckbox(page, "赛博朋克 2077")).not.toBeChecked();

  await page.getByRole("button", {name: "全选", exact: true}).click();
  await expect(gameCheckbox(page, "星露谷物语")).toBeChecked();
  await expect(gameCheckbox(page, "赛博朋克 2077")).toBeChecked();
  expect(await recordedCalls(page, "set_conversation_workspace")).toEqual([]);
  await page.getByRole("button", {name: "取消", exact: true}).click();

  await button.click();
  await expect(gameCheckbox(page, "Minecraft Java Edition")).toBeChecked();
  await expect(gameCheckbox(page, "星露谷物语")).not.toBeChecked();
  await expect(gameCheckbox(page, "赛博朋克 2077")).not.toBeChecked();
  await page.getByRole("button", {name: "全选", exact: true}).click();
  await page.getByRole("button", {name: "应用", exact: true}).click();
  expect(await recordedCalls(page, "set_conversation_workspace")).toEqual([{
    id: "conversation-fixture",
    gameIds: null,
  }]);

  await button.click();
  await expect(gameCheckbox(page, "Minecraft Java Edition")).toBeChecked();
  await expect(gameCheckbox(page, "星露谷物语")).toBeChecked();
  await expect(gameCheckbox(page, "赛博朋克 2077")).toBeChecked();
  await page.getByRole("button", {name: "清空", exact: true}).click();
  expect(await recordedCalls(page, "set_conversation_workspace")).toEqual([{
    id: "conversation-fixture",
    gameIds: null,
  }]);
  await page.getByRole("button", {name: "应用", exact: true}).click();

  await expect(button).toHaveAccessibleName(/未选择游戏/);
  expect(await recordedCalls(page, "set_conversation_workspace")).toEqual([
    {id: "conversation-fixture", gameIds: null},
    {id: "conversation-fixture", gameIds: []},
  ]);
  await button.click();
  await expect(gameCheckbox(page, "Minecraft Java Edition")).not.toBeChecked();
  await expect(gameCheckbox(page, "星露谷物语")).not.toBeChecked();
  await expect(gameCheckbox(page, "赛博朋克 2077")).not.toBeChecked();
});

test("workspace picker opens from the visible control at desktop and narrow viewport sizes", async ({page}) => {
  await installMockTauri(page, {workspaceGames});
  await page.goto("/");
  await openConversation(page, "检查 Minecraft 模组");

  const button = workspaceButton(page);
  await expect(button).toBeVisible();
  const bounds = await button.boundingBox();
  expect(bounds).not.toBeNull();
  expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(page.viewportSize()!.width);

  await button.click();
  await expect(workspaceDialog(page)).toBeVisible();
  await expect(gameCheckbox(page, "Minecraft Java Edition")).toBeVisible();
});

test("new conversation preserves gameId only when exactly one game is selected", async ({page}) => {
  await installMockTauri(page, {workspaceGames});
  await page.goto("/");
  await page.getByRole("button", {name: "新建对话", exact: true}).click();
  await workspaceButton(page).click();
  await page.getByRole("button", {name: "清空", exact: true}).click();
  await gameCheckbox(page, "星露谷物语").check();
  await page.getByRole("button", {name: "应用", exact: true}).click();

  await page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true}).fill("单游戏工作区创建检查");
  await page.getByRole("button", {name: "发送消息", exact: true}).click();
  await expect.poll(() => recordedCalls(page, "create_conversation")).toHaveLength(1);
  expect(await recordedCalls(page, "create_conversation")).toEqual([{
    title: "",
    gameId: "game-stardew",
    workspaceGameIds: ["game-stardew"],
  }]);
});
