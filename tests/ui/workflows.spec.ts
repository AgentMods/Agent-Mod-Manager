import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

test("browser preview never pretends to have native filesystem access", async ({page}) => {
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await expect(page.getByRole("status").filter({hasText: "未连接桌面服务"})).toBeVisible();
  await expect(page.getByRole("button", {name: "扫描游戏", exact: true})).toBeDisabled();
  await expect(page.getByTestId("library-page")).not.toContainText("Minecraft Java Edition");
  await page.getByRole("button", {name: /^Agent( 对话)?$/}).click();
  await expect(page.getByRole("textbox", {name: "告诉 Agent 你想做什么"})).toBeDisabled();
});

test("streams pre-invoke deltas once, previews approval and persists completion", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await expect(page.getByText("Minecraft Java Edition", {exact: true})).toBeVisible();
  await page.getByRole("button", {name: /^Agent( 对话)?$/}).click();
  await page.getByRole("textbox", {name: "告诉 Agent 你想做什么"}).fill("帮我检查 Minecraft 模组");
  await page.getByRole("button", {name: "发送消息", exact: true}).click();
  const approval = page.getByRole("dialog", {name: "操作审批", exact: true});
  await expect(approval).toBeVisible();
  await expect(approval).toContainText("/sandbox/minecraft/mods/Example.jar");
  await expect(approval).toContainText("txn-fixture");
  await expect(page.locator('[data-message-id="assistant-fixture"]')).toHaveText("正在检查模组文件。");
  await approval.getByRole("button", {name: "批准并执行", exact: true}).click();
  await expect(approval).not.toBeVisible();
  await expect(page.locator('[data-message-id="assistant-fixture"]')).toContainText("检查完成，文件计划已执行。");
  await expect(page.getByText("120 tokens", {exact: true})).toBeVisible();
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: /^Agent( 对话)?$/}).click();
  await expect(page.locator('[data-message-id="assistant-fixture"]')).toHaveCount(1);
  await expect(page.locator('[data-message-id="assistant-fixture"]')).toHaveText(/正在检查模组文件。\s*检查完成，文件计划已执行。/);
  const responses = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "respond_approval"));
  expect(responses).toHaveLength(1);
  expect(responses[0].args.response.approved).toBe(true);
});

test("trash restore needs a visible plan and separate apply confirmation", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "回收站", exact: true}).click();
  await page.getByRole("checkbox", {name: "选择 Example.jar", exact: true}).check();
  await page.getByRole("button", {name: "恢复所选", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "确认文件变更", exact: true});
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText("/sandbox/minecraft/mods/Example.jar");
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === "apply_plan").length)).toBe(0);
  await dialog.getByRole("button", {name: "取消", exact: true}).click();
  await expect(page.getByRole("checkbox", {name: "选择 Example.jar", exact: true})).toBeVisible();
  await page.getByRole("button", {name: "恢复所选", exact: true}).click();
  await dialog.getByRole("button", {name: "批准并执行", exact: true}).click();
  await expect(dialog).not.toBeVisible();
  await expect(page.getByText("回收站为空", {exact: true})).toBeVisible();
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === "apply_plan").length)).toBe(1);
});

test("permanent trash purge needs both acknowledgement and exact confirmation", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "回收站", exact: true}).click();
  await page.getByRole("checkbox", {name: "选择 Example.jar", exact: true}).check();
  await page.getByRole("button", {name: "永久删除所选", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "永久删除回收站文件", exact: true});
  const confirm = dialog.getByRole("button", {name: "永久删除", exact: true});
  await expect(confirm).toBeDisabled();
  await dialog.getByRole("textbox", {name: "永久删除确认文本", exact: true}).fill("PURGE_TRASH");
  await expect(confirm).toBeDisabled();
  await dialog.getByRole("checkbox").check();
  await expect(confirm).toBeEnabled();
  await confirm.click();
  await expect(dialog).not.toBeVisible();
  await expect(page.getByText("回收站为空", {exact: true})).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === "purge_trash"));
  expect(calls).toHaveLength(1);
  expect(calls[0].args).toEqual({ids: ["trash_fixture"], confirmation: "PURGE_TRASH"});
});

