import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const gameId = "game-fixture";
const archives = [
  {
    artifactId: "artifact-batch-one", gameId, gameName: "Minecraft Java Edition", name: "Batch Archive One",
    filename: "Batch-One.jar", version: "1.0", sha256: "b".repeat(64), projectPage: null,
  },
  {
    artifactId: "artifact-batch-two", gameId, gameName: "Minecraft Java Edition", name: "Batch Archive Two",
    filename: "Batch-Two.jar", version: "2.0", sha256: "c".repeat(64), projectPage: null,
  },
];

async function gameDetails(page: Page) {
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button").filter({has: page.getByRole("heading", {name: "Minecraft Java Edition", exact: true})}).click();
  await expect(page.getByTestId("game-detail-page")).toBeVisible();
}

function calls(page: Page, ...commands: string[]) {
  return page.evaluate(names => (window as any).__AMM_TEST__.calls.filter((call: any) => names.includes(call.command)), commands);
}

function createdConversationIds(page: Page) {
  return page.evaluate(() => (window as any).__AMM_TEST__.conversations
    .filter((conversation: any) => conversation.id !== "conversation-fixture")
    .map((conversation: any) => conversation.id));
}

function modTable(page: Page) {
  return page.getByTestId("game-detail-page").getByRole("table", {name: "Mod 管理", exact: true});
}

function selection(page: Page, name: string) {
  return modTable(page).getByRole("checkbox", {name: `选择 ${name}`, exact: true});
}

async function openBatchMode(page: Page, holdAgentTurn = false) {
  await installMockTauri(page, {modLibrary: archives, holdAgentTurn});
  await page.goto("/");
  await gameDetails(page);
  const toggle = page.getByRole("switch", {name: "批量操作", exact: true});
  await expect(toggle).not.toBeChecked();
  await toggle.click();
  await expect(modTable(page).getByRole("checkbox", {name: "全选 Mod", exact: true})).toBeVisible();
  return toggle;
}

async function addUninstalledRecord(page: Page) {
  await page.addInitScript(() => {
    const target = window as any;
    const invoke = target.__TAURI_INTERNALS__.invoke;
    let removed = false;
    target.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, any> = {}) => {
      if (command === "remove_managed_mod" && args.target?.kind === "record" && args.target.id === "install-uninstalled-fixture") {
        target.__AMM_TEST__.calls.push({command, args});
        removed = true;
        return null;
      }
      const result = await invoke(command, args);
      if (command !== "list_installs" || removed) return result;
      return [...result, {
        id: "install-uninstalled-fixture", gameId: "game-fixture", name: "Previously Uninstalled",
        version: "0.9", source: "local", status: "uninstalled", installRoot: "/sandbox/minecraft/mods",
        files: [], metadata: {}, installedAt: "2026-11-18T00:00:00Z",
      }];
    };
  });
}

async function addEnabledRecord(page: Page) {
  await page.addInitScript(() => {
    const target = window as any;
    const invoke = target.__TAURI_INTERNALS__.invoke;
    target.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, any> = {}) => {
      if (command === "start_turn" && args.request?.content?.includes('"installId":"install-second-fixture"')) {
        target.__AMM_TEST__.calls.push({command, args});
        return "turn-second-fixture";
      }
      const result = await invoke(command, args);
      if (command !== "list_installs") return result;
      return [...result, {
        id: "install-second-fixture", gameId: "game-fixture", name: "Second Example",
        version: "1.1.0", source: "local", status: "enabled", installRoot: "/sandbox/minecraft/mods",
        files: [], metadata: {}, installedAt: "2026-11-18T00:00:00Z",
      }];
    };
  });
}

async function selectMixedRows(page: Page) {
  // The fixture's Example row is installed; both archive rows are pending.
  for (const name of ["Example", "Batch Archive One", "Batch Archive Two"]) {
    await selection(page, name).click();
  }
}

