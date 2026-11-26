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

async function mockModMetadataCommands(page: Page) {
  await page.addInitScript(() => {
    const target = window as any;
    const transport = target.__TAURI_INTERNALS__;
    const originalInvoke = transport.invoke;
    transport.invoke = async (command: string, args: Record<string, unknown> = {}, options?: unknown) => {
      if (["open_mod_storage_directory", "update_mod_metadata"].includes(command)) {
        target.__AMM_TEST__.calls.push({command, args});
        return null;
      }
      return originalInvoke(command, args, options);
    };
  });
}

function calls(page: Page, ...commands: string[]) {
  return page.evaluate(names => (window as any).__AMM_TEST__.calls.filter((call: any) => names.includes(call.command)), commands);
}

function modTable(page: Page) {
  return page.getByTestId("game-detail-page").getByRole("table", {name: "Mod 管理", exact: true});
}

function archiveRow(page: Page, filename: string) {
  return modTable(page).getByRole("row").filter({hasText: filename});
}

function installedRow(page: Page, name: string) {
  return modTable(page).getByRole("row").filter({hasText: name});
}

function exactNameRow(page: Page, name: string) {
  const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return modTable(page).getByRole("row", {name: new RegExp(`^${escaped} \\d+ 个文件`)});
}

async function expectModMenu(row: ReturnType<typeof installedRow>) {
  await row.getByRole("button", {name: "更多", exact: true}).click();
  for (const name of ["目录", "信息", "网站", "编辑", "更新"]) {
    await expect(row.page().getByRole("menuitem", {name, exact: true})).toBeVisible();
  }
}

function detailApproval(page: Page) {
  return page.getByTestId("game-detail-page").getByRole("group", {name: "操作审批"});
}

function approval(page: Page) {
  return page.getByRole("group", {name: "操作审批"});
}

function installSwitch(row: ReturnType<typeof archiveRow>, name = "使用 Agent 安装") {
  return row.getByRole("switch", {name, exact: true});
}

function expectNotice(page: Page, text: RegExp) {
  return expect(page.getByRole("status").filter({hasText: text}).last()).toBeVisible();
}

async function expectStart(page: Page, target: {artifactId?: string; installId?: string}) {
  await expect(page.getByTestId("game-detail-page")).toBeVisible();
  await expect(page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true})).toHaveCount(0);
  const starts = await calls(page, "start_turn");
  expect(starts).toHaveLength(1);
  expect(starts[0].args.request.permissionMode).toBeNull();
  expect(starts[0].args.request.content).toContain(JSON.stringify({gameId, ...target}));
  expect((await calls(page, "create_conversation")).map(call => call.args.gameId)).toEqual([gameId]);
  expect((await calls(page, "create_conversation")).map(call => call.args.category)).toEqual([
    target.artifactId ? "install" : "uninstall",
  ]);
  expect(await calls(page, ...unavailableWrites)).toEqual([]);
}

async function seedComposerPermission(page: Page, permissionMode: string | null) {
  await page.addInitScript((savedPermission) => {
    const target = window as any;
    const invoke = target.__TAURI_INTERNALS__.invoke;
    target.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      if (command === "get_composer_preferences") {
        return {model: null, reasoning_effort: null, permission_mode: savedPermission};
      }
      return invoke(command, args);
    };
  }, permissionMode);
}

async function deleteInstalledModFromDetail(page: Page) {
  await modTable(page).getByRole("row").filter({hasText: "Example"})
    .getByRole("button", {name: "从管理列表删除", exact: true}).click();
  await page.getByRole("dialog", {name: "从管理列表删除", exact: true})
    .getByRole("button", {name: "使用 Agent 删除", exact: true}).click();
  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(1);
  return (await calls(page, "start_turn"))[0];
}

test("unlinked archives appear as pending rows and keep the chosen exact archive available for approval", async ({page}) => {
  await installMockTauri(page, {modLibrary: [{...archive, artifactId: "artifact-release-1", filename: "Example-1.jar", version: "1.0"}, archive]});
  await page.emulateMedia({colorScheme: "dark"});
  await page.goto("/");
  await gameDetails(page);
  const currentArchiveRow = archiveRow(page, archive.filename);
  const olderArchiveRow = archiveRow(page, "Example-1.jar");
  await expect(currentArchiveRow).toHaveCount(1);
  await expect(currentArchiveRow).toContainText("待安装");
  await expect(olderArchiveRow).toHaveCount(1);
  await expect(olderArchiveRow).toContainText("待安装");
  await expect(page.getByRole("list", {name: "Mod 库归档", exact: true})).toHaveCount(0);
  expect((await calls(page, "list_mod_library")).map(call => call.args)).toEqual([{gameId}]);
  expect(await calls(page, "start_turn", "respond_approval", "apply_plan")).toEqual([]);

  const selected = currentArchiveRow;
  const toggle = installSwitch(selected);
  await toggle.click();
  await expect(detailApproval(page)).toContainText("install_library_mod");
  await expect(detailApproval(page).getByRole("table")).toContainText("/sandbox/minecraft/mods/");
  await expect(detailApproval(page)).toContainText(archive.filename);
  await expect(page.getByTestId("game-detail-page")).toBeVisible();
  await expectStart(page, {artifactId: archive.artifactId});
  const [start] = await calls(page, "start_turn");
  expect(start.args.request.content).toContain("明确选择");
  expect(start.args.request.content).toMatch(/match_mod_install.*本地证据不足.*来源 MCP.*Playwright.*作者说明都不足.*可选偏好.*才向我提问.*install_library_mod/);
  expect(start.args.request.content).not.toContain("artifact-release-1");
  expect(await calls(page, "respond_approval", "apply_plan")).toEqual([]);

  await detailApproval(page).getByRole("button", {name: "拒绝", exact: true}).click();
  await expect(toggle).not.toBeChecked();
  await expect(selected).toContainText("待安装");
  await expectNotice(page, /拒绝|未执行|未能|失败/);
  expect((await calls(page, "respond_approval")).map(call => call.args.response)).toEqual([{approved: false, answer: null}]);
  expect(await calls(page, "apply_plan", ...unavailableWrites)).toEqual([]);
});

