import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const launchOptions = [
  {id: "steam", label: null},
  {id: "direct", label: null},
  {id: "smapi", label: null},
  {id: "mod-engine-2", label: null},
  {id: "custom", label: "Prism Launcher"},
];

const calls = (page: Page, command: string) => page.evaluate((name) =>
  (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === name), command);

const gameCard = (page: Page) => page.locator('[data-slot="card"]').filter({
  has: page.getByRole("heading", {name: "Minecraft Java Edition", exact: true}),
});

async function openLibrary(page: Page, mockOptions: Parameters<typeof installMockTauri>[1] = {}) {
  await installMockTauri(page, mockOptions);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  return gameCard(page);
}

async function openLaunchMenu(game: ReturnType<typeof gameCard>, page: Page) {
  await game.getByRole("button", {name: "选择启动方式", exact: true}).click();
  return page.getByRole("menu");
}

test.describe("游戏启动方式", () => {
  test("保留默认启动并显式选择全部启动方式", async ({page}) => {
    const game = await openLibrary(page, {
      gameLaunchPath: "/sandbox/minecraft/Minecraft.exe",
      gameSteamAppId: "12345",
      gameLaunchOptions: launchOptions,
    });

    const defaultLaunch = game.getByRole("button", {name: "启动游戏", exact: true});
    const chooseMethod = game.getByRole("button", {name: "选择启动方式", exact: true});
    await expect(defaultLaunch).toBeVisible();
    await expect(defaultLaunch).toBeEnabled();
    await expect(chooseMethod).toBeVisible();
    await expect(chooseMethod).toBeEnabled();

    // The original primary action keeps using the game's configured default.
    await defaultLaunch.click();
    await expect.poll(async () => (await calls(page, "launch_game")).length).toBe(1);
    expect((await calls(page, "launch_game")).map((call: any) => call.args)).toEqual([{id: "game-fixture"}]);

    const narrowViewport = (page.viewportSize()?.width ?? 0) <= 500;
    if (narrowViewport) {
      const [primaryBox, arrowBox] = await Promise.all([defaultLaunch.boundingBox(), chooseMethod.boundingBox()]);
      expect(primaryBox).not.toBeNull();
      expect(arrowBox).not.toBeNull();
      expect(primaryBox!.x + primaryBox!.width).toBeLessThanOrEqual(arrowBox!.x);
      expect(arrowBox!.x + arrowBox!.width).toBeLessThanOrEqual(page.viewportSize()!.width);
    }

    const menu = await openLaunchMenu(game, page);
    await expect.poll(async () => (await calls(page, "list_game_launch_options")).length).toBe(1);
    expect((await calls(page, "list_game_launch_options")).map((call: any) => call.args)).toEqual([{id: "game-fixture"}]);
    for (const label of ["Steam 启动", "直接启动", "SMAPI 启动", "Mod Engine 2 启动", "Prism Launcher"]) {
      await expect(menu.getByRole("menuitem", {name: label, exact: true})).toBeVisible();
    }

    if (narrowViewport) {
      const menuBox = await menu.boundingBox();
      expect(menuBox).not.toBeNull();
      expect(menuBox!.x).toBeGreaterThanOrEqual(0);
      expect(menuBox!.x + menuBox!.width).toBeLessThanOrEqual(page.viewportSize()!.width);
    }

    const selections = [
      ["Steam 启动", "steam"],
      ["直接启动", "direct"],
      ["SMAPI 启动", "smapi"],
      ["Mod Engine 2 启动", "mod-engine-2"],
      ["Prism Launcher", "custom"],
    ] as const;
    for (const [index, [label]] of selections.entries()) {
      const activeMenu = index === 0 ? menu : await openLaunchMenu(game, page);
      await activeMenu.getByRole("menuitem", {name: label, exact: true}).click();
      await expect.poll(async () => (await calls(page, "launch_game")).length).toBe(index + 2);
    }
    expect((await calls(page, "launch_game")).map((call: any) => call.args)).toEqual([
      {id: "game-fixture"},
      {id: "game-fixture", method: "steam"},
      {id: "game-fixture", method: "direct"},
      {id: "game-fixture", method: "smapi"},
      {id: "game-fixture", method: "mod-engine-2"},
      {id: "game-fixture", method: "custom"},
    ]);
    expect((await calls(page, "list_game_launch_options")).map((call: any) => call.args)).toEqual([
      {id: "game-fixture"}, {id: "game-fixture"}, {id: "game-fixture"},
      {id: "game-fixture"}, {id: "game-fixture"},
    ]);
  });

  test("启动器配置保存后可读取并移除", async ({page}) => {
    const game = await openLibrary(page, {gameWithoutMods: true});
    let menu = await openLaunchMenu(game, page);
    await menu.getByRole("menuitem", {name: "设置 Mod 启动器", exact: true}).click();

    let dialog = page.getByRole("dialog", {name: "设置 Mod 启动器", exact: true});
    await expect(dialog).toBeVisible();
    const name = dialog.getByLabel("启动器名称", {exact: true});
    const path = dialog.getByLabel("启动器程序", {exact: true});
    const args = dialog.getByLabel("启动参数", {exact: true});
    await name.fill("Fixture Mod Launcher");
    await path.fill("/sandbox/minecraft/mod-launcher.exe");
    await args.fill("--profile\nfixture profile");
    await dialog.getByRole("button", {name: "保存", exact: true}).click();
    await expect(dialog).toBeHidden();

    const saved = {name: "Fixture Mod Launcher", path: "/sandbox/minecraft/mod-launcher.exe", args: ["--profile", "fixture profile"]};
    expect((await calls(page, "set_game_mod_launcher")).map((call: any) => call.args)).toEqual([
      {id: "game-fixture", launcher: saved},
    ]);
    expect(await page.evaluate(() => (window as any).__AMM_TEST__.games[0].metadata.modLauncher)).toEqual(saved);

    menu = await openLaunchMenu(game, page);
    await expect(menu.getByRole("menuitem", {name: "Fixture Mod Launcher", exact: true})).toBeVisible();
    await menu.getByRole("menuitem", {name: "设置 Mod 启动器", exact: true}).click();
    dialog = page.getByRole("dialog", {name: "设置 Mod 启动器", exact: true});
    await expect(dialog).toBeVisible();
    await expect(dialog.getByLabel("启动器名称", {exact: true})).toHaveValue(saved.name);
    await expect(dialog.getByLabel("启动器程序", {exact: true})).toHaveValue(saved.path);
    await expect(dialog.getByLabel("启动参数", {exact: true})).toHaveValue("--profile\nfixture profile");
    if ((page.viewportSize()?.width ?? 0) <= 500) {
      const dialogBox = await dialog.boundingBox();
      expect(dialogBox).not.toBeNull();
      expect(dialogBox!.x).toBeGreaterThanOrEqual(0);
      expect(dialogBox!.x + dialogBox!.width).toBeLessThanOrEqual(page.viewportSize()!.width);
    }

    await dialog.getByRole("button", {name: "移除启动器", exact: true}).click();
    await expect(dialog).toBeHidden();
    expect((await calls(page, "set_game_mod_launcher")).map((call: any) => call.args)).toEqual([
      {id: "game-fixture", launcher: saved},
      {id: "game-fixture", launcher: null},
    ]);
    expect(await page.evaluate(() => Object.hasOwn((window as any).__AMM_TEST__.games[0].metadata, "modLauncher"))).toBe(false);

    menu = await openLaunchMenu(game, page);
    await menu.getByRole("menuitem", {name: "设置 Mod 启动器", exact: true}).click();
    dialog = page.getByRole("dialog", {name: "设置 Mod 启动器", exact: true});
    await expect(dialog.getByLabel("启动器名称", {exact: true})).toHaveValue("");
    await expect(dialog.getByLabel("启动器程序", {exact: true})).toHaveValue("");
    await expect(dialog.getByLabel("启动参数", {exact: true})).toHaveValue("");
  });

  test("读取启动方式失败时不启动", async ({page}) => {
    const unavailable = await openLibrary(page, {failGameLaunchOptions: true});
    await unavailable.getByRole("button", {name: "选择启动方式", exact: true}).click();
    await expect.poll(async () => (await calls(page, "list_game_launch_options")).length).toBe(1);
    await expect(page.getByRole("menu").getByRole("alert")).toContainText("模拟读取启动方式失败");
    await expect(page.getByRole("menuitem", {name: "设置 Mod 启动器"})).toBeEnabled();
    expect(await calls(page, "launch_game")).toHaveLength(0);
  });

  test("显式启动失败时不回退到其他方法", async ({page}) => {
    const failing = await openLibrary(page, {
      gameSteamAppId: "12345",
      gameLaunchOptions: [
        {id: "steam", label: null},
        {id: "mod-engine-2", label: null},
      ],
      failGameLaunchMethods: ["mod-engine-2"],
    });
    const menu = await openLaunchMenu(failing, page);
    await menu.getByRole("menuitem", {name: "Mod Engine 2 启动", exact: true}).click();
    await expect(page.locator("main [role=alert]")).toContainText("模拟启动失败: mod-engine-2");
    await expect.poll(async () => (await calls(page, "launch_game")).length).toBe(1);
    expect((await calls(page, "launch_game")).map((call: any) => call.args)).toEqual([
      {id: "game-fixture", method: "mod-engine-2"},
    ]);
  });
});
