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

async function installBatchProtocolMock(page: Page) {
  await page.addInitScript(() => {
    const target = window as any;
    const wrap = (transport: any) => {
      if (!transport || transport.invoke.__batchProtocolWrapped) return;
      const invoke = transport.invoke;
      const wrapped = async (command: string, args: Record<string, any> = {}) => {
        const origin = args.origin;
        if (command !== "start_turn" || origin?.kind !== "mod_request" || !origin.batch || !origin.targets.length) {
          return invoke(command, args);
        }

        const controls = target.__AMM_TEST__;
        const failuresRemaining = controls.batchStartFailuresRemaining ?? 0;
        if (failuresRemaining > 0) {
          controls.batchStartFailuresRemaining = failuresRemaining - 1;
          controls.calls.push({command, args});
          throw new Error("模拟批量 Agent 启动失败");
        }

        // The shared mock simulates one target. Preserve the real request and
        // its provenance for history and return them unchanged in assertions.
        const callStart = controls.calls.length;
        const mockArgs = { ...args, mockFirstBatchTarget: true };
        try {
          return await invoke(command, mockArgs);
        } finally {
          const recorded = controls.calls.slice(callStart).findLast((call: any) => call.command === command);
          if (recorded) recorded.args = args;
        }
      };
      (wrapped as any).__batchProtocolWrapped = true;
      transport.invoke = wrapped;
    };

    let transport = target.__TAURI_INTERNALS__;
    wrap(transport);
    Object.defineProperty(target, "__TAURI_INTERNALS__", {
      configurable: true,
      enumerable: true,
      get: () => transport,
      set: (value: any) => { transport = value; wrap(transport); },
    });
  });
}

function exactTargets(start: any) {
  const match = /目标（精确 ID）：(\[[^\n]+\])。/.exec(start.args.request.content);
  expect(match, "batch turn should include a JSON target list").not.toBeNull();
  return JSON.parse(match![1]);
}

async function expectBatchSession(page: Page, category: "install" | "uninstall" | "remove", displayTitle: string, targets: Record<string, string>[]) {
  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(1);
  const starts = await calls(page, "start_turn");
  expect(starts).toHaveLength(1);
  expect(exactTargets(starts[0])).toEqual(targets);
  expect(starts[0].args.origin).toEqual({ kind: "mod_request", version: 1, action: category, batch: true, targets });
  expect(starts[0].args.request.permissionMode).toBeNull();

  const conversations = await calls(page, "create_conversation");
  const categoryTitle = {install: "安装 Mod", uninstall: "卸载 Mod", remove: "删除 Mod"}[category];
  expect(conversations).toEqual([{
    command: "create_conversation",
    args: {title: `${categoryTitle}：${displayTitle}`, gameId, category},
  }]);
  expect(await createdConversationIds(page)).toHaveLength(1);
  return starts[0];
}

function modTable(page: Page) {
  return page.getByTestId("game-detail-page").getByRole("table", {name: "Mod 管理", exact: true});
}

function selection(page: Page, name: string) {
  return modTable(page).getByRole("checkbox", {name: `选择 ${name}`, exact: true});
}

async function openBatchMode(page: Page, holdAgentTurn = false) {
  await installMockTauri(page, {modLibrary: archives, holdAgentTurn});
  await installBatchProtocolMock(page);
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
      if (command === "start_turn" && args.request?.content?.includes('"installId":"install-second-fixture"') &&
          !args.origin?.batch) {
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
  await expectBatchSession(page, "install", "批量安装 2 个 Mod", [
    {gameId, artifactId: archives[0].artifactId},
    {gameId, artifactId: archives[1].artifactId},
  ]);
  expect(await calls(page, "plan_install", "plan_uninstall", "preview_fomod")).toEqual([]);
});

