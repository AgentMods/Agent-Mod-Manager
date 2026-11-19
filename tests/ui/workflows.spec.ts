import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

test("browser preview never pretends to have native filesystem access", async ({page}) => {
  await page.goto("/");
  await expect(page.getByRole("status").filter({hasText: "未连接桌面服务"})).toBeVisible();
  await expect(page.getByRole("button", {name: "扫描游戏", exact: true})).toBeDisabled();
  await expect(page.getByTestId("library-page")).not.toContainText("Minecraft Java Edition");
  await page.getByRole("button", {name: "Agent 对话", exact: true}).click();
  await expect(page.getByRole("textbox", {name: "告诉 Agent 你想做什么"})).toBeDisabled();
});

test("streams pre-invoke deltas once, previews approval and persists completion", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await expect(page.getByText("Minecraft Java Edition", {exact: true})).toBeVisible();
  await page.getByRole("button", {name: "Agent 对话", exact: true}).click();
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
  await page.getByRole("button", {name: "Agent 对话", exact: true}).click();
  await expect(page.locator('[data-message-id="assistant-fixture"]')).toHaveCount(1);
  await expect(page.locator('[data-message-id="assistant-fixture"]')).toHaveText(/正在检查模组文件。\s*检查完成，文件计划已执行。/);
  const responses = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "respond_approval"));
  expect(responses).toHaveLength(1);
  expect(responses[0].args.response.approved).toBe(true);
});

test("trash restore needs a visible plan and separate apply confirmation", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
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

test("model credentials use the dedicated secret command, not public settings", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "设置", exact: true}).click();
  await page.getByRole("textbox", {name: "模型名称", exact: true}).fill("test-provider-model");
  await page.getByLabel("API 密钥", {exact: true}).fill("sk-fixture-only");
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
  await page.getByRole("button", {name: "Agent 对话", exact: true}).click();
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

test("standalone Skill removal requires review and commits only after approval", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
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
  await page.getByRole("button").filter({hasText: "Minecraft Java Edition"}).click();
  await page.getByRole("cell", {name: "options.txt", exact: true}).click();
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
  await expect(page.getByRole("cell", {name: "options.txt", exact: true})).toHaveCount(0);
  await expect(page.locator("main [role=alert]")).toHaveCount(0);
});

test("directory picker fills the path without granting root access", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  const directory = page.getByLabel("游戏目录", {exact: true});
  await directory.fill("/sandbox/original");
  await page.getByRole("button", {name: "选择游戏目录", exact: true}).click();
  await expect(directory).toHaveValue("/sandbox/new-game");
  await page.getByRole("button", {name: "选择游戏目录", exact: true}).click();
  await expect(directory).toHaveValue("/sandbox/new-game");
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((call: any) => ["register_game", "identify_game", "register_user_root"].includes(call.command))).toHaveLength(0);
  expect(calls.filter((call: any) => call.command === "plugin:dialog|open")).toHaveLength(2);
});