test("history can be archived and restored without deleting conversation data", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "历史对话", exact: true}).click();
  await page.getByRole("button", {name: "归档", exact: true}).click();
  await expect(page.getByText("暂无历史对话", {exact: true})).toBeVisible();
  await page.getByRole("tab", {name: "已归档", exact: true}).click();
  await expect(page.getByRole("button", {name: "打开对话：检查 Minecraft 模组", exact: true})).toBeVisible();
  await page.getByRole("button", {name: "取消归档", exact: true}).click();
  await expect(page.getByText("暂无归档对话", {exact: true})).toBeVisible();
  await page.getByRole("tab", {name: "活动对话", exact: true}).click();
  await page.getByRole("button", {name: "打开对话：检查 Minecraft 模组", exact: true}).click();
  await expect(page.locator(".chat-header h1")).toHaveText("检查 Minecraft 模组");
  const conversations = await page.evaluate(() => (window as any).__AMM_TEST__.conversations);
  expect(conversations).toHaveLength(1);
  expect(conversations[0].archived).toBe(false);
});

for (const theme of ["light", "dark"] as const) {
  test(`long provider model lists scroll within their panel in ${theme} theme`, async ({page}) => {
    const longId = "provider/" + "a".repeat(240);
    const providerModels = [longId, ...Array.from({length: 80}, (_, index) => `fixture-model-${index + 1}`)];
    await page.emulateMedia({colorScheme: theme});
    await installMockTauri(page, {providerModels});
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
    await page.getByRole("button", {name: "设置", exact: true}).click();
    await page.getByRole("button", {name: "获取模型列表"}).click();

    const picker = page.getByRole("group", {name: "服务商模型（可多选）"});
    await expect(picker.getByRole("checkbox")).toHaveCount(providerModels.length);
    const viewport = picker.locator('[data-slot="scroll-area-viewport"]');
    const add = picker.getByRole("button", {name: "添加所选（0）"});
    const bounds = await viewport.evaluate((element) => ({
      bottom: element.getBoundingClientRect().bottom,
      height: element.clientHeight,
      scrollHeight: element.scrollHeight,
      width: element.clientWidth,
      scrollWidth: element.scrollWidth,
    }));
    expect(bounds.height).toBeLessThanOrEqual(192);
    expect(bounds.scrollHeight).toBeGreaterThan(bounds.height);
    expect(bounds.scrollWidth).toBeLessThanOrEqual(bounds.width);
    expect((await add.boundingBox())?.y).toBeGreaterThan(bounds.bottom);

    const last = picker.getByRole("checkbox", {name: "fixture-model-80", exact: true});
    await last.check();
    expect(await viewport.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
    await picker.getByRole("button", {name: "添加所选（1）"}).click();
    await expect(page.getByLabel("fixture-model-80 上下文预算")).toHaveValue("32768");
    await expect(page.getByRole("button", {name: "保存设置", exact: true})).toBeEnabled();
  });
}

test("model credentials use the dedicated secret command, not public settings", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "设置", exact: true}).click();
  await page.getByLabel("API 密钥", {exact: true}).fill("sk-fixture-only");
  await page.getByRole("button", {name: "手动添加", exact: true}).click();
  await page.getByRole("textbox", {name: "新模型 实际请求模型", exact: true}).fill("test-provider-model");
  await page.getByRole("combobox", {name: "默认模型", exact: true}).selectOption("test-provider-model");
  await page.getByRole("button", {name: "保存设置", exact: true}).click();
  await expect(page.getByLabel("API 密钥", {exact: true})).toHaveValue("");
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.find((c: any) => c.command === "set_secret").args).toEqual({key: "model.api_key", value: "sk-fixture-only"});
  const settings = calls.find((c: any) => c.command === "set_settings").args.settings;
  expect(settings.model).toBe("test-provider-model");
  expect(settings.api_key_ref).toBe("model.api_key");
  expect(JSON.stringify(settings)).not.toContain("sk-fixture-only");
  expect(await page.evaluate(() => JSON.stringify({...localStorage}))).not.toContain("sk-fixture-only");
  await page.getByRole("tab", {name: "MCP 服务", exact: true}).click();
  await expect(page.getByRole("button", {name: "开启服务", exact: true})).toBeEnabled();
  await expect(page.locator("main [role=alert]")).toHaveCount(0);
  await page.getByRole("tab", {name: "Skills", exact: true}).click();
  await expect(page.getByRole("button", {name: "导入 Skill", exact: true})).toBeEnabled();
});