test("batch uninstall targets the installed row and skips pending rows in a mixed selection", async ({page}) => {
  await installMockTauri(page, {modLibrary: archives, holdAgentTurn: true});
  await installBatchProtocolMock(page);
  await addEnabledRecord(page);
  await page.goto("/");
  await gameDetails(page);
  await page.getByRole("switch", {name: "批量操作", exact: true}).click();
  await selectMixedRows(page);
  await selection(page, "Second Example").click();

  await page.getByRole("button", {name: "批量卸载", exact: true}).click();
  await expectBatchSession(page, "uninstall", "批量卸载 2 个 Mod", [
    {gameId, installId: "install-fixture"},
    {gameId, installId: "install-second-fixture"},
  ]);
  expect(await calls(page, "plan_install", "plan_uninstall", "preview_fomod")).toEqual([]);
});

test("batch install skips a row that already has an Agent action in progress", async ({page}) => {
  await installMockTauri(page, {modLibrary: archives, holdAgentTurn: true});
  await installBatchProtocolMock(page);
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
  const starts = await calls(page, "start_turn");
  const batchStart = starts.find((call: any) => call.args.origin?.batch);
  expect(batchStart).toBeDefined();
  expect(exactTargets(batchStart)).toEqual([{gameId, artifactId: archives[0].artifactId}]);
  expect(await calls(page, "create_conversation")).toHaveLength(2);
  expect(await createdConversationIds(page)).toHaveLength(2);
});

test("batch delete can be cancelled and removes selected archives after confirmation", async ({page}) => {
  await installMockTauri(page, {modLibrary: archives});
  await installBatchProtocolMock(page);
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
  expect(await calls(page, "create_conversation", "start_turn")).toEqual([]);
});

test("batch delete keeps successful removals and reports a failed item", async ({page}) => {
  const failedArtifactId = archives[1].artifactId;
  await installMockTauri(page, {modLibrary: archives});
  await installBatchProtocolMock(page);
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
  await installBatchProtocolMock(page);
  await addEnabledRecord(page);
  await addUninstalledRecord(page);
  await page.goto("/");
  await gameDetails(page);
  await page.getByRole("switch", {name: "批量操作", exact: true}).click();
  await selection(page, "Example").click();
  await selection(page, "Second Example").click();
  await selection(page, "Previously Uninstalled").click();
  await selection(page, "Batch Archive One").click();

  await page.getByRole("button", {name: "批量删除", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "批量删除 Mod", exact: true});
  await dialog.getByRole("button", {name: "确认删除", exact: true}).click();

  await expectBatchSession(page, "remove", "批量删除 2 个 Mod", [
    {gameId, installId: "install-fixture"},
    {gameId, installId: "install-second-fixture"},
  ]);
  await expect.poll(async () => (await calls(page, "remove_managed_mod")).length).toBe(2);
  const directRemovals = (await calls(page, "remove_managed_mod")).map(call => call.args).sort((a, b) =>
    `${a.target.kind}:${a.target.id}`.localeCompare(`${b.target.kind}:${b.target.id}`));
  expect(directRemovals).toEqual([
    {gameId, target: {kind: "archive", id: archives[0].artifactId}},
    {gameId, target: {kind: "record", id: "install-uninstalled-fixture"}},
  ].sort((a, b) => `${a.target.kind}:${a.target.id}`.localeCompare(`${b.target.kind}:${b.target.id}`)));
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
  await expectBatchSession(page, "uninstall", "批量卸载 1 个 Mod", [{gameId, installId: "install-fixture"}]);
  await approval.getByRole("button", {name: "批准并执行", exact: true}).click();

  await expect(modTable(page).getByRole("row").filter({hasText: "Example"})).toContainText("已卸载");
  await expect(selection(page, "Example")).not.toBeChecked();
  await expect(page.getByTestId("game-detail-page")).toBeVisible();
});

