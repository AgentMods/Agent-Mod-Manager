import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const calls = (page: Page, command: string) => page.evaluate((name) => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === name), command);
const card = (page: Page, name: string) => page.locator('[data-slot="card"]').filter({has: page.getByRole("heading", {name, exact: true})});

test.describe("游戏卡片操作", () => {
  test("卡片只显示名称、平台与 Mod 数量，不显示安装目录", async ({page}) => {
    await installMockTauri(page);
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    const minecraft = card(page, "Minecraft Java Edition");
    await expect(minecraft).toContainText("windows · 1 个 Mod");
    await expect(minecraft).not.toContainText("/sandbox/minecraft");
    for (const name of ["启动游戏", "更多操作"]) {
      await expect(minecraft.getByRole("button", {name, exact: true})).toBeVisible();
    }
    for (const name of ["打开目录", "Mod 文件夹"]) {
      await expect(minecraft.getByRole("button", {name, exact: true})).toHaveCount(0);
    }
  });

  test("打开目录与 Mod 文件夹只传游戏 ID 和固定目标，不传路径", async ({page}) => {
    await installMockTauri(page);
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    const minecraft = card(page, "Minecraft Java Edition");
    const moreActions = minecraft.getByRole("button", {name: "更多操作", exact: true});
    const menu = page.getByRole("menu");
    await moreActions.click();
    await menu.getByRole("menuitem", {name: "打开目录", exact: true}).click();
    await moreActions.click();
    await menu.getByRole("menuitem", {name: "Mod 文件夹", exact: true}).click();
    await expect.poll(async () => (await calls(page, "open_game_location")).length).toBe(2);
    expect((await calls(page, "open_game_location")).map((call: any) => call.args)).toEqual([
      {id: "game-fixture", target: "root"},
      {id: "game-fixture", target: "mods"},
    ]);
    await expect(page.locator("main [role=alert]")).toHaveCount(0);
  });

  test("Mod 文件夹不存在时显示后端给出的原因", async ({page}) => {
    await installMockTauri(page);
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    await page.evaluate(() => { (window as any).__AMM_TEST__.modsFolderMissing = true; });
    await card(page, "Minecraft Java Edition").getByRole("button", {name: "更多操作", exact: true}).click();
    await page.getByRole("menu").getByRole("menuitem", {name: "Mod 文件夹", exact: true}).click();
    await expect(page.locator("main [role=alert]")).toContainText("Mod 目录不存在或不是目录");
  });

  test("没有启动方式时不会假装启动，提示去编辑里选择程序", async ({page}) => {
    await installMockTauri(page);
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    await card(page, "Minecraft Java Edition").getByRole("button", {name: "启动游戏", exact: true}).click();
    await expect(page.locator("main [role=alert]")).toContainText("尚未设置启动方式");
    expect(await calls(page, "launch_game")).toHaveLength(1);
  });

  test("编辑名称与启动程序，保存后卡片和启动都使用新设置", async ({page}) => {
    await installMockTauri(page);
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    const minecraft = card(page, "Minecraft Java Edition");
    await minecraft.getByRole("button", {name: "更多操作", exact: true}).click();
    await page.getByRole("menuitem", {name: "编辑", exact: true}).click();
    const dialog = page.getByRole("dialog", {name: "编辑游戏", exact: true});
    await expect(dialog.getByLabel("游戏名称", {exact: true})).toHaveValue("Minecraft Java Edition");
    await dialog.getByLabel("游戏名称", {exact: true}).fill("我的世界");
    await dialog.getByLabel("启动程序", {exact: true}).fill("/sandbox/minecraft/launcher.exe");
    await dialog.getByRole("button", {name: "保存", exact: true}).click();
    await expect(dialog).not.toBeVisible();
    await expect(card(page, "我的世界")).toBeVisible();
    expect((await calls(page, "update_game"))[0].args).toEqual({id: "game-fixture", name: "我的世界", launchPath: "/sandbox/minecraft/launcher.exe"});
    await card(page, "我的世界").getByRole("button", {name: "启动游戏", exact: true}).click();
    await expect.poll(async () => (await calls(page, "launch_game")).length).toBe(1);
    await expect(page.locator("main [role=alert]")).toHaveCount(0);
  });

  test("启动程序在游戏目录之外时保存失败并保留对话框", async ({page}) => {
    await installMockTauri(page);
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    await card(page, "Minecraft Java Edition").getByRole("button", {name: "更多操作", exact: true}).click();
    await page.getByRole("menuitem", {name: "编辑", exact: true}).click();
    const dialog = page.getByRole("dialog", {name: "编辑游戏", exact: true});
    await dialog.getByLabel("启动程序", {exact: true}).fill("/elsewhere/game.exe");
    await dialog.getByRole("button", {name: "保存", exact: true}).click();
    await expect(dialog.getByRole("alert")).toContainText("启动程序必须位于该游戏的根目录内");
    await expect(dialog).toBeVisible();
  });

  test("移除需要确认，只移出管理，仍有 Mod 时被拒绝", async ({page}) => {
    await installMockTauri(page);
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    const minecraft = card(page, "Minecraft Java Edition");
    await minecraft.getByRole("button", {name: "更多操作", exact: true}).click();
    await page.getByRole("menuitem", {name: "移除", exact: true}).click();
    const dialog = page.getByRole("alertdialog", {name: "移除游戏", exact: true});
    await expect(dialog).toContainText("不会删除游戏文件");
    expect(await calls(page, "remove_game")).toHaveLength(0);
    await dialog.getByRole("button", {name: "取消", exact: true}).click();
    await expect(dialog).not.toBeVisible();
    expect(await calls(page, "remove_game")).toHaveLength(0);

    await minecraft.getByRole("button", {name: "更多操作", exact: true}).click();
    await page.getByRole("menuitem", {name: "移除", exact: true}).click();
    await dialog.getByRole("button", {name: "移除", exact: true}).click();
    await expect(dialog.getByRole("alert")).toContainText("该游戏仍有 1 个已安装的 Mod");
    // While the modal is open the page behind it is aria-hidden, so close it before asserting on the card.
    await dialog.getByRole("button", {name: "取消", exact: true}).click();
    await expect(minecraft).toBeVisible();
    expect(await calls(page, "remove_game")).toHaveLength(1);
  });

  test("没有 Mod 的游戏可以移除，卡片随之消失", async ({page}) => {
    // The onboarding fixture registers "Fixture Game" with zero installed Mods.
    await installMockTauri(page, {onboarding: true});
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    await page.getByRole("button", {name: "扫描已安装游戏"}).click();
    const fixture = page.getByRole("listitem").filter({hasText: "Fixture Game"});
    await fixture.getByRole("checkbox", {name: "选择 Fixture Game"}).check();
    await fixture.getByRole("button", {name: "添加"}).click();
    await expect(fixture.getByText("管理中", {exact: true})).toBeVisible();
    await page.getByRole("button", {name: "退出引导"}).click();

    const game = card(page, "Fixture Game");
    await expect(game).toContainText("0 个 Mod");
    await game.getByRole("button", {name: "更多操作", exact: true}).click();
    await page.getByRole("menuitem", {name: "移除", exact: true}).click();
    await page.getByRole("alertdialog", {name: "移除游戏", exact: true}).getByRole("button", {name: "移除", exact: true}).click();
    await expect(game).toHaveCount(0);
    await expect(page.getByText("尚无管理中的游戏", {exact: true})).toBeVisible();
    expect((await calls(page, "remove_game")).map((call: any) => call.args)).toEqual([{id: "game-fixture-game"}]);
  });
  test("浏览器预览中所有卡片操作保持禁用", async ({page}) => {
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    await expect(page.getByRole("status").filter({hasText: "未连接桌面服务"})).toBeVisible();
    await expect(page.locator('[data-slot="card"]').filter({has: page.getByRole("button", {name: "启动游戏"})})).toHaveCount(0);
  });
});