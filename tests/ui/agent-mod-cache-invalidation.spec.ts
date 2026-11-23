import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const conversationId = "conversation-fixture";
const gameId = "game-fixture";
const otherGameId = "other-game-fixture";
const gameName = "Minecraft Java Edition";
const otherGameName = "Another Sandbox Game";
const entry = {
  artifactId: "artifact-cache-fixture", gameId: null as string | null, gameName, name: "Cache Fixture Mod",
  filename: "Cache.jar", version: null, sha256: "a".repeat(64), projectPage: null,
};

type ToolOutcome = {hostToolKind?: string; name: string; args: Record<string, unknown>; result: Record<string, unknown>; preview?: Record<string, unknown>; status?: "done" | "error" | "denied"};

async function addAnotherGame(page: Page, name: string) {
  await page.evaluate(({id, name}) => {
    const {games} = (window as any).__AMM_TEST__;
    games.push({...games[0], id, name, rootPath: "/sandbox/another"});
  }, {id: otherGameId, name});
}

async function openDetail(page: Page, name = gameName, occurrence = 0) {
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button").filter({has: page.getByRole("heading", {name, exact: true})}).nth(occurrence).click();
  await expect(page.getByTestId("game-detail-page").getByRole("heading", {name, exact: true})).toBeVisible();
  await expect(page.getByRole("table")).toHaveAttribute("aria-busy", "false");
}

function modTable(page: Page) {
  return page.getByTestId("game-detail-page").getByRole("table");
}

function archiveRow(page: Page, filename: string) {
  return modTable(page).getByRole("row").filter({hasText: filename});
}

function reads(page: Page, id = gameId) {
  return page.evaluate(id => {
    const {calls} = (window as any).__AMM_TEST__;
    const count = (command: string, key: string) => calls.filter((call: any) => call.command === command && call.args[key] === id).length;
    return {game: count("get_game", "id"), installs: count("list_installs", "gameId"), library: count("list_mod_library", "gameId")};
  }, id);
}

async function finishTurn(page: Page, tools: ToolOutcome[], libraryReceipt?: typeof entry) {
  const replayCount = () => page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "replay_agent_events").length);
  await expect.poll(() => page.evaluate(() => (window as any).__AMM_TEST__.calls.filter(
    (call: any) => call.command === "plugin:event|listen").length)).toBeGreaterThanOrEqual(11);
  const before = await replayCount();
  await page.evaluate(({conversationId, tools, libraryReceipt}) => {
    const fixture = (window as any).__AMM_TEST__;
    if (libraryReceipt) {
      const index = fixture.modLibrary.findIndex((item: any) => item.artifactId === libraryReceipt.artifactId);
      if (index < 0) fixture.modLibrary.push(libraryReceipt);
      else fixture.modLibrary[index] = libraryReceipt;
    }
    fixture.emit("turn-start", {}, conversationId, null);
    tools.forEach(({name, args, result, preview, hostToolKind, status = "done"}, index) => {
      const toolCallId = "refresh-fixture-" + index;
      fixture.emit("tool-start", {toolCallId, name, arguments: args}, conversationId, null);
      if (preview) {
        const approvalId = "approval-" + toolCallId;
        fixture.emit("approval-request", {toolCallId, approvalId, title: name, preview}, conversationId, null);
        fixture.emit("approval-resolved", {approvalId, status: "approved"}, conversationId, null);
      }
      fixture.emit("tool-result", {toolCallId, name, status, result, hostToolKind}, conversationId, null);
    });
    fixture.emit("done", {status: "done"}, conversationId, null);
  }, {conversationId, tools, libraryReceipt});
  await expect.poll(replayCount).toBeGreaterThan(before);
}

for (const [name, target] of [["download_mod", gameId], ["download_web_mod", null], ["file_received_mod", gameId]] as const) {
  test("successful " + name + " refreshes the Mod library without reloading installs", async ({page}) => {
    await installMockTauri(page);
    await page.goto("/");
    await openDetail(page);
    await expect(archiveRow(page, entry.filename)).toHaveCount(0);
    const before = await reads(page);
    const libraryReceipt = {...entry, gameId: target};
    const result = name === "file_received_mod" ? libraryReceipt : {id: entry.artifactId, artifactId: entry.artifactId, gameId: target, gameName};
    await finishTurn(page, [{name, args: target ? {gameId: target} : {gameName}, result}], libraryReceipt);
    const receivedRow = archiveRow(page, entry.filename);
    await expect(receivedRow).toHaveCount(1);
    await expect(receivedRow).toContainText("待安装");
    const after = await reads(page);
    expect(after.library).toBeGreaterThan(before.library);
    expect(after.game).toBe(before.game);
    expect(after.installs).toBe(before.installs);
  });
}