test("one batch turn displays one shared approval for all deployed rows", async ({page}) => {
  await installMockTauri(page, {modLibrary: archives});
  await installBatchProtocolMock(page);
  await addEnabledRecord(page);
  await page.goto("/");
  await gameDetails(page);
  await page.getByRole("switch", {name: "批量操作", exact: true}).click();
  await selection(page, "Example").click();
  await selection(page, "Second Example").click();

  await page.getByRole("button", {name: "批量卸载", exact: true}).click();
  await expectBatchSession(page, "uninstall", "批量卸载 2 个 Mod", [
    {gameId, installId: "install-fixture"},
    {gameId, installId: "install-second-fixture"},
  ]);
  await expect(page.getByTestId("game-detail-page").getByRole("group", {name: "操作审批"})).toHaveCount(1);
  await expect.poll(() => page.evaluate(() => (window as any).__AMM_TEST__.events
    .filter((event: any) => event.kind === "approval-request").length)).toBe(1);
  expect(await calls(page, "respond_approval")).toEqual([]);
});

test("one batch turn displays one shared questionnaire for selected archives", async ({page}) => {
  await installMockTauri(page, {modLibrary: archives});
  await installBatchProtocolMock(page);
  await page.goto("/");
  await gameDetails(page);
  await page.evaluate(() => { (window as any).__AMM_TEST__.modActionControls.outcome = "question"; });
  await page.getByRole("switch", {name: "批量操作", exact: true}).click();
  await selection(page, "Batch Archive One").click();
  await selection(page, "Batch Archive Two").click();

  await page.getByRole("button", {name: "批量安装", exact: true}).click();
  await expectBatchSession(page, "install", "批量安装 2 个 Mod", [
    {gameId, artifactId: archives[0].artifactId},
    {gameId, artifactId: archives[1].artifactId},
  ]);
  const question = page.getByTestId("game-detail-page").getByRole("region", {name: "安装前确认", exact: true});
  await expect(question).toHaveCount(1);
  await expect(question.getByText("请确认此 Mod 的安装目录。", {exact: true})).toHaveCount(1);
  await expect.poll(() => page.evaluate(() => (window as any).__AMM_TEST__.events
    .filter((event: any) => event.kind === "approval-request").length)).toBe(1);
  await question.getByRole("textbox", {name: "填写回答"}).fill("保持默认目录");
  await question.getByRole("button", {name: "提交回答", exact: true}).click();
  await expect.poll(async () => (await calls(page, "respond_approval")).length).toBe(1);
  expect((await calls(page, "respond_approval"))[0].args.response).toEqual({
    approved: true, answer: JSON.stringify(["保持默认目录"]),
  });
});

test("failed batch Agent startup keeps selection and reuses one conversation across retries", async ({page}) => {
  await installMockTauri(page, {modLibrary: archives});
  await installBatchProtocolMock(page);
  await page.goto("/");
  await gameDetails(page);
  await page.getByRole("switch", {name: "批量操作", exact: true}).click();
  await selection(page, "Batch Archive One").click();
  await page.evaluate(() => { (window as any).__AMM_TEST__.batchStartFailuresRemaining = 2; });

  const install = page.getByRole("button", {name: "批量安装", exact: true});
  for (const attempt of [1, 2]) {
    await install.click();
    await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(attempt);
    await expect(page.getByText(/Batch Archive One.*模拟批量 Agent 启动失败/)).toBeVisible();
    await expect(selection(page, "Batch Archive One")).toBeChecked();
  }
  await install.click();
  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(3);
  await expect(page.getByTestId("game-detail-page").getByRole("group", {name: "操作审批"})).toBeVisible();
  const starts = await calls(page, "start_turn");
  expect(starts.map(exactTargets)).toEqual(Array.from({length: 3}, () => [{gameId, artifactId: archives[0].artifactId}]));
  expect(new Set(starts.map(start => start.args.request.conversationId)).size).toBe(1);
  expect(await calls(page, "create_conversation")).toEqual([{
    command: "create_conversation",
    args: {title: "安装 Mod：批量安装 1 个 Mod", gameId, category: "install"},
  }]);
  expect(await createdConversationIds(page)).toHaveLength(1);
});

