import { expect, test, type Locator, type Page } from "@playwright/test";
import { adapterText, openCatalogFixture, release, skillText } from "./manager-plugin-catalog-fixture";
import { GAME_COVER_URL } from "./publication-fixture";

function calls(page: Page, command: string) {
  return page.evaluate(name => (window as any).__AMM_TEST__.calls.filter((call: {command: string}) => call.command === name).map((call: {args: unknown}) => call.args), command);
}
async function catalogTab(page: Page) {
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "插件中心", exact: true}).click();
  await page.getByRole("tab", {name: "云端插件", exact: true}).click();
  return page.getByRole("tabpanel", {name: "云端插件", exact: true});
}
const pluginCard = (scope: Page | Locator) => scope.getByRole("region", {name: "云端插件目录", exact: true}).getByRole("listitem").filter({hasText: "Minecraft Java Edition"});
const installButton = (scope: Page | ReturnType<typeof pluginCard>) => scope.getByRole("button", {name: /(?=.*安装)(?=.*Minecraft Java Edition)/});
const detailButton = (scope: Page | ReturnType<typeof pluginCard>) => scope.getByRole("button", {name: /(?=.*查看详情)(?=.*Minecraft Java Edition)/});
const plan = (page: Page) => page.getByRole("dialog", { name: "确认文件变更", exact: true });

