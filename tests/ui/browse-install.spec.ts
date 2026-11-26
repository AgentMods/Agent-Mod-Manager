import { expect, test, type Page } from "@playwright/test";
import { BROWSE_ITEMS, browseCalls, installBrowseFixture } from "./browse-fixture";

const work = BROWSE_ITEMS[0];
const singleGameWork = BROWSE_ITEMS[1];
const conversationId = "browse-install-conversation";
const turnId = "browse-install-turn";

async function startPage(page: Page) {
  await installBrowseFixture(page);
  await page.goto("/");
  await page.evaluate(({ work, singleGameWork, conversationId, turnId }) => {
    const win = window as any;
    const fixture = win.__AMM_TEST__;
    const internals = win.__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let seq = 0;
    let attempts = 0;
    let activeConversation = conversationId;
    let activeTurn = turnId;
    let task: Record<string, unknown> | null = null;
    const tasks = new Map<string, { itemId: string; gameId: string; task: Record<string, unknown> }>();
    const taskReads: { itemId: string; gameId: string; task: Record<string, unknown> | null }[] = [];
    let activeItemId = work.id;
    let activeGameId = "game-stardew";
    let stage: "question" | "approve" | "progress" = "question";
    let failNextStart = false;
    let deferNextStart = false;
    let pendingStartResolve: (() => void) | null = null;
    const event = (kind: string, payload: Record<string, unknown>, live = false) => {
      const row = {
        seq: ++seq, conversationId: activeConversation, turnId: activeTurn, kind, messageId: "browse-reply",
        payload: kind === "approval-request" ? { ...payload, reviewSurface: "browseInstall" } : payload,
        createdAt: "2026-11-25T00:00:00Z",
      };
      fixture.seedEvents([row]);
      if (live) fixture.emitEvent(`agent://${kind}`, row);
    };
    const question = () => event("approval-request", { approvalId: "question", toolCallId: "question-call", title: "ask_user", preview: {
      title: "选择作品版本", questions: [{ question: "想安装哪个版本？", options: ["稳定版", "预览版"] }],
    } });
    internals.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      if (command === "list_browse_install_tasks") {
        fixture.calls.push({ command, args: structuredClone(args) });
        return [...tasks.values()].map(({ itemId, gameId }) => ({
          searchId: "browse-search-1", itemId, gameId,
          title: (itemId === work.id ? work : singleGameWork).title,
        }));
      }
      if (command === "browse_install_task") {
        fixture.calls.push({ command, args: structuredClone(args) });
        const itemId = String(args.itemId);
        const gameId = String(args.gameId);
        const record = tasks.get(JSON.stringify([itemId, gameId]));
        const response = record ? { ...record.task } : null;
        taskReads.push({ itemId, gameId, task: response && { ...response } });
        return response;
      }
      if (command === "start_browse_install") {
        fixture.calls.push({ command, args: structuredClone(args) });
        if (failNextStart) {
          failNextStart = false;
          throw new Error("fixture start failed");
        }
        if (deferNextStart) {
          deferNextStart = false;
          await new Promise<void>((resolve) => { pendingStartResolve = resolve; });
          pendingStartResolve = null;
        }
        attempts++;
        activeItemId = String(args.itemId);
        activeGameId = String(args.gameId);
        activeConversation = attempts === 1 ? conversationId : conversationId + "-" + attempts;
        activeTurn = attempts === 1 ? turnId : turnId + "-" + attempts;
        task = { id: "browse-install-task-" + attempts, conversationId: activeConversation, turnId: activeTurn, status: "running", installId: null, transactionId: null };
        tasks.set(JSON.stringify([activeItemId, activeGameId]), { itemId: activeItemId, gameId: activeGameId, task });
        event("turn-start", {});
        if (stage === "question") question();
        return { ...task };
      }
      if (command === "respond_approval" && args.conversationId === activeConversation) {
        fixture.calls.push({ command, args: structuredClone(args) });
        const response = args.response as { approved: boolean };
        event("approval-resolved", { approvalId: args.approvalId, status: response.approved ? "approved" : "rejected" }, true);
        if (args.approvalId === "question") {
          stage = "approve";
          event("approval-request", { approvalId: "write", toolCallId: "install-call", title: "download_mod", preview: { filename: "stable.zip" } }, true);
        } else if (!response.approved && task) {
          task.status = "rejected";
          event("done", { status: "done" }, true);
        }
        return null;
      }
      if (command === "cancel_turn" && args.conversationId === activeConversation) {
        fixture.calls.push({ command, args: structuredClone(args) });
        if (task) task.status = "cancelled";
        event("done", { status: "cancelled" }, true);
        return null;
      }
      return invoke(command, args);
    };
    win.__BROWSE_INSTALL_SPEC__ = {
      skipQuestion: () => { stage = "progress"; },
      failNextStart: () => { failNextStart = true; },
      deferNextStart: () => { deferNextStart = true; },
      resolvePendingStart: () => {
        if (!pendingStartResolve) throw new Error("No deferred start_browse_install call");
        pendingStartResolve();
      },
      finishWithEvidence: () => {
        if (!task) throw new Error("Missing fixture task");
        Object.assign(task, { status: "installed", installId: "installed-mod", transactionId: "committed-install" });
        event("done", { status: "done" }, true);
      },
      finishFailed: () => {
        if (!task) throw new Error("Missing fixture task");
        task.status = "failed";
        event("done", { status: "error", error: "fixture install failed" }, true);
      },
      finishWithoutEvidence: () => {
        if (!task) throw new Error("Missing fixture task");
        event("tool-result", { name: "install_library_mod", status: "done", result: { installId: "model-invented", transactionId: "model-invented" } }, true);
        task.status = "unverified";
        event("done", { status: "done" }, true);
      },
      getTaskReads: () => structuredClone(taskReads),
    };
  }, { work, singleGameWork, conversationId, turnId });
  await page.getByRole("button", { name: "游览", exact: true }).click();
  await page.getByRole("button", { name: "选择游览的游戏", exact: true }).click();
  const picker = page.getByRole("dialog", { name: "选择游览的游戏" });
  await picker.getByRole("checkbox", { name: "Stardew Valley" }).check();
  await picker.getByRole("button", { name: "确定", exact: true }).click();
  await expect.poll(async () => (await browseCalls(page, "browse_authorize")).length).toBe(1);
  await expect.poll(async () => (await browseCalls(page, "browse_start_search")).length).toBe(1);
  await expect(page.getByText(work.title, { exact: true })).toBeVisible();
}