test("installed and pending Mod rows expose the same menu and open the Mod editor", async ({page}, testInfo) => {
  await installMockTauri(page, {modLibrary: [archive]});
  await mockModMetadataCommands(page);
  await page.goto("/");
  await gameDetails(page);

  const installed = exactNameRow(page, "Example");
  await expect(installed).toContainText("1.0.0");
  await expectModMenu(installed);
  if (testInfo.project.name === "desktop-1440") {
    await page.screenshot({path: "output/playwright/mod-more-menu.png", fullPage: true});
  }
  await page.getByRole("menuitem", {name: "编辑", exact: true}).click();
  const installedEditor = page.getByRole("dialog", {name: "编辑 Mod", exact: true});
  const installedName = installedEditor.getByRole("textbox", {name: "显示名称", exact: true});
  await expect(installedName).toBeVisible();
  await expect(installedEditor.getByRole("button", {name: "保存", exact: true})).toBeVisible();
  await installedName.fill("Example 改名");
  await installedEditor.getByRole("button", {name: "保存", exact: true}).click();
  await expect.poll(async () => calls(page, "update_mod_metadata")).toHaveLength(1);
  expect((await calls(page, "update_mod_metadata"))[0].args).toEqual({
    gameId, target: {kind: "install", id: "install-fixture"}, patch: {name: "Example 改名", tagIds: []},
  });
  await page.keyboard.press("Escape");

  const pending = archiveRow(page, archive.filename);
  await expect(pending).toContainText("2.0");
  await expectModMenu(pending);
  await page.getByRole("menuitem", {name: "编辑", exact: true}).click();
  const pendingEditor = page.getByRole("dialog", {name: "编辑 Mod", exact: true});
  const pendingName = pendingEditor.getByRole("textbox", {name: "显示名称", exact: true});
  await expect(pendingName).toBeVisible();
  await expect(pendingEditor.getByRole("button", {name: "保存", exact: true})).toBeVisible();
  await pendingName.fill("未安装改名");
  await pendingEditor.getByRole("button", {name: "保存", exact: true}).click();
  await expect.poll(async () => calls(page, "update_mod_metadata")).toHaveLength(2);
  expect((await calls(page, "update_mod_metadata"))[1].args).toEqual({
    gameId, target: {kind: "archive", id: archive.artifactId}, patch: {name: "未安装改名", tagIds: []},
  });
});

test("the directory menu sends install and archive targets to the storage command", async ({page}) => {
  await installMockTauri(page, {modLibrary: [archive]});
  await mockModMetadataCommands(page);
  await page.goto("/");
  await gameDetails(page);

  await expectModMenu(exactNameRow(page, "Example"));
  await page.getByRole("menuitem", {name: "目录", exact: true}).click();
  await expect.poll(async () => calls(page, "open_mod_storage_directory")).toHaveLength(1);
  expect((await calls(page, "open_mod_storage_directory"))[0].args).toEqual({
    gameId, target: {kind: "install", id: "install-fixture"},
  });

  await expectModMenu(archiveRow(page, archive.filename));
  await page.getByRole("menuitem", {name: "目录", exact: true}).click();
  await expect.poll(async () => calls(page, "open_mod_storage_directory")).toHaveLength(2);
  expect((await calls(page, "open_mod_storage_directory"))[1].args).toEqual({
    gameId, target: {kind: "archive", id: archive.artifactId},
  });
});

test("Mod information shows all available metadata and disables the website action without a source page", async ({page}) => {
  const documented = {
    ...archive, artifactId: "artifact-documented", name: "Documented Mod", filename: "Documented.jar",
    version: "3.2.1", authorHint: "Author Example", sourceId: "modrinth", projectId: "DocumentedProject",
    projectPage: "https://modrinth.com/mod/DocumentedProject",
  };
  const local = {
    ...archive, artifactId: "artifact-local-no-source", name: "Local Mod", filename: "Local.jar",
    version: null, authorHint: null, sourceId: "local", projectId: null, projectPage: null,
  };
  await installMockTauri(page, {modLibrary: [documented, local]});
  await page.goto("/");
  await gameDetails(page);

  const documentedRow = archiveRow(page, documented.filename);
  await documentedRow.getByRole("button", {name: "更多", exact: true}).click();
  await page.getByRole("menuitem", {name: "信息", exact: true}).click();
  const info = page.getByRole("dialog", {name: "Mod 信息", exact: true});
  await expect(info).toBeVisible();
  await expect(info.locator("dt")).toHaveText(["名称", "版本", "作者", "来源", "网址"]);
  await expect(info.locator("dd")).toHaveText([
    "Documented Mod", "3.2.1", "Author Example", "modrinth", documented.projectPage,
  ]);
  await expect(info.getByRole("link", {name: documented.projectPage, exact: true})).toHaveAttribute("href", documented.projectPage);
  await page.keyboard.press("Escape");

  const localRow = archiveRow(page, local.filename);
  await localRow.getByRole("button", {name: "更多", exact: true}).click();
  await expect(page.getByRole("menuitem", {name: "网站", exact: true})).toBeDisabled();
});

