import { expect, test, type Page } from "@playwright/test";
import type { GameCandidate } from "../../src/lib/types";
import { openGameAdapterMatchingFixture } from "./game-adapter-matching-fixture";
import { release } from "./manager-plugin-catalog-fixture";

type TestCandidate = Omit<GameCandidate, "sources"> & {
  sources?: { kind: string; steamAppId?: string | null; completeInstall?: boolean | null }[];
};

function candidate({
  rootPath,
  name,
  ...overrides
}: Partial<TestCandidate> & Pick<TestCandidate, "rootPath" | "name">): GameCandidate {
  return {
    rootPath,
    name,
    confidence: 0.72,
    status: "candidate",
    suggestedAdapterSlug: null,
    registeredGameId: null,
    platform: "unknown",
    warnings: [],
    ...overrides,
  } as GameCandidate;
}

async function continueToGameScan(page: Page) {
  await page.getByRole("button", { name: "下一步", exact: true }).click();
  await page.getByRole("button", { name: "跳过配置", exact: true }).click();
  await page.getByRole("button", { name: "下一步", exact: true }).click();
  await expect(page.getByRole("heading", { name: "扫描游戏列表", exact: true })).toBeVisible();
}

async function finishOnboardingToLibrary(page: Page) {
  await continueToGameScan(page);
  await page.getByRole("button", { name: "稍后再扫", exact: true }).click();
  await page.getByRole("button", { name: "完成并进入游戏库", exact: true }).click();
  await expect(page.getByTestId("library-page")).toBeVisible();
}

async function startOnboardingScan(page: Page) {
  await continueToGameScan(page);
  await page.getByRole("button", { name: "扫描已安装游戏", exact: true }).click();
  await expect(page.getByRole("list", { name: "扫描候选游戏", exact: true })).toBeVisible();
}

function commandArgs(page: Page, command: string) {
  return page.evaluate((name) => (window as any).__AMM_TEST__.calls
    .filter((call: any) => call.command === name).map((call: any) => call.args), command);
}

