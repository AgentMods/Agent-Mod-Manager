import { expect, test, type Locator, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

async function expectAdapterResearchPrompt(composer: Locator) {
  await expect(composer).toHaveValue(/使用已有的 Mod 平台 MCP.*不同安装类型的代表性 Mod.*分析并核实各类型的安装规则/);
  await expect(composer).toHaveValue(/自行核实游戏识别标记，只在信息无法核实或需要我决定偏好时向我提问/);
}

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
  const approval = page.getByRole("group", {name: "操作审批", exact: true});
  await expect(page.getByRole("dialog", {name: "操作审批", exact: true})).toHaveCount(0);
  await expect(approval).toBeVisible();
  await expect(approval).toContainText("/sandbox/minecraft/mods/Example.jar");
  await expect(approval).toContainText("txn-fixture");
  await expect(page.locator('[data-message-id="assistant-fixture"]')).toHaveText("正在检查模组文件。");
  const approve = approval.getByRole("button", {name: "批准并执行", exact: true});
  await approve.click();
  await expect(approve).toHaveCount(0);
  await expect(page.locator('[data-message-id="assistant-fixture"]')).toContainText("检查完成，文件计划已执行。");
  await expect(page.getByText("120 tokens", {exact: true})).toBeVisible();
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: /^Agent( 对话)?$/}).click();
  await expect(page.locator('[data-message-id="assistant-fixture"]')).toHaveCount(1);
  await expect(page.locator('[data-message-id="assistant-fixture"]')).toHaveText(/正在检查模组文件。\s*检查完成，文件计划已执行。/);
  await expect(page.getByRole("button", {name: "apply_file_ops 已执行", exact: true})).toBeVisible();
  const responses = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "respond_approval"));
  expect(responses).toHaveLength(1);
  expect(responses[0].args.response).toEqual({approved: true, answer: null});
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

test("permanent trash purge requires the acknowledgement switch to be on", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "回收站", exact: true}).click();
  await page.getByRole("checkbox", {name: "选择 Example.jar", exact: true}).check();
  await page.getByRole("button", {name: "永久删除所选", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "永久删除回收站文件", exact: true});
  const confirm = dialog.getByRole("button", {name: "永久删除", exact: true});
  const acknowledgement = dialog.getByRole("switch", {name: "我确认永久删除以上文件，且无法再从回收站恢复。", exact: true});
  await expect(dialog.getByRole("textbox")).toHaveCount(0);
  await expect(acknowledgement).toHaveAttribute("aria-checked", "false");
  await expect(confirm).toBeDisabled();
  await acknowledgement.click();
  await expect(acknowledgement).toHaveAttribute("aria-checked", "true");
  await expect(confirm).toBeEnabled();
  await dialog.getByRole("button", {name: "取消", exact: true}).click();
  await expect(dialog).not.toBeVisible();
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === "purge_trash"))).toHaveLength(0);

  await page.getByRole("button", {name: "永久删除所选", exact: true}).click();
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("textbox")).toHaveCount(0);
  await expect(acknowledgement).toHaveAttribute("aria-checked", "false");
  await expect(confirm).toBeDisabled();
  await acknowledgement.click();
  await expect(confirm).toBeEnabled();
  await acknowledgement.press("Space");
  await expect(acknowledgement).toHaveAttribute("aria-checked", "false");
  await expect(confirm).toBeDisabled();
  await acknowledgement.press("Space");
  await expect(acknowledgement).toHaveAttribute("aria-checked", "true");
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
    await page.getByRole("tab", {name: "AI 与模型", exact: true}).click();
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
    await expect(page.getByLabel("fixture-model-80 实际请求模型")).toHaveValue("fixture-model-80");
    await expect(page.getByRole("button", {name: "保存设置", exact: true})).toBeEnabled();
  });
}

test("model credentials use the dedicated secret command, not public settings", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "设置", exact: true}).click();
  await page.getByRole("tab", {name: "AI 与模型", exact: true}).click();
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
  await page.getByRole("tab", {name: "工具", exact: true}).click();
  await expect(page.getByRole("button", {name: "开启工具", exact: true})).toBeEnabled();
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
  const approval = page.getByRole("group", {name: "操作审批", exact: true});
  await expect(page.getByRole("dialog", {name: "操作审批", exact: true})).toHaveCount(0);
  await expect(approval).toBeVisible();
  const reject = approval.getByRole("button", {name: "拒绝", exact: true});
  await reject.click();
  await expect(reject).toHaveCount(0);
  await expect(page.locator('[data-message-id="assistant-fixture"]')).toContainText("未执行文件操作。");
  await expect(page.getByRole("button", {name: "apply_file_ops 已拒绝", exact: true})).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((call: any) => call.command === "apply_plan")).toHaveLength(0);
  expect(calls.find((call: any) => call.command === "respond_approval")?.args.response).toEqual({approved: false, answer: null});
});

