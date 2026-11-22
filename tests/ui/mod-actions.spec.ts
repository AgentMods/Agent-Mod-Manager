import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const gameId = "game-fixture";
const archive = {
  artifactId: "artifact-release-2", gameId, gameName: "Minecraft Java Edition", name: "Example Mod",
  filename: "Example-2.jar", version: "2.0", sha256: "a".repeat(64), projectPage: null,
};
const unavailableWrites = ["plan_install", "plan_uninstall", "preview_fomod"];

async function gameDetails(page: Page) {
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button").filter({has: page.getByRole("heading", {name: "Minecraft Java Edition", exact: true})}).click();
  await expect(page.getByTestId("game-detail-page")).toBeVisible();
}

function calls(page: Page, ...commands: string[]) {
  return page.evaluate(names => (window as any).__AMM_TEST__.calls.filter((call: any) => names.includes(call.command)), commands);
}

function library(page: Page) {
  return page.getByRole("list", {name: "Mod 库归档"});
}

function approval(page: Page) {
  return page.getByRole("group", {name: "操作审批"});
}

async function expectStart(page: Page, target: {artifactId?: string; installId?: string}) {
  const starts = await calls(page, "start_turn");
  expect(starts).toHaveLength(1);
  expect(starts[0].args.request.permissionMode).toBe("request_approval");
  expect(starts[0].args.request.content).toContain(JSON.stringify({gameId, ...target}));
  expect((await calls(page, "create_conversation")).map(call => call.args.gameId)).toEqual([gameId]);
  expect(await calls(page, ...unavailableWrites)).toEqual([]);
}

test("library install targets the chosen exact archive and waits for Agent approval", async ({page}) => {
  await installMockTauri(page, {modLibrary: [{...archive, artifactId: "artifact-release-1", filename: "Example-1.jar", version: "1.0"}, archive]});
  await page.emulateMedia({colorScheme: "dark"});
  await page.goto("/");
  await gameDetails(page);
  await expect(library(page).getByRole("listitem")).toHaveCount(2);
  expect((await calls(page, "list_mod_library")).map(call => call.args)).toEqual([{gameId}]);
  expect(await calls(page, "start_turn", "respond_approval", "apply_plan")).toEqual([]);

  const selected = library(page).getByRole("listitem").filter({hasText: archive.filename});
  await selected.getByRole("button", {name: "使用 Agent 安装", exact: true}).click();
  await expect(approval(page)).toContainText("install_library_mod");
  await expect(approval(page).getByRole("table")).toContainText("/sandbox/minecraft/mods/");
  await expect(approval(page)).toContainText(archive.filename);
  await expect(page.getByRole("button", {name: "match_mod_install 已执行", exact: true})).toBeVisible();
  await expectStart(page, {artifactId: archive.artifactId});
  const [start] = await calls(page, "start_turn");
  expect(start.args.request.content).toContain("明确选择");
  expect(start.args.request.content).toMatch(/match_mod_install.*本地证据不足.*来源 MCP.*Playwright.*作者说明都不足.*可选偏好.*才向我提问.*install_library_mod/);
  expect(start.args.request.content).not.toContain("artifact-release-1");
  expect(await calls(page, "respond_approval", "apply_plan")).toEqual([]);

  await approval(page).getByRole("button", {name: "拒绝", exact: true}).click();
  await expect(page.getByRole("button", {name: "install_library_mod 已拒绝", exact: true})).toBeVisible();
  expect((await calls(page, "respond_approval")).map(call => call.args.response)).toEqual([{approved: false, answer: null}]);
  expect(await calls(page, "apply_plan", ...unavailableWrites)).toEqual([]);
});

test("local picker approval only imports the archive; a separate click starts its installation", async ({page}) => {
  await installMockTauri(page);
  await page.emulateMedia({colorScheme: "light"});
  await page.goto("/");
  await gameDetails(page);
  const pick = page.getByRole("button", {name: "导入本地归档", exact: true});
  const review = page.getByRole("dialog", {name: "确认文件变更", exact: true});
  await pick.click();
  await expect(review).toContainText("导入本地归档到管理器");
  await expect(review).not.toContainText("/sandbox/minecraft/mods");
  await review.getByRole("button", {name: "取消", exact: true}).click();
  expect(await calls(page, "apply_plan", "register_imported_mod", "start_turn")).toEqual([]);

  await pick.click();
  await review.getByRole("button", {name: "批准并执行", exact: true}).click();
  await expect(library(page).getByRole("listitem")).toContainText("Fixture.jar");
  expect(await calls(page, "apply_plan", "register_imported_mod", "start_turn")).toEqual([
    {command: "apply_plan", args: {planId: "txn-import-fixture"}},
    {command: "register_imported_mod", args: {gameId, transactionId: "txn-import-fixture"}},
  ]);
  await library(page).getByRole("button", {name: "使用 Agent 安装", exact: true}).click();
  await expect(approval(page)).toContainText("install_library_mod");
  await expectStart(page, {artifactId: "artifact-import-fixture"});
  expect(await calls(page, "bind_mod_library_entry")).toEqual([]);
  await approval(page).getByRole("button", {name: "批准并执行", exact: true}).click();
  await expect(page.getByRole("button", {name: "install_library_mod 已执行", exact: true})).toBeVisible();
  expect((await calls(page, "respond_approval")).map(call => call.args.response)).toEqual([{approved: true, answer: null}]);
  expect(await calls(page, "apply_plan")).toHaveLength(1);
});