test("游戏库扫描按 Steam 来源和云目录别名匹配，冲突与未知游戏仍可本地添加", async ({ page }) => {
  const steam = candidate({
    rootPath: "/sandbox/steam/id-only",
    name: "启动器显示的其他名称",
    platform: "unknown",
    sources: [{ kind: "steam", steamAppId: "413150" }],
  });
  const alias = candidate({
    rootPath: "/sandbox/games/方块世界",
    name: "方块世界",
    platform: "unknown",
  });
  const localSuggestion = candidate({
    rootPath: "/sandbox/games/local-suggestion",
    name: "本地明确建议",
    suggestedAdapterSlug: "local-adapter",
    sources: [{ kind: "steam", steamAppId: "413150" }],
  });
  const incomplete = candidate({
    rootPath: "/sandbox/steam/incomplete",
    name: "未完成的 Steam 安装",
    platform: "unknown",
    sources: [{ kind: "steam", steamAppId: "413150", completeInstall: false }],
  });
  const ambiguous = candidate({
    rootPath: "/sandbox/steam/ambiguous",
    name: "多个来源 ID",
    sources: [{ kind: "steam", steamAppId: "413150" }, { kind: "steam", steamAppId: "900" }],
  });
  const unmatched = candidate({ rootPath: "/sandbox/games/unlisted", name: "未收录的本地游戏" });
  const candidates = [steam, alias, localSuggestion, incomplete, ambiguous, unmatched];

  const groupName = "可信目录游戏名";
  const pluginTitle = "目录插件标题";
  const releaseGameName = "发布字段游戏名";
  const cloudCatalog = await openGameAdapterMatchingFixture(page, {
    onboarding: true, candidates, catalogGroupName: groupName, pluginTitle, releaseGameName,
  });
  const cloudPluginName = cloudCatalog.games.flatMap((game) => game.plugins)
    .find((plugin) => plugin.id === release.pluginId && plugin.releaseId === release.releaseId)!.name;
  expect(cloudCatalog.games.flatMap((game) => game.plugins)
    .find((plugin) => plugin.id === release.pluginId && plugin.releaseId === release.releaseId)).toHaveProperty("gameName", releaseGameName);
  await finishOnboardingToLibrary(page);
  await page.getByRole("button", { name: "扫描游戏", exact: true }).click();
  const rows = page.getByRole("list", { name: "扫描候选游戏", exact: true });
  const row = (item: TestCandidate) => rows.locator('[data-slot="item"]').filter({ has: page.getByText(item.rootPath, { exact: true }) });
  const steamRow = row(steam);
  const aliasRow = row(alias);

  for (const [item, matchedRow] of [[steam, steamRow], [alias, aliasRow]] as const) {
    await expect(matchedRow.getByText(releaseGameName, { exact: true })).toBeVisible();
    await expect(matchedRow.getByText(groupName, { exact: true })).toHaveCount(0);
    await expect(matchedRow.getByText(item.name, { exact: true })).toHaveCount(0);
    await expect(matchedRow.locator("p").filter({ hasText: pluginTitle })).toBeVisible();
    await expect(matchedRow.getByText("云端已匹配", { exact: true })).toBeVisible();
  }
  for (const item of [steamRow, aliasRow]) {
    await expect(item.getByRole("button", { name: "添加", exact: true })).toBeEnabled();
    await expect(item.getByRole("button", { name: "联网识别", exact: true })).toHaveCount(0);
    await expect(item.getByRole("button", { name: "仅添加本地", exact: true })).toHaveCount(0);
    await expect(item.getByRole("button", { name: "添加", exact: true })).toHaveCount(1);
  }
  for (const candidateItem of [localSuggestion, incomplete, ambiguous, unmatched]) {
    const item = row(candidateItem);
    await expect(item.getByText(candidateItem.name, { exact: true })).toBeVisible();
    await expect(item.getByText(cloudPluginName, { exact: true })).toHaveCount(0);
    await expect(item.getByRole("button", { name: "添加", exact: true })).toBeEnabled();
  }

  for (const item of candidates) {
    await row(item).getByRole("button", { name: "添加", exact: true }).click();
    await expect(row(item).getByRole("button", { name: "管理中", exact: true })).toBeDisabled();
  }

  const registrations = await commandArgs(page, "register_game");
  expect(registrations).toHaveLength(candidates.length);
  expect(registrations[0]).toMatchObject({
    directory: steam.rootPath,
    adapterSlug: null,
    fromDiscovery: true,
    cloudAdapter: { pluginId: release.pluginId, releaseId: release.releaseId },
    discoveryName: releaseGameName,
  });
  expect(registrations[1]).toMatchObject({
    directory: alias.rootPath,
    adapterSlug: null,
    fromDiscovery: true,
    cloudAdapter: { pluginId: release.pluginId, releaseId: release.releaseId },
    discoveryName: releaseGameName,
  });
  for (const [index, item] of candidates.slice(2).entries()) {
    const args = registrations[index + 2];
    expect(args).toMatchObject({ directory: item.rootPath, adapterSlug: item.suggestedAdapterSlug, fromDiscovery: true });
    expect(args).not.toHaveProperty("cloudAdapter");
    expect(args).not.toHaveProperty("discoveryName");
  }
  expect((await commandArgs(page, "list_manager_plugin_games")).every((args: unknown) => JSON.stringify(args) === "{}")).toBe(true);
  const state = await page.evaluate(() => ({
    registerResults: (window as any).__AMM_TEST__.registerResults,
    installedPlugins: (window as any).__AMM_TEST__.installedPlugins,
    installCalls: (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "install_manager_plugin"),
  }));
  expect(state.registerResults.slice(0, 2).map((game: any) => game.adapterSlug)).toEqual(["minecraft-java", "minecraft-java"]);
  expect(state.registerResults.slice(0, 2).map((game: any) => game.name)).toEqual([releaseGameName, releaseGameName]);
  expect(state.installedPlugins.filter((plugin: any) => plugin.slug === "minecraft-java")).toHaveLength(1);
  expect(state.installCalls).toEqual([]);
});

test("已登记的扫描结果显示游戏库名称", async ({ page }) => {
  const scanned = candidate({
    rootPath: "/sandbox/minecraft",
    name: "识别得到的默认名称",
    registeredGameId: "game-fixture",
  });
  await openGameAdapterMatchingFixture(page, {
    candidates: [scanned], catalogGroupName: "未登记云目录游戏名", pluginTitle: "云插件独立标题",
  });
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await page.getByRole("button", { name: "扫描游戏", exact: true }).click();
  const rows = page.getByRole("list", { name: "扫描候选游戏", exact: true });
  await expect(rows.getByText("Minecraft Java Edition", { exact: true })).toBeVisible();
  await expect(rows.getByText(scanned.name, { exact: true })).toHaveCount(0);
  await expect(rows.getByText("未登记云目录游戏名", { exact: true })).toHaveCount(0);
  await expect(rows.getByText("云插件独立标题", { exact: true })).toHaveCount(0);
  await expect(rows.getByText("云端已匹配", { exact: true })).toHaveCount(0);
  await expect(rows.getByRole("button", { name: "管理中", exact: true })).toBeDisabled();
});