test("rejecting approval never reports a committed file operation", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: /^Agent( 对话)?$/}).click();
  await page.getByRole("textbox", {name: "告诉 Agent 你想做什么"}).fill("检查文件");
  await page.getByRole("button", {name: "发送消息", exact: true}).click();
  const approval = page.getByRole("dialog", {name: "操作审批", exact: true});
  await expect(approval).toBeVisible();
  await approval.getByRole("button", {name: "拒绝", exact: true}).click();
  await expect(page.locator('[data-message-id="assistant-fixture"]')).toContainText("未执行文件操作。");
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((call: any) => call.command === "apply_plan")).toHaveLength(0);
});

test("plugin enable control uses the backend and survives navigation", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "插件中心", exact: true}).click();
  await page.getByRole("button").filter({hasText: "Minecraft Java Edition"}).click();
  const toggle = page.getByLabel("启用适配包 Minecraft Java Edition", {exact: true});
  await expect(toggle).toBeChecked();
  await toggle.uncheck();
  await expect(toggle).not.toBeChecked();
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "插件中心", exact: true}).click();
  await page.getByRole("button").filter({hasText: "Minecraft Java Edition"}).click();
  await expect(toggle).not.toBeChecked();
  await toggle.check();
  await expect(toggle).toBeChecked();
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((call: any) => call.command === "set_plugin_enabled").map((call: any) => call.args)).toEqual([
    {slug: "minecraft-java", enabled: false}, {slug: "minecraft-java", enabled: true},
  ]);
  expect(calls.filter((call: any) => call.command === "apply_plan")).toHaveLength(0);
});

test("adapter center previews the game Skill and prefills Agent creation and revision", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  const adapterCenter = page.getByRole("button", {name: "插件中心", exact: true});
  const composer = page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true});
  await adapterCenter.click();
  await page.getByRole("button", {name: "用 Agent 创建", exact: true}).click();
  await expect(composer).toHaveValue(/创建.*适配包/);

  await adapterCenter.click();
  await page.getByRole("row").getByRole("button", {name: /Minecraft Java Edition/}).click();
  await page.getByRole("button", {name: "查看游戏 Skill", exact: true}).click();
  const skill = page.getByRole("dialog", {name: "游戏 Skill · minecraft-java", exact: true});
  await expect(skill).toContainText("Minecraft Java Edition 适配指导");
  const enabled = skill.getByRole("checkbox", {name: "启用游戏 Skill"});
  await expect(enabled).toBeChecked();
  await enabled.uncheck();
  await expect(enabled).not.toBeChecked();
  await enabled.check();
  await expect(enabled).toBeChecked();
  await page.keyboard.press("Escape");
  await expect(skill).toBeHidden();
  await page.getByRole("button", {name: "用 Agent 修改", exact: true}).click();
  await expect(composer).toHaveValue(/修改.*minecraft-java/);
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((call: any) => call.command === "set_skill_enabled").map((call: any) => call.args)).toEqual([
    {name: "minecraft-java", enabled: false},
    {name: "minecraft-java", enabled: true},
  ]);
  expect(calls.some((call: any) => call.command === "start_turn")).toBe(false);
});

test("library prefills adapter creation for an unbound game and displays its bound cover", async ({page}) => {
  await installMockTauri(page, {gameWithoutAdapter: true});
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  const library = page.getByTestId("library-page");
  const card = library.locator('[data-slot="card"]').filter({has: page.getByRole("heading", {name: "Minecraft Java Edition", exact: true})});
  const cover = card.locator('img[alt=""]');
  await expect(cover).toHaveCount(0);
  await card.getByRole("button", {name: "创建适配包", exact: true}).click();
  const composer = page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true});
  await expect(composer).toHaveValue(/Minecraft Java Edition.*创建适配包/);

  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await card.getByRole("button", {name: "更多操作", exact: true}).click();
  await page.getByRole("menuitem", {name: "选择适配包", exact: true}).click();
  const binding = page.getByRole("dialog", {name: "选择适配包", exact: true});
  await binding.getByRole("combobox", {name: "游戏适配包", exact: true}).selectOption("minecraft-java");
  await binding.getByRole("button", {name: "应用适配包", exact: true}).click();
  await expect(binding).toBeHidden();
  await expect(card.getByRole("button", {name: "创建适配包", exact: true})).toHaveCount(0);
  await expect(cover).toHaveAttribute("src", "/favicon.png");
  await expect.poll(() => cover.evaluate((image: HTMLImageElement) => image.naturalWidth)).toBeGreaterThan(0);

  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((call: any) => call.command === "set_game_adapter").map((call: any) => call.args)).toEqual([
    {id: "game-fixture", adapterSlug: "minecraft-java"},
  ]);
  expect(calls.some((call: any) => call.command === "start_turn")).toBe(false);
});