test("cancelled tool approval keeps its own status in conversation history", async ({page}) => {
  await installMockTauri(page, {holdAgentTurn: true});
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: /^Agent( 对话)?$/}).click();
  await page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true}).fill("检查文件");
  await page.getByRole("button", {name: "发送消息", exact: true}).click();
  await expect(page.getByRole("button", {name: "停止生成", exact: true})).toBeVisible();

  await page.evaluate(() => {
    const fixture = (window as any).__AMM_TEST__;
    const plan = {
      id: "txn-fixture",
      summary: "还原 Example.jar",
      total_bytes: 1024,
      created_at: "2026-11-18T00:00:00Z",
      requires_review: true,
      entries: [{
        operation: {op: "move", src: "/sandbox/data/trash/trash_fixture/payload", dst: "/sandbox/minecraft/mods/Example.jar"},
        before: null,
        source: {kind: "file", size: 1024, sha256: "a".repeat(64)},
        impact: "move",
      }],
    };
    fixture.emit("tool-start", {toolCallId: "tool-fixture", name: "apply_file_ops", arguments: {plan_id: plan.id}});
    fixture.emit("approval-request", {approvalId: "approval-fixture", toolCallId: "tool-fixture", title: "apply_file_ops", preview: plan});
  });

  const approval = page.getByRole("group", {name: "操作审批", exact: true});
  await expect(approval).toBeVisible();
  await expect(approval.getByRole("button", {name: "批准并执行", exact: true})).toBeVisible();
  await page.evaluate(() => {
    const fixture = (window as any).__AMM_TEST__;
    fixture.emit("approval-resolved", {approvalId: "approval-fixture", status: "cancelled"}, undefined, null);
    fixture.emit("done", {status: "cancelled"}, undefined, null);
  });

  await expect(page.getByRole("button", {name: "apply_file_ops 已取消", exact: true})).toBeVisible();
  await expect(page.getByRole("button", {name: "apply_file_ops 已拒绝", exact: true})).toHaveCount(0);
});

test("approval can retry an IPC failure and waits for the definitive tool outcome", async ({page}) => {
  await installMockTauri(page, {holdAgentTurn: true});
  await page.goto("/");
  await page.getByRole("textbox", {name: "告诉 Agent 你想做什么"}).fill("下载模组");
  await page.getByRole("button", {name: "发送消息", exact: true}).click();
  await expect(page.getByRole("button", {name: "停止生成", exact: true})).toBeVisible();
  await page.evaluate(() => {
    const {__AMM_TEST__: mock, __TAURI_INTERNALS__: ipc} = window as any;
    const invoke = ipc.invoke;
    let attempts = 0;
    ipc.invoke = (command: string, args: unknown) => {
      if (command !== "respond_approval") return invoke(command, args);
      if (++attempts === 1) return Promise.reject(new Error("暂时无法提交授权"));
      return Promise.resolve(); // The durable resolution and tool result can arrive later.
    };
    mock.emit("approval-request", {approvalId: "download-request", toolCallId: "download-call", title: "download_mod", preview: {filename: "Example.jar"}});
  });
  const card = page.getByRole("group", {name: "操作审批", exact: true});
  const approve = card.getByRole("button", {name: "批准并执行", exact: true});
  await expect(card).toContainText("Example.jar");
  await approve.click();
  await expect(card.getByRole("alert")).toHaveText("暂时无法提交授权");
  await approve.click();
  await expect(approve).toBeDisabled();
  await expect(card.getByRole("button", {name: "拒绝", exact: true})).toBeDisabled();
  await page.evaluate(() => (window as any).__AMM_TEST__.emit("approval-resolved", {approvalId: "download-request", status: "approved"}));
  await expect(card).toHaveCount(0);
  await expect(page.getByRole("button", {name: "download_mod 已批准，正在执行", exact: true})).toBeVisible();
  await page.evaluate(() => (window as any).__AMM_TEST__.emit("tool-result", {toolCallId: "download-call", status: "error", result: {error: "下载失败"}}));
  await expect(page.getByRole("button", {name: "download_mod 执行失败", exact: true})).toBeVisible();
});

test("plugin enable control uses the backend and survives navigation", async ({page}) => {
  await installMockTauri(page, {withExternalAdapter: true});
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
  await installMockTauri(page, {withExternalAdapter: true});
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  const adapterCenter = page.getByRole("button", {name: "插件中心", exact: true});
  const composer = page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true});
  await adapterCenter.click();
  await page.getByRole("button", {name: "用 Agent 创建", exact: true}).click();
  await expect(composer).toHaveValue(/创建.*适配包.*若目标游戏不明确，请先向我确认/);
  await expectAdapterResearchPrompt(composer);

  await adapterCenter.click();
  await page.getByRole("row").filter({hasText: "Minecraft Java Edition"}).getByRole("cell").first().getByRole("button").click();
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

