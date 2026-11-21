import { expect, test, type Page } from "@playwright/test";
import { adapterText, openCatalogFixture, release, skillText } from "./manager-plugin-catalog-fixture";

function calls(page: Page, command: string) {
  return page.evaluate(name => (window as any).__AMM_TEST__.calls.filter((call: {command: string}) => call.command === name).map((call: {args: unknown}) => call.args), command);
}
async function catalogTab(page: Page) {
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "插件中心", exact: true}).click();
  await page.getByRole("tab", {name: "云端插件", exact: true}).click();
  return page.getByRole("tabpanel", {name: "云端插件", exact: true});
}
const candidate = (page: Page) => page.getByRole("button", {name: "查看并导入 Minecraft Java Edition · 山雀 · 1.1.0"});
const plan = (page: Page) => page.getByRole("dialog", { name: "确认文件变更", exact: true });

// The catalog fixture includes IDs larger than JS's safe integer limit and two authors for one game.
test("云端目录本地搜索、精确选版，并显示两个原始文件、命令和覆盖版本", async ({page}) => {
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
    await expect(candidate(page)).toBeVisible();
  }
  await search.fill("未知游戏");
  await expect(tab.getByText("没有匹配结果", {exact: true})).toBeVisible();
  await tab.getByRole("button", {name: "清空搜索"}).click();
  expect(await calls(page, "list_manager_plugin_games")).toHaveLength(1);
  await candidate(page).click();
  await expect(tab).toContainText(release.releaseId);
  await expect(tab).toContainText("已安装同标识适配包 minecraft-java 1.0.0");
  await expect(tab.getByRole("region", {name: "声明的 MCP 服务"})).toContainText('catalog-helper "--mode" "list only"');
  for (const [name, text] of [["adapter.json", adapterText], ["SKILL.md", skillText]]) {
    await tab.getByText(name, {exact: true}).click();
    expect(await tab.getByLabel(`${name} 原文`).textContent()).toBe(text);
  }
  expect(await calls(page, "preview_manager_plugin")).toEqual([release]);
  expect(await calls(page, "prepare_manager_plugin_import")).toEqual([]);
  const size = await tab.boundingBox();
  expect(size!.x + size!.width).toBeLessThanOrEqual(page.viewportSize()!.width + 1);
  await tab.getByRole("button", {name: "返回目录"}).click();
  await expect(candidate(page)).toBeFocused();
});

test("空目录显示暂无已发布插件", async ({page}) => {
  await openCatalogFixture(page, {directory: "empty"});
  const tab = await catalogTab(page);
  await expect(tab.getByText("暂无已发布插件", {exact: true})).toBeVisible();
});

test("目录加载失败可重试，下架版本无法导入", async ({page}) => {
  await openCatalogFixture(page, {directory: "fail-once", previewFails: true});
  const tab = await catalogTab(page);
  await expect(tab.getByRole("alert")).toContainText("云端目录连接超时");
  await tab.getByRole("button", {name: "刷新云端目录"}).click();
  await candidate(page).click();
  await expect(tab.getByRole("alert")).toContainText("所选云插件版本已下架");
  await expect(tab.getByRole("button", {name: "准备导入草稿"})).toHaveCount(0);
  expect(await calls(page, "apply_plan")).toEqual([]);
});

test("游戏库按 Steam ID 查找，分别审核草稿与安装，返回后显式绑定到原游戏", async ({page}) => {
  await openCatalogFixture(page, {changeAtStage: true});
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  const game = page.locator('[data-slot="card"]').filter({has: page.getByRole("heading", {name: "Minecraft Java Edition", exact: true})});
  await game.getByRole("button", {name: "更多操作"}).click();
  await page.getByRole("menuitem", {name: "选择适配包", exact: true}).click();
  const chooser = page.getByRole("dialog", {name: "选择适配包", exact: true});
  await chooser.getByRole("button", {name: "查找云端插件"}).click();
  await expect(chooser.getByRole("textbox", {name: "搜索云端插件"})).toHaveValue("413150");
  await candidate(page).click();
  await chooser.getByRole("button", {name: "准备导入草稿"}).click();
  await expect(chooser.getByRole("alert")).toContainText("待导入的内容或本地安装版本已变化");
  await chooser.getByText("SKILL.md", {exact: true}).click();
  await expect(chooser.getByLabel("SKILL.md 原文")).toContainText("更新后的草稿。");
  await chooser.getByRole("button", {name: "审核导入草稿"}).click();
  await expect(plan(page)).toContainText("保存外部适配包草稿");
  await plan(page).getByRole("button", {name: "取消", exact: true}).click();
  expect(await calls(page, "apply_plan")).toEqual([]);
  await chooser.getByRole("button", {name: "审核导入草稿"}).click();
  await plan(page).getByRole("button", {name: "批准并执行"}).click();
  await expect(chooser.getByText("已导入草稿", {exact: true})).toBeVisible();
  expect(await calls(page, "install_plugin")).toEqual([]);
  await chooser.getByRole("button", {name: "验证并预览安装"}).click();
  await expect(plan(page)).toContainText("安装外部适配包");
  await plan(page).getByRole("button", {name: "批准并执行"}).click();
  await expect(chooser.getByText("已安装 1.1.0", {exact: true})).toBeVisible();
  expect(await calls(page, "prepare_manager_plugin_import")).toEqual([release]);
  expect(await calls(page, "preview_manager_plugin")).toEqual([release]);
  expect(await calls(page, "apply_plan")).toEqual([{planId: "txn-write-minecraft-java"}, {planId: "txn-install-minecraft-java"}]);
  await chooser.getByRole("button", {name: "返回并选择此适配包"}).click();
  await expect(chooser.getByLabel("游戏适配包", {exact: true})).toHaveValue("minecraft-java");
  expect(await calls(page, "set_game_adapter")).toEqual([]);
  await chooser.getByRole("button", {name: "应用适配包"}).click();
  await expect(chooser).not.toBeVisible();
  expect(await calls(page, "set_game_adapter")).toEqual([{id: "game-fixture", adapterSlug: "minecraft-java"}]);
});

test("已导入的草稿被编辑后，拒绝继续安装为原云端版本", async ({page}) => {
  await openCatalogFixture(page, {draftChanges: true});
  const tab = await catalogTab(page);
  await candidate(page).click();
  await tab.getByRole("button", {name: "准备导入草稿"}).click();
  await tab.getByRole("button", {name: "审核导入草稿"}).click();
  await plan(page).getByRole("button", {name: "批准并执行"}).click();
  await tab.getByRole("button", {name: "验证并预览安装"}).click();
  await expect(tab.getByRole("alert")).toContainText("草稿内容与云端版本的预览不一致");
  expect(await calls(page, "install_plugin")).toEqual([]);
});
