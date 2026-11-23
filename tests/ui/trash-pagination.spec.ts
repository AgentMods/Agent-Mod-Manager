import { expect, test, type Page } from "@playwright/test";
import type { TrashItem } from "../../src/lib/types";
import { installMockTauri } from "./mock-tauri";

const createdAt = "2026-11-18T00:00:00Z";

function makeTrashItems(): TrashItem[] {
  return Array.from({ length: 21 }, (_, index) => {
    const number = index + 1;
    const label = number === 21 ? "回收文件 21 独有搜索词" : `回收文件 ${String(number).padStart(2, "0")}`;
    return {
      id: `trash-${String(number).padStart(2, "0")}`,
      original_path: `/sandbox/game-${number % 2 === 1 ? "one" : "two"}/mods/${label}.jar`,
      trash_path: `/sandbox/data/trash/trash-${number}/payload`,
      transaction_id: `txn-trash-${number}`,
      size: 1024 + number,
      is_dir: false,
      game_id: number % 2 === 1 ? "game-fixture" : "game-fixture-two",
      label,
      reason: "Playwright 回归测试 fixture",
      created_at: createdAt,
      restored_at: null,
      exists: true,
      restore_requires_rollback: false,
    };
  });
}

async function openTrashWithFixture(page: Page, items = makeTrashItems(), failAfterFirst = false) {
  await installMockTauri(page);
  await page.goto("/");
  await page.evaluate(({ items, failAfterFirst }) => {
    const browser = window as any;
    const mock = browser.__AMM_TEST__;
    const originalInvoke = browser.__TAURI_INTERNALS__.invoke;
    let currentItems = structuredClone(items);
    const games = [...mock.games, {...mock.games[0], id: "game-fixture-two", name: "Stardew Valley Fixture"}];

    browser.__TAURI_INTERNALS__.invoke = async (command: string, args: any = {}) => {
      if (command === "list_games") {
        mock.calls.push({ command, args });
        return structuredClone(games);
      }
      if (command === "list_trash") {
        mock.calls.push({ command, args });
        return structuredClone(currentItems);
      }
      if (command === "purge_trash") {
        mock.calls.push({ command, args });
        if (args.confirmation !== "PURGE_TRASH") throw new Error("confirmation missing");
        if (failAfterFirst) {
          failAfterFirst = false;
          currentItems = currentItems.filter(({id}) => id !== args.ids[0]);
          throw new Error("文件被占用，未能完成删除");
        }
        const ids = new Set<string>(args.ids);
        const freed = currentItems.filter((item) => ids.has(item.id)).reduce((sum, item) => sum + item.size, 0);
        currentItems = currentItems.filter((item) => !ids.has(item.id));
        return freed;
      }
      return originalInvoke(command, args);
    };
  }, { items, failAfterFirst });

  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await page.getByRole("button", { name: "回收站", exact: true }).click();
  return page.getByTestId("trash-page");
}

function trashRows(page: Page) {
  return page.getByTestId("trash-page").getByRole("table", { name: "回收站文件" }).locator("tbody tr");
}

async function purgeCalls(page: Page) {
  return page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "purge_trash"));
}

test("回收站分页、页内全选、跨页选择和筛选重置", async ({ page }) => {
  const trash = await openTrashWithFixture(page);
  const rows = trashRows(page);
  const previous = trash.getByRole("button", { name: "上一页", exact: true });
  const next = trash.getByRole("button", { name: "下一页", exact: true });
  const selectPage = trash.getByRole("checkbox", { name: "选择本页全部文件", exact: true });
  const selectedCount = trash.getByRole("status").filter({ hasText: /^已选择 / });

  await expect(rows).toHaveCount(10);
  await expect(previous).toBeDisabled();
  await expect(next).toBeEnabled();

  await selectPage.check();
  await expect(selectedCount).toContainText("已选择 10 项");
  await selectPage.uncheck();
  await expect(selectedCount).toContainText("已选择 0 项");
  await trash.getByRole("checkbox", { name: "选择 回收文件 01", exact: true }).check();

  await next.click();
  await expect(rows).toHaveCount(10);
  await expect(rows.filter({ hasText: "回收文件 01" })).toHaveCount(0);
  await expect(selectedCount).toContainText("已选择 1 项");
  await selectPage.check();
  await expect(selectedCount).toContainText("已选择 11 项");
  await selectPage.uncheck();
  await expect(selectedCount).toContainText("已选择 1 项");
  await selectPage.check();

  await next.click();
  await expect(rows).toHaveCount(1);
  await expect(rows).toContainText("独有搜索词");
  await expect(next).toBeDisabled();

  const search = trash.getByRole("searchbox", { name: "搜索回收站", exact: true });
  await search.fill("独有搜索词");
  await expect(rows).toHaveCount(1);
  await expect(previous).toBeDisabled();
  await expect(next).toBeDisabled();
  await expect(selectedCount).toContainText("已选择 11 项");

  await search.fill("");
  await next.click();
  await next.click();
  await expect(rows).toHaveCount(1);
  await trash.getByRole("combobox", { name: "按游戏筛选回收站", exact: true }).selectOption("game-fixture");
  await expect(rows).toHaveCount(10);
  await expect(rows.first()).toContainText("回收文件 01");
  await expect(previous).toBeDisabled();
  await expect(next).toBeEnabled();
  await expect(selectedCount).toContainText("已选择 11 项");
});