test("batch mode starts unchecked and clears row selections when toggled off", async ({page}) => {
  await installMockTauri(page, {modLibrary: archives});
  await page.goto("/");
  await gameDetails(page);

  const toggle = page.getByRole("switch", {name: "批量操作", exact: true});
  await expect(toggle).not.toBeChecked();
  await toggle.click();
  await expect(modTable(page).getByRole("checkbox", {name: "全选 Mod", exact: true})).toBeVisible();
  for (const name of ["Example", "Batch Archive One", "Batch Archive Two"]) {
    await expect(selection(page, name)).not.toBeChecked();
  }

  await selection(page, "Batch Archive One").click();
  await expect(selection(page, "Batch Archive One")).toBeChecked();
  await toggle.click();
  await expect(modTable(page).getByRole("checkbox", {name: "全选 Mod", exact: true})).toHaveCount(0);
  await toggle.click();
  for (const name of ["Example", "Batch Archive One", "Batch Archive Two"]) {
    await expect(selection(page, name)).not.toBeChecked();
  }
  expect(await calls(page, "start_turn", "remove_managed_mod")).toEqual([]);
});

test("select all checks every Mod row", async ({page}) => {
  await openBatchMode(page);
  const selectAll = modTable(page).getByRole("checkbox", {name: "全选 Mod", exact: true});

  await selectAll.click();
  for (const name of ["Example", "Batch Archive One", "Batch Archive Two"]) {
    await expect(selection(page, name)).toBeChecked();
  }
  await expect(selectAll).toBeChecked();
});

test("batch install targets pending rows and skips an installed row in a mixed selection", async ({page}) => {
  await openBatchMode(page, true);
  await selectMixedRows(page);

  await page.getByRole("button", {name: "批量安装", exact: true}).click();
  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(2);
  const requests = (await calls(page, "start_turn")).map(call => call.args.request.content as string).join("\n");
  expect(requests).toContain(archives[0].artifactId);
  expect(requests).toContain(archives[1].artifactId);
  expect(requests).not.toContain('"installId":"install-fixture"');
  expect((await calls(page, "start_turn")).every(call => call.args.request.permissionMode === null)).toBe(true);
  const conversations = await calls(page, "create_conversation");
  expect(conversations.map(call => call.args.category)).toEqual(["install", "install"]);
  expect(new Set(await createdConversationIds(page)).size).toBe(2);
  expect(await calls(page, "plan_install", "plan_uninstall", "preview_fomod")).toEqual([]);
});

test("batch uninstall targets the installed row and skips pending rows in a mixed selection", async ({page}) => {
  await installMockTauri(page, {modLibrary: archives, holdAgentTurn: true});
  await addEnabledRecord(page);
  await page.goto("/");
  await gameDetails(page);
  await page.getByRole("switch", {name: "批量操作", exact: true}).click();
  await selectMixedRows(page);
  await selection(page, "Second Example").click();

  await page.getByRole("button", {name: "批量卸载", exact: true}).click();
  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(2);
  const requests = (await calls(page, "start_turn")).map(call => call.args.request.content as string).join("\n");
  expect(requests).toContain('"installId":"install-fixture"');
  expect(requests).toContain('"installId":"install-second-fixture"');
  expect(requests).not.toContain(archives[0].artifactId);
  expect(requests).not.toContain(archives[1].artifactId);
  expect((await calls(page, "start_turn")).every(call => call.args.request.permissionMode === null)).toBe(true);
  const conversations = await calls(page, "create_conversation");
  expect(conversations.map(call => call.args.category)).toEqual(["uninstall", "uninstall"]);
  expect(new Set(await createdConversationIds(page)).size).toBe(2);
  expect(await calls(page, "plan_install", "plan_uninstall", "preview_fomod")).toEqual([]);
});

test("batch install skips a row that already has an Agent action in progress", async ({page}) => {
  await installMockTauri(page, {modLibrary: archives, holdAgentTurn: true});
  await page.goto("/");
  await gameDetails(page);
  const busyRow = modTable(page).getByRole("row").filter({hasText: "Batch-Two.jar"});
  await busyRow.getByRole("switch", {name: "使用 Agent 安装", exact: true}).click();
  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(1);

  await page.getByRole("switch", {name: "批量操作", exact: true}).click();
  const busySelection = selection(page, "Batch Archive Two");
  if (await busySelection.isDisabled()) {
    await expect(busySelection).not.toBeChecked();
  } else {
    await busySelection.click();
    await expect(busySelection).toBeChecked();
  }
  await selection(page, "Batch Archive One").click();
  await selection(page, "Example").click();

  await page.getByRole("button", {name: "批量安装", exact: true}).click();
  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(2);
  const requests = (await calls(page, "start_turn")).map(call => call.args.request.content as string).join("\n");
  expect(requests).toContain(archives[0].artifactId);
  expect(requests.match(new RegExp(archives[1].artifactId, "g")) ?? []).toHaveLength(1);
  expect(requests).not.toContain('"installId":"install-fixture"');
});

