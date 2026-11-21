import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const calls = (page: Page, command: string) => page.evaluate(
  name => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === name),
  command,
);

const draftRow = (page: Page, transactionId: string) => page.getByRole("row").filter({
  has: page.getByText(transactionId, {exact: true}),
});

async function openDrafts(page: Page) {
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "插件中心", exact: true}).click();
  await page.getByRole("tab", {name: /^草稿/}).click();
}

test("同名草稿可取消并按 draftId 单独删除，发布记录继续保留", async ({page}) => {
  await installMockTauri(page, {
    duplicatePluginDraftSlug: true,
    seedPublishedPluginRecord: true,
  });
  await page.goto("/");
  await openDrafts(page);

  const original = draftRow(page, "txn-draft-fixture");
  const duplicate = draftRow(page, "txn-draft-fixture-duplicate");
  const deleteName = "删除草稿 fixture-game";
  await expect(page.getByRole("tab", {name: "草稿 2", exact: true})).toBeVisible();
  await expect(page.getByRole("button", {name: deleteName, exact: true})).toHaveCount(2);
  await expect(original.getByRole("button", {name: deleteName, exact: true})).toBeVisible();
  await expect(duplicate.getByRole("button", {name: deleteName, exact: true})).toBeVisible();
  await expect(original.getByText("/sandbox/data/staging/plugin-drafts/fixture", {exact: true})).toBeVisible();
  await expect(duplicate.getByText("/sandbox/data/staging/plugin-drafts/fixture-duplicate", {exact: true})).toBeVisible();

  await original.getByRole("button", {name: deleteName, exact: true}).click();
  const dialog = page.getByRole("alertdialog", {name: "删除草稿？", exact: true});
  await expect(dialog.getByRole("button", {name: "取消", exact: true})).toBeVisible();
  await expect(dialog.getByRole("button", {name: "删除草稿", exact: true})).toBeVisible();
  await dialog.getByRole("button", {name: "取消", exact: true}).click();
  await expect(dialog).not.toBeVisible();
  await expect(original.getByRole("button", {name: deleteName, exact: true})).toBeFocused();
  expect(await calls(page, "delete_plugin_draft")).toHaveLength(0);
  await expect(original).toBeVisible();
  await expect(duplicate).toBeVisible();

  await duplicate.getByRole("button", {name: deleteName, exact: true}).click();
  const confirm = page.getByRole("alertdialog", {name: "删除草稿？", exact: true});
  await expect(confirm.getByText("/sandbox/data/staging/plugin-drafts/fixture-duplicate", {exact: true})).toBeVisible();
  await confirm.getByRole("button", {name: "删除草稿", exact: true}).click();
  await expect.poll(async () => (await calls(page, "delete_plugin_draft")).length).toBe(1);
  expect((await calls(page, "delete_plugin_draft")).map((call: any) => call.args)).toEqual([
    {draftId: "draft-fixture-duplicate"},
  ]);
  await expect(duplicate).toHaveCount(0);
  await expect(page.getByRole("tab", {name: "草稿 1", exact: true})).toBeFocused();
  await expect(original).toBeVisible();
  await expect(page.getByRole("button", {name: deleteName, exact: true})).toHaveCount(1);
  await expect(original.getByText("/sandbox/data/staging/plugin-drafts/fixture", {exact: true})).toBeVisible();

  await page.getByRole("tab", {name: "发布记录", exact: true}).click();
  await expect(page.getByRole("button", {name: "查看发布 已发布 fixture-game", exact: true})).toBeVisible();
});

test("删除草稿 IPC 失败时保留草稿并显示错误", async ({page}) => {
  await installMockTauri(page, {failPluginDraftDeletion: true});
  await page.goto("/");
  await openDrafts(page);

  const row = draftRow(page, "txn-draft-fixture");
  await row.getByRole("button", {name: "删除草稿 fixture-game", exact: true}).click();
  const dialog = page.getByRole("alertdialog", {name: "删除草稿？", exact: true});
  await dialog.getByRole("button", {name: "删除草稿", exact: true}).click();

  await expect.poll(async () => (await calls(page, "delete_plugin_draft")).length).toBe(1);
  expect((await calls(page, "delete_plugin_draft")).map((call: any) => call.args)).toEqual([
    {draftId: "draft-fixture"},
  ]);
  await expect(page.getByRole("alert").filter({hasText: "模拟删除草稿失败"})).toBeVisible();
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", {name: "取消", exact: true}).click();
  await expect(dialog).not.toBeVisible();
  await expect(row).toBeVisible();
});
