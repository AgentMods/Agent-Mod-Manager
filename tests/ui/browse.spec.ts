import { expect, test, type Page } from "@playwright/test";
import { BROWSE_ITEMS, browseCalls, installBrowseFixture, type BrowsePageFixture } from "./browse-fixture";

async function enterBrowse(page: Page, pages?: BrowsePageFixture[], theme?: "light" | "dark") {
  await installBrowseFixture(page, pages ? { pages } : undefined);
  if (theme) await page.addInitScript(value => localStorage.setItem("amm.theme", value), theme);
  await page.goto("/");
  await page.getByRole("button", { name: "游览", exact: true }).click();
  await expect(page.getByLabel("搜索 Mod")).toBeVisible();
}

async function chooseTwoGames(page: Page) {
  await page.getByRole("button", { name: "选择游览的游戏", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "选择游览的游戏", exact: true });
  await dialog.getByRole("checkbox", { name: "Minecraft Java Edition", exact: true }).check();
  await dialog.getByRole("checkbox", { name: "Stardew Valley", exact: true }).check();
  await expect(dialog.getByRole("checkbox", { name: "Valheim", exact: true })).not.toBeChecked();
  await dialog.getByRole("button", { name: "确定", exact: true }).click();
}

async function authorizeBrowse(page: Page, expectedTitle = BROWSE_ITEMS[0].title) {
  await page.getByRole("button", { name: "允许本次游览", exact: true }).click();
  await expect.poll(async () => (await browseCalls(page, "browse_authorize")).length).toBe(1);
  // The page automatically starts the empty-query feed after consent.
  await expect.poll(async () => (await browseCalls(page, "browse_start_search")).length).toBe(1);
  await expect(page.getByText(expectedTitle, { exact: true })).toBeVisible();
}

async function search(page: Page, query: string) {
  await page.getByLabel("搜索 Mod").fill(query);
  await page.getByLabel("搜索 Mod").press("Enter");
  await expect.poll(async () => (await browseCalls(page, "browse_start_search")).length).toBeGreaterThan(1);
  await expect(page.getByText(BROWSE_ITEMS[0].title, { exact: true })).toBeVisible();
}

async function browseWithTwoGames(page: Page, query?: string, pages?: BrowsePageFixture[], theme?: "light" | "dark", expectedTitle?: string) {
  await enterBrowse(page, pages, theme);
  await chooseTwoGames(page);
  await authorizeBrowse(page, expectedTitle);
  if (query) await search(page, query);
}

async function openFiltersWhenCollapsed(page: Page) {
  const toggle = page.getByRole("button", { name: "筛选", exact: true });
  if (await toggle.isVisible()) await toggle.click();
}

test("授权后自动加载空关键词；搜索只发送所选游戏与公开查询词", async ({ page }) => {
  await enterBrowse(page);
  await chooseTwoGames(page);
  expect(await browseCalls(page, "browse_start_search")).toHaveLength(0);

  await authorizeBrowse(page);

  const callsAfterFeed = await browseCalls(page, "browse_start_search");
  expect(callsAfterFeed[0].args).toMatchObject({
    authorizationId: "browse-authorization-fixture",
    gameIds: ["game-fixture", "game-stardew"],
    query: "",
  });
  expect(callsAfterFeed[0].args.sourceIds).toBeNull();
  // The unrelated Valheim-only result was present in the fixture input but
  // filtered out by the backend mock because Valheim was not selected.
  await expect(page.getByText(BROWSE_ITEMS[3].title, { exact: true })).toHaveCount(0);

  await search(page, "farm tools");

  const calls = await browseCalls(page);
  const sourceListIndex = calls.findIndex(call => call.command === "browse_list_sources");
  const authorizationIndex = calls.findIndex(call => call.command === "browse_authorize");
  expect(sourceListIndex).toBeGreaterThanOrEqual(0);
  expect(authorizationIndex).toBeGreaterThan(sourceListIndex);

  const authorization = calls[authorizationIndex].args;
  const sourceIds = authorization.sourceIds as string[];
  expect(sourceIds).toContain("glossmod_http");
  expect(sourceIds).toContain("thunderstore");
  expect(sourceIds).not.toContain("nexusmods");

  const searchCalls = await browseCalls(page, "browse_start_search");
  expect(searchCalls).toHaveLength(2);
  expect(searchCalls[1].args).toMatchObject({
    authorizationId: "browse-authorization-fixture",
    gameIds: ["game-fixture", "game-stardew"],
    query: "farm tools",
    sourceIds: null,
  });

  const appCalls = await page.evaluate(() => (window as any).__AMM_TEST__.calls as { command: string }[]);
  expect(appCalls.some(call => ["start_browse_install", "install_library_mod", "download_mod"].includes(call.command))).toBe(false);
});