test("batch delete can be cancelled and removes selected archives after confirmation", async ({page}) => {
  await installMockTauri(page, {modLibrary: archives});
  await addUninstalledRecord(page);
  await page.goto("/");
  await gameDetails(page);
  await page.getByRole("switch", {name: "批量操作", exact: true}).click();
  await selection(page, "Batch Archive One").click();
  await selection(page, "Batch Archive Two").click();
  await selection(page, "Previously Uninstalled").click();

  await page.getByRole("button", {name: "批量删除", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "批量删除 Mod", exact: true});
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText("Batch Archive One");
  await expect(dialog).toContainText("Batch Archive Two");
  await expect(dialog).toContainText("Previously Uninstalled");
  await dialog.getByRole("button", {name: "取消", exact: true}).click();
  await expect(selection(page, "Batch Archive One")).toBeChecked();
  await expect(modTable(page).getByRole("row").filter({hasText: "Batch-One.jar"})).toBeVisible();
  expect(await calls(page, "remove_managed_mod")).toEqual([]);

  await page.getByRole("button", {name: "批量删除", exact: true}).click();
  const confirmation = page.getByRole("dialog", {name: "批量删除 Mod", exact: true});
  await confirmation.getByRole("button", {name: "确认删除", exact: true}).click();
  await expect(modTable(page).getByRole("row").filter({hasText: "Batch-One.jar"})).toHaveCount(0);
  await expect(modTable(page).getByRole("row").filter({hasText: "Batch-Two.jar"})).toHaveCount(0);
  await expect(modTable(page).getByRole("row").filter({hasText: "Previously Uninstalled"})).toHaveCount(0);
  await expect(modTable(page).getByRole("row").filter({hasText: "Example"}).getByRole("checkbox", {name: "选择 Example"})).not.toBeChecked();
  await expect(page.getByRole("group", {name: "批量操作工具栏"}).getByRole("status")).toHaveText("已选 0 项");
  const removed = (await calls(page, "remove_managed_mod")).map(call => call.args.target).sort((a, b) => a.id.localeCompare(b.id));
  expect(removed).toEqual([
    {kind: "archive", id: archives[0].artifactId},
    {kind: "archive", id: archives[1].artifactId},
    {kind: "record", id: "install-uninstalled-fixture"},
  ].sort((a, b) => a.id.localeCompare(b.id)));
});

test("batch delete keeps successful removals and reports a failed item", async ({page}) => {
  const failedArtifactId = archives[1].artifactId;
  await installMockTauri(page, {modLibrary: archives});
  await page.addInitScript((id) => {
    const target = window as any;
    const invoke = target.__TAURI_INTERNALS__.invoke;
    target.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, any> = {}) => {
      if (command === "remove_managed_mod" && args.target?.kind === "archive" && args.target.id === id) {
        target.__AMM_TEST__.calls.push({command, args});
        throw new Error("批量删除测试故障");
      }
      return invoke(command, args);
    };
  }, failedArtifactId);
  await page.goto("/");
  await gameDetails(page);
  await page.getByRole("switch", {name: "批量操作", exact: true}).click();
  await selection(page, "Batch Archive One").click();
  await selection(page, "Batch Archive Two").click();
  await page.getByRole("button", {name: "批量删除", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "批量删除 Mod", exact: true});
  await dialog.getByRole("button", {name: "确认删除", exact: true}).click();

  await expect(modTable(page).getByRole("row").filter({hasText: "Batch-One.jar"})).toHaveCount(0);
  await expect(modTable(page).getByRole("row").filter({hasText: "Batch-Two.jar"})).toBeVisible();
  await expect(selection(page, "Batch Archive Two")).toBeChecked();
  await expect(page.getByRole("group", {name: "批量操作工具栏"}).getByRole("status")).toHaveText("已选 1 项");
  await expect(page.getByText("批量删除测试故障")).toBeVisible();
  const removed = await calls(page, "remove_managed_mod");
  expect(removed.map(call => call.args.target.id).sort()).toEqual(archives.map(item => item.artifactId).sort());
});