test("library explains adapter conflicts and lets the player release pending tasks", async ({page}) => {
  await installMockTauri(page, {
    gameWithoutAdapter: true,
    gamePlanConflicts: [
      {id: "txn-pbz-mods", kind: "file_plan", status: "planned", title: "文件任务", summary: "创建 Mods 目录", paths: ["/sandbox/minecraft/Content/Paks/~mods"], detail: "这项任务尚未执行，不会自动修改游戏文件。", canResolve: true, resolutionHint: null},
      {id: "approval-pbz", kind: "approval", status: "pending", title: "安装 PBZ", summary: "等待确认安装计划", paths: [], detail: "这是一个等待审批的安装任务。", canResolve: true, resolutionHint: null},
    ],
  });
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  const card = page.getByTestId("library-page").locator('[data-slot="card"]').filter({has: page.getByRole("heading", {name: "Minecraft Java Edition", exact: true})});
  await card.getByRole("button", {name: "更多操作", exact: true}).click();
  await page.getByRole("menuitem", {name: "选择适配包", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "选择适配包", exact: true});
  await expect(dialog).toContainText("适配包选择已阻止");
  await expect(dialog).toContainText("创建 Mods 目录");
  await expect(dialog).toContainText("/sandbox/minecraft/Content/Paks/~mods");
  await expect(dialog.getByRole("button", {name: "应用适配包", exact: true})).toBeDisabled();
  await dialog.getByRole("button", {name: "查看", exact: true}).first().click();
  await expect(dialog).toContainText("这项任务尚未执行");
  await dialog.getByRole("button", {name: "解除", exact: true}).first().click();
  await expect(dialog).not.toContainText("创建 Mods 目录");
  await dialog.getByRole("button", {name: "解除", exact: true}).click();
  await expect(dialog).toContainText("没有发现会阻止适配包选择");
  await dialog.getByRole("combobox", {name: "游戏适配包", exact: true}).selectOption("minecraft-java");
  await dialog.getByRole("button", {name: "应用适配包", exact: true}).click();
  await expect(dialog).toBeHidden();

  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((call: any) => call.command === "resolve_game_conflict").map((call: any) => call.args.conflictId)).toEqual(["txn-pbz-mods", "approval-pbz"]);
  expect(calls.filter((call: any) => call.command === "set_game_adapter").map((call: any) => call.args)).toEqual([
    {id: "game-fixture", adapterSlug: "minecraft-java"},
  ]);
});

test("library retains the game cover placeholder when the adapter image fails", async ({page}) => {
  await installMockTauri(page);
  await page.route("**/favicon.png", (route) => route.abort());
  const failedImage = page.waitForEvent("requestfailed", (request) => request.resourceType() === "image" && request.url().endsWith("/favicon.png"));
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await failedImage;
  const cardButton = page.getByTestId("library-page").getByRole("button", {name: /Minecraft Java Edition/});
  await expect(cardButton.locator('img[alt=""]')).toHaveCount(0);
  await expect(cardButton.locator(".lucide-gamepad-2")).toBeVisible();
});