test("已安装同 slug 的目录卡片展示本地版本，详情只读且不覆盖", async ({page}) => {
  await page.emulateMedia({colorScheme: "dark"});
  await openCatalogFixture(page, {installed: true});
  const tab = await catalogTab(page);
  const search = tab.getByRole("textbox", {name: "搜索云端插件"});
  await expect(tab).toContainText("目录中有 1 个无效条目，已跳过。");
  await expect(tab).toContainText("远山 · 2.0.0 · macOS");
  await expect(tab).toContainText("山雀 · 1.1.0 · Windows / Linux");
  await expect(tab).toContainText("Steam App ID 413150");
  for (const term of ["方块世界", "413150", "MINECRAFT-JAVA"]) {
    await search.fill(term);
    await expect(detailButton(pluginCard(page))).toBeVisible();
  }
  await search.fill("未知游戏");
  await expect(tab.getByText("没有匹配结果", {exact: true})).toBeVisible();
  await tab.getByRole("button", {name: "清空搜索"}).click();
  expect(await calls(page, "list_manager_plugin_games")).toHaveLength(1);
  const card = pluginCard(page);
  await expect(card).toContainText("已安装");
  await expect(card).toContainText("1.0.0");
  await expect(installButton(card)).toHaveCount(0);
  const listedCover = card.getByRole("img", {name: /游戏封面/});
  await expect(listedCover).toHaveAttribute("src", GAME_COVER_URL);
  await expect.poll(() => listedCover.evaluate(el => (el as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
  await expect(tab.locator('img[src^="data:"]')).toHaveCount(0);

  await detailButton(card).click();
  await expect(tab).toContainText(release.releaseId);
  const previewCover = tab.getByRole("img", {name: /游戏封面/});
  await expect(previewCover).toHaveAttribute("src", GAME_COVER_URL);
  await expect.poll(() => previewCover.evaluate(el => (el as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
  await expect(tab.locator('img[src^="data:"]')).toHaveCount(0);
  await expect(tab).toContainText("管理这款游戏已声明的模组安装规则。");
  await expect(tab).toContainText("本地版本 1.0.0");
  await expect(tab.getByRole("region", {name: "声明的 MCP 服务"})).toContainText('catalog-helper "--mode" "list only"');
  for (const [name, text] of [["adapter.json", adapterText], ["SKILL.md", skillText]]) {
    await tab.getByText(name, {exact: true}).click();
    expect(await tab.getByLabel(`${name} 原文`).textContent()).toBe(text);
  }
  expect(await calls(page, "preview_manager_plugin")).toEqual([release]);
  expect(await calls(page, "get_manager_plugin_display")).toContainEqual({releaseId: release.releaseId});
  expect(await calls(page, "prepare_manager_plugin_import")).toEqual([]);
  expect(await calls(page, "install_manager_plugin")).toEqual([]);
  expect(await calls(page, "apply_plan")).toEqual([]);
  await expect(plan(page)).toHaveCount(0);
  const size = await tab.boundingBox();
  expect(size!.x + size!.width).toBeLessThanOrEqual(page.viewportSize()!.width + 1);
  await tab.getByRole("button", {name: "返回目录"}).click();
  await expect(detailButton(card)).toBeFocused();
});

test("空目录显示暂无已发布插件", async ({page}) => {
  await openCatalogFixture(page, {directory: "empty"});
  const tab = await catalogTab(page);
  await expect(tab.getByText("暂无已发布插件", {exact: true})).toBeVisible();
});

test("云目录和详情都没有游戏封面时显示统一占位", async ({page}) => {
  await openCatalogFixture(page, {missingCover: true});
  const tab = await catalogTab(page);
  await expect(tab.getByText("无游戏封面").first()).toBeVisible();
  await detailButton(pluginCard(page)).click();
  await expect(tab.getByText("无游戏封面").last()).toBeVisible();
  await expect(tab.locator("img")).toHaveCount(0);
  await expect(tab.locator('img[src^="data:"]')).toHaveCount(0);
});

test("不同游戏的插件横向排卡，窄屏切换单列且不溢出", async ({page}, testInfo) => {
  await openCatalogFixture(page, {separateGames: true});
  const tab = await catalogTab(page);
  const cards = tab.getByRole("listitem");
  await expect(cards).toHaveCount(2);
  const featured = cards.filter({has: installButton(page)});
  const cover = featured.getByRole("img", {name: /游戏封面/});
  await expect(cover).toBeVisible();
  const coverBox = (await cover.boundingBox())!;
  const titleBox = (await featured.getByRole("heading", {name: "Minecraft Java Edition", exact: true}).boundingBox())!;
  expect(coverBox.x + coverBox.width).toBeLessThanOrEqual(titleBox.x);

  const first = (await cards.nth(0).boundingBox())!;
  const next = (await cards.nth(1).boundingBox())!;
  if (testInfo.project.name === "narrow-390") {
    expect(next.x).toBeCloseTo(first.x, 0);
    expect(next.y).toBeGreaterThanOrEqual(first.y + first.height);
    expect(await tab.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    expect(first.x + first.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  } else {
    expect(next.y).toBeCloseTo(first.y, 0);
    expect(next.x).toBeGreaterThanOrEqual(first.x + first.width);
  }
});

test("目录加载和直接安装失败后均可重试，重复点击只安装一次", async ({page}) => {
  await openCatalogFixture(page, {directory: "fail-once", installFailsOnce: true});
  const tab = await catalogTab(page);
  await expect(tab.getByRole("alert")).toContainText("云端目录连接超时");
  await tab.getByRole("button", {name: "刷新云端目录"}).click();
  const button = installButton(pluginCard(page));
  await button.dblclick();
  await expect(tab.getByRole("alert")).toContainText("模拟云端插件安装失败");
  expect(await calls(page, "install_manager_plugin")).toEqual([release]);
  await expect(button).toBeEnabled();
  await button.click();
  await expect(pluginCard(page)).toContainText("已安装");
  expect(await calls(page, "install_manager_plugin")).toEqual([release, release]);
  expect(await calls(page, "prepare_manager_plugin_import")).toEqual([]);
  expect(await calls(page, "preview_manager_plugin")).toEqual([]);
  expect(await calls(page, "apply_plan")).toEqual([]);
  await expect(plan(page)).toHaveCount(0);
});

test("目录卡片直接安装并刷新已安装列表", async ({page}) => {
  await openCatalogFixture(page);
  const tab = await catalogTab(page);
  await expect(installButton(pluginCard(page))).toBeVisible();
  await expect.poll(async () => page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "list_plugins").length)).toBeGreaterThan(0);
  const before = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "list_plugins").length);
  await installButton(pluginCard(page)).click();
  await expect(pluginCard(page)).toContainText("已安装");
  expect(await calls(page, "install_manager_plugin")).toEqual([release]);
  await expect.poll(async () => page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "list_plugins").length)).toBeGreaterThan(before);
  expect(await calls(page, "prepare_manager_plugin_import")).toEqual([]);
  expect(await calls(page, "preview_manager_plugin")).toEqual([]);
  expect(await calls(page, "apply_plan")).toEqual([]);
  await expect(plan(page)).toHaveCount(0);
  expect(await tab.getByRole("button", {name: /(?=.*安装)(?=.*Minecraft Java Edition)/}).count()).toBe(0);
  await page.getByRole("tab", {name: "已安装", exact: true}).click();
  await expect(page.getByRole("row").filter({hasText: "Minecraft Java Edition"})).toBeVisible();
});

test("游戏库中安装后可返回并显式选择适配包", async ({page}) => {
  await openCatalogFixture(page);
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  const game = page.locator('[data-slot="card"]').filter({has: page.getByRole("heading", {name: "Minecraft Java Edition", exact: true})});
  await game.getByRole("button", {name: "更多操作"}).click();
  await page.getByRole("menuitem", {name: "选择适配包", exact: true}).click();
  const chooser = page.getByRole("dialog", {name: "选择适配包", exact: true});
  await chooser.getByRole("button", {name: "查找云端插件"}).click();
  await expect(chooser.getByRole("textbox", {name: "搜索云端插件"})).toHaveValue("413150");
  const button = installButton(pluginCard(chooser));
  await expect(button).toBeEnabled();
  // A successful install replaces this button with the installed badge while the click is settling.
  await button.dispatchEvent("click");
  await expect.poll(async () => calls(page, "install_manager_plugin")).toEqual([release]);
  await expect(pluginCard(chooser)).toContainText("已安装");
  expect(await calls(page, "install_manager_plugin")).toEqual([release]);
  expect(await calls(page, "set_game_adapter")).toEqual([]);
  expect(await calls(page, "apply_plan")).toEqual([]);
  await pluginCard(chooser).getByRole("button", {name: "返回并选择此适配包", exact: true}).click();
  await expect(chooser.getByLabel("游戏适配包", {exact: true})).toHaveValue("minecraft-java");
  expect(await calls(page, "set_game_adapter")).toEqual([]);
  await chooser.getByRole("button", {name: "应用适配包"}).click();
  await expect(chooser).not.toBeVisible();
  expect(await calls(page, "set_game_adapter")).toEqual([{id: "game-fixture", adapterSlug: "minecraft-java"}]);
});