test("uninstall passes the installed record ID and refreshes status after approval", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await gameDetails(page);
  const row = page.getByRole("row").filter({hasText: "Example"});
  await row.getByRole("button", {name: "使用 Agent 卸载", exact: true}).click();
  await expect(approval(page)).toContainText("uninstall_mod");
  await expect(approval(page)).toContainText("install-fixture");
  await expectStart(page, {installId: "install-fixture"});
  const [start] = await calls(page, "start_turn");
  expect(start.args.request.content).toMatch(/system-mod-uninstall.*inspect_install.*safe_uninstall.*uninstall_mod.*事务 ID.*rollback_transaction/);
  await expect(page.getByRole("button", {name: "inspect_install 已执行", exact: true})).toBeVisible();
  expect(await calls(page, "respond_approval", "apply_plan")).toEqual([]);
  await approval(page).getByRole("button", {name: "批准并执行", exact: true}).click();
  await expect(page.getByRole("button", {name: "uninstall_mod 已执行", exact: true})).toBeVisible();
  await gameDetails(page);
  await expect(row).toContainText("已卸载");
  await expect(row.getByRole("button", {name: "使用 Agent 卸载", exact: true})).toBeDisabled();
  expect(await calls(page, "apply_plan", ...unavailableWrites)).toEqual([]);
});

test("missing Agent configuration and a failed start never fall back to direct planning", async ({page}) => {
  await installMockTauri(page, {modLibrary: [archive]});
  await page.goto("/");
  await gameDetails(page);
  await page.evaluate(() => {
    const target = window as any;
    const invoke = target.__TAURI_INTERNALS__.invoke;
    const controls = target.__AMM_TEST__.agentModAction = {missing: true, failStart: true};
    target.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      if (command === "get_settings") {
        const settings = await invoke(command, args);
        return {...settings, model: controls.missing ? "" : settings.model, api_key_ref: null};
      }
      if (command === "start_turn" && controls.failStart) {
        target.__AMM_TEST__.calls.push({command, args});
        throw new Error("服务未能启动测试会话");
      }
      return invoke(command, args);
    };
  });
  const install = library(page).getByRole("button", {name: "使用 Agent 安装", exact: true});
  await install.click();
  await expect(page.getByRole("alert")).toContainText("请先在「设置 → 模型服务」中配置 Agent 模型");
  expect(await calls(page, "create_conversation", "start_turn", "apply_plan", ...unavailableWrites)).toEqual([]);
  await page.evaluate(() => { (window as any).__AMM_TEST__.agentModAction.missing = false; });
  await install.click();
  await expect(page.getByRole("alert")).toContainText("无法启动 Agent：服务未能启动测试会话");
  expect(await calls(page, "respond_approval", "apply_plan", ...unavailableWrites)).toEqual([]);
  await expect(install).toBeEnabled();

  await page.evaluate(() => { (window as any).__AMM_TEST__.agentModAction.failStart = false; });
  await install.click();
  await expect(approval(page)).toContainText("install_library_mod");
  expect(await calls(page, "create_conversation")).toHaveLength(1);
  const starts = await calls(page, "start_turn");
  expect(starts).toHaveLength(2);
  expect(starts[1].args.request).toEqual(starts[0].args.request);
});

test("an indexing failure can retry the committed import without approving it again", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await gameDetails(page);
  await page.evaluate(() => {
    const target = window as any;
    const invoke = target.__TAURI_INTERNALS__.invoke;
    let failOnce = true;
    target.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      if (command === "register_imported_mod" && failOnce) {
        failOnce = false;
        target.__AMM_TEST__.calls.push({command, args});
        throw new Error("测试索引暂时不可用");
      }
      return invoke(command, args);
    };
  });
  await page.getByRole("button", {name: "导入本地归档", exact: true}).click();
  await page.getByRole("dialog", {name: "确认文件变更"}).getByRole("button", {name: "批准并执行", exact: true}).click();
  await expect(page.getByRole("alert")).toContainText("测试索引暂时不可用");
  await expect(library(page)).toBeHidden();
  await page.getByRole("button", {name: "重试添加到 Mod 库", exact: true}).click();
  await expect(library(page)).toContainText("Fixture.jar");
  expect(await calls(page, "apply_plan", "register_imported_mod", "start_turn")).toEqual([
    {command: "apply_plan", args: {planId: "txn-import-fixture"}},
    {command: "register_imported_mod", args: {gameId, transactionId: "txn-import-fixture"}},
    {command: "register_imported_mod", args: {gameId, transactionId: "txn-import-fixture"}},
  ]);
});