test("empty package center stays empty until an external draft is installed", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "插件中心", exact: true}).click();

  await expect(page.getByRole("heading", {name: "插件中心", exact: true})).toBeVisible();
  await expect(page.getByText("暂无已安装适配包", {exact: true})).toBeVisible();
  await expect(page.getByRole("row")).toHaveCount(1);
  await expect(page.getByRole("heading", {name: "适配包详情", exact: true})).toBeVisible();

  await page.getByRole("button", {name: "新建草稿", exact: true}).click();
  const draft = page.getByRole("dialog", {name: "声明式适配包草稿", exact: true});
  await draft.getByLabel("Slug", {exact: true}).fill("external-fixture");
  await draft.getByLabel("adapter.json", {exact: true}).fill(JSON.stringify({
    schemaVersion: 1,
    slug: "external-fixture",
    name: "External Fixture",
    version: "1.0.0",
    platforms: ["windows"],
    markers: [{path: "game.exe", kind: "file", required: true, platforms: ["windows"]}],
    steamAppId: null,
    installRoots: [{id: "game", kind: "game", relativePath: ".", platforms: ["windows"], discoveryHints: [], notes: "Fixture root."}],
    modTypes: [{id: "game-file", name: "Game file", root: "game", extensions: ["zip"], notes: "Review fixture files."}],
    archiveRules: [{modType: "game-file", layout: "single_file", sourcePrefix: null, requiredFiles: [], notes: "Keep the archive intact."}],
    loadOrder: {kind: "none", paths: [], notes: "No load order."},
    savePaths: [],
    logPaths: [],
    uninstallNotes: ["Remove only files from this installation."],
    sources: [{title: "Fixture documentation", url: "https://example.com/fixture", checkedAt: "2026-11-21"}],
  }, null, 2));
  await draft.getByLabel("SKILL.md", {exact: true}).fill("---\nname: external-fixture\ndescription: External fixture\n---\n\nTest package.");
  await draft.getByRole("button", {name: "预览保存草稿", exact: true}).click();
  const writeReview = page.getByRole("dialog", {name: "确认文件变更", exact: true});
  await expect(writeReview).toContainText("保存外部适配包草稿 external-fixture");
  await writeReview.getByRole("button", {name: "批准并执行", exact: true}).click();

  await page.getByRole("tab", {name: /^草稿/}).click();
  const row = page.getByRole("row").filter({hasText: "external-fixture"});
  await expect(row).toBeVisible();
  await row.getByRole("button", {name: "验证", exact: true}).click();
  const validation = page.getByRole("dialog", {name: "验证：external-fixture", exact: true});
  await expect(validation).toContainText("验证通过");
  await validation.getByRole("button", {name: "安装预览", exact: true}).click();
  const installReview = page.getByRole("dialog", {name: "确认文件变更", exact: true});
  await expect(installReview).toContainText("安装外部适配包 external-fixture");
  await installReview.getByRole("button", {name: "批准并执行", exact: true}).click();

  await page.getByRole("tab", {name: "已安装", exact: true}).click();
  const installedRow = page.getByRole("row").filter({hasText: "External Fixture"});
  await expect(installedRow).toContainText("External Fixture");
  await expect(installedRow).toContainText("本地导入");
  await installedRow.getByRole("cell").first().getByRole("button").click();
  await expect(page.getByText("external-fixture", {exact: true})).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((call: any) => call.command === "apply_plan").map((call: any) => call.args.planId)).toEqual([
    "txn-write-external-fixture", "txn-install-external-fixture",
  ]);
});