test("batch deletion renders a reusable task card while keeping the full Agent message for history, copy and edit", async ({page}) => {
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await installMockTauri(page, {modLibrary: archives});
  await addEnabledRecord(page);
  await page.goto("/");
  await gameDetails(page);
  await page.getByRole("switch", {name: "批量操作", exact: true}).click();
  await selection(page, "Example").click();
  await selection(page, "Second Example").click();

  await page.getByRole("button", {name: "批量删除", exact: true}).click();
  const confirmation = page.getByRole("dialog", {name: "批量删除 Mod", exact: true});
  await confirmation.getByRole("button", {name: "确认删除", exact: true}).click();

  const start = await expectBatchSession(page, "remove", "批量删除 2 个 Mod", [
    {gameId, installId: "install-fixture"},
    {gameId, installId: "install-second-fixture"},
  ]);
  const originalContent = start.args.request.content as string;
  expect(originalContent).toContain(`目标（精确 ID）：${JSON.stringify([
    {gameId, installId: "install-fixture"},
    {gameId, installId: "install-second-fixture"},
  ])}。`);

  await page.getByRole("button", {name: /^Agent( 对话)?$/}).click();
  await page.getByRole("button", {name: "删除 Mod", exact: true}).click();
  const conversationTitle = "删除 Mod：批量删除 2 个 Mod";
  await page.getByRole("button", {name: conversationTitle, exact: true}).click();

  const task = page.getByRole("group", {name: "批量删除 Mod", exact: true});
  await expect(task).toBeVisible();
  await expect(task).toContainText("Example");
  await expect(task).toContainText("Second Example");
  const identifierSummaries = task.locator("details > summary");
  await expect(identifierSummaries.first()).toHaveText("查看标识");
  for (let index = 0; index < await identifierSummaries.count(); index++) {
    await identifierSummaries.nth(index).click();
  }
  await expect(task.getByText("install-fixture", {exact: true})).toBeVisible();
  await expect(task.getByText("install-second-fixture", {exact: true})).toBeVisible();
  await expect(task.getByText(gameId, {exact: true})).toBeVisible();

  const approval = page.getByRole("group", {name: "操作审批", exact: true});
  await expect(approval).toBeVisible();
  await approval.getByRole("button", {name: "批准并执行", exact: true}).click();
  await expect(approval).toHaveCount(0);

  const userMessage = page.locator("article.chat-message").filter({has: page.locator('[data-message-id="user-fixture"]')});
  const actions = userMessage.getByRole("group", {name: "消息操作", exact: true});
  await actions.getByRole("button", {name: "复制消息", exact: true}).click();
  await expect(actions.getByRole("button", {name: "已复制", exact: true})).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(originalContent);

  await actions.getByRole("button", {name: "编辑消息", exact: true}).click();
  const composer = page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true});
  await expect(composer).toHaveValue(originalContent);
  await page.getByRole("button", {name: "取消编辑", exact: true}).click();

  await page.getByRole("button", {name: "检查 Minecraft 模组", exact: true}).click();
  await expect(page.locator(".chat-header h1")).toHaveText("检查 Minecraft 模组");
  await page.getByRole("button", {name: conversationTitle, exact: true}).click();
  await expect(page.getByRole("group", {name: "批量删除 Mod", exact: true})).toBeVisible();
});

test("ordinary Markdown messages remain plain chat instead of becoming Mod task cards", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");

  const content = "这是一条**普通文本**消息，请正常回答。";
  await page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true}).fill(content);
  await page.getByRole("button", {name: "发送消息", exact: true}).click();

  const userMessage = page.locator('[data-message-id="user-fixture"]');
  await expect(userMessage.locator("strong")).toHaveText("普通文本");
  for (const label of ["批量删除 Mod", "删除 Mod", "批量安装 Mod", "安装 Mod", "批量卸载 Mod", "卸载 Mod"]) {
    await expect(page.getByRole("group", {name: label, exact: true})).toHaveCount(0);
  }
  const starts = await calls(page, "start_turn");
  expect(starts).toHaveLength(1);
  expect(starts[0].args.request.content).toBe(content);
});