function installDialog(page: Page) {
  return page.getByRole("dialog", { name: "安装 Mod", exact: true });
}

function taskButton(page: Page) {
  return page.getByRole("button", { name: "安装任务", exact: true });
}

function taskList(page: Page) {
  return page.getByRole("dialog", { name: "安装任务", exact: true });
}

function taskPanel(page: Page) {
  return taskList(page).getByRole("region", { name: "Agent 安装", exact: true });
}

function cardFor(page: Page, item = work) {
  return page.getByRole("article", { name: item.title, exact: true });
}

async function startWork(page: Page) {
  const card = cardFor(page);
  await card.getByRole("button", { name: "安装", exact: true }).click();
  const game = card.getByRole("combobox", { name: "目标游戏" });
  await expect(game).toBeVisible();
  await expect(installDialog(page)).toHaveCount(0);
  await game.selectOption("game-stardew");
  await expect.poll(async () => (await getCommands(page, "start_browse_install")).length).toBe(1);
}

async function openTask(page: Page, item = work, status?: string) {
  await taskButton(page).click();
  const row = taskList(page).getByRole("button", { name: item.title, exact: true });
  if (status) await expect(row).toHaveAccessibleDescription(`Stardew Valley ${status}`);
  await expect(row).toHaveAttribute("aria-expanded", "false");
  await row.click();
  await expect(row).toHaveAttribute("aria-expanded", "true");
  await expect(taskPanel(page)).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(1);
  await expect(installDialog(page)).toHaveCount(0);
  return row;
}