test("首次设置等待云目录后以 release.gameName 显示并注册", async ({ page }) => {
  const scannedName = "Steam 扫描侧游戏名";
  const gameName = "发布元数据规范游戏名";
  const groupName = "目录分组候选名称";
  const pluginTitle = "独立的插件标题";
  const steam = candidate({
    rootPath: "/sandbox/steam/release-game-name",
    name: scannedName,
    sources: [{ kind: "steam", steamAppId: "413150" }],
  });
  await openGameAdapterMatchingFixture(page, {
    onboarding: true,
    candidates: [steam],
    catalogGroupName: groupName,
    pluginTitle,
    releaseGameName: gameName,
    deferCatalog: true,
  });
  await startOnboardingScan(page);
  const row = page.getByRole("listitem").filter({ has: page.getByText(steam.rootPath, { exact: true }) });
  await expect(row.getByRole("checkbox", { name: `选择 ${scannedName}`, exact: true })).toBeVisible();
  const waitingButton = row.getByRole("button", { name: "正在匹配…", exact: true });
  await expect(waitingButton).toBeVisible();
  await expect(waitingButton).toBeDisabled();
  await page.evaluate(() => (window as any).__AMM_TEST__.releaseManagerPluginCatalog());

  const checkbox = row.getByRole("checkbox", { name: `选择 ${gameName}`, exact: true });
  await expect(checkbox).toBeVisible();
  await expect(row.getByText(scannedName, { exact: true })).toHaveCount(0);
  await expect(row.getByText(gameName, { exact: true })).toBeVisible();
  await expect(row.locator("p").filter({ hasText: pluginTitle })).toBeVisible();
  await expect(row.getByText("云端已匹配", { exact: true })).toBeVisible();

  await checkbox.check();
  await row.getByRole("button", { name: "添加", exact: true }).click();
  await expect(row.getByText("管理中", { exact: true })).toBeVisible();
  expect(await commandArgs(page, "register_game")).toMatchObject([{
    directory: steam.rootPath,
    fromDiscovery: true,
    cloudAdapter: { pluginId: release.pluginId, releaseId: release.releaseId },
    discoveryName: gameName,
  }]);
  const registered = await page.evaluate(() => (window as any).__AMM_TEST__.registerResults.at(-1));
  expect(registered).toMatchObject({ name: gameName, rootPath: steam.rootPath, adapterSlug: "minecraft-java" });
});

test("目录分组名与插件标题相同时云匹配回退到本地扫描名", async ({ page }) => {
  const localName = "扫描器发现的本地名称";
  const sharedCatalogAndPluginName = "游戏适配插件标题";
  const local = candidate({
    rootPath: "/sandbox/games/group-name-is-plugin-title",
    name: localName,
    sources: [{ kind: "steam", steamAppId: "413150" }],
  });
  await openGameAdapterMatchingFixture(page, {
    candidates: [local],
    catalogGroupName: sharedCatalogAndPluginName,
    pluginTitle: sharedCatalogAndPluginName,
    releaseGameName: null,
  });
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await page.getByRole("button", { name: "扫描游戏", exact: true }).click();
  const rows = page.getByRole("list", { name: "扫描候选游戏", exact: true });
  const row = rows.locator('[data-slot="item"]').filter({ has: page.getByText(local.rootPath, { exact: true }) });
  await expect(row.getByText(localName, { exact: true })).toBeVisible();
  await expect(row.locator("p").filter({ hasText: sharedCatalogAndPluginName })).toBeVisible();
  await expect(row.getByText("云端已匹配", { exact: true })).toBeVisible();

  await row.getByRole("button", { name: "添加", exact: true }).click();
  expect(await commandArgs(page, "register_game")).toMatchObject([{
    directory: local.rootPath,
    fromDiscovery: true,
    cloudAdapter: { pluginId: release.pluginId, releaseId: release.releaseId },
    discoveryName: localName,
  }]);
  const registered = await page.evaluate(() => (window as any).__AMM_TEST__.registerResults.at(-1));
  expect(registered).toMatchObject({ name: localName, rootPath: local.rootPath });
});