test("筛选状态下全局清空请求所有项目并要求 acknowledgement", async ({ page }) => {
  const allItems = makeTrashItems();
  const trash = await openTrashWithFixture(page);
  const rows = trashRows(page);
  const gameFilter = trash.getByRole("combobox", { name: "按游戏筛选回收站", exact: true });
  await gameFilter.selectOption("game-fixture");
  await expect(rows).toHaveCount(10);
  await trash.getByRole("button", { name: "下一页", exact: true }).click();
  await expect(rows).toHaveCount(1);

  const clear = trash.getByRole("button", { name: "清空回收站", exact: true });
  await clear.click();
  const dialog = page.getByRole("dialog", { name: "清空回收站", exact: true });
  const acknowledgement = dialog.getByRole("switch", {
    name: "我确认永久删除以上文件，且无法再从回收站恢复。",
    exact: true,
  });
  const confirm = dialog.getByRole("button", { name: "确认清空", exact: true });

  await expect(acknowledgement).toHaveAttribute("aria-checked", "false");
  await expect(confirm).toBeDisabled();
  expect(await purgeCalls(page)).toHaveLength(0);
  await dialog.getByRole("button", { name: "取消", exact: true }).click();
  await expect(dialog).not.toBeVisible();
  expect(await purgeCalls(page)).toHaveLength(0);

  await clear.click();
  const reopenedDialog = page.getByRole("dialog", { name: "清空回收站", exact: true });
  const reopenedAcknowledgement = reopenedDialog.getByRole("switch", {
    name: "我确认永久删除以上文件，且无法再从回收站恢复。",
    exact: true,
  });
  const reopenedConfirm = reopenedDialog.getByRole("button", { name: "确认清空", exact: true });
  await reopenedAcknowledgement.click();
  await expect(reopenedConfirm).toBeEnabled();
  await reopenedConfirm.click();

  await expect(reopenedDialog).not.toBeVisible();
  await expect(clear).toBeDisabled();
  await expect(page.getByText("回收站为空", { exact: true })).toBeVisible();
  const calls = await purgeCalls(page);
  expect(calls).toHaveLength(1);
  expect(calls[0].args.confirmation).toBe("PURGE_TRASH");
  expect(calls[0].args.ids).toHaveLength(allItems.length);
  expect([...calls[0].args.ids].sort()).toEqual(allItems.map((item) => item.id).sort());
});

test("删除唯一末页项目后回到仍有内容的上一页", async ({ page }) => {
  const trash = await openTrashWithFixture(page);
  const rows = trashRows(page);
  await trash.getByRole("button", { name: "下一页", exact: true }).click();
  await trash.getByRole("button", { name: "下一页", exact: true }).click();
  await expect(rows).toHaveCount(1);
  await expect(rows).toContainText("回收文件 21 独有搜索词");

  await trash.getByRole("checkbox", { name: "选择 回收文件 21 独有搜索词", exact: true }).check();
  await trash.getByRole("button", { name: "永久删除所选", exact: true }).click();
  const dialog = page.getByRole("dialog");
  const acknowledgement = dialog.getByRole("switch", {
    name: "我确认永久删除以上文件，且无法再从回收站恢复。",
    exact: true,
  });
  const confirm = dialog.getByRole("button", { name: "永久删除", exact: true });
  await expect(confirm).toBeDisabled();
  await acknowledgement.click();
  await confirm.click();

  await expect(dialog).not.toBeVisible();
  await expect(rows).toHaveCount(10);
  await expect(rows.filter({ hasText: "回收文件 21 独有搜索词" })).toHaveCount(0);
  await expect(trash.getByRole("button", { name: "上一页", exact: true })).toBeEnabled();
  await expect(trash.getByRole("button", { name: "下一页", exact: true })).toBeDisabled();
  const calls = await purgeCalls(page);
  expect(calls).toHaveLength(1);
  expect(calls[0].args).toEqual({ ids: ["trash-21"], confirmation: "PURGE_TRASH" });
});