test("the version column falls back to its linked archive and labels missing versions", async ({page}) => {
  const linkedArchive = {...archive, artifactId: "artifact-linked-version", version: "2.4.1"};
  const unknownArchive = {...archive, artifactId: "artifact-unknown-version", filename: "Unknown.jar", version: null};
  await installMockTauri(page, {modLibrary: [linkedArchive, unknownArchive]});
  await page.addInitScript(() => {
    const transport = (window as any).__TAURI_INTERNALS__;
    const originalInvoke = transport.invoke;
    transport.invoke = async (command: string, args: Record<string, unknown> = {}, options?: unknown) => {
      const result = await originalInvoke(command, args, options);
      if (command !== "list_installs" || args.gameId !== "game-fixture") return result;
      const records = result as any[];
      return [
        {...records[0], version: null, metadata: {libraryEntryId: "artifact-linked-version"}},
        {...records[0], id: "install-unknown-version", name: "Unknown Installed Mod", version: null, metadata: {}},
      ];
    };
  });
  await page.goto("/");
  await gameDetails(page);

  await expect(exactNameRow(page, "Example")).toContainText("2.4.1");
  await expect(installedRow(page, "Unknown Installed Mod")).toContainText("未知版本");
  await expect(archiveRow(page, unknownArchive.filename)).toContainText("未知版本");
});

test("each pending archive switch reports progress without blocking another row", async ({page}) => {
  const secondArchive = {...archive, artifactId: "artifact-release-3", filename: "Example-3.jar", version: "3.0"};
  await installMockTauri(page, {modLibrary: [archive, secondArchive], holdAgentTurn: true});
  await page.goto("/");
  await gameDetails(page);

  const firstRow = archiveRow(page, archive.filename);
  const secondRow = archiveRow(page, secondArchive.filename);
  const first = installSwitch(firstRow);
  const second = installSwitch(secondRow);
  await first.click();
  await expect(first).toHaveAttribute("aria-busy", "true");
  await expect(first).toBeDisabled();
  await expect(firstRow).toContainText("安装中");
  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(1);
  await expect(second).toBeEnabled();

  await second.click();
  await expect(second).toHaveAttribute("aria-busy", "true");
  await expect(second).toBeDisabled();
  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(2);
  await expect(first).toHaveAttribute("aria-busy", "true");
  await expect(page.getByTestId("game-detail-page")).toBeVisible();
});

test("an unarchived partial install keeps its explanation and cannot be toggled", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.evaluate(() => {
    const transport = (window as any).__TAURI_INTERNALS__;
    const invoke = transport.invoke;
    transport.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      const result = await invoke(command, args);
      if (command !== "list_installs" || args.gameId !== "game-fixture") return result;
      return [...result, {...result[0], id: "partially-restored-fixture", name: "Partial Fixture", status: "partially_restored"}];
    };
  });
  await gameDetails(page);

  const row = installedRow(page, "Partial Fixture");
  const toggle = row.getByRole("switch", {name: "使用 Agent 安装", exact: true});
  await expect(row).toContainText("部分文件已恢复");
  await expect(toggle).toBeDisabled();
  await expect(row.locator('[title="部分恢复：需先处理剩余回收站文件"]')).toHaveCount(1);
});

test("Agent questions for a library install appear beside the Mod table", async ({page}) => {
  await installMockTauri(page, {modLibrary: [archive]});
  await page.goto("/");
  await gameDetails(page);
  await page.evaluate(() => { (window as any).__AMM_TEST__.modActionControls.outcome = "question"; });

  const row = archiveRow(page, archive.filename);
  const toggle = installSwitch(row);
  await toggle.click();
  const question = page.getByTestId("game-detail-page").getByRole("region", {name: "安装前确认", exact: true});
  await expect(question).toBeVisible();
  await expect(question).toContainText("请确认此 Mod 的安装目录。");
  await expect(toggle).toHaveAttribute("aria-busy", "true");
  await expect(row).toContainText("安装中");
  await expect(page.getByTestId("game-detail-page")).toBeVisible();
  expect(await calls(page, "create_conversation")).toHaveLength(1);
  expect(await calls(page, "start_turn")).toHaveLength(1);
  await question.getByRole("textbox", {name: "填写回答"}).fill("保持默认目录");
  await question.getByRole("button", {name: "提交回答"}).click();
  await expectNotice(page, /未执行.*安装/);
  await expect(toggle).toBeEnabled();
  await expect(toggle).not.toBeChecked();
  expect((await calls(page, "respond_approval"))[0].args.response).toEqual({approved: true, answer: JSON.stringify(["保持默认目录"])});
});