test("batch deletion runs deployed removal in its own background Agent session", async ({page}) => {
  await installMockTauri(page, {modLibrary: archives, holdAgentTurn: true});
  await addEnabledRecord(page);
  await page.goto("/");
  await gameDetails(page);
  await page.getByRole("switch", {name: "批量操作", exact: true}).click();
  await selection(page, "Example").click();
  await selection(page, "Second Example").click();
  await selection(page, "Batch Archive One").click();

  await page.getByRole("button", {name: "批量删除", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "批量删除 Mod", exact: true});
  await dialog.getByRole("button", {name: "确认删除", exact: true}).click();

  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(2);
  await expect.poll(async () => (await calls(page, "remove_managed_mod")).length).toBe(1);
  const starts = await calls(page, "start_turn");
  const requestContent = starts.map(start => start.args.request.content as string).join("\n");
  expect(requestContent).toContain('"installId":"install-fixture"');
  expect(requestContent).toContain('"installId":"install-second-fixture"');
  expect(starts.every(start => start.args.request.permissionMode === null)).toBe(true);
  const conversations = await calls(page, "create_conversation");
  expect(conversations.map(call => call.args.category)).toEqual(["remove", "remove"]);
  expect(new Set(await createdConversationIds(page)).size).toBe(2);
  expect((await calls(page, "remove_managed_mod"))[0].args).toEqual({
    gameId, target: {kind: "archive", id: archives[0].artifactId},
  });
  await expect(page.getByTestId("game-detail-page")).toBeVisible();
  await expect(selection(page, "Example")).not.toBeChecked();
  await expect(selection(page, "Second Example")).not.toBeChecked();
  await expect(modTable(page).getByRole("row").filter({hasText: "Batch-One.jar"})).toHaveCount(0);
});

test("successful batch uninstall clears that row's selection after Agent completion", async ({page}) => {
  await openBatchMode(page);
  await selection(page, "Example").click();

  await page.getByRole("button", {name: "批量卸载", exact: true}).click();
  const approval = page.getByTestId("game-detail-page").getByRole("group", {name: "操作审批"});
  await expect(approval).toBeVisible();
  expect((await calls(page, "create_conversation")).map(call => call.args.category)).toEqual(["uninstall"]);
  await approval.getByRole("button", {name: "批准并执行", exact: true}).click();

  await expect(modTable(page).getByRole("row").filter({hasText: "Example"})).toContainText("已卸载");
  await expect(selection(page, "Example")).not.toBeChecked();
  await expect(page.getByTestId("game-detail-page")).toBeVisible();
});

test("failed batch Agent startup keeps the item selected and shows its name and reason", async ({page}) => {
  await installMockTauri(page, {modLibrary: archives, holdAgentTurn: true});
  await page.addInitScript((artifactId) => {
    const target = window as any;
    const invoke = target.__TAURI_INTERNALS__.invoke;
    target.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, any> = {}) => {
      if (command === "start_turn" && args.request?.content?.includes(artifactId)) {
        target.__AMM_TEST__.calls.push({command, args});
        throw new Error("模拟 Agent 启动失败");
      }
      return invoke(command, args);
    };
  }, archives[0].artifactId);
  await page.goto("/");
  await gameDetails(page);
  await page.getByRole("switch", {name: "批量操作", exact: true}).click();
  await selection(page, "Batch Archive One").click();

  await page.getByRole("button", {name: "批量安装", exact: true}).click();
  await expect(page.getByText(/Batch Archive One.*模拟 Agent 启动失败/)).toBeVisible();
  await expect(selection(page, "Batch Archive One")).toBeChecked();
});