test("类型和标签只筛本地结果；来源筛选会下推并重新搜索", async ({ page }) => {
  await browseWithTwoGames(page, "farm");
  await openFiltersWhenCollapsed(page);
  await expect(page.getByText(BROWSE_ITEMS[0].title, { exact: true })).toBeVisible();
  await expect(page.getByText(BROWSE_ITEMS[1].title, { exact: true })).toBeVisible();

  await page.getByRole("combobox", { name: "类型" }).selectOption("visual");
  await expect(page.getByText(BROWSE_ITEMS[0].title, { exact: true })).toBeVisible();
  await expect(page.getByText(BROWSE_ITEMS[1].title, { exact: true })).toHaveCount(0);
  await page.getByRole("combobox", { name: "标签" }).selectOption("environment");
  await expect(page.getByText(BROWSE_ITEMS[0].title, { exact: true })).toBeVisible();

  // Empty feed + keyword query; the two local filters must not restart either.
  expect(await browseCalls(page, "browse_start_search")).toHaveLength(2);
  await page.getByRole("combobox", { name: "来源" }).selectOption("glossmod_http");
  await expect.poll(async () => (await browseCalls(page, "browse_start_search")).length).toBe(3);
  await expect(page.getByText(BROWSE_ITEMS[0].title, { exact: true })).toBeVisible();
  await expect(page.getByText(BROWSE_ITEMS[1].title, { exact: true })).toHaveCount(0);

  const searches = await browseCalls(page, "browse_start_search");
  expect(searches[2].args).toMatchObject({
    authorizationId: "browse-authorization-fixture",
    gameIds: ["game-fixture", "game-stardew"],
    query: "farm",
    sourceIds: ["glossmod_http"],
  });
  expect((await browseCalls(page, "browse_get_page")).some(call => call.args.searchId === "browse-search-3" && call.args.page === 0)).toBe(true);
});