test("Settings separates system and standalone Skills from game Skills", async ({page}) => {
  await page.emulateMedia({colorScheme: "dark"});
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "设置", exact: true}).click();
  await page.getByRole("tab", {name: "Skills", exact: true}).click();

  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(page.getByRole("tablist", {name: "设置分类"}).getByRole("tab")).toHaveText([
    "AI 与模型", "MCP 服务", "Skills", "外观与存储",
  ]);
  const panel = page.getByRole("tabpanel", {name: "Skills", exact: true});
  const system = panel.getByRole("region", {name: "系统 Skills", exact: true});
  await expect(system.getByRole("row")).toHaveCount(4);
  for (const name of ["创建与修改适配包", "联网搜索", "从网站下载 Mod"]) {
    const row = system.getByRole("row").filter({has: page.getByRole("button", {name, exact: true})});
    await expect(row.getByRole("cell", {name: "系统内置", exact: true})).toBeVisible();
    await expect(row.getByText("已校验", {exact: true})).toBeVisible();
    await expect(row.getByRole("checkbox", {name: `启用 ${name}`, exact: true})).toBeChecked();
  }
  await expect(panel).not.toContainText("minecraft-java");
  await expect(panel).not.toContainText("Minecraft 模组安装指引");

  const standalone = panel.getByRole("region", {name: "手动安装的 Skills", exact: true});
  await expect(standalone.getByRole("row")).toHaveCount(2);
  await expect(standalone.getByRole("button", {name: "my-game-notes", exact: true})).toBeVisible();
  const importSkill = standalone.getByRole("button", {name: "导入 Skill", exact: true});
  await importSkill.click();
  const importDialog = page.getByRole("dialog", {name: "导入声明式 Skill", exact: true});
  await expect(importDialog.getByRole("textbox", {name: "SKILL.md", exact: true})).toBeVisible();
  await importDialog.getByRole("button", {name: "取消", exact: true}).click();

  await system.getByRole("button", {name: "联网搜索", exact: true}).click();
  const details = page.getByRole("dialog", {name: "联网搜索", exact: true});
  await expect(details).toContainText("仅发送公开的搜索关键词。");
  const braveKey = details.getByLabel("Brave Search API Key", {exact: true});
  await expect(braveKey).toHaveAttribute("type", "password");
  await braveKey.fill("brave-fixture-key");
  await details.getByRole("button", {name: "保存Brave Search API Key", exact: true}).click();
  await expect(braveKey).toHaveValue("");
  const secret = await page.evaluate(() => (window as any).__AMM_TEST__.calls.find((call: any) => call.command === "set_secret"));
  expect(secret?.args).toEqual({key: "sources.brave", value: "brave-fixture-key"});
  await expect(details.getByRole("button", {name: /卸载 Skill/})).toHaveCount(0);
  await page.keyboard.press("Escape");

  const toggle = system.getByRole("checkbox", {name: "启用 联网搜索", exact: true});
  await toggle.uncheck();
  await expect(toggle).not.toBeChecked();
  await page.getByRole("tab", {name: "AI 与模型", exact: true}).click();
  await page.getByRole("tab", {name: "Skills", exact: true}).click();
  await expect(toggle).not.toBeChecked();
  await toggle.check();
  await expect(toggle).toBeChecked();

  const skillCalls = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter(
    (call: any) => call.command === "get_skill" || call.command === "set_skill_enabled",
  ));
  expect(skillCalls).toEqual([
    {command: "get_skill", args: {name: "system-web-search"}},
    {command: "set_skill_enabled", args: {name: "system-web-search", enabled: false}},
    {command: "set_skill_enabled", args: {name: "system-web-search", enabled: true}},
  ]);
});

test("standalone Skill removal requires review and commits only after approval", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "设置", exact: true}).click();
  await page.getByRole("tab", {name: "Skills", exact: true}).click();
  await page.getByRole("button", {name: "my-game-notes", exact: true}).click();
  await page.getByRole("button", {name: "预览卸载 Skill my-game-notes", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "确认文件变更", exact: true});
  await expect(dialog).toContainText("/sandbox/data/skills/my-game-notes/SKILL.md");
  await expect(dialog).toContainText("/sandbox/data/trash/trash_skill/payload");
  await dialog.getByRole("button", {name: "取消", exact: true}).click();
  await expect(page.getByLabel("启用 my-game-notes", {exact: true})).toBeChecked();
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === "apply_plan"))).toHaveLength(0);
  await page.getByRole("button", {name: "my-game-notes", exact: true}).click();
  await page.getByRole("button", {name: "预览卸载 Skill my-game-notes", exact: true}).click();
  await dialog.getByRole("button", {name: "批准并执行", exact: true}).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByLabel("启用 my-game-notes", {exact: true})).not.toBeChecked();
  await expect(page.getByLabel("启用 my-game-notes", {exact: true})).toBeDisabled();
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((call: any) => call.command === "apply_plan").map((call: any) => call.args.planId)).toEqual(["txn-skill-fixture"]);
});

