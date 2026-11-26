import { expect, test, type Page } from "@playwright/test";
import { BROWSE_ITEMS, browseCalls, installBrowseFixture, type BrowsePageFixture } from "./browse-fixture";

async function openFeed(page: Page, pages: BrowsePageFixture[]) {
  await installBrowseFixture(page, { workspaceGames: [], pages });
  await page.goto("/");
  await page.getByRole("button", { name: "游览", exact: true }).click();
  await expect.poll(async () => (await browseCalls(page, "browse_authorize")).length).toBe(1);
  await expect.poll(async () => (await browseCalls(page, "browse_start_search")).length).toBe(1);
}

test("作品封面和可聚焦标题打开同一作品，缺少网页的卡片仍可独立安装", async ({ page }) => {
  const linked = BROWSE_ITEMS[0];
  const unlinked = { ...BROWSE_ITEMS[1], gameIds: linked.gameIds, canOpenProject: false };
  await openFeed(page, [{ items: [linked, unlinked] }]);
  const card = page.getByRole("article", { name: linked.title, exact: true });
  await card.locator(":scope > button").click();
  const title = card.getByRole("heading").getByRole("button");
  await title.focus();
  await title.press("Enter");
  const openCalls = await browseCalls(page, "browse_open_project");
  expect(openCalls.map(call => call.args)).toEqual(Array(2).fill({ searchId: "browse-search-1", itemId: linked.id }));
  const other = page.getByRole("article", { name: unlinked.title, exact: true });
  await expect(other.getByText("来源未提供可核验的作品网页")).toBeVisible();
  await expect(other.getByRole("heading").getByRole("button")).toHaveCount(0);
  await expect(other.getByRole("button", { name: "安装", exact: true })).toBeEnabled();
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("多游戏选择遵守每次八项的限制，清空后再次进入仍保留空选择", async ({ page }) => {
  const games = Array.from({ length: 9 }, (_, i) => ({ id: `game-${i}`, name: `Game ${i}` }));
  await installBrowseFixture(page, { workspaceGames: games });
  await page.goto("/");
  const browse = page.getByRole("button", { name: "游览", exact: true });
  const picker = page.getByRole("button", { name: "选择游览的游戏", exact: true });
  await browse.click();
  await picker.click();
  const dialog = page.getByRole("dialog");
  const checkbox = (name: string) => dialog.getByRole("checkbox", { name, exact: true });
  for (const game of games.slice(0, 8)) await checkbox(game.name).check();
  await expect(checkbox(games[8].name)).toBeDisabled();
  await dialog.getByRole("button", { name: "确定", exact: true }).click();
  await expect.poll(async () => (await browseCalls(page, "browse_authorize")).length).toBe(1);
  await expect.poll(async () => (await browseCalls(page, "browse_start_search")).length).toBe(1);
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await browse.click();
  await picker.click();
  await expect(checkbox(games[7].name)).toBeChecked();
  await dialog.getByRole("button", { name: "清空", exact: true }).click();
  await dialog.getByRole("button", { name: "确定", exact: true }).click();
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await browse.click();
  await expect(page.getByText("先选游戏，再开始游览", { exact: true })).toBeVisible();
  expect(await browseCalls(page, "browse_authorize")).toHaveLength(1);
  expect(await browseCalls(page, "browse_start_search")).toHaveLength(2);
});