async function getCommands(page: Page, command: string) {
  return page.evaluate((name) => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === name).map((c: any) => c.args), command);
}

test("游览安装仅传原生句柄，翻页后从任务索引回答并拒绝页内审批", async ({ page }) => {
  await startPage(page);
  // Prime the empty global index while its query is still fresh. The new task
  // needs to invalidate that query, not just update the card panel cache.
  await taskButton(page).click();
  await expect(taskList(page).getByText("暂无安装任务")).toBeVisible();
  await page.keyboard.press("Escape");
  expect(await getCommands(page, "list_browse_install_tasks")).toHaveLength(1);
  await startWork(page);
  await expect(taskButton(page)).toHaveAccessibleDescription("等待你回答");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.getByText("查看待授权操作", { exact: true })).toHaveCount(0);
  expect(await getCommands(page, "start_browse_install")).toEqual([{ searchId: "browse-search-1", itemId: work.id, gameId: "game-stardew" }]);
  await page.getByRole("button", { name: "下一页" }).click();
  await expect(page.getByText(BROWSE_ITEMS[2].title, { exact: true })).toBeVisible();
  await openTask(page, work, "等待你回答");
  expect(await getCommands(page, "list_browse_install_tasks")).toHaveLength(2);
  expect(await getCommands(page, "start_browse_install")).toHaveLength(1);
  await expect(taskPanel(page).getByText("选择作品版本", { exact: true })).toBeVisible();
  await taskPanel(page).getByText("稳定版", { exact: true }).click();
  await taskPanel(page).getByRole("button", { name: "提交回答" }).click();
  await expect(taskPanel(page).getByText("等待你审批", { exact: true })).toBeVisible();
  await expect(page.getByText("查看待授权操作", { exact: true })).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(taskList(page)).toHaveCount(0);
  await expect(page.getByText("查看待授权操作", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await expect(page.getByText("查看待授权操作", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "游览", exact: true }).click();
  await expect(taskButton(page)).toHaveAccessibleDescription("等待你审批");
  await openTask(page, work, "等待你审批");
  await taskPanel(page).getByRole("button", { name: "拒绝", exact: true }).click();
  await expect(taskPanel(page).getByText("安装操作已拒绝", { exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(taskButton(page)).not.toHaveAccessibleDescription("等待你审批");
  expect(await getCommands(page, "respond_approval")).toMatchObject([
    { conversationId, approvalId: "question", response: { approved: true } },
    { conversationId, approvalId: "write", response: { approved: false } },
  ]);
  expect(await getCommands(page, "start_turn")).toEqual([]);
});

test("单目标卡片显示安装、正在安装和原生证据确认的已安装状态", async ({ page }) => {
  await startPage(page);
  await page.evaluate(() => {
    const spec = (window as any).__BROWSE_INSTALL_SPEC__;
    spec.skipQuestion();
    spec.deferNextStart();
  });
  const card = cardFor(page, singleGameWork);
  const install = card.getByRole("button", { name: "安装", exact: true });
  await expect(install).toBeEnabled();
  await install.click();
  const installing = card.getByRole("button", { name: "正在安装", exact: true });
  await expect(installing).toBeDisabled();
  await expect.poll(async () => (await getCommands(page, "start_browse_install")).length).toBe(1);
  await installing.evaluate((button: HTMLButtonElement) => button.click());
  expect(await getCommands(page, "start_browse_install")).toHaveLength(1);
  await page.evaluate(() => (window as any).__BROWSE_INSTALL_SPEC__.resolvePendingStart());
  await expect(installing).toBeDisabled();
  await page.evaluate(() => (window as any).__BROWSE_INSTALL_SPEC__.finishWithEvidence());
  const installed = card.getByRole("button", { name: "已安装", exact: true });
  await expect(installed).toBeDisabled();
  await expect.poll(async () => page.evaluate(() =>
    (window as any).__BROWSE_INSTALL_SPEC__.getTaskReads().some((read: any) =>
      read.itemId === "work-gentle-combat" && read.gameId === "game-stardew" &&
      read.task?.status === "installed" && read.task.installId === "installed-mod" &&
      read.task.transactionId === "committed-install",
    ),
  )).toBe(true);

  await page.getByRole("button", { name: "下一页" }).click();
  await expect(card).toHaveCount(0);
  await page.getByRole("button", { name: "上一页" }).click();
  await expect(card.getByRole("button", { name: "已安装", exact: true })).toBeDisabled();
  expect(await getCommands(page, "start_browse_install")).toEqual([
    { searchId: "browse-search-1", itemId: singleGameWork.id, gameId: "game-stardew" },
  ]);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const row = await openTask(page, singleGameWork, "安装已验证");
  await expect(taskPanel(page).getByText("安装已验证", { exact: true })).toBeVisible();
  await row.press("Enter");
  await expect(row).toHaveAttribute("aria-expanded", "false");
  await expect(taskPanel(page)).toHaveCount(0);
  await expect(taskList(page)).toBeVisible();
});

test("多目标卡片按游戏显示状态，结束后仍可查看全部目标", async ({ page }) => {
  await startPage(page);
  await page.evaluate(() => (window as any).__BROWSE_INSTALL_SPEC__.skipQuestion());
  const card = cardFor(page);
  const install = card.getByRole("button", { name: "安装", exact: true });
  const choose = card.getByRole("button", { name: "选择游戏", exact: true });
  const game = card.getByRole("combobox", { name: "目标游戏" });
  await expect(install).toBeEnabled();
  await expect(choose).toHaveCount(0);
  await install.click();
  await expect(game).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(choose).toHaveCount(0);
  await expect(game.locator('option[value="game-fixture"]')).toContainText("Minecraft Java Edition");
  await expect(game.locator('option[value="game-stardew"]')).toContainText("Stardew Valley");
  expect(await getCommands(page, "start_browse_install")).toHaveLength(0);
  await game.selectOption("game-stardew");
  await expect.poll(async () => (await getCommands(page, "start_browse_install")).length).toBe(1);
  await expect(card.getByRole("button", { name: "正在安装", exact: true })).toBeDisabled();
  await expect(choose).toBeEnabled();
  await choose.click();
  await expect(game.locator('option[value="game-stardew"]')).toBeDisabled();
  await expect(game.locator('option[value="game-stardew"]')).toContainText("正在安装");
  expect(await getCommands(page, "start_browse_install")).toEqual([
    { searchId: "browse-search-1", itemId: work.id, gameId: "game-stardew" },
  ]);

  await page.evaluate(() => (window as any).__BROWSE_INSTALL_SPEC__.finishWithEvidence());
  await expect(card.getByRole("button", { name: "已安装", exact: true })).toBeDisabled();
  await expect(game.locator('option[value="game-stardew"]')).toContainText("已安装");
  await expect.poll(async () => page.evaluate(() =>
    (window as any).__BROWSE_INSTALL_SPEC__.getTaskReads().some((read: any) =>
      read.itemId === "work-starlit-fields" && read.gameId === "game-stardew" &&
      read.task?.status === "installed" && read.task.installId === "installed-mod" &&
      read.task.transactionId === "committed-install",
    ),
  )).toBe(true);
  await expect(choose).toBeEnabled();
  await expect(game.locator('option[value="game-fixture"]')).toBeEnabled();

  await game.selectOption("game-fixture");
  await expect.poll(async () => (await getCommands(page, "start_browse_install")).length).toBe(2);
  await expect(card.getByRole("button", { name: "正在安装", exact: true })).toBeDisabled();
  await choose.click();
  await expect(game.locator('option[value="game-fixture"]')).toBeDisabled();
  await expect(game.locator('option[value="game-fixture"]')).toContainText("正在安装");
  await expect(game.locator('option[value="game-stardew"]')).toBeDisabled();
  await expect(game.locator('option[value="game-stardew"]')).toContainText("已安装");
  expect(await getCommands(page, "start_browse_install")).toEqual([
    { searchId: "browse-search-1", itemId: work.id, gameId: "game-stardew" },
    { searchId: "browse-search-1", itemId: work.id, gameId: "game-fixture" },
  ]);

  await page.evaluate(() => (window as any).__BROWSE_INSTALL_SPEC__.finishWithEvidence());
  await expect(card.getByRole("button", { name: "已安装", exact: true })).toBeDisabled();
  await expect(game.locator('option[value="game-fixture"]')).toContainText("已安装");
  await expect(choose).toBeEnabled();
  await expect.poll(async () => page.evaluate(() =>
    (window as any).__BROWSE_INSTALL_SPEC__.getTaskReads().some((read: any) =>
      read.itemId === "work-starlit-fields" && read.gameId === "game-fixture" &&
      read.task?.status === "installed" && read.task.installId === "installed-mod" &&
      read.task.transactionId === "committed-install",
    ),
  )).toBe(true);
  await expect(game.locator('option[value="game-stardew"]')).toBeDisabled();
  await expect(game.locator('option[value="game-stardew"]')).toContainText("已安装");
});

test("启动 IPC 失败在卡片显示，重试后仍在后台启动", async ({ page }) => {
  await startPage(page);
  await page.evaluate(() => {
    const spec = (window as any).__BROWSE_INSTALL_SPEC__;
    spec.skipQuestion();
    spec.failNextStart();
  });
  const card = cardFor(page, singleGameWork);
  await card.getByRole("button", { name: "安装", exact: true }).click();
  await expect(card.getByText("fixture start failed", { exact: false })).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(await getCommands(page, "start_browse_install")).toHaveLength(1);

  await card.getByRole("button", { name: "重新尝试", exact: true }).click();
  await expect(page.getByText("安装任务已在后台启动，可在「安装任务」中查看。", { exact: true })).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(await getCommands(page, "start_browse_install")).toEqual([
    { searchId: "browse-search-1", itemId: singleGameWork.id, gameId: "game-stardew" },
    { searchId: "browse-search-1", itemId: singleGameWork.id, gameId: "game-stardew" },
  ]);
  await openTask(page, singleGameWork, "安装进行中");
});

test("延迟启动期间卡片禁重试，返回后刷新手动打开的任务列表", async ({ page }) => {
  await startPage(page);
  await page.evaluate(() => (window as any).__BROWSE_INSTALL_SPEC__.deferNextStart());
  const card = cardFor(page, singleGameWork);
  await card.getByRole("button", { name: "安装", exact: true }).click();
  const installing = card.getByRole("button", { name: "正在安装", exact: true });
  await expect(installing).toBeDisabled();
  await expect.poll(async () => (await getCommands(page, "start_browse_install")).length).toBe(1);
  await installing.evaluate((button: HTMLButtonElement) => button.click());
  expect(await getCommands(page, "start_browse_install")).toHaveLength(1);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("button", { name: "下一页" }).click();
  await expect(cardFor(page, BROWSE_ITEMS[2])).toBeVisible();
  await page.getByRole("button", { name: "上一页" }).click();
  await expect(installing).toBeDisabled();
  await taskButton(page).click();
  await expect(taskList(page).getByText("暂无安装任务")).toBeVisible();

  await page.evaluate(() => (window as any).__BROWSE_INSTALL_SPEC__.resolvePendingStart());
  const row = taskList(page).getByRole("button", { name: singleGameWork.title, exact: true });
  await expect(row).toHaveAccessibleDescription("Stardew Valley 等待你回答");
  await expect(installDialog(page)).toHaveCount(0);
  expect(await getCommands(page, "start_browse_install")).toEqual([
    { searchId: "browse-search-1", itemId: singleGameWork.id, gameId: "game-stardew" },
  ]);
  await row.click();
  await expect(taskPanel(page).getByText("选择作品版本", { exact: true })).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(1);
  await expect(installDialog(page)).toHaveCount(0);
});

test("模型的成功事件不会验证安装，重试仍可取消", async ({ page }) => {
  await startPage(page);
  await page.evaluate(() => (window as any).__BROWSE_INSTALL_SPEC__.skipQuestion());
  await cardFor(page, singleGameWork).getByRole("button", { name: "安装", exact: true }).click();
  await expect.poll(async () => (await getCommands(page, "start_browse_install")).length).toBe(1);
  await openTask(page, singleGameWork);
  await expect(taskPanel(page).getByText("安装进行中", { exact: true })).toBeVisible();
  await page.evaluate(() => (window as any).__BROWSE_INSTALL_SPEC__.finishWithoutEvidence());
  await expect(taskPanel(page).getByText("未验证到安装结果", { exact: true })).toBeVisible();
  await expect(taskPanel(page).getByText("安装已验证", { exact: true })).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(cardFor(page, singleGameWork).getByRole("button", { name: "安装", exact: true })).toBeEnabled();
  await openTask(page, singleGameWork, "未验证到安装结果");
  // A new browsing panel can make another attempt using the active search.
  await taskPanel(page).getByRole("button", { name: "重新尝试" }).click();
  await expect.poll(async () => (await getCommands(page, "start_browse_install")).length).toBe(2);
  await expect(taskPanel(page).getByText("安装进行中", { exact: true })).toBeVisible();
  await expect(cardFor(page, singleGameWork).getByRole("button", { name: "正在安装", exact: true })).toBeDisabled();
  await taskPanel(page).getByRole("button", { name: "取消", exact: true }).click();
  await expect(taskPanel(page).getByText("安装已取消", { exact: true })).toBeVisible();
  expect(await getCommands(page, "cancel_turn")).toEqual([{ conversationId: conversationId + "-2", turnId: turnId + "-2" }]);
});

test("终态失败时卡片恢复安装按钮，任务详情仍可重新尝试", async ({ page }) => {
  await startPage(page);
  await page.evaluate(() => (window as any).__BROWSE_INSTALL_SPEC__.skipQuestion());
  await cardFor(page, singleGameWork).getByRole("button", { name: "安装", exact: true }).click();
  await expect.poll(async () => (await getCommands(page, "start_browse_install")).length).toBe(1);
  await openTask(page, singleGameWork);
  await expect(taskPanel(page).getByText("安装进行中", { exact: true })).toBeVisible();
  await page.evaluate(() => (window as any).__BROWSE_INSTALL_SPEC__.finishFailed());
  await expect(taskPanel(page).getByText("安装失败", { exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(cardFor(page, singleGameWork).getByRole("button", { name: "安装", exact: true })).toBeEnabled();
  await openTask(page, singleGameWork, "安装失败");
  await taskPanel(page).getByRole("button", { name: "重新尝试", exact: true }).click();
  await expect.poll(async () => (await getCommands(page, "start_browse_install")).length).toBe(2);
  await expect(taskPanel(page).getByText("安装进行中", { exact: true })).toBeVisible();
  await expect(cardFor(page, singleGameWork).getByRole("button", { name: "正在安装", exact: true })).toBeDisabled();
  expect(await getCommands(page, "start_browse_install")).toEqual([
    { searchId: "browse-search-1", itemId: singleGameWork.id, gameId: "game-stardew" },
    { searchId: "browse-search-1", itemId: singleGameWork.id, gameId: "game-stardew" },
  ]);
});

test("页内批准后由后端事务回执确认安装", async ({ page }) => {
  await startPage(page);
  await startWork(page);
  await openTask(page);
  await taskPanel(page).getByText("稳定版", { exact: true }).click();
  await taskPanel(page).getByRole("button", { name: "提交回答" }).click();
  await taskPanel(page).getByRole("button", { name: "批准并执行" }).click();
  await expect(taskPanel(page).getByText("安装进行中", { exact: true })).toBeVisible();
  await expect(taskPanel(page).getByText("安装已验证", { exact: true })).toHaveCount(0);
  await page.evaluate(() => (window as any).__BROWSE_INSTALL_SPEC__.finishWithEvidence());
  await expect(taskPanel(page).getByText("安装已验证", { exact: true })).toBeVisible();
  expect((await getCommands(page, "respond_approval")).at(-1)).toMatchObject({ conversationId, approvalId: "write", response: { approved: true } });
});
