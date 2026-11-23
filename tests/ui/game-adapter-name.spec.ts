import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const gameName = "Minecraft Java Edition";

const calls = (page: Page, command: string) => page.evaluate(
  name => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === name),
  command,
);

function gameCard(page: Page) {
  return page.getByTestId("library-page").locator('[data-slot="card"]').filter({
    has: page.getByRole("heading", {name: gameName, exact: true}),
  });
}

async function openAdapterCreation(page: Page) {
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await gameCard(page).getByRole("button", {name: "创建适配包", exact: true}).click();
}

test("更改游戏库显示名称时保留目标游戏并在适配包绑定完成后刷新游戏库", async ({page}) => {
  await installMockTauri(page, {holdAgentTurn: true});
  await openAdapterCreation(page);

  const name = page.getByRole("textbox", {name: "游戏库显示名称", exact: true});
  const composer = page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true});
  const originalPrompt = await composer.inputValue();
  await expect(name).toHaveValue(gameName);
  await expect(composer).toHaveValue(/Minecraft Java Edition.*创建适配包/);

  const newName = "我的世界 Java ".repeat(80) + "完整版";
  await name.fill(newName);
  await page.getByRole("button", {name: "发送消息", exact: true}).click();

  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(1);
  const request = (await calls(page, "start_turn"))[0].args.request;
  expect((await calls(page, "create_conversation"))[0].args).toMatchObject({gameId: "game-fixture", category: "adapter"});
  expect(request.content).toBe(
    `${originalPrompt}\n\n请在绑定适配包时，将这款游戏在游戏库中的显示名称改为 ${JSON.stringify(newName)}。`,
  );
  expect(await calls(page, "update_game")).toHaveLength(0);
  await expect(page.getByRole("button", {name: "适配包", exact: true})).toHaveAttribute("aria-expanded", "true");
  await expect(page.locator('ul[aria-label="适配包"] button[aria-current="true"]')).toHaveCount(1);

  await page.evaluate(({oldName, targetName}) => {
    const mock = (window as any).__AMM_TEST__;
    mock.emit("tool-start", {
      toolCallId: "tool-bind-fixture",
      providerCallId: "provider-bind-fixture",
      name: "set_game_adapter",
      arguments: {slug: "minecraft-java", game_name: targetName},
    });
    mock.emit("approval-request", {
      approvalId: "approval-bind-fixture",
      toolCallId: "tool-bind-fixture",
      title: "set_game_adapter",
      preview: {gameName: oldName, targetGameName: targetName},
    });
  }, {oldName: gameName, targetName: newName});
  const approval = page.getByRole("group", {name: "操作审批", exact: true});
  await expect(approval).toContainText(`绑定后，游戏库中的「${gameName}」将改名为「${newName}」。`);

  const gameListReadsBeforeCompletion = (await calls(page, "list_games")).length;
  await page.evaluate(targetName => {
    const mock = (window as any).__AMM_TEST__;
    mock.emit("tool-result", {
      toolCallId: "tool-bind-fixture",
      name: "set_game_adapter",
      status: "done",
      result: {status: "bound", gameId: "game-fixture", gameName: targetName, adapterSlug: "minecraft-java", adapterVersion: "1.0.0"},
    });
    mock.emit("done", {status: "done"}, undefined, null);
  }, newName);
  await expect.poll(async () => (await calls(page, "list_games")).length, {timeout: 10_000})
    .toBeGreaterThan(gameListReadsBeforeCompletion);
});

test("名称未更改时原始创建提示不变且不请求 Agent 改名", async ({page}) => {
  await installMockTauri(page, {holdAgentTurn: true});
  await openAdapterCreation(page);

  const name = page.getByRole("textbox", {name: "游戏库显示名称", exact: true});
  const composer = page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true});
  await expect(name).toHaveValue(gameName);
  const originalPrompt = await composer.inputValue();

  await page.getByRole("button", {name: "发送消息", exact: true}).click();
  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(1);

  const request = (await calls(page, "start_turn"))[0].args.request;
  expect((await calls(page, "create_conversation"))[0].args).toMatchObject({gameId: "game-fixture", category: "adapter"});
  expect(request.content).toBe(originalPrompt);
  expect(request.content).not.toMatch(/请在绑定适配包时，将这款游戏在游戏库中的显示名称改为/);
  expect(await calls(page, "update_game")).toHaveLength(0);
});

test("游戏库显示名称为空或含控制字符时阻止发送", async ({page}) => {
  await installMockTauri(page, {holdAgentTurn: true});
  await openAdapterCreation(page);

  const name = page.getByRole("textbox", {name: "游戏库显示名称", exact: true});
  const send = page.getByRole("button", {name: "发送消息", exact: true});

  await name.fill("");
  await expect(name).toHaveAttribute("aria-invalid", "true");
  await expect(send).toBeDisabled();

  await name.fill("无效\u007f名称");
  await expect(name).toHaveAttribute("aria-invalid", "true");
  await expect(send).toBeDisabled();
  expect(await calls(page, "start_turn")).toHaveLength(0);
});

test("通用适配包创建不显示游戏库名称字段", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "插件中心", exact: true}).click();
  await page.getByRole("button", {name: "用 Agent 创建", exact: true}).click();

  await expect(page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true})).toBeVisible();
  await expect(page.getByRole("textbox", {name: "游戏库显示名称", exact: true})).toHaveCount(0);
});