test("cancelling an inline install question restores the actual unchecked state", async ({page}) => {
  await installMockTauri(page, {modLibrary: [archive]});
  await page.goto("/");
  await gameDetails(page);
  await page.evaluate(() => { (window as any).__AMM_TEST__.modActionControls.outcome = "question"; });
  const row = archiveRow(page, archive.filename);
  const toggle = installSwitch(row);
  await toggle.click();
  const question = page.getByTestId("game-detail-page").getByRole("region", {name: "安装前确认", exact: true});
  await expect(question).toBeVisible();

  await question.getByRole("button", {name: "停止生成", exact: true}).click();
  await expect(row).toContainText("待安装");
  await expect(toggle).not.toBeChecked();
  await expectNotice(page, /已取消安装/);
  expect(await calls(page, "cancel_turn")).toHaveLength(1);
  expect(await calls(page, "respond_approval")).toEqual([]);
  expect(await calls(page, "start_turn")).toHaveLength(1);
});

test("a failed install can be retried and only reports success after the install record changes", async ({page}) => {
  await installMockTauri(page, {modLibrary: [archive]});
  await page.goto("/");
  await gameDetails(page);
  const row = archiveRow(page, archive.filename);
  const toggle = installSwitch(row);
  await page.evaluate(() => { (window as any).__AMM_TEST__.modActionControls.outcome = "error"; });

  await toggle.click();
  await detailApproval(page).getByRole("button", {name: "批准并执行", exact: true}).click();
  await expect(row).toContainText("待安装");
  await expect(toggle).not.toBeChecked();
  await expectNotice(page, /失败|未能|未完成/);

  await page.evaluate(() => { (window as any).__AMM_TEST__.modActionControls.outcome = "success"; });
  await toggle.click();
  await detailApproval(page).getByRole("button", {name: "批准并执行", exact: true}).click();
  const installed = installedRow(page, archive.name);
  await expect(installed).toContainText("已安装");
  await expect(installed.getByRole("switch", {name: "使用 Agent 卸载", exact: true})).toBeChecked();
  await expectNotice(page, /已安装|安装成功/);
  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(2);
});

test("a done tool result without a changed install record is reported as unsuccessful", async ({page}) => {
  await installMockTauri(page, {modLibrary: [archive]});
  await page.goto("/");
  await gameDetails(page);
  const row = archiveRow(page, archive.filename);
  const toggle = installSwitch(row);
  await page.evaluate(() => { (window as any).__AMM_TEST__.modActionControls.outcome = "done-without-state"; });

  await toggle.click();
  await detailApproval(page).getByRole("button", {name: "批准并执行", exact: true}).click();
  await expect(row).toContainText("待安装");
  await expect(toggle).not.toBeChecked();
  await expectNotice(page, /未完成|未能|失败/);
  expect(await calls(page, "start_turn")).toHaveLength(1);
});

test("a failed status readback can be retried without replaying the Agent action", async ({page}) => {
  await installMockTauri(page, {modLibrary: [archive]});
  await page.goto("/");
  await gameDetails(page);
  const row = archiveRow(page, archive.filename);
  const toggle = installSwitch(row);
  await toggle.click();
  await expect(detailApproval(page)).toContainText("install_library_mod");
  await page.evaluate(() => { (window as any).__AMM_TEST__.modActionControls.failNextReadback = true; });

  await detailApproval(page).getByRole("button", {name: "批准并执行", exact: true}).click();
  await expect(row).toContainText("状态待核实");
  await expect(toggle).toBeDisabled();
  await expect(page.getByRole("button", {name: "重新读取状态", exact: true})).toBeVisible();
  await expectNotice(page, /无法读取.*最新安装状态/);
  const readsBeforeRetry = (await calls(page, "list_installs")).length;

  const retry = page.getByRole("button", {name: "重新读取状态", exact: true});
  await retry.focus();
  await retry.press("Enter");
  const installed = installedRow(page, archive.name);
  await expect(installed).toContainText("已安装");
  await expect(installed.getByRole("switch", {name: "使用 Agent 卸载", exact: true})).toBeChecked();
  await expect.poll(async () => (await calls(page, "list_installs")).length).toBeGreaterThan(readsBeforeRetry);
  await expectNotice(page, /已安装|安装成功/);
  expect(await calls(page, "start_turn")).toHaveLength(1);
  expect(await calls(page, "respond_approval")).toHaveLength(1);
  expect(await calls(page, "apply_plan", ...unavailableWrites)).toEqual([]);
});

test("the background approval shortcut for a Mod switch returns to Game Details, not chat", async ({page}) => {
  await installMockTauri(page, {modLibrary: [archive]});
  await page.goto("/");
  await gameDetails(page);
  await installSwitch(archiveRow(page, archive.filename)).click();
  await expect(detailApproval(page)).toBeVisible();
  await page.getByRole("button", {name: "返回游戏库", exact: true}).click();
  const shortcut = page.getByRole("button", {name: "查看待授权操作", exact: true});
  await shortcut.click();
  await expect(detailApproval(page)).toBeVisible();
  await shortcut.click();
  await expect(page.getByRole("region", {name: `处理 ${archive.name}`})).toBeFocused();
  await expect(installSwitch(archiveRow(page, archive.filename))).toBeDisabled();
  expect(await calls(page, "start_turn")).toHaveLength(1);
});