test("library prefills adapter creation for an unbound game and displays its bound cover", async ({page}) => {
  await installMockTauri(page, {gameWithoutAdapter: true, withExternalAdapter: true});
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  const library = page.getByTestId("library-page");
  const card = library.locator('[data-slot="card"]').filter({has: page.getByRole("heading", {name: "Minecraft Java Edition", exact: true})});
  const cover = card.locator('img[alt=""]');
  await expect(cover).toHaveCount(0);
  await card.getByRole("button", {name: "创建适配包", exact: true}).click();
  const composer = page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true});
  await expect(composer).toHaveValue(/Minecraft Java Edition.*创建适配包.*校验通过后安装并绑定到这款游戏/);
  await expectAdapterResearchPrompt(composer);

  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await card.getByRole("button", {name: "更多操作", exact: true}).click();
  await page.getByRole("menuitem", {name: "选择适配包", exact: true}).click();
  const binding = page.getByRole("dialog", {name: "选择适配包", exact: true});
  await binding.getByRole("combobox", {name: "游戏适配包", exact: true}).selectOption("minecraft-java");
  await binding.getByRole("button", {name: "应用适配包", exact: true}).click();
  await expect(binding).toBeHidden();
  await expect(card.getByRole("button", {name: "创建适配包", exact: true})).toHaveCount(0);
  await expect(cover).toHaveAttribute("src", "https://covers.example.org/minecraft-java.jpg");
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
    withExternalAdapter: true,
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
  await installMockTauri(page, {withExternalAdapter: true});
  await page.route("https://covers.example.org/minecraft-java.jpg", (route) => route.abort());
  const failedImage = page.waitForEvent("requestfailed", (request) => request.resourceType() === "image" && request.url() === "https://covers.example.org/minecraft-java.jpg");
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
    "外观与存储", "AI 与模型", "工具", "Skills",
  ]);
  const panel = page.getByRole("tabpanel", {name: "Skills", exact: true});
  const system = panel.getByRole("region", {name: "系统 Skills", exact: true});
  await expect(system.getByRole("row")).toHaveCount(8);
  for (const name of ["创建与修改适配包", "联网搜索", "从网站下载 Mod", "定制 Mod 委托", "安装 Mod", "安装后整理 Mod", "卸载 Mod"]) {
    const button = system.getByRole("button", {name, exact: true});
    await expect(button).toBeVisible();
    const row = system.getByRole("row").filter({has: page.getByRole("button", {name, exact: true})});
    await expect(row.getByRole("cell", {name: "系统内置", exact: true})).toBeVisible();
    await expect(row.getByText("已校验", {exact: true})).toBeVisible();
    await expect(row.getByRole("checkbox", {name: `启用 ${name}`, exact: true})).toBeChecked();
  }
  const organizerToggle = system.getByRole("checkbox", {name: "启用 安装后整理 Mod", exact: true});
  await organizerToggle.uncheck();
  await expect(organizerToggle).not.toBeChecked();
  await organizerToggle.check();
  await expect(organizerToggle).toBeChecked();
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
  await expect(details.getByLabel("Brave Search API Key", {exact: true})).toHaveCount(0);
  const braveSecret = await page.evaluate(() => (window as any).__AMM_TEST__.calls.find((call: any) => call.command === "set_secret" && call.args.key === "sources.brave"));
  expect(braveSecret).toBeUndefined();
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
    {command: "set_skill_enabled", args: {name: "system-mod-organize", enabled: false}},
    {command: "set_skill_enabled", args: {name: "system-mod-organize", enabled: true}},
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
  await installMockTauri(page, {withExternalAdapter: true});
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "扫描游戏", exact: true}).click();
  const discovery = page.getByRole("dialog", {name: "扫描游戏", exact: true});
  await expect(discovery).toBeVisible();
  await expect(discovery.getByRole("status").filter({hasText: "已检查"})).toBeVisible();
  const directory = discovery.getByLabel("游戏目录", {exact: true});
  await directory.fill("/sandbox/original");
  await discovery.getByRole("button", {name: "选择游戏目录", exact: true}).click();
  await expect(directory).toHaveValue("/sandbox/new-game");
  const adapted = discovery.getByText("已适配", {exact: true});
  await expect(adapted).toHaveAttribute("data-variant", "success");
  await expect(adapted.locator("svg")).toBeVisible();
  await expect(discovery.getByText("Fixture Game", {exact: true})).toBeVisible();
  await expect(discovery.getByText("未匹配", {exact: true})).toBeVisible();
  await expect(discovery.getByText("60% 匹配", {exact: true})).toHaveCount(0);
  await discovery.getByRole("button", {name: "选择游戏目录", exact: true}).click();
  await expect(directory).toHaveValue("/sandbox/new-game");
  await expect(adapted).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((call: any) => ["register_game", "identify_game", "register_user_root"].includes(call.command))).toHaveLength(0);
  expect(calls.filter((call: any) => call.command === "plugin:dialog|open")).toHaveLength(2);
  expect(calls.filter((call: any) => call.command === "scan_games").map((call: any) => call.args.options)).toEqual([
    {userRoots: [], includeCommonRoots: true, page: { requestId: expect.any(String) }},
    {userRoots: ["/sandbox/new-game"], includeCommonRoots: false, page: { requestId: expect.any(String) }},
  ]);

  await directory.fill("/sandbox/empty  ");
  await directory.press("Enter");
  await expect(discovery.getByText(/未发现候选游戏/)).toBeVisible();
  await expect(adapted).toHaveCount(0);
  const lastScan = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "scan_games").at(-1));
  expect(lastScan.args.options).toEqual({userRoots: ["/sandbox/empty  "], includeCommonRoots: false, page: { requestId: expect.any(String) }});
  await page.keyboard.press("Escape");
  await expect(discovery).toBeHidden();
  await expect(page.getByTestId("library-page")).toBeVisible();
});

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
  await page.getByRole('tab', {name: 'Mod 管理', exact: true}).click();
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
