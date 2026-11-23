import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const conversationId = "conversation-fixture";
const gameId = "game-fixture";
const gameName = "Minecraft Java Edition";
const renamedGame = "Minecraft Adapter Fixture";

const callCount = (page: Page, command: string) => page.evaluate((name) =>
  (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === name).length, command);

async function waitForNextReplay(page: Page) {
  const before = await callCount(page, "replay_agent_events");
  await expect.poll(() => callCount(page, "replay_agent_events")).toBeGreaterThan(before);
}

async function openGameDetail(page: Page, name = gameName) {
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  const card = page.getByTestId("library-page").locator('[data-slot="card"]').filter({
    has: page.getByRole("heading", { name, exact: true }),
  });
  await card.getByRole("button", { name: new RegExp(name) }).click();
  await expect(page.getByTestId("game-detail-page")).toBeVisible();
}

async function openPlugins(page: Page) {
  await page.getByRole("button", { name: "插件中心", exact: true }).click();
  const pluginsPage = page.getByTestId("plugins-page");
  await expect(pluginsPage).toBeVisible();
  await expect(pluginsPage.getByRole("heading", { name: "插件中心", exact: true })).toBeVisible();
  await expect.poll(() => callCount(page, "list_plugins")).toBeGreaterThan(0);
  await expect.poll(() => callCount(page, "list_plugin_drafts")).toBeGreaterThan(0);
}

async function prepareCachedQueries(page: Page, activePage: "detail" | "plugins") {
  await installMockTauri(page, {withExternalAdapter: true});
  await page.goto("/");
  await expect(page.getByRole("button", { name: "游戏库", exact: true })).toBeVisible();

  await openGameDetail(page);
  await expect.poll(() => callCount(page, "list_games")).toBeGreaterThan(0);
  await expect.poll(() => callCount(page, "get_game")).toBeGreaterThan(0);
  await expect.poll(() => callCount(page, "list_mod_library")).toBeGreaterThan(0);

  await page.getByRole("button", { name: "设置", exact: true }).click();
  await page.getByRole("tab", { name: "Skills", exact: true }).click();
  await expect(page.getByRole("button", { name: "导入 Skill", exact: true })).toBeVisible();
  await expect.poll(() => callCount(page, "list_skills")).toBeGreaterThan(0);

  await openPlugins(page);
  if (activePage === "detail") await openGameDetail(page);
  await expect.poll(() => page.evaluate(() => (window as any).__AMM_TEST__.calls.filter(
    (call: any) => call.command === "plugin:event|listen").length)).toBeGreaterThanOrEqual(11);
}

async function emitTurnStart(page: Page, name: string, toolCallId: string) {
  await page.evaluate(({ conversationId, gameId, name, toolCallId }) => {
    const fixture = (window as any).__AMM_TEST__;
    fixture.emit("turn-start", {}, conversationId, null);
    fixture.emit("tool-start", {
      toolCallId,
      name,
      arguments: name === "set_game_adapter"
        ? { slug: "minecraft-java", game_id: gameId }
        : { slug: "fixture-game" },
    }, conversationId, null);
  }, { conversationId, gameId, name, toolCallId });
}

async function emitToolStart(page: Page, name: string, toolCallId: string) {
  await page.evaluate(({ conversationId, gameId, name, toolCallId }) => {
    (window as any).__AMM_TEST__.emit("tool-start", {
      toolCallId,
      name,
      arguments: { slug: "minecraft-java", game_id: gameId },
    }, conversationId, null);
  }, { conversationId, gameId, name, toolCallId });
}

async function emitToolResult(page: Page, name: string, toolCallId: string, status: "done" | "error") {
  await page.evaluate(({ conversationId, gameId, renamedGame, name, toolCallId, status }) => {
    const fixture = (window as any).__AMM_TEST__;
    if (name === "set_game_adapter" && status === "done") {
      const index = fixture.games.findIndex((game: any) => game.id === gameId);
      fixture.games[index] = {...fixture.games[index], name: renamedGame};
    }
    fixture.emit("tool-result", {
      name,
      toolCallId,
      status,
      result: status !== "done"
        ? { error: `${name} rejected` }
        : name === "set_game_adapter"
          ? { status: "bound", gameId, gameName: renamedGame, adapterSlug: "minecraft-java", adapterVersion: "1.0.0" }
          : { slug: "fixture-game" },
    }, conversationId, null);
  }, { conversationId, gameId, renamedGame, name, toolCallId, status });
}

async function emitTurnDone(page: Page) {
  await page.evaluate((id) => {
    const fixture = (window as any).__AMM_TEST__;
    fixture.emit("done", { status: "done" }, id, null);
  }, conversationId);
}

test("does not refresh on tool start or failed adapter binding", async ({ page }) => {
  await prepareCachedQueries(page, "detail");
  const before = {
    game: await callCount(page, "get_game"),
    library: await callCount(page, "list_mod_library"),
    games: await callCount(page, "list_games"),
    skills: await callCount(page, "list_skills"),
  };

  await emitTurnStart(page, "set_game_adapter", "adapter-binding-tool");
  await waitForNextReplay(page);
  expect(await callCount(page, "get_game")).toBe(before.game);
  expect(await callCount(page, "list_mod_library")).toBe(before.library);

  await emitToolResult(page, "set_game_adapter", "adapter-binding-tool", "error");
  await emitTurnDone(page);
  await waitForNextReplay(page);
  expect(await callCount(page, "get_game")).toBe(before.game);
  expect(await callCount(page, "list_mod_library")).toBe(before.library);
  expect(await callCount(page, "list_games")).toBe(before.games);
  expect(await callCount(page, "list_skills")).toBe(before.skills);
});