test("an install completed while away from game details is reflected after returning", async ({page}) => {
  await installMockTauri(page, {modLibrary: [archive]});
  await page.goto("/");
  await gameDetails(page);
  await installSwitch(archiveRow(page, archive.filename)).click();
  await expect(detailApproval(page)).toContainText("install_library_mod");
  const [turn] = await calls(page, "start_turn");

  await page.getByRole("button", {name: "返回游戏库", exact: true}).click();
  await expect(page.getByTestId("game-detail-page")).toHaveCount(0);
  await page.evaluate(async (conversationId) => {
    await (window as any).__TAURI_INTERNALS__.invoke("respond_approval", {
      conversationId, approvalId: "approval-fixture", response: {approved: true, answer: null},
    });
  }, turn.args.request.conversationId);

  await gameDetails(page);
  const installed = installedRow(page, archive.name);
  await expect(installed).toContainText("已安装");
  await expect(installed.getByRole("switch", {name: "使用 Agent 卸载", exact: true})).toBeChecked();
});

test("local picker approval only imports the archive; a separate click starts its installation", async ({page}) => {
  await installMockTauri(page);
  await page.emulateMedia({colorScheme: "light"});
  await page.goto("/");
  await gameDetails(page);
  const pick = page.getByRole("button", {name: "导入本地归档", exact: true});
  const review = page.getByRole("dialog", {name: "确认文件变更", exact: true});
  await pick.click();
  await expect(review.getByText("导入本地归档到管理器：Fixture.jar", {exact: true})).toBeVisible();
  await expect(review.getByText("新增：1", {exact: true})).toBeVisible();
  const operation = page.locator('td[data-operation="copy"]');
  const copyRow = review.getByRole("row").filter({has: operation});
  await expect(copyRow.locator('td[data-operation="copy"]')).toHaveText("复制");
  await expect(copyRow.getByText("/sandbox/data/staging/import-scratch/fixture/Fixture.jar", {exact: true})).toBeVisible();
  await expect(copyRow.getByText("/sandbox/data/staging/imports/fixture/Fixture.jar", {exact: true})).toBeVisible();
  await expect(review.getByText("事务 txn-import-fixture", {exact: true})).toBeVisible();
  await expect(review).not.toContainText("/sandbox/minecraft/mods");
  await review.getByRole("button", {name: "取消", exact: true}).click();
  expect(await calls(page, "apply_plan", "register_imported_mod", "start_turn")).toEqual([]);

  await pick.click();
  await review.getByRole("button", {name: "批准并执行", exact: true}).click();
  const importedRow = archiveRow(page, "Fixture.jar");
  await expect(importedRow).toHaveCount(1);
  await expect(importedRow).toContainText("待安装");
  expect(await calls(page, "apply_plan", "register_imported_mod", "start_turn")).toEqual([
    {command: "apply_plan", args: {planId: "txn-import-fixture"}},
    {command: "register_imported_mod", args: {gameId, transactionId: "txn-import-fixture"}},
  ]);
  const importedToggle = installSwitch(importedRow);
  await importedToggle.click();
  await expect(detailApproval(page)).toContainText("install_library_mod");
  await expectStart(page, {artifactId: "artifact-import-fixture"});
  expect(await calls(page, "bind_mod_library_entry")).toEqual([]);
  await detailApproval(page).getByRole("button", {name: "批准并执行", exact: true}).click();
  await expect(installedRow(page, "Fixture")).toContainText("已安装");
  await expect(installedRow(page, "Fixture").getByRole("switch", {name: "使用 Agent 卸载", exact: true})).toBeChecked();
  await expectNotice(page, /已安装|安装成功/);
  expect((await calls(page, "respond_approval")).map(call => call.args.response)).toEqual([{approved: true, answer: null}]);
  expect(await calls(page, "apply_plan")).toHaveLength(1);
});

test("uninstall passes the installed record ID and refreshes status after approval", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await gameDetails(page);
  const row = modTable(page).getByRole("row").filter({hasText: "Example"});
  const toggle = row.getByRole("switch", {name: "使用 Agent 卸载", exact: true});
  await toggle.click();
  await expect(detailApproval(page)).toContainText("uninstall_mod");
  await expect(detailApproval(page)).toContainText("install-fixture");
  await expect(toggle).toHaveAttribute("aria-busy", "true");
  await expect(row).toContainText("卸载中");
  await expect(toggle).toBeDisabled();
  await expectStart(page, {installId: "install-fixture"});
  const [start] = await calls(page, "start_turn");
  expect(start.args.request.content).toMatch(/system-mod-uninstall.*inspect_install.*safe_uninstall.*uninstall_mod.*事务 ID.*rollback_transaction/);
  const inspected = await page.evaluate(() => (window as any).__AMM_TEST__.events
    .some((event: any) => event.kind === "tool-result" && event.payload.name === "inspect_install" && event.payload.status === "done"));
  expect(inspected).toBe(true);
  await expect(page.getByTestId("game-detail-page")).toBeVisible();
  expect(await calls(page, "respond_approval", "apply_plan")).toEqual([]);
  await detailApproval(page).getByRole("button", {name: "批准并执行", exact: true}).click();
  await gameDetails(page);
  await expect(row).toContainText("已卸载");
  await expect(row.getByRole("switch", {name: "使用 Agent 安装", exact: true})).toBeDisabled();
  await expectNotice(page, /已卸载|卸载成功/);
  expect(await calls(page, "apply_plan", ...unavailableWrites)).toEqual([]);
});