test("file details discard cached content after a reviewed trash commit", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button").filter({hasText: "Minecraft Java Edition"}).click();
  await page.getByRole("tab", {name: "文件", exact: true}).click();
  await page.getByRole("button", {name: "options.txt", exact: true}).click();
  await expect(page.locator(".file-viewer")).toContainText("fixture log");
  await page.getByRole("button", {name: "移入回收站", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "确认文件变更", exact: true});
  await dialog.getByRole("button", {name: "取消", exact: true}).click();
  await expect(page.locator(".file-viewer")).toContainText("fixture log");
  await page.getByRole("button", {name: "移入回收站", exact: true}).click();
  await dialog.getByRole("button", {name: "批准并执行", exact: true}).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByText("未选择文件", {exact: true})).toBeVisible();
  await expect(page.locator(".file-viewer")).toHaveCount(0);
  await expect(page.getByRole("button", {name: "options.txt", exact: true})).toHaveCount(0);
  await expect(page.locator("main [role=alert]")).toHaveCount(0);
});

test("directory picker scans only the chosen library without registering games", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  const directory = page.getByLabel("游戏目录", {exact: true});
  await directory.fill("/sandbox/original");
  await page.getByRole("button", {name: "选择游戏目录", exact: true}).click();
  await expect(directory).toHaveValue("/sandbox/new-game");
  const adapted = page.getByText("已适配", {exact: true});
  await expect(adapted).toHaveAttribute("data-variant", "success");
  await expect(adapted.locator("svg")).toBeVisible();
  await expect(page.getByText("Fixture Game", {exact: true})).toBeVisible();
  await expect(page.getByText("未匹配", {exact: true})).toBeVisible();
  await expect(page.getByText("60% 匹配", {exact: true})).toHaveCount(0);
  await page.getByRole("button", {name: "选择游戏目录", exact: true}).click();
  await expect(directory).toHaveValue("/sandbox/new-game");
  await expect(adapted).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((call: any) => ["register_game", "identify_game", "register_user_root"].includes(call.command))).toHaveLength(0);
  expect(calls.filter((call: any) => call.command === "plugin:dialog|open")).toHaveLength(2);
  expect(calls.filter((call: any) => call.command === "scan_games").map((call: any) => call.args.options)).toEqual([{userRoots: ["/sandbox/new-game"], includeCommonRoots: false}]);

  await directory.fill("/sandbox/empty  ");
  await directory.press("Enter");
  await expect(page.getByText(/未发现候选游戏/)).toBeVisible();
  await expect(adapted).toHaveCount(0);
  const lastScan = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "scan_games").at(-1));
  expect(lastScan.args.options).toEqual({userRoots: ["/sandbox/empty  "], includeCommonRoots: false});
});

test("local archive import and game installation have separate approvals", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button").filter({hasText: "Minecraft Java Edition"}).click();
  await page.getByRole("button", {name: "安装归档", exact: true}).click();
  await page.getByRole("button", {name: "导入本地归档", exact: true}).click();
  const review = page.getByRole("dialog", {name: "确认文件变更", exact: true});
  await expect(review).toContainText("导入本地归档到管理器");
  await expect(review).not.toContainText("/sandbox/minecraft/mods");
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === "plan_install"))).toHaveLength(0);
  await review.getByRole("button", {name: "批准并执行", exact: true}).click();
  const install = page.getByRole("dialog", {name: "归档安装预览", exact: true});
  await expect(install.getByLabel("管理器内归档路径", {exact: true})).toHaveValue("/sandbox/data/staging/imports/fixture/Fixture.jar");
  await install.getByLabel("Mod 名称", {exact: true}).fill("Fixture");
  await install.getByLabel("适配包 Mod 类型", {exact: true}).selectOption("java-mod");
  await install.getByRole("button", {name: "生成变更预览", exact: true}).click();
  await expect(review).toContainText("/sandbox/minecraft/mods/Fixture.jar");
  await review.getByRole("button", {name: "取消", exact: true}).click();
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((c: any) => c.command === "apply_plan").map((c: any) => c.args.planId)).toEqual(["txn-import-fixture"]);
  expect(calls.find((c: any) => c.command === "plan_install").args.request.expectedSha256).toBe("b".repeat(64));
});

