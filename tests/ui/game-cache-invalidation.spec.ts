import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const conversationId = "conversation-fixture";
const gameId = "game-fixture";
const now = "2026-11-23T00:00:00Z";
const gameConversation = {
  id: conversationId,
  title: "检查 Minecraft 模组",
  game_id: gameId,
  workspace_game_ids: [gameId],
  category: null,
  task_summary: null,
  archived: false,
  pinned: false,
  message_count: 0,
  created_at: now,
  updated_at: now,
  permission_mode: null,
};

const callCount = (page: Page, command: string) => page.evaluate((name) =>
  (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === name).length, command);

async function openLibrary(page: Page) {
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await expect(page.getByTestId("library-page")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Minecraft Java Edition", exact: true })).toBeVisible();
}

async function openGameDetail(page: Page, name: string) {
  await page.getByRole("button").filter({ has: page.getByRole("heading", { name, exact: true }) }).click();
  await expect(page.getByTestId("game-detail-page")).toBeVisible();
}

async function emitMcpMutation(page: Page, name: string, args: Record<string, unknown>, result: unknown) {
  await page.evaluate(({ conversationId, name, args, result }) => {
    const fixture = (window as any).__AMM_TEST__;
    const toolCallId = `mcp-${name}`;
    fixture.emit("turn-start", { origin: "mcp" }, conversationId, null);
    fixture.emit("tool-start", { toolCallId, name, arguments: args, origin: "mcp" }, conversationId, null);
    if (name === "register_game") {
      const game = { ...fixture.games[0], id: "game-mcp-added", name: "Registered by MCP", rootPath: "/sandbox/mcp-added" };
      fixture.games.push(game);
    } else if (name === "update_game") {
      const game = fixture.games.find((item: any) => item.id === (args.id ?? args.game_id));
      if (game) game.name = "Updated by MCP";
    } else if (name === "remove_game") {
      const index = fixture.games.findIndex((item: any) => item.id === (args.id ?? args.game_id));
      if (index >= 0) fixture.games.splice(index, 1);
      const conversation = fixture.conversations.find((item: any) => item.id === conversationId);
      if (conversation) {
        conversation.game_id = null;
        conversation.workspace_game_ids = [];
      }
    } else if (name === "set_game_adapter") {
      const game = fixture.games.find((item: any) => item.id === (args.id ?? args.game_id));
      if (game) {
        game.adapterSlug = "minecraft-java";
        game.adapterVersion = "1.0.0";
      }
    }
    fixture.emit("tool-result", { toolCallId, name, status: "done", result }, conversationId, null);
  }, { conversationId, name, args, result });
}

async function finishMcpTurn(page: Page) {
  await page.evaluate((id) => {
    (window as any).__AMM_TEST__.emit("done", { status: "done", origin: "mcp" }, id, null);
  }, conversationId);
}

async function waitForReplayAfter(page: Page, before: number) {
  await expect.poll(() => callCount(page, "replay_agent_events")).toBeGreaterThan(before);
}

test("register_game event refreshes the library and its workspace game choices", async ({ page }) => {
  await installMockTauri(page);
  await page.goto("/");
  await openLibrary(page);
  const beforeGames = await callCount(page, "list_games");
  const beforeReplay = await callCount(page, "replay_agent_events");

  await emitMcpMutation(page, "register_game", { directory: "/sandbox/mcp-added" }, { gameId: "game-mcp-added" });
  await expect.poll(() => callCount(page, "list_games")).toBeGreaterThan(beforeGames);
  await expect(page.getByRole("heading", { name: "Registered by MCP", exact: true })).toBeVisible();
  await finishMcpTurn(page);
  await waitForReplayAfter(page, beforeReplay);

  await page.getByRole("button", { name: "Agent", exact: true }).click();
  await page.getByRole("button", { name: "检查 Minecraft 模组", exact: true }).click();
  await page.getByRole("button", { name: /切换工作区/ }).click();
  await expect(page.getByRole("dialog", { name: "选择工作区游戏", exact: true })
    .getByRole("checkbox", { name: "Registered by MCP", exact: true })).toBeVisible();
});

test("update_game event refreshes the game card and detail cache", async ({ page }) => {
  await installMockTauri(page);
  await page.goto("/");
  await openLibrary(page);
  await openGameDetail(page, "Minecraft Java Edition");
  await expect(page.getByTestId("game-detail-page").getByRole("heading", { name: "Minecraft Java Edition", exact: true })).toBeVisible();
  await expect.poll(() => callCount(page, "list_mod_library")).toBeGreaterThan(0);
  await page.getByRole("button", { name: "游戏库", exact: true }).click();

  const beforeGames = await callCount(page, "list_games");
  const beforeGame = await callCount(page, "get_game");
  const beforeLibrary = await callCount(page, "list_mod_library");
  const beforeReplay = await callCount(page, "replay_agent_events");
  await emitMcpMutation(page, "update_game", { id: gameId }, { gameId, name: "Updated by MCP" });
  await expect.poll(() => callCount(page, "list_games")).toBeGreaterThan(beforeGames);
  await expect(page.getByRole("heading", { name: "Updated by MCP", exact: true })).toBeVisible();
  await finishMcpTurn(page);
  await waitForReplayAfter(page, beforeReplay);

  await page.getByRole("button").filter({ has: page.getByRole("heading", { name: "Updated by MCP", exact: true }) }).click();
  await expect(page.getByTestId("game-detail-page").getByRole("heading", { name: "Updated by MCP", exact: true })).toBeVisible();
  await expect.poll(() => callCount(page, "get_game")).toBeGreaterThan(beforeGame);
  await expect.poll(() => callCount(page, "list_mod_library")).toBeGreaterThan(beforeLibrary);
});

test("remove_game event refreshes the library and saved conversation workspaces", async ({ page }) => {
  await installMockTauri(page, {
    gameWithoutMods: true,
    conversationFixtures: [gameConversation],
  });
  await page.goto("/");
  await page.getByRole("button", { name: "检查 Minecraft 模组", exact: true }).click();
  await expect(page.getByRole("button", { name: /切换工作区：Minecraft Java Edition/ })).toBeVisible();
  await openLibrary(page);
  const beforeGames = await callCount(page, "list_games");
  const beforeConversations = await callCount(page, "list_conversations");
  const beforeConversation = await callCount(page, "get_conversation");
  const beforeReplay = await callCount(page, "replay_agent_events");

  await openGameDetail(page, "Minecraft Java Edition");
  await expect(page.getByTestId("game-detail-page").getByRole("heading", { name: "Minecraft Java Edition", exact: true })).toBeVisible();
  await emitMcpMutation(page, "remove_game", { id: gameId }, { gameId });
  await expect(page.getByTestId("library-page")).toBeVisible();
  await expect.poll(() => callCount(page, "list_games")).toBeGreaterThan(beforeGames);
  await expect(page.getByText("尚无管理中的游戏", { exact: true })).toBeVisible();
  await finishMcpTurn(page);
  await waitForReplayAfter(page, beforeReplay);

  await page.getByRole("button", { name: "Agent", exact: true }).click();
  await expect(page.getByRole("button", { name: /切换工作区：未选择游戏/ })).toBeVisible();
  await expect.poll(() => callCount(page, "list_conversations")).toBeGreaterThan(beforeConversations);
  await expect.poll(() => callCount(page, "get_conversation")).toBeGreaterThan(beforeConversation);
});

test("set_game_adapter event refreshes game, detail and Mod library caches", async ({ page }) => {
  await installMockTauri(page);
  await page.goto("/");
  await openLibrary(page);
  await openGameDetail(page, "Minecraft Java Edition");
  await expect(page.getByTestId("game-detail-page").getByRole("heading", { name: "Minecraft Java Edition", exact: true })).toBeVisible();
  await expect.poll(() => callCount(page, "list_mod_library")).toBeGreaterThan(0);
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  const beforeGames = await callCount(page, "list_games");
  const beforeGame = await callCount(page, "get_game");
  const beforeLibrary = await callCount(page, "list_mod_library");
  const beforeReplay = await callCount(page, "replay_agent_events");

  await emitMcpMutation(page, "set_game_adapter", { game_id: gameId }, { status: "bound", gameId });
  await expect.poll(() => callCount(page, "list_games")).toBeGreaterThan(beforeGames);
  await finishMcpTurn(page);
  await waitForReplayAfter(page, beforeReplay);
  await openGameDetail(page, "Minecraft Java Edition");
  await expect(page.getByTestId("game-detail-page").getByRole("heading", { name: "Minecraft Java Edition", exact: true })).toBeVisible();
  await expect.poll(() => callCount(page, "get_game")).toBeGreaterThan(beforeGame);
  await expect.poll(() => callCount(page, "list_mod_library")).toBeGreaterThan(beforeLibrary);
});

test("set_game_adapter refreshes when its result arrives through durable replay", async ({ page }) => {
  await installMockTauri(page);
  await page.goto("/");
  await openLibrary(page);
  await expect.poll(() => page.evaluate(() => (window as any).__AMM_TEST__.calls.filter(
    (call: any) => call.command === "plugin:event|listen").length)).toBeGreaterThanOrEqual(11);

  const beforeGames = await callCount(page, "list_games");
  const beforeReplay = await callCount(page, "replay_agent_events");
  await page.evaluate(({ conversationId, gameId }) => {
    const fixture = (window as any).__AMM_TEST__;
    const toolCallId = "adapter-binding-replay";
    fixture.emit("turn-start", { origin: "mcp" }, conversationId, null);
    fixture.emit("tool-start", {
      toolCallId,
      name: "set_game_adapter",
      arguments: { game_id: gameId, slug: "minecraft-java" },
      origin: "mcp",
    }, conversationId, null);
    const index = fixture.games.findIndex((game: any) => game.id === gameId);
    fixture.games[index] = { ...fixture.games[index], adapterSlug: "minecraft-java", adapterVersion: "1.0.0" };
    // Persist the actual MCP result but simulate loss of its live event delivery.
    fixture.persistOnly("tool-result", {
      name: "set_game_adapter", toolCallId, status: "done", result: { status: "bound", gameId },
    }, conversationId, null);
  }, { conversationId, gameId });

  await expect.poll(() => callCount(page, "list_games"), { timeout: 10_000 }).toBeGreaterThan(beforeGames);
  await finishMcpTurn(page);
  await expect.poll(() => callCount(page, "replay_agent_events")).toBeGreaterThan(beforeReplay);
});

test("launch_game does not refresh game caches", async ({ page }) => {
  await installMockTauri(page, { gameLaunchPath: "/sandbox/minecraft/launcher.exe" });
  await page.goto("/");
  await openLibrary(page);
  const beforeGames = await callCount(page, "list_games");
  await page.getByRole("button", { name: "启动游戏", exact: true }).click();
  await expect.poll(() => callCount(page, "launch_game")).toBe(1);
  expect(await callCount(page, "list_games")).toBe(beforeGames);

  const beforeReplay = await callCount(page, "replay_agent_events");
  await emitMcpMutation(page, "launch_game", { id: gameId }, { gameId });
  expect(await callCount(page, "list_games")).toBe(beforeGames);
  await finishMcpTurn(page);
  await waitForReplayAfter(page, beforeReplay);
});