test("uninstall reports all missing files without removing the managed record", async ({page}) => {
  await installMockTauri(page, {holdAgentTurn: true});
  await page.goto("/");
  await gameDetails(page);
  const row = installedRow(page, "Example");
  const toggle = row.getByRole("switch", {name: "使用 Agent 卸载", exact: true});
  await toggle.click();
  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(1);
  await expectStart(page, {installId: "install-fixture"});
  const [start] = await calls(page, "start_turn");
  const content = start.args.request.content;
  expect(content).toMatch(/forget_missing_files.*清单文件已不在.*保留管理记录.*不执行写操作/);
  expect(content).not.toContain("forget_uninstalled_mod");

  await page.evaluate(({gameId, conversationId}) => {
    const {emit} = (window as any).__AMM_TEST__;
    const installId = "install-fixture";
    const toolCallId = "tool-inspect-missing";
    const name = "inspect_install";
    emit("tool-start", {toolCallId, name, arguments: {game_id: gameId, install_id: installId}}, conversationId);
    emit("tool-result", {toolCallId, name, status: "done", result: {
      gameId, installId, reconciliation: {recommendedAction: "forget_missing_files"},
    }}, conversationId);
    emit("done", {status: "done"}, conversationId, null);
  }, {gameId, conversationId: start.args.request.conversationId});

  await expectNotice(page, /安装文件已不在，管理记录已保留/);
  await expect(toggle).toBeEnabled();
  await expect(toggle).toBeChecked();
  await expect(row).toContainText("已安装");
  await expect(detailApproval(page)).toHaveCount(0);
  expect(await calls(page, "remove_managed_mod", "respond_approval", "apply_plan", ...unavailableWrites)).toEqual([]);
});

test("rollback approval completes the uninstall switch only after the rolled-back record is read back", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await gameDetails(page);
  const row = installedRow(page, "Example");
  const toggle = row.getByRole("switch", {name: "使用 Agent 卸载", exact: true});
  await page.evaluate(() => { (window as any).__AMM_TEST__.modActionControls.outcome = "rollback"; });

  await toggle.click();
  await expect(detailApproval(page)).toContainText("rollback_transaction");
  await expectStart(page, {installId: "install-fixture"});
  const rollbackRequested = await page.evaluate(() => (window as any).__AMM_TEST__.events
    .some((event: any) => event.kind === "tool-start" && event.payload.name === "rollback_transaction"
      && event.payload.arguments.transaction_id === "txn-mod-rollback"));
  expect(rollbackRequested).toBe(true);
  await detailApproval(page).getByRole("button", {name: "批准并执行", exact: true}).click();

  await expect(row).toContainText("已卸载（已回滚）");
  await expect(row.getByRole("switch", {name: "使用 Agent 安装", exact: true})).toBeDisabled();
  await expectNotice(page, /已卸载/);
  expect(await calls(page, "rollback_transaction", "plan_uninstall", ...unavailableWrites)).toEqual([]);
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
  const install = installSwitch(archiveRow(page, archive.filename));
  await install.click();
  await expect(page.getByRole("alert")).toContainText("请先在「设置 → 模型服务」中配置 Agent 模型");
  await expect(install).not.toBeChecked();
  expect(await calls(page, "create_conversation", "start_turn", "apply_plan", ...unavailableWrites)).toEqual([]);
  await page.evaluate(() => { (window as any).__AMM_TEST__.agentModAction.missing = false; });
  await install.click();
  await expect(page.getByRole("alert")).toContainText("无法启动 Agent：服务未能启动测试会话");
  await expect(install).not.toBeChecked();
  expect(await calls(page, "respond_approval", "apply_plan", ...unavailableWrites)).toEqual([]);
  await expect(install).toBeEnabled();

  await page.evaluate(() => { (window as any).__AMM_TEST__.agentModAction.failStart = false; });
  await install.click();
  await expect(detailApproval(page)).toContainText("install_library_mod");
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
  await expect(modTable(page)).toBeVisible();
  await expect(archiveRow(page, "Fixture.jar")).toHaveCount(0);
  await page.getByRole("button", {name: "重试添加到 Mod 库", exact: true}).click();
  const importedRow = archiveRow(page, "Fixture.jar");
  await expect(importedRow).toHaveCount(1);
  await expect(importedRow).toContainText("待安装");
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
  const select = installSwitch(archiveRow(page, "Example-unbound.jar"));
  expect(await calls(page, "bind_mod_library_entry", "create_conversation", "start_turn")).toEqual([]);
  await select.click();
  await expect(page.getByRole("alert")).toContainText("测试绑定冲突：需明确选择游戏实例");
  await expect(page.getByRole("alert").filter({hasText: "测试绑定冲突：需明确选择游戏实例"})).toBeInViewport();
  expect(await calls(page, "create_conversation", "start_turn", ...unavailableWrites)).toEqual([]);
  await select.click();
  await expect(detailApproval(page)).toContainText("Example-unbound.jar");
  await expectStart(page, {artifactId: "artifact-unbound"});
  const timeline = await calls(page, "bind_mod_library_entry", "create_conversation", "start_turn");
  expect(timeline.map(call => call.command)).toEqual(["bind_mod_library_entry", "bind_mod_library_entry", "create_conversation", "start_turn"]);
  expect(timeline.slice(0, 2).map(call => call.args)).toEqual([
    {gameId, artifactId: "artifact-unbound"}, {gameId, artifactId: "artifact-unbound"},
  ]);
});