async function openFomodInstall(page: Page) {
  await page.getByRole("button").filter({hasText: "Minecraft Java Edition"}).click();
  await page.getByRole("button", {name: "安装归档", exact: true}).click();
  await page.getByRole("button", {name: "导入本地归档", exact: true}).click();
  const importReview = page.getByRole("dialog", {name: "确认文件变更", exact: true});
  await importReview.getByRole("button", {name: "批准并执行", exact: true}).click();
  const install = page.getByRole("dialog", {name: "归档安装预览", exact: true});
  await install.getByLabel("Mod 名称", {exact: true}).fill("Fixture FOMOD");
  await install.getByLabel("适配包 Mod 类型", {exact: true}).selectOption("java-mod");
  return install;
}

test("FOMOD preview requires an explicit option before sending mappings to install", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  const install = await openFomodInstall(page);
  await install.getByRole("button", {name: "扫描 FOMOD 配置", exact: true}).click();
  await expect(install).toContainText("Fixture FOMOD");
  await expect(install.getByRole("status").filter({hasText: "需要用户选择"})).toBeVisible();
  await expect(install.getByRole("radio", {name: /HD/})).not.toBeChecked();
  await expect(install.getByRole("radio", {name: /SD/})).not.toBeChecked();
  await expect(install.getByRole("button", {name: "生成变更预览", exact: true })).toBeDisabled();
  await install.getByRole("radio", {name: /HD/}).check();
  await expect(install.getByRole("button", {name: "生成变更预览", exact: true })).toBeEnabled();
  await install.getByRole("button", {name: "生成变更预览", exact: true}).click();
  await expect(page.getByRole("dialog", {name: "确认文件变更", exact: true})).toContainText("/sandbox/minecraft/mods/Fixture.jar");
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  const request = calls.find((call: any) => call.command === "plan_install").args.request;
  expect(request.mappings).toEqual([
    {source: "Data/base.esp", destination: "base.esp"},
    {source: "Data/hd.dds", destination: "textures/fixture.dds"},
  ]);
  expect(calls.filter((call: any) => call.command === "apply_plan").map((call: any) => call.args.planId)).toEqual(["txn-import-fixture"]);
});

for (const theme of ["light", "dark"] as const) {
  test(`safe FOMOD local archive previews deterministic mappings for review in ${theme} theme`, async ({page}) => {
    await installMockTauri(page, {fomodSelection: "safe"});
    await page.emulateMedia({colorScheme: theme});
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", theme);

    const install = await openFomodInstall(page);
    await install.getByRole("button", {name: "扫描 FOMOD 配置", exact: true}).click();
    await expect(install.getByRole("status").filter({hasText: "已预选确定性选项"})).toBeVisible();
    await expect(install.getByRole("checkbox", {name: "Terrain textures", exact: true})).toBeChecked();
    await expect(install.getByRole("checkbox", {name: "Interface icons", exact: true})).toBeChecked();
    await expect(install.getByRole("radio", {name: "Core plugin", exact: true})).toBeChecked();

    const mappings = [
      {source: "Data/base.esp", destination: "base.esp"},
      {source: "Data/terrain.dds", destination: "textures/terrain.dds"},
      {source: "Data/icons.dds", destination: "textures/ui/icons.dds"},
      {source: "Data/core.esp", destination: "core.esp"},
    ];
    const table = install.getByRole("table", {name: "文件映射预览", exact: true});
    await expect(table.getByRole("columnheader")).toHaveText(["归档源文件", "安装目标"]);
    await expect(table.getByRole("cell")).toHaveCount(mappings.length * 2);
    for (const {source, destination} of mappings) {
      const row = table.getByRole("row").filter({has: page.getByRole("cell", {name: source, exact: true})});
      await expect(row.getByRole("cell")).toHaveText([source, destination]);
    }
    expect(await install.locator('[data-slot="modal-body"]').evaluate(body => {
      const bounds = body.getBoundingClientRect();
      return bounds.left >= 0 && bounds.right <= innerWidth + 1 && body.scrollWidth <= body.clientWidth + 1;
    })).toBe(true);

    const installCalls = () => page.evaluate(() => (window as any).__AMM_TEST__.calls.filter(
      (call: any) => ["plan_install", "apply_plan", "respond_approval"].includes(call.command),
    ));
    const importApproval = [{command: "apply_plan", args: {planId: "txn-import-fixture"}}];
    const review = page.getByRole("dialog", {name: "确认文件变更", exact: true});
    await expect(review).not.toBeVisible();
    expect(await installCalls()).toEqual(importApproval);

    const generate = install.getByRole("button", {name: "生成变更预览", exact: true});
    await expect(generate).toBeEnabled();
    await generate.click();
    await expect(review.getByRole("button", {name: "批准并执行", exact: true})).toBeVisible();
    expect(await installCalls()).toEqual([
      ...importApproval,
      {command: "plan_install", args: {request: expect.objectContaining({mappings})}},
    ]);
  });
}