test("首次设置的云适配包添加失败后可重试，成功仍按 Game 响应更新候选", async ({ page }) => {
  const steam = candidate({
    rootPath: "/sandbox/steam/first-run",
    name: "首次设置候选",
    sources: [{ kind: "steam", steamAppId: "413150" }],
  });
  const cloudCatalog = await openGameAdapterMatchingFixture(page, {
    onboarding: true,
    candidates: [steam],
    failCloudRegistrationOnce: true,
  });
  const cloudPluginName = cloudCatalog.games.flatMap((game) => game.plugins)
    .find((plugin) => plugin.id === release.pluginId && plugin.releaseId === release.releaseId)!.name;
  await startOnboardingScan(page);
  const matchedGameName = cloudCatalog.games.find((game) => game.plugins.some((plugin) => plugin.id === release.pluginId && plugin.releaseId === release.releaseId))!.name;
  const row = page.getByRole("listitem").filter({ has: page.getByText(steam.rootPath, { exact: true }) });
  await expect(row.getByText(steam.name, { exact: true })).toHaveCount(0);
  await expect(row.getByText(matchedGameName, { exact: true })).toBeVisible();
  await expect(row.locator("p").filter({ hasText: cloudPluginName })).toBeVisible();
  await expect(row.getByText("云端已匹配", { exact: true })).toBeVisible();
  await expect(row).toContainText(cloudPluginName);
  await expect(row.getByRole("button", { name: "添加", exact: true })).toBeDisabled();
  await expect(row.getByRole("button", { name: "添加", exact: true })).toHaveCount(1);
  await expect(row.getByRole("button", { name: "联网识别", exact: true })).toHaveCount(0);
  await row.getByRole("checkbox", { name: `选择 ${matchedGameName}`, exact: true }).check();
  await row.getByRole("button", { name: "添加", exact: true }).click();
  await expect(row.getByRole("alert")).toContainText("模拟云端适配包安装失败");
  await expect(row.getByRole("button", { name: "添加", exact: true })).toBeEnabled();
  await expect(row.getByRole("button", { name: "仅添加本地", exact: true })).toBeEnabled();

  await row.getByRole("button", { name: "添加", exact: true }).click();
  await expect(row.getByText("管理中", { exact: true })).toBeVisible();
  const registrations = await commandArgs(page, "register_game");
  expect(registrations).toHaveLength(2);
  for (const args of registrations) {
    expect(args).toMatchObject({
      directory: steam.rootPath,
      fromDiscovery: true,
      cloudAdapter: { pluginId: release.pluginId, releaseId: release.releaseId },
      discoveryName: matchedGameName,
    });
  }
  expect(await commandArgs(page, "start_onboarding")).toEqual([{}]);
  const game = await page.evaluate(() => (window as any).__AMM_TEST__.registerResults.at(-1));
  expect(game).toMatchObject({ id: expect.any(String), name: matchedGameName, rootPath: steam.rootPath, adapterSlug: "minecraft-java" });
});

test("云目录离线时首次设置仍保留本地添加", async ({ page }) => {
  const local = candidate({ rootPath: "/sandbox/games/offline", name: "离线候选" });
  await openGameAdapterMatchingFixture(page, { onboarding: true, candidates: [local], catalogStatus: "offline" });
  await startOnboardingScan(page);
  const row = page.getByRole("listitem").filter({ has: page.getByText(local.name, { exact: true }) });
  await expect(row.getByRole("button", { name: "添加", exact: true })).toBeDisabled();
  await row.getByRole("checkbox", { name: `选择 ${local.name}`, exact: true }).check();
  await expect(row.getByRole("button", { name: "添加", exact: true })).toBeEnabled();
  await row.getByRole("button", { name: "添加", exact: true }).click();
  await expect(row.getByText("管理中", { exact: true })).toBeVisible();
  expect(await commandArgs(page, "register_game")).toEqual([
    { directory: local.rootPath, adapterSlug: null, fromDiscovery: true },
  ]);
  expect(await commandArgs(page, "list_manager_plugin_games")).toEqual([{}]);
});

test("已安装但版本不匹配的本地适配包不会被云命中覆盖", async ({ page }) => {
  const steam = candidate({
    rootPath: "/sandbox/steam/installed-conflict",
    name: "已有本地适配包的候选",
    sources: [{ kind: "steam", steamAppId: "413150" }],
  });
  await openGameAdapterMatchingFixture(page, { onboarding: true, candidates: [steam], installedPlugin: true });
  await startOnboardingScan(page);
  const row = page.getByRole("listitem").filter({ has: page.getByText(steam.name, { exact: true }) });
  await expect(row.getByRole("button", { name: "添加", exact: true })).toBeDisabled();
  await row.getByRole("checkbox", { name: `选择 ${steam.name}`, exact: true }).check();
  await row.getByRole("button", { name: "添加", exact: true }).click();
  await expect(row.getByText("管理中", { exact: true })).toBeVisible();
  const args = (await commandArgs(page, "register_game"))[0];
  expect(args).toMatchObject({ directory: steam.rootPath, fromDiscovery: true });
  expect(args).not.toHaveProperty("cloudAdapter");
  expect(await commandArgs(page, "install_manager_plugin")).toEqual([]);
});