test("deleting a pending archive row removes only that row after confirmation", async ({page}) => {
  await installMockTauri(page, {modLibrary: [archive]});
  await page.goto("/");
  await gameDetails(page);
  const row = archiveRow(page, archive.filename);
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
  await expect(modTable(page).getByRole("row").filter({hasText: "Example"})).toContainText("已安装");
});

test("an archive linked to an install record appears once and can reinstall that record after uninstall", async ({page}) => {
  await installMockTauri(page, {modLibrary: [archive]});
  await page.goto("/");
  await page.evaluate(archiveId => {
    const target = window as any;
    const invoke = target.__TAURI_INTERNALS__.invoke;
    target.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      const result = await invoke(command, args);
      if (command !== "list_installs") return result;
      return result.map((install: any) => install.id === "install-fixture"
        ? {...install, name: "Example Mod", metadata: {...install.metadata, libraryEntryId: archiveId}}
        : install);
    };
  }, archive.artifactId);
  await gameDetails(page);

  const table = modTable(page);
  const row = table.getByRole("row").filter({hasText: archive.name});
  await expect(table.getByRole("row")).toHaveCount(2);
  await expect(table.getByText(archive.name, {exact: true})).toHaveCount(1);
  await expect(page.getByRole("list", {name: "Mod 库归档", exact: true})).toHaveCount(0);
  await expect(row).toContainText("已安装");
  const toggle = row.getByRole("switch", {name: "使用 Agent 卸载", exact: true});
  await expect(toggle).toBeChecked();
  await toggle.click();
  await expect(detailApproval(page)).toContainText("uninstall_mod");
  await detailApproval(page).getByRole("button", {name: "批准并执行", exact: true}).click();

  await gameDetails(page);
  await expect(row).toContainText("已卸载");
  await expect(table.getByRole("row")).toHaveCount(2);
  const reinstall = row.getByRole("switch", {name: "使用 Agent 重新安装", exact: true});
  await expect(reinstall).not.toBeChecked();
  await expect(reinstall).toBeEnabled();
  await reinstall.click();
  await expect(detailApproval(page)).toContainText("install_library_mod");
  const starts = await calls(page, "start_turn");
  expect(starts).toHaveLength(2);
  expect(starts[1].args.request.permissionMode).toBeNull();
  expect(starts[1].args.request.content).toContain(JSON.stringify({gameId, artifactId: archive.artifactId}));
  expect(await calls(page, ...unavailableWrites)).toEqual([]);
  await detailApproval(page).getByRole("button", {name: "批准并执行", exact: true}).click();
  const activeRow = row.filter({has: page.getByRole("switch", {name: "使用 Agent 卸载", exact: true})});
  await expect(activeRow).toContainText("已安装");
  await expect(activeRow.getByRole("switch")).toBeChecked();
  await expect(row.filter({hasText: "已卸载"}).getByRole("switch")).toBeDisabled();
  await expectNotice(page, /已安装|安装成功/);
});

test("deleting an installed mod scopes record-only removal to all missing files", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await gameDetails(page);
  await installedRow(page, "Example").getByRole("button", {name: "从管理列表删除", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "从管理列表删除", exact: true});
  await expect(dialog).toContainText("若清单文件全部缺失，且无覆盖、共享或替换关系，只移除管理记录");
  await expect(dialog).toContainText("其他异常需先审核");
  expect(await calls(page, "start_turn", "remove_managed_mod")).toEqual([]);
  await dialog.getByRole("button", {name: "使用 Agent 删除", exact: true}).click();
  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(1);
  const [turn] = await calls(page, "start_turn");
  expect((await calls(page, "create_conversation"))[0].args.category).toBe("remove");
  const content = turn.args.request.content;
  expect(content).toContain(JSON.stringify({gameId, installId: "install-fixture"}));
  expect(content).toMatch(/inspect_install.*reconciliation\.recommendedAction.*forget_missing_files.*直接调用 forget_uninstalled_mod\(\{game_id, install_id\}\).*无需卸载或回滚/);
  expect(content).toMatch(/safe_uninstall.*uninstall_mod.*rollback_transaction.*成功后才.*forget_uninstalled_mod.*其他异常.*审核/);
  expect(await calls(page, "remove_managed_mod")).toEqual([]);
  await expect(page.getByRole("button", {name: "删除 Mod", exact: true})).toHaveAttribute("aria-expanded", "true");
  await expect(page.locator('ul[aria-label="删除 Mod"] button[aria-current="true"]')).toHaveCount(1);
});

async function expectAutomaticUninstall(page: Page) {
  await expect(page.getByRole("button", {name: "uninstall_mod 已执行", exact: true})).toBeVisible();
  await expect(approval(page)).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.events.some((e: any) => e.kind === "approval-request"))).toBe(false);
  expect(await calls(page, "respond_approval")).toEqual([]);
}