test("installed mods distinguish exact download origins from legacy labels", async ({page}) => {
  await installMockTauri(page);
  await page.goto('/');
  await page.getByRole('button', {name: '游戏库', exact: true}).click();
  await page.evaluate(() => {
    const tauri = (window as any).__TAURI_INTERNALS__;
    const invoke = tauri.invoke;
    tauri.invoke = async (command: string, args: unknown) => {
      const result = await invoke(command, args);
      if (command !== 'list_installs') return result;
      const [record] = result;
      return [
        {...record, name: 'Catalog Fixture', downloadOrigin: {artifactId: 'artifact-fixture', sourceId: 'modrinth', projectId: 'Project1', versionId: 'PINNED01'}},
        {...record, id: 'legacy-install', name: 'Legacy Fixture', downloadOrigin: null},
        {...record, id: 'local-install', name: 'Local Fixture', source: 'local', version: null},
      ];
    };
  });
  await page.getByRole('button').filter({hasText: 'Minecraft Java Edition'}).click();
  await page.getByRole('tab', {name: '已装 Mod', exact: true}).click();
  const catalog = page.getByRole('row').filter({hasText: 'Catalog Fixture'});
  await expect(catalog).toContainText('1.0.0');
  await expect(catalog).toContainText('版本 ID：PINNED01');
  await expect(catalog).toContainText('项目 ID：Project1');
  const legacy = page.getByRole('row').filter({hasText: 'Legacy Fixture'});
  await expect(legacy).toContainText('未绑定下载记录');
  await expect(legacy).not.toContainText('版本 ID：');
  await expect(page.getByRole('row').filter({hasText: 'Local Fixture'})).toContainText('本地归档');
  expect(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1)).toBe(false);
});

test("all views fit both themes without viewport overflow", async ({page}, info) => {
  await installMockTauri(page);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  const views = ["Agent", "游戏库", "插件中心", "回收站", "历史对话", "设置"];
  await expect(page.getByRole("button", {name: "Mod 发现", exact: true})).toHaveCount(0);
  for (const theme of ["light", "dark"]) {
    const current = await page.locator("html").getAttribute("data-theme");
    if (current !== theme) await page.getByRole("button", {name: theme === "dark" ? "切换暗色主题" : "切换亮色主题", exact: true}).click();
    for (const [index, view] of views.entries()) {
      await page.getByRole("button", {name: view, exact: true}).click();
      await expect(page.locator("main")).toBeVisible();
      await expect(page.locator("main h1")).not.toBeEmpty();
      await expect(page.locator("main [role=alert]")).toHaveCount(0);
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1);
      expect(overflow, `${view}/${theme} overflows ${info.project.name}`).toBe(false);
      await page.screenshot({path: `output/playwright/screenshots/${info.project.name}-${theme}-${index}.png`, fullPage: true});
    }
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    await page.getByRole("button").filter({hasText: "Minecraft Java Edition"}).click();
    await expect(page.locator("main h1")).toContainText("Minecraft");
    expect(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1)).toBe(false);
    await page.screenshot({path: `output/playwright/screenshots/${info.project.name}-${theme}-detail.png`, fullPage: true});
  }
  expect(errors).toEqual([]);
});