test("local archive import and game installation have separate approvals", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
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

type FomodEntry = "local archive" | "download";

async function openFomodInstall(page: Page, entry: FomodEntry) {
  if (entry === "download") {
    await page.evaluate(() => { (window as any).__AMM_TEST__.seedDownload = true; });
    await page.getByRole("button", {name: "Mod 发现", exact: true}).click();
    await page.getByRole("button", {name: "安装预览", exact: true}).click();
    const install = page.getByRole("dialog", {name: "下载归档安装预览", exact: true});
    await install.getByLabel("目标游戏", {exact: true}).selectOption("game-fixture");
    await install.getByLabel("适配包 Mod 类型", {exact: true}).selectOption("java-mod");
    return install;
  }
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
  const install = await openFomodInstall(page, "local archive");
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

for (const entry of ["local archive", "download"] as const) {
  for (const theme of ["light", "dark"] as const) {
    test(`safe FOMOD ${entry} previews deterministic mappings for review in ${theme} theme`, async ({page}) => {
      await installMockTauri(page, {fomodSelection: "safe"});
      await page.emulateMedia({colorScheme: theme});
      await page.goto("/");
      await expect(page.locator("html")).toHaveAttribute("data-theme", theme);

      const install = await openFomodInstall(page, entry);
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
      expect(await install.locator(".modal-body").evaluate(body => {
        const bounds = body.getBoundingClientRect();
        return bounds.left >= 0 && bounds.right <= innerWidth + 1 && body.scrollWidth <= body.clientWidth + 1;
      })).toBe(true);

      const installCalls = () => page.evaluate(() => (window as any).__AMM_TEST__.calls.filter(
        (call: any) => ["plan_install", "apply_plan", "respond_approval"].includes(call.command),
      ));
      const importApproval = entry === "local archive" ? [{command: "apply_plan", args: {planId: "txn-import-fixture"}}] : [];
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
}

test("download install uses declared adapter types and keeps the recorded hash", async ({page}) => {
  await installMockTauri(page);
  await page.goto('/');
  await page.evaluate(() => { (window as any).__AMM_TEST__.seedDownload = true; });
  await page.getByRole('button', {name: 'Mod 发现', exact: true}).click();
  await page.getByRole('button', {name: '安装预览', exact: true}).click();
  const modal = page.getByRole('dialog', {name: '下载归档安装预览', exact: true});
  const type = modal.getByLabel('适配包 Mod 类型', {exact: true});
  const generate = modal.getByRole('button', {name: '生成变更预览', exact: true});
  await expect(type).toBeDisabled();
  await modal.getByLabel('目标游戏', {exact: true}).selectOption('game-fixture');
  await expect(type).toBeEnabled();
  await type.selectOption('java-mod');
  await expect(modal).toContainText('请核对加载器和依赖');
  await expect(generate).toBeEnabled();
  await modal.getByLabel('目标游戏', {exact: true}).selectOption('');
  await expect(type).toHaveValue('');
  await expect(generate).toBeDisabled();
  await modal.getByLabel('目标游戏', {exact: true}).selectOption('game-fixture');
  await type.selectOption('java-mod');
  await generate.click();
  const plan = page.getByRole('dialog', {name: '确认文件变更', exact: true});
  await expect(plan).toContainText('/sandbox/data/downloads/fixture/Fixture.jar');
  const summary = plan.locator('.plan-preview > p');
  await expect(summary).toContainText(`项目 ID：fixture，版本 ID：${'V'.repeat(128)}`);
  expect(await summary.evaluate(p => p.scrollWidth <= p.clientWidth + 1)).toBe(true);
  await plan.getByRole('button', {name: '取消', exact: true}).click();
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.find((call: any) => call.command === 'plan_install').args.request).toMatchObject({gameId: 'game-fixture', modType: 'java-mod', expectedSha256: 'b'.repeat(64)});
  expect(calls.filter((call: any) => call.command === 'apply_plan')).toHaveLength(0);
});

test("download install exposes the same explicit FOMOD selection flow", async ({page}) => {
  await installMockTauri(page);
  await page.goto('/');
  const modal = await openFomodInstall(page, "download");
  await modal.getByRole('button', {name: '扫描 FOMOD 配置', exact: true}).click();
  await expect(modal).toContainText('Fixture FOMOD');
  await expect(modal.getByRole('status').filter({hasText: '需要用户选择'})).toBeVisible();
  await expect(modal.getByRole('radio', {name: /HD/})).not.toBeChecked();
  await expect(modal.getByRole('radio', {name: /SD/})).not.toBeChecked();
  await expect(modal.getByRole('button', {name: '生成变更预览', exact: true })).toBeDisabled();
  await modal.getByRole('radio', {name: /HD/}).check();
  await modal.getByRole('button', {name: '生成变更预览', exact: true}).click();
  await expect(page.getByRole('dialog', {name: '确认文件变更', exact: true})).toContainText('/sandbox/minecraft/mods/Fixture.jar');
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  const request = calls.find((call: any) => call.command === 'plan_install').args.request;
  expect(request.mappings).toEqual([
    {source: 'Data/base.esp', destination: 'base.esp'},
    {source: 'Data/hd.dds', destination: 'textures/fixture.dds'},
  ]);
  expect(calls.filter((call: any) => call.command === 'apply_plan')).toHaveLength(0);
});

test("installed mods distinguish exact download origins from legacy labels", async ({page}) => {
  await installMockTauri(page);
  await page.goto('/');
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
  for (const theme of ["light", "dark"]) {
    const current = await page.locator("html").getAttribute("data-theme");
    if (current !== theme) await page.getByRole("button", {name: theme === "dark" ? "切换暗色主题" : "切换亮色主题", exact: true}).click();
    for (const view of ["游戏库", "Mod 发现", "Agent 对话", "插件中心", "回收站", "历史对话", "设置"]) {
      await page.getByRole("button", {name: view, exact: true}).click();
      await expect(page.locator("main")).toBeVisible();
      await expect(page.locator("main h1")).not.toBeEmpty();
      await expect(page.locator("main [role=alert]")).toHaveCount(0);
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1);
      expect(overflow, `${view}/${theme} overflows ${info.project.name}`).toBe(false);
      await page.screenshot({path: `output/playwright/screenshots/${info.project.name}-${theme}-${["游戏库", "Mod 发现", "Agent 对话", "插件中心", "回收站", "历史对话", "设置"].indexOf(view)}.png`, fullPage: true});
    }
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    await page.getByRole("button").filter({hasText: "Minecraft Java Edition"}).click();
    await expect(page.locator("main h1")).toContainText("Minecraft");
    expect(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1)).toBe(false);
    await page.screenshot({path: `output/playwright/screenshots/${info.project.name}-${theme}-detail.png`, fullPage: true});
  }
  expect(errors).toEqual([]);
});
