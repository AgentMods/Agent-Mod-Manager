import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const current = String.raw`C:\Users\fixture\AppData\Roaming\com.glossmod.amm`;
const parent = String.raw`D:\应用数据`;

async function migrationCalls(page: Page) {
  return page.evaluate(() => {
    const {calls} = (window as typeof window & {
      __AMM_TEST__: {calls: {command: string; args: {parent?: string}}[]};
    }).__AMM_TEST__;
    return calls.filter((call) => call.command === "migrate_storage").map((call) => call.args);
  });
}

test("存储目录显示普通 Windows 路径，确认后重启，失败时可重试", async ({page}) => {
  const prefix = "\\\\?\\";
  await installMockTauri(page, {storage: {
    dataRoot: prefix + current,
    parent,
    previousError: "数据库仍在使用，暂时无法迁移",
    rejectFirstMigration: true,
  }});
  await page.goto("/");
  await page.getByRole("button", {name: "设置", exact: true}).click();
  await page.getByRole("tab", {name: "外观与存储", exact: true}).click();
  await expect(page.getByText(current, {exact: true})).toBeVisible();
  await expect(page.getByText(/迁移失败，仍在使用原目录/)).toBeVisible();

  const change = page.getByRole("button", {name: "更改存储目录"});
  const dialog = page.getByRole("alertdialog", {name: "迁移存储目录"});
  await change.click();
  await expect(dialog.getByText(parent + "\\Agent Mod Manager", {exact: true})).toBeVisible();
  await dialog.getByRole("button", {name: "取消", exact: true}).click();
  await expect(dialog).toBeHidden();
  expect(await migrationCalls(page)).toEqual([]);

  await change.click();
  const confirm = dialog.getByRole("button", {name: "迁移并重启"});
  await confirm.click();
  await expect(dialog.getByText("所选位置已包含 Agent Mod Manager 文件夹", {exact: true})).toBeVisible();
  await expect(confirm).toBeEnabled();
  await confirm.click();
  await expect(dialog.getByRole("button", {name: "正在重启…"})).toBeDisabled();
  expect(await migrationCalls(page)).toEqual([{parent}, {parent}]);
});
