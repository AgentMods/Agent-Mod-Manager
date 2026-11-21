import {expect, test, type Page} from "@playwright/test";
import {installMockTauri} from "./mock-tauri";

async function openLibrary(page: Page) {
  await installMockTauri(page, {withExternalAdapter: true});
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "扫描游戏", exact: true}).click();
  const discovery = page.getByRole("dialog", {name: "扫描游戏", exact: true});
  await expect(discovery).toBeVisible();
  await expect(discovery.getByRole("status").filter({hasText: "已检查"})).toBeVisible();
  return page.getByTestId("library-page");
}

async function inspectDirectory(page: Page, directory: string) {
  const dialog = page.getByRole("dialog", {name: "扫描游戏", exact: true});
  await dialog.getByLabel("游戏目录", {exact: true}).fill(directory);
  await dialog.getByRole("button", {name: "检查单个游戏", exact: true}).click();
  const row = dialog.locator('[data-slot="item"]').filter({has: page.getByText(directory, {exact: true})});
  await expect(row).toBeVisible();
  return row;
}

function registrationArgs(page: Page) {
  return page.evaluate(() => (window as any).__AMM_TEST__.calls
    .filter((call: any) => call.command === "register_game").map((call: any) => call.args));
}

test("单目录检查不显示弱匹配分数，按自动建议提供联网识别", async ({page}) => {
  const library = await openLibrary(page);
  const weak = await inspectDirectory(page, "/sandbox/unknown-game");
  await expect(weak.getByText("unknown-game", {exact: true})).toBeVisible();
  await expect(weak.getByText("未匹配", {exact: true})).toBeVisible();
  await expect(weak.getByText("8% 匹配", {exact: true})).toHaveCount(0);
  await expect(weak.getByRole("button", {name: "联网识别"})).toBeEnabled();

  const below = await inspectDirectory(page, "/sandbox/below-threshold");
  await expect(below.getByText("未匹配", {exact: true})).toBeVisible();
  await expect(below.getByText("80% 匹配", {exact: true})).toHaveCount(0);
  await expect(below.getByRole("button", {name: "联网识别"})).toBeEnabled();

  const tied = await inspectDirectory(page, "/sandbox/near-tie");
  await expect(tied.getByText("待确认", {exact: true})).toBeVisible();
  await expect(tied.getByText("92% 匹配", {exact: true})).toHaveCount(0);
  await expect(tied.getByRole("button", {name: "联网识别"})).toBeEnabled();

  const matched = await inspectDirectory(page, "/sandbox/at-threshold");
  await expect(matched.getByText("80% 匹配", {exact: true})).toBeVisible();
  await expect(matched.getByRole("button", {name: "联网识别"})).toHaveCount(0);
  expect(await registrationArgs(page)).toEqual([]);
  await matched.getByRole("button", {name: "添加", exact: true}).click();
  await expect(page.getByText("已将「Fixture Game」添加到游戏库", {exact: true})).toBeVisible();
  const discovery = page.getByRole("dialog", {name: "扫描游戏", exact: true});
  await expect(discovery).toBeVisible();
  await expect(matched.getByRole("button", {name: "管理中", exact: true})).toBeDisabled();
  await page.keyboard.press("Escape");
  await expect(discovery).toBeHidden();
  await expect(library).toBeVisible();
  await expect(library.getByRole("heading", {name: "Fixture Game", exact: true})).toBeVisible();
  expect(await registrationArgs(page)).toEqual([
    {directory: "/sandbox/at-threshold", adapterSlug: "minecraft-java", fromDiscovery: true},
  ]);
});

test("游戏库添加无建议的启动器候选时不采用单独检查的适配包", async ({page}) => {
  const library = await openLibrary(page);
  const discovery = page.getByRole("dialog", {name: "扫描游戏", exact: true});
  const local = await inspectDirectory(page, "/sandbox/new-game/Broken");
  await expect(local.getByText("已适配", {exact: true})).toBeVisible();

  await discovery.getByLabel("游戏目录", {exact: true}).fill("/sandbox/new-game");
  await discovery.getByRole("button", {name: "扫描此目录", exact: true}).click();
  const scanned = discovery.locator('[data-slot="item"]').filter({has: page.getByText("Broken Fixture", {exact: true})});
  await expect(scanned.getByText("未匹配", {exact: true})).toBeVisible();
  await expect(scanned.getByRole("button", {name: "联网识别"})).toBeEnabled();
  await scanned.getByRole("button", {name: "添加", exact: true}).click();
  await expect(page.getByText("已将「Broken」添加到游戏库", {exact: true})).toBeVisible();
  await expect(discovery).toBeVisible();
  await expect(scanned.getByRole("button", {name: "管理中", exact: true})).toBeDisabled();
  const fixture = discovery.locator('[data-slot="item"]').filter({has: page.getByText("Fixture Game", {exact: true})});
  await expect(fixture.getByRole("button", {name: "添加", exact: true})).toBeEnabled();
  await fixture.getByRole("button", {name: "添加", exact: true}).click();
  await expect(page.getByText("已将「Fixture Game」添加到游戏库", {exact: true})).toBeVisible();
  await expect(discovery).toBeVisible();
  await expect(fixture.getByRole("button", {name: "管理中", exact: true})).toBeDisabled();
  expect(await registrationArgs(page)).toEqual([
    {directory: "/sandbox/new-game/Broken", adapterSlug: null, fromDiscovery: true},
    {directory: "/sandbox/new-game/Fixture", adapterSlug: "minecraft-java", fromDiscovery: true},
  ]);
  await page.keyboard.press("Escape");
  await expect(discovery).toBeHidden();
  await expect(library).toBeVisible();
  await expect(library.getByRole("heading", {name: "Broken", exact: true})).toBeVisible();
  await expect(library.getByRole("heading", {name: "Fixture Game", exact: true})).toBeVisible();
  const game = library.locator('[data-slot="card"]').filter({has: page.getByText("Broken", {exact: true})});
  await expect(game.getByRole("button", {name: "创建适配包"})).toBeVisible();
});