test("切换每页数量后回到首页，长文件路径和原因可按需展开", async ({page}, info) => {
  const items = makeTrashItems();
  items[0].reason = "保留原始移入原因".repeat(45);
  items[0].original_path = `/sandbox/${"very-long-component/".repeat(32)}Example.jar`;
  const trash = await openTrashWithFixture(page, items);
  const rows = trashRows(page);
  await expect(rows).toHaveCount(10);
  expect((await rows.first().boundingBox())!.height).toBeLessThan(50);
  await rows.first().locator("summary").press("Enter");
  await expect(rows.first().getByText(`移入原因：${items[0].reason}`, {exact: true})).toBeVisible();
  await expect(rows.first().getByText(`原始位置：${items[0].original_path}`, {exact: true})).toBeVisible();
  await rows.first().locator("summary").press("Enter");
  await trash.getByRole("button", {name: "下一页", exact: true}).click();
  await trash.getByRole("combobox", {name: "每页显示数量"}).selectOption("25");
  await expect(rows).toHaveCount(21);
  await expect(trash.getByRole("button", {name: "上一页", exact: true})).toBeDisabled();
  await trash.getByRole("combobox", {name: "每页显示数量"}).selectOption("10");
  await page.getByRole("button", {name: "切换暗色主题", exact: true}).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  expect(await trash.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  await page.locator("main").evaluate((main) => { main.scrollTop = 0; });
  await page.screenshot({path: `output/playwright/screenshots/trash-compact-${info.project.name}.png`});
});

test("大型回收站按后端上限分批删除确认时的 ID", async ({page}, info) => {
  test.skip(info.project.name !== "desktop-1280", "IPC 数量限制与视口无关");
  const [sample] = makeTrashItems();
  const items = Array.from({length: 5001}, (_, i) => ({...sample, id: `trash_${i}`}));
  const trash = await openTrashWithFixture(page, items);
  await trash.getByRole("button", {name: "清空回收站", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "清空回收站", exact: true});
  await expect(dialog.getByRole("alert")).toContainText("5001 项");
  await expect(dialog.getByRole("listitem")).toHaveCount(10);
  await dialog.getByRole("switch").click();
  await dialog.getByRole("button", {name: "确认清空", exact: true}).click();
  await expect(trash.getByText("回收站为空", {exact: true})).toBeVisible();
  const calls = await purgeCalls(page);
  expect(calls.map((call: {args: {ids: string[]}}) => call.args.ids.length)).toEqual([5000, 1]);
  expect(calls.flatMap((call: {args: {ids: string[]}}) => call.args.ids)).toEqual(items.map(({id}) => id));
});

test("回收站部分删除失败后刷新剩余项目并要求再次确认", async ({page}) => {
  const items = makeTrashItems().slice(0, 3);
  const trash = await openTrashWithFixture(page, items, true);
  const clear = trash.getByRole("button", {name: "清空回收站", exact: true});
  const dialog = page.getByRole("dialog", {name: "清空回收站", exact: true});
  const confirm = dialog.getByRole("button", {name: "确认清空", exact: true});
  await clear.click();
  await dialog.getByRole("switch").click();
  await confirm.click();
  await expect(trash.getByRole("alert")).toContainText("文件被占用");
  await expect(trashRows(page)).toHaveCount(2);
  expect(await purgeCalls(page)).toHaveLength(1);
  await clear.click();
  await expect(confirm).toBeDisabled();
  await expect(dialog.getByRole("listitem")).toHaveCount(2);
  await dialog.getByRole("switch").click();
  await confirm.click();
  await expect(trash.getByText("回收站为空", {exact: true})).toBeVisible();
  expect((await purgeCalls(page))[1].args.ids).toEqual(items.slice(1).map(({id}) => id));
});