for (const install of [true, false]) {
  test(`Full Trust completes the game-detail ${install ? "install" : "uninstall"} switch without opening a review`, async ({page}) => {
    await installMockTauri(page, {modLibrary: install ? [archive] : []});
    await seedComposerPermission(page, "full_trust");
    await page.goto("/");
    await gameDetails(page);
    const row = install ? archiveRow(page, archive.filename) : installedRow(page, "Example");
    await row.getByRole("switch", {name: install ? "使用 Agent 安装" : "使用 Agent 卸载", exact: true}).click();
    await expectNotice(page, install ? /已安装|安装成功/ : /已卸载|卸载成功/);
    await expect(page.getByTestId("game-detail-page")).toBeVisible();
    await expect(detailApproval(page)).toHaveCount(0);
    expect((await calls(page, "start_turn"))[0].args.request.permissionMode).toBe("full_trust");
    expect(await page.evaluate(() => (window as any).__AMM_TEST__.events.some((e: any) => e.kind === "approval-request"))).toBe(false);
    expect(await calls(page, "respond_approval")).toEqual([]);
  });
}

test("game-detail deletion inherits Full Trust from global settings with no composer override", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.evaluate(async () => {
    const {invoke} = (window as any).__TAURI_INTERNALS__;
    const settings = await invoke("get_settings");
    await invoke("set_settings", {settings: {...settings, permission_mode: "full_trust"}});
  });
  await gameDetails(page);
  const turn = await deleteInstalledModFromDetail(page);
  expect(turn.args.request.permissionMode).toBeNull();
  await expectAutomaticUninstall(page);
});

test("explicit game action can run if preferences cannot be read without overwriting unknown settings", async ({page}) => {
  await installMockTauri(page);
  await page.addInitScript(() => {
    const transport = (window as any).__TAURI_INTERNALS__;
    const invoke = transport.invoke;
    transport.invoke = (command: string, args: unknown) => {
      if (command === "get_composer_preferences") throw new Error("preferences unavailable");
      return invoke(command, args);
    };
  });
  await page.goto("/#/agent");
  await page.getByRole("button", {name: "本轮权限策略", exact: true}).click();
  await page.getByRole("menuitemradio", {name: "完全信任", exact: true}).click();
  await gameDetails(page);
  const turn = await deleteInstalledModFromDetail(page);
  expect(turn.args.request.permissionMode).toBe("full_trust");
  await expectAutomaticUninstall(page);
  expect(await calls(page, "set_composer_preferences")).toEqual([]);
});

test("game-detail deletion uses a Full Trust choice made in the composer without sending", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "Agent", exact: true}).click();
  await expect(page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true})).toBeVisible();

  await page.getByRole("button", {name: "本轮权限策略", exact: true}).click();
  await page.getByRole("menuitemradio", {name: "完全信任", exact: true}).click();
  expect(await calls(page, "start_turn")).toEqual([]);

  await gameDetails(page);
  const turn = await deleteInstalledModFromDetail(page);
  expect(turn.args.request.permissionMode).toBe("full_trust");
  await expectAutomaticUninstall(page);
  expect(turn.args.request.content).toContain('"installId":"install-fixture"');
  expect(turn.args.request.content).toContain("forget_uninstalled_mod");
  expect(await calls(page, "remove_managed_mod")).toEqual([]);
});

for (const {label, permissionMode} of [
  {label: "saved Full Trust", permissionMode: "full_trust"},
  {label: "saved game auto-approval", permissionMode: "auto_approve"},
  {label: "no saved permission", permissionMode: null},
]) {
  test(`game-detail deletion forwards ${label} from composer preferences`, async ({page}) => {
    await installMockTauri(page);
    await seedComposerPermission(page, permissionMode);
    await page.goto("/");
    await gameDetails(page);

    const turn = await deleteInstalledModFromDetail(page);
    expect(turn.args.request.permissionMode).toBe(permissionMode);
    if (permissionMode === "full_trust") await expectAutomaticUninstall(page);
    else await expect(approval(page)).toContainText("uninstall_mod");
    expect(turn.args.request.content).toContain('"installId":"install-fixture"');
    expect(await calls(page, "remove_managed_mod")).toEqual([]);
  });
}

test("choosing session settings in the composer overrides saved Full Trust for game-detail deletion", async ({page}) => {
  await installMockTauri(page);
  await seedComposerPermission(page, "full_trust");
  await page.goto("/");
  await page.getByRole("button", {name: "Agent", exact: true}).click();
  const permission = page.getByRole("button", {name: "本轮权限策略", exact: true});
  await expect(permission).toContainText("完全信任");

  await permission.click();
  await page.getByRole("menuitemradio", {name: "会话设置", exact: true}).click();
  expect(await calls(page, "start_turn")).toEqual([]);

  await gameDetails(page);
  const turn = await deleteInstalledModFromDetail(page);
  expect(turn.args.request.permissionMode).toBe(null);
  await expect(approval(page)).toContainText("uninstall_mod");
  expect(turn.args.request.content).toContain('"installId":"install-fixture"');
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
  const pending = modTable(page).getByRole("row").filter({hasText: "待审批"});
  await pending.getByRole("button", {name: "取消安装计划"}).click();
  await page.getByRole("dialog").getByRole("button", {name: "确认取消计划"}).click();
  await expect(pending).toHaveCount(0);
  await expect(modTable(page).getByRole("row").filter({hasText: "Example"})).toContainText("已安装");
  expect(await calls(page, "start_turn", "plan_uninstall")).toEqual([]);
});