test("an unbound archive binds to the exact selected game before Agent start", async ({page}) => {
  await installMockTauri(page, {modLibrary: [archive, {...archive, artifactId: "artifact-unbound", gameId: null, filename: "Example-unbound.jar"}]});
  await page.goto("/");
  await gameDetails(page);
  await page.evaluate(() => {
    const target = window as any;
    const invoke = target.__TAURI_INTERNALS__.invoke;
    let failOnce = true;
    target.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      if (command === "bind_mod_library_entry" && failOnce) {
        failOnce = false;
        target.__AMM_TEST__.calls.push({command, args});
        throw new Error("测试绑定冲突：需明确选择游戏实例");
      }
      return invoke(command, args);
    };
  });
  const select = library(page).getByRole("listitem").filter({hasText: "Example-unbound.jar"})
    .getByRole("button", {name: "使用 Agent 安装", exact: true});
  expect(await calls(page, "bind_mod_library_entry", "create_conversation", "start_turn")).toEqual([]);
  await select.click();
  await expect(page.getByRole("alert")).toContainText("测试绑定冲突：需明确选择游戏实例");
  await expect(page.getByRole("alert").filter({hasText: "测试绑定冲突：需明确选择游戏实例"})).toBeInViewport();
  expect(await calls(page, "create_conversation", "start_turn", ...unavailableWrites)).toEqual([]);
  await select.click();
  await expect(approval(page)).toContainText("Example-unbound.jar");
  await expectStart(page, {artifactId: "artifact-unbound"});
  const timeline = await calls(page, "bind_mod_library_entry", "create_conversation", "start_turn");
  expect(timeline.map(call => call.command)).toEqual(["bind_mod_library_entry", "bind_mod_library_entry", "create_conversation", "start_turn"]);
  expect(timeline.slice(0, 2).map(call => call.args)).toEqual([
    {gameId, artifactId: "artifact-unbound"}, {gameId, artifactId: "artifact-unbound"},
  ]);
});

test("archive deletion removes only the selected library item after confirmation", async ({page}) => {
  await installMockTauri(page, {modLibrary: [archive]});
  await page.goto("/");
  await gameDetails(page);
  const row = library(page).getByRole("listitem").filter({hasText: archive.filename});
  const remove = row.getByRole("button", {name: "从管理列表删除"});
  await remove.click();
  const dialog = page.getByRole("dialog", {name: "从管理列表删除"});
  await dialog.getByRole("button", {name: "返回"}).click();
  await expect(row).toBeVisible();
  expect(await calls(page, "remove_managed_mod")).toEqual([]);
  await remove.click();
  await dialog.getByRole("button", {name: "确认删除"}).click();
  await expect(row).toHaveCount(0);
  expect((await calls(page, "remove_managed_mod"))[0].args).toEqual({gameId, target: {kind: "archive", id: archive.artifactId}});
  await expect(page.getByRole("row").filter({hasText: "Example"})).toContainText("已安装");
});

test("deleting an installed mod starts a scoped Agent uninstall before list removal", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await gameDetails(page);
  await page.getByRole("row").filter({hasText: "Example"}).getByRole("button", {name: "从管理列表删除"}).click();
  await page.getByRole("dialog").getByRole("button", {name: "使用 Agent 卸载并删除"}).click();
  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(1);
  const [turn] = await calls(page, "start_turn");
  expect(turn.args.request.content).toContain('"installId":"install-fixture"');
  expect(turn.args.request.content).toContain("forget_uninstalled_mod");
  expect(await calls(page, "remove_managed_mod")).toEqual([]);
});

test("cancelling one pending record leaves the installed namesake visible", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.evaluate(() => {
    const transport = (window as any).__TAURI_INTERNALS__;
    const invoke = transport.invoke;
    let pending = true;
    transport.invoke = async (command: string, args: any) => {
      if (command === "list_installs") {
        const installed = await invoke(command, args);
        return pending ? [...installed, {...installed[0], id: "pending-copy", status: "planned"}] : installed;
      }
      if (command === "remove_managed_mod" && args.gameId === "game-fixture" && args.target.kind === "plan" && args.target.id === "pending-copy") {
        pending = false;
        return;
      }
      return invoke(command, args);
    };
  });
  await gameDetails(page);
  const pending = page.getByRole("row").filter({hasText: "待审批"});
  await pending.getByRole("button", {name: "取消安装计划"}).click();
  await page.getByRole("dialog").getByRole("button", {name: "确认取消计划"}).click();
  await expect(pending).toHaveCount(0);
  await expect(page.getByRole("row").filter({hasText: "Example"})).toContainText("已安装");
  expect(await calls(page, "start_turn", "plan_uninstall")).toEqual([]);
});