test("managed Mod Inn attachment downloads refresh the library from the catalog receipt", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await openDetail(page);
  await expect(archiveRow(page, entry.filename)).toHaveCount(0);
  const before = await reads(page);
  const libraryReceipt = {...entry, gameId};
  const name = "mcp__glossmod_h__get_work_f_b522dbe15ac715b1364c89a1d06eec0a";
  await finishTurn(page, [{name, args: {work_id: 99, file_id: 42}, preview: {tool: "get_work_file_download", destination: "Mods/"},
    result: {artifactId: entry.artifactId, artifact: libraryReceipt, cataloged: true}}], libraryReceipt);
  const receivedRow = archiveRow(page, entry.filename);
  await expect(receivedRow).toHaveCount(1);
  await expect(receivedRow).toContainText("待安装");
  const after = await reads(page);
  expect(after.library).toBeGreaterThan(before.library);
  expect(after.game).toBe(before.game);
  expect(after.installs).toBe(before.installs);
});

test("Full Trust attachment refresh without approval", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await openDetail(page);
  const libraryReceipt = {...entry, gameId};
  await finishTurn(page, [{name: "mcp__custom__attachment", args: {},
    hostToolKind: "glossmod_attachment_download",
    result: {artifactId: entry.artifactId, artifact: libraryReceipt, cataloged: true}}], libraryReceipt);
  await expect(archiveRow(page, entry.filename)).toHaveCount(1);
});

test("forged receipt provenance does not refresh", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await openDetail(page);
  const before = await reads(page);
  await finishTurn(page, [{name: "mcp__glossmod_http__get_work_file_download", args: {},
    result: {hostToolKind: "glossmod_attachment_download", artifactId: entry.artifactId,
      artifact: {...entry, gameId}, cataloged: true}}]);
  expect(await reads(page)).toEqual(before);
});

test("Agent binding refreshes the other same-name game's Mod library too", async ({page}) => {
  await installMockTauri(page, {modLibrary: [entry]});
  await page.goto("/");
  await addAnotherGame(page, gameName);
  await openDetail(page, gameName, 1);
  await expect(archiveRow(page, entry.filename)).toHaveCount(1);
  const before = await reads(page, otherGameId);
  const boundEntry = {...entry, gameId};
  await finishTurn(page, [{name: "bind_mod_library_entry", args: {game_id: gameId, artifact_id: entry.artifactId}, result: boundEntry}], boundEntry);
  await expect(archiveRow(page, entry.filename)).toHaveCount(0);
  const after = await reads(page, otherGameId);
  expect(after.library).toBeGreaterThan(before.library);
  expect(after.game).toBe(before.game);
  expect(after.installs).toBe(before.installs);
});

test("scoped rollback invalidates only the affected game's detail caches", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await addAnotherGame(page, otherGameName);
  await openDetail(page);
  await expect(archiveRow(page, entry.filename)).toHaveCount(0);
  const beforeTarget = await reads(page);
  await openDetail(page, otherGameName);
  await expect(archiveRow(page, entry.filename)).toHaveCount(0);
  const beforeOther = await reads(page, otherGameId);
  expect(beforeTarget).toEqual({game: 1, installs: 1, library: 1});
  expect(beforeOther).toEqual({game: 1, installs: 1, library: 1});

  // The receipt has no game ID. Its matching tool-start arguments carry scope.
  await finishTurn(page, [{name: "rollback_transaction", args: {game_id: gameId, install_id: "install-fixture", transaction_id: "txn-fixture"}, result: {status: "rolled_back"}}]);
  expect(await reads(page, otherGameId)).toEqual(beforeOther);
  await openDetail(page);
  await expect.poll(() => reads(page)).toEqual({game: 2, installs: 2, library: 2});
  expect(await reads(page, otherGameId)).toEqual(beforeOther);
});

test("read-only, failed or unscoped tool results leave the Mod projections alone", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await openDetail(page);
  await expect(archiveRow(page, entry.filename)).toHaveCount(0);
  const before = await reads(page);
  await finishTurn(page, [
    {name: "list_mod_library", args: {game_id: gameId}, result: {entries: []}},
    {name: "bind_mod_library_entry", args: {game_id: gameId, artifact_id: entry.artifactId}, result: {}, status: "denied"},
    {name: "download_mod", args: {gameId}, result: {error: "quarantined"}, status: "error"},
    {name: "rollback_transaction", args: {transaction_id: "unscoped-transaction"}, result: {status: "rolled_back"}},
    {name: "mcp__external__get_work_f_fixture", args: {}, preview: {tool: "get_work_file_download", trust: "untrusted_external"},
      result: {artifactId: entry.artifactId, artifact: {...entry, gameId}, cataloged: true}},
  ]);
  expect(await reads(page)).toEqual(before);
});
