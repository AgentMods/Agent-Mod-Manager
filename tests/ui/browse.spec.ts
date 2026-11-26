import { expect, test, type Page } from "@playwright/test";
import { BROWSE_ITEMS, BROWSE_SOURCES, browseCalls, installBrowseFixture, type BrowsePageFixture } from "./browse-fixture";

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

async function waitForInitialBrowse(page: Page, expectedTitle = BROWSE_ITEMS[0].title) {
  await expect.poll(async () => (await browseCalls(page, "browse_authorize")).length).toBe(1);
  await expect.poll(async () => (await browseCalls(page, "browse_start_search")).length).toBe(1);
  expect((await browseCalls(page, "browse_authorize"))[0].args.sourceIds).toEqual(
    BROWSE_SOURCES.filter(source => source.status === "available").map(source => source.id),
  );
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
  await waitForInitialBrowse(page, expectedTitle);
  if (query) await search(page, query);
}

async function openFiltersWhenCollapsed(page: Page) {
  const toggle = page.getByRole("button", { name: "筛选", exact: true });
  if (await toggle.isVisible() && await toggle.getAttribute("aria-expanded") === "false") await toggle.click();
}

test("自动授权所有可用来源并加载空关键词；搜索只发送所选游戏与公开查询词", async ({ page }) => {
  await enterBrowse(page);
  await chooseTwoGames(page);
  await waitForInitialBrowse(page);

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

test("返回游览时沿用已确认的来源和搜索词，以新搜索打开缓存的作品", async ({ page }) => {
  await browseWithTwoGames(page, "farm", [
    { items: [BROWSE_ITEMS[0]], hasNext: true },
    { items: [BROWSE_ITEMS[2]], hasNext: false },
  ]);
  await page.getByRole("button", { name: "下一页", exact: true }).click();
  await expect(page.getByText("第 2 页", { exact: true })).toBeVisible();
  await expect(page.getByText(BROWSE_ITEMS[2].title, { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await expect.poll(async () => (await browseCalls(page, "browse_cancel_search")).some(call => call.args.searchId === "browse-search-2")).toBe(true);
  await page.getByRole("button", { name: "游览", exact: true }).click();
  await expect(page.getByLabel("搜索 Mod")).toHaveValue("farm");
  await expect(page.getByText("第 2 页", { exact: true })).toBeVisible();
  const card = page.getByRole("article", { name: BROWSE_ITEMS[2].title, exact: true });
  await expect(card).toBeVisible();
  expect(await browseCalls(page, "browse_authorize")).toHaveLength(1);
  expect((await browseCalls(page, "browse_start_search")).at(-1)?.args).toMatchObject({
    authorizationId: "browse-authorization-fixture", gameIds: ["game-fixture", "game-stardew"], query: "farm",
  });
  await card.getByText(BROWSE_ITEMS[2].title, { exact: true }).click();
  await expect.poll(async () => (await browseCalls(page, "browse_open_project")).at(-1)?.args.searchId).toBe("browse-search-3");
});

test("短时间返回会恢复筛选，离开五分钟后重新按全部可用来源加载", async ({ page }) => {
  await browseWithTwoGames(page);
  const now = new Date();
  await page.clock.setFixedTime(now);
  await openFiltersWhenCollapsed(page);
  const source = page.getByRole("combobox", { name: "来源", exact: true });
  const type = page.getByRole("combobox", { name: "类型", exact: true });
  const tag = page.getByRole("combobox", { name: "标签", exact: true });
  await source.selectOption("glossmod_http");
  await type.selectOption("visual");
  await tag.selectOption("environment");
  const library = page.getByRole("button", { name: "游戏库", exact: true });
  const browse = page.getByRole("button", { name: "游览", exact: true });
  await library.click();
  await expect(page.getByRole("heading", { name: "游戏库", exact: true })).toBeVisible();
  await browse.click();
  await expect(page.getByLabel("搜索 Mod")).toBeVisible();
  await openFiltersWhenCollapsed(page);
  await expect(source).toHaveValue("glossmod_http");
  await expect(type).toHaveValue("visual");
  await expect(tag).toHaveValue("environment");
  await expect(page.getByText(BROWSE_ITEMS[0].title, { exact: true })).toBeVisible();
  await library.click();
  await expect(page.getByRole("heading", { name: "游戏库", exact: true })).toBeVisible();
  await page.clock.setFixedTime(new Date(now.getTime() + 5 * 60_000));
  await browse.click();
  await expect.poll(async () => (await browseCalls(page, "browse_authorize")).length).toBe(2);
  await expect.poll(async () => (await browseCalls(page, "browse_start_search")).length).toBe(4);
  expect((await browseCalls(page, "browse_authorize")).at(-1)?.args.sourceIds).toEqual(
    BROWSE_SOURCES.filter(source => source.status === "available").map(source => source.id),
  );
  await expect(page.getByText(BROWSE_ITEMS[0].title, { exact: true })).toBeVisible();
});

test("来源弹层只应用可用项；零选择禁用应用，关闭会丢弃草稿", async ({ page }) => {
  const sources = [
    ...BROWSE_SOURCES,
    { id: "denied-source", name: "Denied Source", status: "permissionDenied" as const },
    { id: "unsupported-source", name: "Unsupported Source", status: "unsupported" as const },
  ];
  await installBrowseFixture(page, { sources });
  await page.goto("/");
  await page.getByRole("button", { name: "游览", exact: true }).click();
  await chooseTwoGames(page);
  await waitForInitialBrowse(page);

  await openFiltersWhenCollapsed(page);
  const leftSource = page.getByRole("combobox", { name: "来源", exact: true });
  await leftSource.selectOption("glossmod_http");
  await expect.poll(async () => (await browseCalls(page, "browse_start_search")).length).toBe(2);
  expect((await browseCalls(page, "browse_start_search"))[1].args.sourceIds).toEqual(["glossmod_http"]);
  const trigger = page.getByRole("button", { name: "选择来源", exact: true });
  const openChooser = async () => {
    await trigger.click();
    return page.getByRole("dialog", { name: "选择来源", exact: true });
  };
  const chooser = await openChooser();
  const checkbox = (name: string) => chooser.getByRole("checkbox", { name, exact: true });
  await expect(checkbox("GlossMod")).toBeChecked();
  await expect(checkbox("Thunderstore")).toBeChecked();
  await expect(checkbox("Nexus Mods")).toBeDisabled();
  await expect(checkbox("Denied Source")).toBeDisabled();
  await expect(checkbox("Unsupported Source")).toBeDisabled();
  const apply = chooser.getByRole("button", { name: "应用", exact: true });

  await checkbox("GlossMod").uncheck();
  await checkbox("Thunderstore").uncheck();
  await expect(apply).toBeDisabled();
  await page.keyboard.press("Escape");
  await expect(chooser).toBeHidden();
  expect(await browseCalls(page, "browse_authorize")).toHaveLength(1);
  expect(await browseCalls(page, "browse_start_search")).toHaveLength(2);

  const reopened = await openChooser();
  await expect(reopened.getByRole("checkbox", { name: "GlossMod", exact: true })).toBeChecked();
  await expect(reopened.getByRole("checkbox", { name: "Thunderstore", exact: true })).toBeChecked();
  await reopened.getByRole("checkbox", { name: "Thunderstore", exact: true }).uncheck();
  await trigger.click();
  await expect(reopened).toBeHidden();
  expect(await browseCalls(page, "browse_authorize")).toHaveLength(1);
  expect(await browseCalls(page, "browse_start_search")).toHaveLength(2);

  const applied = await openChooser();
  await expect(applied.getByRole("checkbox", { name: "GlossMod", exact: true })).toBeChecked();
  await expect(applied.getByRole("checkbox", { name: "Thunderstore", exact: true })).toBeChecked();
  await applied.getByRole("checkbox", { name: "GlossMod", exact: true }).uncheck();
  await applied.getByRole("button", { name: "应用", exact: true }).click();
  await expect(applied).toBeHidden();
  await expect.poll(async () => (await browseCalls(page, "browse_authorize")).length).toBe(2);
  await expect.poll(async () => (await browseCalls(page, "browse_start_search")).length).toBe(3);

  const authorizations = await browseCalls(page, "browse_authorize");
  expect(authorizations[1].args.sourceIds).toEqual(["thunderstore"]);
  expect(await leftSource.inputValue()).toBe("");
  expect((await browseCalls(page, "browse_start_search"))[2].args).toMatchObject({
    authorizationId: "browse-authorization-fixture-2",
    gameIds: ["game-fixture", "game-stardew"],
    query: "",
    sourceIds: null,
  });
  await expect(page.getByText(BROWSE_ITEMS[0].title, { exact: true })).toHaveCount(0);
  await expect(page.getByText(BROWSE_ITEMS[1].title, { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await expect(page.getByRole("heading", { name: "游戏库", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "游览", exact: true }).click();
  await expect.poll(async () => (await browseCalls(page, "browse_start_search")).length).toBe(4);
  await expect(page.getByText(BROWSE_ITEMS[1].title, { exact: true })).toBeVisible();
  expect(await browseCalls(page, "browse_authorize")).toHaveLength(2);
  await openFiltersWhenCollapsed(page);
  const returnedChooser = await openChooser();
  await expect(returnedChooser.getByRole("checkbox", { name: "GlossMod", exact: true })).not.toBeChecked();
  await expect(returnedChooser.getByRole("checkbox", { name: "Thunderstore", exact: true })).toBeChecked();
  await page.keyboard.press("Escape");
  await expect(returnedChooser).toBeHidden();
});

test("授权失败保留旧结果，重试同一来源选择后换新授权", async ({ page }) => {
  await browseWithTwoGames(page);
  await page.evaluate(() => {
    (window as any).__BROWSE_TEST__.remainingAuthorizationFailures = 1;
  });

  await openFiltersWhenCollapsed(page);
  const trigger = page.getByRole("button", { name: "选择来源", exact: true });
  await trigger.click();
  const chooser = page.getByRole("dialog", { name: "选择来源", exact: true });
  await chooser.getByRole("checkbox", { name: "Thunderstore", exact: true }).uncheck();
  await chooser.getByRole("button", { name: "应用", exact: true }).click();

  await expect.poll(async () => (await browseCalls(page, "browse_authorize")).length).toBe(2);
  await expect(page.getByRole("alert").filter({ hasText: "fixture browse authorization failed" })).toBeVisible();
  await expect(page.getByText(BROWSE_ITEMS[0].title, { exact: true })).toBeVisible();
  await expect(page.getByText(BROWSE_ITEMS[1].title, { exact: true })).toBeVisible();

  const failedAuthorization = (await browseCalls(page, "browse_authorize"))[1].args;
  await page.getByRole("button", { name: "重试", exact: true }).click();
  await expect.poll(async () => (await browseCalls(page, "browse_authorize")).length).toBe(3);
  await expect.poll(async () => (await browseCalls(page, "browse_start_search")).length).toBe(2);
  await expect(page.getByText(BROWSE_ITEMS[0].title, { exact: true })).toBeVisible();
  await expect(page.getByText(BROWSE_ITEMS[1].title, { exact: true })).toHaveCount(0);
  const authorizations = await browseCalls(page, "browse_authorize");
  expect(authorizations[1].args).toEqual(failedAuthorization);
  expect(authorizations[2].args).toEqual(failedAuthorization);
  expect((await browseCalls(page, "browse_start_search"))[1].args).toMatchObject({
    authorizationId: "browse-authorization-fixture-2",
    sourceIds: null,
  });
});

test("新授权后的搜索失败不复用旧卡片，重试同一搜索范围", async ({ page }) => {
  await browseWithTwoGames(page);
  await page.evaluate(() => {
    (window as any).__BROWSE_TEST__.remainingSearchFailures = 1;
  });

  await openFiltersWhenCollapsed(page);
  await page.getByRole("button", { name: "选择来源", exact: true }).click();
  const chooser = page.getByRole("dialog", { name: "选择来源", exact: true });
  await chooser.getByRole("checkbox", { name: "Thunderstore", exact: true }).uncheck();
  await chooser.getByRole("button", { name: "应用", exact: true }).click();

  await expect.poll(async () => (await browseCalls(page, "browse_authorize")).length).toBe(2);
  await expect.poll(async () => (await browseCalls(page, "browse_start_search")).length).toBe(2);
  await expect(page.getByRole("alert").filter({ hasText: "fixture browse search failed" })).toBeVisible();
  await expect(page.getByText(BROWSE_ITEMS[0].title, { exact: true })).toHaveCount(0);
  await expect(page.getByText(BROWSE_ITEMS[1].title, { exact: true })).toHaveCount(0);

  const failedSearch = (await browseCalls(page, "browse_start_search"))[1].args;
  await page.getByRole("button", { name: "重试", exact: true }).click();
  await expect.poll(async () => (await browseCalls(page, "browse_start_search")).length).toBe(3);
  await expect(page.getByText(BROWSE_ITEMS[0].title, { exact: true })).toBeVisible();
  await expect(page.getByText(BROWSE_ITEMS[1].title, { exact: true })).toHaveCount(0);
  expect((await browseCalls(page, "browse_start_search"))[2].args).toEqual(failedSearch);
  expect(await browseCalls(page, "browse_authorize")).toHaveLength(2);
});

test("没有可用来源时显示设置与刷新，不发起授权或搜索", async ({ page }) => {
  await installBrowseFixture(page, {
    sources: [
      { id: "nexusmods", name: "Nexus Mods", status: "notConnected" },
      { id: "denied-source", name: "Denied Source", status: "permissionDenied" },
      { id: "unsupported-source", name: "Unsupported Source", status: "unsupported" },
    ],
  });
  await page.goto("/");
  await page.getByRole("button", { name: "游览", exact: true }).click();
  await chooseTwoGames(page);
  const empty = page.locator('[data-slot="empty"]');
  await expect(empty.getByText("没有可用的 Mod 来源", { exact: true })).toBeVisible();
  await expect(empty.getByRole("button", { name: "设置", exact: true })).toBeVisible();
  await empty.getByRole("button", { name: "刷新", exact: true }).click();
  await expect.poll(async () => (await browseCalls(page, "browse_list_sources")).length).toBeGreaterThan(1);
  expect(await browseCalls(page, "browse_authorize")).toHaveLength(0);
  expect(await browseCalls(page, "browse_start_search")).toHaveLength(0);
});

test("来源匹配提示默认折叠，展开后可手动选择游戏", async ({ page }) => {
  await browseWithTwoGames(page, undefined, [{
    items: [BROWSE_ITEMS[0]],
    issues: [{ gameId: "game-fixture", sourceId: "thunderstore", code: "noGameMatch" }],
    gameChoices: [{ gameId: "game-stardew", sourceId: "glossmod_http", options: [{ id: "42", name: "Stardew Valley" }] }],
  }]);
  const header = page.getByText("来源匹配提示（2）", { exact: true });
  const issue = page.getByText(/Thunderstore — 未找到对应游戏/);
  const choice = page.getByRole("combobox", { name: "匹配来源游戏" });
  await expect(header).toBeVisible();
  await expect(issue).toBeHidden();
  await expect(choice).toBeHidden();
  await header.press("Enter");
  await expect(issue).toBeVisible();
  await expect(choice).toBeVisible();
  await header.click();
  await expect(issue).toBeHidden();
  await header.click();
  await choice.selectOption("42");
  await expect.poll(async () => (await browseCalls(page, "browse_select_game_match"))[0]?.args).toMatchObject({
    gameId: "game-stardew", sourceId: "glossmod_http", providerGameId: "42",
  });
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

  await expect(page.getByRole("combobox", { name: "类型" })).toHaveValue("");
  await expect(page.getByRole("combobox", { name: "标签" })).toHaveValue("");

  const searches = await browseCalls(page, "browse_start_search");
  expect(searches[2].args).toMatchObject({
    authorizationId: "browse-authorization-fixture",
    gameIds: ["game-fixture", "game-stardew"],
    query: "farm",
    sourceIds: ["glossmod_http"],
  });
  expect((await browseCalls(page, "browse_get_page")).some(call => call.args.searchId === "browse-search-3" && call.args.page === 0)).toBe(true);
});

test("新增类型的选项和卡片标签一致，历史及陌生类型归入其他", async ({ page }) => {
  const types = [
    ["tools", "工具与修改器"],
    ["fixes", "修复与兼容"],
    ["interface", "界面"],
    ["vehicles", "载具"],
    ["characters", "角色与外观"],
    ["performance", "性能优化"],
  ] as const;
  const items = [...types.map(([key]) => key), "other", "unknown", "constructor"].map((typeKey) => ({
    ...BROWSE_ITEMS[0],
    id: `work-type-${typeKey}`,
    title: `分类作品 ${typeKey}`,
    typeKey,
  }));
  await browseWithTwoGames(page, undefined, [{ items }], undefined, items[0].title);
  await openFiltersWhenCollapsed(page);

  const filter = page.getByRole("combobox", { name: "类型" });
  await expect(filter.locator('option[value="unknown"]')).toHaveCount(0);
  for (const [key, label] of types) {
    await expect(filter.locator(`option[value="${key}"]`)).toHaveText(label);
    await filter.selectOption(key);
    await expect(page.getByRole("article")).toHaveCount(1);
    const card = page.getByRole("article", { name: `分类作品 ${key}`, exact: true });
    await expect(card.getByText(label, { exact: true })).toBeVisible();
  }

  await filter.selectOption("other");
  await expect(page.getByRole("article")).toHaveCount(3);
  for (const item of items.slice(types.length)) {
    const card = page.getByRole("article", { name: item.title, exact: true });
    await expect(card.getByText("其他", { exact: true })).toBeVisible();
  }
  expect(await browseCalls(page, "browse_start_search")).toHaveLength(1);
});

test("异步分类会更新当前类型筛选的结果和卡片标签", async ({ page }) => {
  const outgoing = { ...BROWSE_ITEMS[0], typeKey: "interface" };
  const incoming = { ...BROWSE_ITEMS[1], typeKey: "other" };
  await browseWithTwoGames(page, undefined, [{ items: [outgoing, incoming], isEnriching: true }]);
  await openFiltersWhenCollapsed(page);

  const filter = page.getByRole("combobox", { name: "类型" });
  await filter.selectOption("interface");
  const outgoingCard = page.getByRole("article", { name: outgoing.title, exact: true });
  const incomingCard = page.getByRole("article", { name: incoming.title, exact: true });
  const enriching = page.getByText("Agent 正在整理作品资料…", { exact: true });
  await expect(enriching).toBeVisible();
  await expect(outgoingCard.getByText("界面", { exact: true })).toBeVisible();
  await expect(incomingCard).toHaveCount(0);

  await page.evaluate((items) => {
    const [fixture] = (window as any).__BROWSE_TEST__.pages as BrowsePageFixture[];
    fixture.enrichedItems = items;
    fixture.isEnriching = false;
  }, [{ ...outgoing, typeKey: "tools" }, { ...incoming, typeKey: "interface" }]);

  await expect(incomingCard.getByText("界面", { exact: true })).toBeVisible();
  await expect(outgoingCard).toHaveCount(0);
  await expect(enriching).toBeHidden();
  await expect(filter).toHaveValue("interface");
  await filter.selectOption("tools");
  await expect(outgoingCard.getByText("工具与修改器", { exact: true })).toBeVisible();
  expect(await browseCalls(page, "browse_start_search")).toHaveLength(1);
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

  await openFiltersWhenCollapsed(page);
  await page.getByRole("button", { name: "选择来源", exact: true }).click();
  const sourceDialog = page.getByRole("dialog", { name: "选择来源", exact: true });
  await expect(sourceDialog).toBeVisible();
  const sourceScreenshotPath = theme === "dark"
    ? "output/playwright/browse-sources-dark-desktop.png"
    : "output/playwright/browse-sources-light-narrow.png";
  await page.screenshot({ path: sourceScreenshotPath, fullPage: true, animations: "disabled" });
});

test("本地筛选会补满当前页，填满后继续使用下一页", async ({ page }) => {
  const matches = Array.from({ length: 25 }, (_, index) => ({
    ...BROWSE_ITEMS[0],
    id: `work-filter-match-${index + 1}`,
    title: `筛选匹配作品 ${index + 1}`,
    originalTitle: `Filtered match ${index + 1}`,
    typeKey: "tools",
  }));
  const pages: BrowsePageFixture[] = [
    { items: [matches[0], BROWSE_ITEMS[1]], hasNext: true, isEnriching: false },
    { items: matches.slice(1), hasNext: false, isEnriching: false },
  ];
  await browseWithTwoGames(page, undefined, pages, undefined, matches[0].title);
  await openFiltersWhenCollapsed(page);
  await page.getByRole("combobox", { name: "类型" }).selectOption("tools");

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
