import { expect, test, type Page } from "@playwright/test";
import { openCatalogFixture } from "./manager-plugin-catalog-fixture";

const calls = (page: Page, command: string) => page.evaluate(
  name => (window as any).__AMM_TEST__.calls.filter((call: {command: string}) => call.command === name),
  command,
);

test("移除适配包失败可重试，成功后刷新已安装和云端目录列表", async ({page}) => {
  await openCatalogFixture(page, {installed: true, removeFailsOnce: true});
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "插件中心", exact: true}).click();
  await page.getByRole("tab", {name: "云端插件", exact: true}).click();
  const cloudCard = page.getByRole("listitem").filter({has: page.getByRole("heading", {name: "Minecraft Java Edition", exact: true})});
  await expect(cloudCard).toContainText("已安装");
  await page.getByRole("tab", {name: "已安装", exact: true}).click();

  const table = page.getByRole("table");
  const row = table.getByRole("row").filter({hasText: "Minecraft Java Edition"});
  await expect(row).toBeVisible();
  const listPluginsBefore = (await calls(page, "list_plugins")).length;

  const remove = row.getByRole("button", {name: /(?=.*移除)(?=.*Minecraft Java Edition)/});
  await expect(remove).toBeVisible();
  await remove.click();
  await expect(page.getByRole("alert").filter({hasText: "模拟移除适配包失败"})).toBeVisible();
  await expect(row).toBeVisible();
  expect((await calls(page, "remove_installed_plugin")).map((call: any) => call.args)).toEqual([
    {slug: "minecraft-java"},
  ]);
  await expect(page.getByRole("alertdialog")).toHaveCount(0);

  await remove.click();
  await expect(row).toHaveCount(0);
  await expect.poll(async () => (await calls(page, "remove_installed_plugin")).length).toBe(2);
  expect((await calls(page, "remove_installed_plugin")).map((call: any) => call.args)).toEqual([
    {slug: "minecraft-java"},
    {slug: "minecraft-java"},
  ]);
  await expect.poll(async () => (await calls(page, "list_plugins")).length).toBeGreaterThan(listPluginsBefore);
  await page.getByRole("tab", {name: "云端插件", exact: true}).click();
  await expect(cloudCard.getByRole("button", {name: /(?=.*安装)(?=.*Minecraft Java Edition)/})).toBeVisible();
  await expect(cloudCard).not.toContainText("已安装");
});