test("分页读取下一页，原名切换保留项目来源且不启动安装", async ({ page }) => {
  await browseWithTwoGames(page);
  await expect(page.getByText(BROWSE_ITEMS[1].title, { exact: true })).toBeVisible();
  await expect(page.getByText(BROWSE_ITEMS[3].title, { exact: true })).toHaveCount(0);

  const card = page.getByText(BROWSE_ITEMS[0].title, { exact: true }).locator("xpath=ancestor::article");
  await card.getByRole("button", { name: "查看原名", exact: true }).click();
  await expect(card.getByText(BROWSE_ITEMS[0].originalTitle!, { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "下一页", exact: true }).click();
  await expect(page.getByText(BROWSE_ITEMS[2].title, { exact: true })).toBeVisible();
  expect((await browseCalls(page, "browse_get_page")).map(call => call.args.page)).toContain(1);
  expect(await browseCalls(page, "browse_start_search")).toHaveLength(1);
  expect(await browseCalls(page, "start_browse_install")).toHaveLength(0);
});

test("卡片封面为全宽 2:1；宽屏多列底部对齐，窄屏单列且无横向溢出", async ({ page }, testInfo) => {
  await browseWithTwoGames(page);

  const items = BROWSE_ITEMS.slice(0, 2);
  const boxes = await Promise.all(items.map(async item => {
    const card = page.getByRole("article", { name: item.title, exact: true });
    await expect(card).toBeVisible();
    const cardBox = await card.boundingBox();
    const coverBox = await card.locator(":scope > button:first-child, :scope > div:first-child").first().boundingBox();
    const footerBox = await card.locator('[data-slot="item-footer"]').boundingBox();
    if (!cardBox || !coverBox || !footerBox) throw new Error(`Missing card, cover, or footer bounds for ${item.title}`);
    return { card: cardBox, cover: coverBox, footer: footerBox };
  }));

  for (const { card, cover } of boxes) {
    expect(cover.width / card.width).toBeGreaterThan(0.9);
    expect(Math.abs(cover.width / cover.height - 2)).toBeLessThan(0.08);
    expect(cover.x - card.x).toBeLessThan(card.width * 0.06);
    expect(card.x + card.width - cover.x - cover.width).toBeLessThan(card.width * 0.06);
  }

  const overflow = await page.evaluate(() => ({
    viewport: document.documentElement.clientWidth,
    document: document.documentElement.scrollWidth,
    body: document.body.scrollWidth,
  }));
  expect(overflow.document).toBeLessThanOrEqual(overflow.viewport);
  expect(overflow.body).toBeLessThanOrEqual(overflow.viewport);

  if (testInfo.project.name === "narrow-390") {
    expect(Math.abs(boxes[0].card.x - boxes[1].card.x)).toBeLessThan(2);
    expect(boxes[1].card.y).toBeGreaterThan(boxes[0].card.y);
  } else {
    expect(Math.abs(boxes[0].card.x - boxes[1].card.x)).toBeGreaterThan(100);
    expect(Math.abs(boxes[0].card.y - boxes[1].card.y)).toBeLessThan(2);
    const footerBottomDelta = Math.abs(
      boxes[0].footer.y + boxes[0].footer.height - boxes[1].footer.y - boxes[1].footer.height,
    );
    expect(footerBottomDelta).toBeLessThan(2);
  }
});

test("卡片主题截图：暗色桌面和亮色窄窗，含真实封面与缺失封面", async ({ page }, testInfo) => {
  test.skip(!["desktop-1280", "narrow-390"].includes(testInfo.project.name), "Capture the requested desktop and narrow layouts only.");
  const theme = testInfo.project.name === "desktop-1280" ? "dark" : "light";
  await browseWithTwoGames(page, undefined, undefined, theme);

  const covered = page.getByRole("article", { name: BROWSE_ITEMS[0].title, exact: true });
  const uncovered = page.getByRole("article", { name: BROWSE_ITEMS[1].title, exact: true });
  const image = covered.locator("img");
  await expect.poll(() => image.evaluate(node => (node as HTMLImageElement).naturalWidth)).toBe(400);
  await expect(uncovered.locator("img")).toHaveCount(0);

  const root = page.locator("html");
  await expect(root).toHaveAttribute("data-theme", theme);
  await page.emulateMedia({ reducedMotion: "reduce" });

  const grid = covered.locator("xpath=..");
  const screenshotPath = theme === "dark"
    ? "output/playwright/browse-layout-dark-desktop.png"
    : "output/playwright/browse-layout-light-narrow.png";
  await grid.screenshot({ path: screenshotPath, animations: "disabled" });
});

test("本地筛选会补满当前页，填满后继续使用下一页", async ({ page }) => {
  const matches = Array.from({ length: 25 }, (_, index) => ({
    ...BROWSE_ITEMS[0],
    id: `work-filter-match-${index + 1}`,
    title: `筛选匹配作品 ${index + 1}`,
    originalTitle: `Filtered match ${index + 1}`,
  }));
  const pages: BrowsePageFixture[] = [
    { items: [matches[0], BROWSE_ITEMS[1]], hasNext: true, isEnriching: false },
    { items: matches.slice(1), hasNext: false, isEnriching: false },
  ];
  await browseWithTwoGames(page, undefined, pages, undefined, matches[0].title);
  await openFiltersWhenCollapsed(page);
  await page.getByRole("combobox", { name: "类型" }).selectOption("visual");

  await expect(page.getByRole("article")).toHaveCount(1);
  await expect(page.getByRole("button", { name: "查找更多匹配", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "查找更多匹配", exact: true }).click();
  await expect(page.getByRole("article")).toHaveCount(24);
  await expect(page.getByRole("button", { name: "下一页", exact: true })).toBeVisible();
  expect((await browseCalls(page, "browse_get_page")).some(call => call.args.searchId === "browse-search-1" && call.args.page === 1)).toBe(true);
  await page.getByRole("button", { name: "下一页", exact: true }).click();
  await expect(page.getByRole("article")).toHaveCount(1);
  await expect(page.getByRole("article", { name: matches[24].title, exact: true })).toBeVisible();
});

test("窄屏先展开筛选，再操作本地类型和服务端来源筛选", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== "narrow-390", "Narrow viewport behavior is covered by the narrow-390 project.");
  await browseWithTwoGames(page);

  await expect(page.getByRole("button", { name: "筛选", exact: true })).toBeVisible();
  await expect(page.getByRole("combobox", { name: "来源" })).not.toBeVisible();
  await page.getByRole("button", { name: "筛选", exact: true }).click();
  await expect(page.getByRole("combobox", { name: "来源" })).toBeVisible();

  await page.getByRole("combobox", { name: "类型" }).selectOption("visual");
  expect(await browseCalls(page, "browse_start_search")).toHaveLength(1);
  await page.getByRole("combobox", { name: "来源" }).selectOption("glossmod_http");
  await expect.poll(async () => (await browseCalls(page, "browse_start_search")).length).toBe(2);
  expect((await browseCalls(page, "browse_start_search"))[1].args.sourceIds).toEqual(["glossmod_http"]);
});