test("refreshes game and package caches only after successful Agent tool results", async ({ page }) => {
  await prepareCachedQueries(page, "plugins");
  const before = {
    game: await callCount(page, "get_game"),
    library: await callCount(page, "list_mod_library"),
    games: await callCount(page, "list_games"),
    skills: await callCount(page, "list_skills"),
    plugins: await callCount(page, "list_plugins"),
    drafts: await callCount(page, "list_plugin_drafts"),
  };

  await emitTurnStart(page, "install_plugin", "plugin-install-tool");
  await waitForNextReplay(page);
  expect(await callCount(page, "list_plugins")).toBe(before.plugins);
  expect(await callCount(page, "list_plugin_drafts")).toBe(before.drafts);

  await emitToolResult(page, "install_plugin", "plugin-install-tool", "done");
  await expect.poll(() => callCount(page, "list_plugin_drafts")).toBeGreaterThan(before.drafts);
  expect(await callCount(page, "list_plugins")).toBe(before.plugins);

  await emitToolStart(page, "set_game_adapter", "adapter-binding-tool");
  await emitToolResult(page, "set_game_adapter", "adapter-binding-tool", "done");
  await emitTurnDone(page);
  await waitForNextReplay(page);

  await expect.poll(() => callCount(page, "list_plugins")).toBeGreaterThan(before.plugins);
  await expect.poll(() => callCount(page, "list_plugin_drafts")).toBeGreaterThan(before.drafts);

  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await expect.poll(() => callCount(page, "list_games")).toBeGreaterThan(before.games);
  await openGameDetail(page, renamedGame);
  await expect(page.getByTestId("game-detail-page").getByRole("heading", { name: renamedGame, exact: true })).toBeVisible();
  await expect.poll(() => callCount(page, "get_game")).toBeGreaterThan(before.game);
  await expect.poll(() => callCount(page, "list_mod_library")).toBeGreaterThan(before.library);

  await page.getByRole("button", { name: "设置", exact: true }).click();
  await page.getByRole("tab", { name: "Skills", exact: true }).click();
  await expect(page.getByRole("button", { name: "导入 Skill", exact: true })).toBeVisible();
  await expect.poll(() => callCount(page, "list_skills")).toBeGreaterThan(before.skills);
});

for (const { tool, replayOnly } of [
  { tool: "install_plugin", replayOnly: false },
  { tool: "publish_plugin", replayOnly: true },
] as const) {
  test(`${tool} refreshes the draft rows and count before the Agent turn ends`, async ({ page }) => {
    await installMockTauri(page, { duplicatePluginDraftSlug: true });
    await page.goto("/");
    await openPlugins(page);
    await page.getByRole("tab", { name: "草稿 2", exact: true }).click();
    const agentDraft = page.getByRole("row").filter({ hasText: "txn-draft-fixture-duplicate" });
    const manualDraft = page.getByRole("row").filter({ has: page.getByText("txn-draft-fixture", { exact: true }) });
    await expect(agentDraft).toBeVisible();
    await expect(manualDraft).toBeVisible();
    await expect.poll(() => callCount(page, "plugin:event|listen")).toBeGreaterThanOrEqual(11);

    const before = await callCount(page, "list_plugin_drafts");
    await emitTurnStart(page, tool, "draft-tool");
    await page.evaluate(({ conversationId, tool }) => {
      const fixture = (window as any).__AMM_TEST__;
      fixture.emit("tool-result", {
        name: tool, toolCallId: "draft-tool", status: tool === "publish_plugin" ? "done" : "error",
        result: tool === "publish_plugin" ? { state: "unknown" } : { error: "安装失败" },
      }, conversationId, null);
    }, { conversationId, tool });
    await waitForNextReplay(page);
    expect(await callCount(page, "list_plugin_drafts")).toBe(before);
    await expect(agentDraft).toBeVisible();

    await page.evaluate(({ conversationId, tool, replayOnly }) => {
      const fixture = (window as any).__AMM_TEST__;
      // The backend consumes only the Agent draft, even when another draft has the same slug.
      const index = fixture.pluginDrafts.findIndex((draft: { id: string }) => draft.id === "draft-fixture-duplicate");
      fixture.pluginDrafts.splice(index, 1);
      const record = replayOnly ? fixture.persistOnly : fixture.emit;
      record("tool-start", { name: tool, toolCallId: "draft-retry", arguments: { draft_id: "draft-fixture-duplicate" } }, conversationId, null);
      record("tool-result", {
        name: tool, toolCallId: "draft-retry", status: "done",
        result: tool === "publish_plugin" ? { state: "completed" } : { slug: "fixture-game" },
      }, conversationId, null);
    }, { conversationId, tool, replayOnly });

    await expect(page.getByRole("tab", { name: "草稿 1", exact: true })).toBeVisible();
    await expect(agentDraft).toHaveCount(0);
    await expect(manualDraft).toBeVisible();
    expect(await callCount(page, "delete_plugin_draft")).toBe(0);

    await emitTurnDone(page);
    await waitForNextReplay(page);
    expect(await callCount(page, "list_plugin_drafts")).toBe(before + 1);
  });
}
