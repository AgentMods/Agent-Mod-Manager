import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const calls = (page: Page, command: string) => page.evaluate((name) => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === name), command);
const card = (page: Page, name: string) => page.locator('[data-slot="card"]').filter({has: page.getByRole("heading", {name, exact: true})});

async function openAdapterDialog(page: Page) {
  await card(page, "Minecraft Java Edition").getByRole("button", {name: "更多操作"}).click();
  await page.getByRole("menuitem", {name: "选择适配包", exact: true}).click();
  return page.getByRole("dialog", {name: "选择适配包", exact: true});
}

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
    const newName = "游戏显示名称 ".repeat(90) + "终章";
    const dialog = page.getByRole("dialog", {name: "编辑游戏", exact: true});
    await expect(dialog.getByLabel("游戏名称", {exact: true})).toHaveValue("Minecraft Java Edition");
    await dialog.getByLabel("游戏名称", {exact: true}).fill(newName);
    await dialog.getByLabel("启动程序", {exact: true}).fill("/sandbox/minecraft/launcher.exe");
    await dialog.getByRole("button", {name: "保存", exact: true}).click();
    await expect(dialog).not.toBeVisible();
    await expect(card(page, newName)).toBeVisible();
    expect((await calls(page, "update_game"))[0].args).toEqual({id: "game-fixture", name: newName, launchPath: "/sandbox/minecraft/launcher.exe"});
    await card(page, newName).getByRole("button", {name: "启动游戏", exact: true}).click();
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
    await installMockTauri(page, {onboarding: true, withExternalAdapter: true});
    await page.goto("/");
    await page.getByRole("button", {name: "开始设置", exact: true}).click();
    await page.getByRole("button", {name: "下一步", exact: true}).click();
    await expect(page.getByRole("heading", {name: "配置 AI 模型服务", exact: true})).toBeVisible();
    await page.getByRole("button", {name: "跳过配置", exact: true}).click();
    await expect(page.getByRole("heading", {name: "启用可用工具", exact: true})).toBeVisible();
    await page.getByRole("button", {name: "下一步", exact: true}).click();
    await expect(page.getByRole("heading", {name: "扫描游戏列表", exact: true})).toBeVisible();
    const fixture = page.getByRole("listitem").filter({hasText: "Fixture Game"});
    await expect(fixture.getByRole("button", {name: "添加", exact: true})).toBeEnabled();
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

  test("待执行 Mod 计划阻止移除，可查看并解除，但必须再次单独确认移除", async ({page}) => {
    await installMockTauri(page, {
      gameWithoutMods: true,
      gamePlanConflicts: [{
        id: "plan-pending-mod-install",
        kind: "file_plan",
        status: "planned",
        title: "待执行 Mod 安装计划",
        summary: "安装 Example Mod 到 Minecraft Java Edition",
        paths: ["/sandbox/minecraft/mods/ExampleMod.jar", "/sandbox/minecraft/mods"],
        detail: "该计划尚未执行，解除只会取消计划。",
        planId: "plan-pending-mod-install",
        canResolve: true,
        resolutionHint: null,
        blockedChanges: ["remove"],
      }],
    });
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();

    const game = card(page, "Minecraft Java Edition");
    await expect(game).toContainText("0 个 Mod");
    await game.getByRole("button", {name: "更多操作", exact: true}).click();
    await page.getByRole("menuitem", {name: "移除", exact: true}).click();

    const dialog = page.getByRole("alertdialog", {name: "移除游戏", exact: true});
    await expect(dialog).toBeVisible();
    const conflict = dialog.getByRole("listitem").filter({hasText: "待执行 Mod 安装计划"});
    await expect(conflict).toContainText("安装 Example Mod 到 Minecraft Java Edition");
    await expect(conflict).toContainText("/sandbox/minecraft/mods/ExampleMod.jar");
    await expect(conflict).toContainText("待执行");
    await conflict.getByRole("button", {name: "查看", exact: true}).click();
    await expect(conflict).toContainText("该计划尚未执行，解除只会取消计划。");
    await expect(conflict).toContainText("/sandbox/minecraft/mods");
    await expect(conflict).toContainText("任务 ID：plan-pending-mod-install");

    const conflictQueries = await calls(page, "list_game_conflicts");
    expect(conflictQueries.some((call: any) => call.args.gameId === "game-fixture" && call.args.change === "remove")).toBe(true);
    expect(await calls(page, "remove_game")).toHaveLength(0);
    await conflict.getByRole("button", {name: "解除", exact: true}).click();
    await expect(dialog.getByRole("listitem")).toHaveCount(0);
    expect((await calls(page, "resolve_game_conflict")).map((call: any) => call.args)).toEqual([
      expect.objectContaining({gameId: "game-fixture", conflictId: "plan-pending-mod-install", change: "remove"}),
    ]);

    // Resolving a planned task cancels only that plan; it must not resume the destructive action.
    expect(await calls(page, "remove_game")).toHaveLength(0);
    expect(await calls(page, "apply_file_ops")).toHaveLength(0);
    expect(await calls(page, "plan_install")).toHaveLength(0);
    const removeButton = dialog.getByRole("button", {name: "移除", exact: true});
    await expect(removeButton).toBeEnabled();
    await removeButton.click();
    await expect(dialog).not.toBeVisible();
    await expect(game).toHaveCount(0);
    await expect(page.getByText("尚无管理中的游戏", {exact: true})).toBeVisible();
    expect((await calls(page, "remove_game")).map((call: any) => call.args)).toEqual([{id: "game-fixture"}]);
    expect(await calls(page, "apply_file_ops")).toHaveLength(0);
    expect(await calls(page, "plan_install")).toHaveLength(0);
  });

  test("取消移除会保留适配包，确认移除后可重新绑定", async ({page}) => {
    await installMockTauri(page, {withExternalAdapter: true});
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    const game = card(page, "Minecraft Java Edition");
    const dialog = await openAdapterDialog(page);
    const adapter = dialog.getByLabel("游戏适配包");
    const remove = dialog.getByRole("button", {name: "移除适配包", exact: true});

    await adapter.selectOption({label: "移除适配包"});
    await dialog.getByRole("button", {name: "取消"}).click();
    await expect(dialog).toBeHidden();
    expect(await calls(page, "remove_game_adapter")).toHaveLength(0);

    await openAdapterDialog(page);
    await expect(adapter).toHaveValue("minecraft-java");
    await adapter.selectOption({label: "移除适配包"});
    await remove.click();
    await expect(dialog).toBeHidden();
    await expect(game.getByRole("button", {name: "创建适配包"})).toBeVisible();
    await expect(game).toContainText("windows · 1 个 Mod");
    expect((await calls(page, "remove_game_adapter")).map((call: any) => call.args)).toEqual([{id: "game-fixture"}]);

    await openAdapterDialog(page);
    await expect(adapter).toHaveValue("");
    await expect(adapter.getByRole("option", {name: "移除适配包"})).toHaveCount(0);
    await expect(dialog.getByRole("button", {name: "应用适配包"})).toBeDisabled();
    await adapter.selectOption("minecraft-java");
    await dialog.getByRole("button", {name: "应用适配包"}).click();
    await expect(dialog).toBeHidden();
    await expect(game.getByRole("button", {name: "创建适配包"})).toHaveCount(0);
  });

  test("当前适配包停用后仍可移除", async ({page}) => {
    await installMockTauri(page, {adapterDisabled: true, withExternalAdapter: true});
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    const dialog = await openAdapterDialog(page);
    await dialog.getByLabel("游戏适配包").selectOption({label: "移除适配包"});
    await dialog.getByRole("button", {name: "移除适配包", exact: true}).click();
    await expect(dialog).toBeHidden();
    await expect(card(page, "Minecraft Java Edition").getByRole("button", {name: "创建适配包"})).toBeVisible();
  });

  test("待处理任务阻止移除适配包，解除冲突后仍需确认", async ({page}) => {
    await installMockTauri(page, {withExternalAdapter: true, gamePlanConflicts: [{id: "plan-pending", kind: "file_plan", status: "planned", title: "更新 Minecraft 文件", paths: [], canResolve: true}]});
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    const dialog = await openAdapterDialog(page);
    await dialog.getByLabel("游戏适配包").selectOption({label: "移除适配包"});
    const remove = dialog.getByRole("button", {name: "移除适配包", exact: true});
    await expect(remove).toBeDisabled();
    await dialog.getByRole("button", {name: "解除", exact: true}).click();
    await expect(remove).toBeEnabled();
    expect(await calls(page, "remove_game_adapter")).toHaveLength(0);
    await remove.click();
    await expect(dialog).toBeHidden();
  });

  test("浏览器预览中所有卡片操作保持禁用", async ({page}) => {
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    await expect(page.getByRole("status").filter({hasText: "未连接桌面服务"})).toBeVisible();
    await expect(page.locator('[data-slot="card"]').filter({has: page.getByRole("button", {name: "启动游戏"})})).toHaveCount(0);
  });
});
