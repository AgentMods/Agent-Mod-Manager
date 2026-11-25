import { expect, test, type Page } from "@playwright/test";
import { BROWSE_ITEMS, installBrowseFixture } from "./browse-fixture";

const work = BROWSE_ITEMS[0];
const conversationId = "browse-install-conversation";
const turnId = "browse-install-turn";

async function startPage(page: Page) {
  await installBrowseFixture(page);
  await page.goto("/");
  await page.evaluate(({ work, conversationId, turnId }) => {
    const win = window as any;
    const fixture = win.__AMM_TEST__;
    const internals = win.__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let seq = 0;
    let attempts = 0;
    let activeConversation = conversationId;
    let activeTurn = turnId;
    let task: Record<string, unknown> | null = null;
    let stage: "question" | "approve" | "progress" = "question";
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
        return task ? [{ searchId: "browse-search-1", itemId: work.id, gameId: "game-stardew", title: work.title }] : [];
      }
      if (command === "browse_install_task") {
        return args.itemId === work.id && args.gameId === "game-stardew" ? task && { ...task } : null;
      }
      if (command === "start_browse_install") {
        fixture.calls.push({ command, args: structuredClone(args) });
        attempts++;
        activeConversation = attempts === 1 ? conversationId : conversationId + "-" + attempts;
        activeTurn = attempts === 1 ? turnId : turnId + "-" + attempts;
        task = { id: "browse-install-task-" + attempts, conversationId: activeConversation, turnId: activeTurn, status: "running", installId: null, transactionId: null };
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
      finishWithEvidence: () => {
        if (!task) throw new Error("Missing fixture task");
        Object.assign(task, { status: "installed", installId: "installed-mod", transactionId: "committed-install" });
        event("done", { status: "done" }, true);
      },
      finishWithoutEvidence: () => {
        if (!task) throw new Error("Missing fixture task");
        event("tool-result", { name: "install_library_mod", status: "done", result: { installId: "model-invented", transactionId: "model-invented" } }, true);
        task.status = "unverified";
        event("done", { status: "done" }, true);
      },
    };
  }, { work, conversationId, turnId });
  await page.getByRole("button", { name: "游览", exact: true }).click();
  await page.getByRole("button", { name: "选择游览的游戏", exact: true }).click();
  const picker = page.getByRole("dialog", { name: "选择游览的游戏" });
  await picker.getByRole("checkbox", { name: "Stardew Valley" }).check();
  await picker.getByRole("button", { name: "确定", exact: true }).click();
  await page.getByRole("button", { name: "允许本次游览", exact: true }).click();
  await expect(page.getByText(work.title, { exact: true })).toBeVisible();
}

function dialog(page: Page) {
  return page.getByRole("dialog", { name: "安装 Mod", exact: true });
}

async function openWork(page: Page) {
  const card = page.getByRole("article").filter({ hasText: work.title });
  await card.getByRole("button", { name: "安装", exact: true }).click();
  await expect(dialog(page)).toBeVisible();
  const game = dialog(page).getByRole("combobox", { name: "目标游戏" });
  if (await game.isVisible()) await game.selectOption("game-stardew");
}

async function getCommands(page: Page, command: string) {
  return page.evaluate((name) => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === name).map((c: any) => c.args), command);
}

test("游览安装只传 opaque 句柄，翻页后从任务索引回答并拒绝页内审批", async ({ page }) => {
  await startPage(page);
  // Prime the empty global index while its query is still fresh. The new task
  // needs to invalidate that query, not just update the card panel cache.
  await page.getByRole("button", { name: "安装任务", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "安装任务" }).getByText("暂无安装任务")).toBeVisible();
  await page.keyboard.press("Escape");
  expect(await getCommands(page, "list_browse_install_tasks")).toHaveLength(1);
  await openWork(page);
  await dialog(page).getByRole("button", { name: "开始安装", exact: true }).click();
  await expect(dialog(page).getByText("选择作品版本", { exact: true })).toBeVisible();
  await expect(page.getByText("查看待授权操作", { exact: true })).toHaveCount(0);
  expect(await getCommands(page, "start_browse_install")).toEqual([{ searchId: "browse-search-1", itemId: work.id, gameId: "game-stardew" }]);
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "下一页" }).click();
  await expect(page.getByText(BROWSE_ITEMS[2].title, { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "安装任务", exact: true }).click();
  await page.getByRole("dialog", { name: "安装任务" }).getByRole("button", { name: work.title, exact: true }).click();
  expect(await getCommands(page, "list_browse_install_tasks")).toHaveLength(2);
  await expect(dialog(page).getByText("选择作品版本", { exact: true })).toBeVisible();
  await dialog(page).getByText("稳定版", { exact: true }).click();
  await dialog(page).getByRole("button", { name: "提交回答" }).click();
  await expect(dialog(page).getByText("等待你审批", { exact: true })).toBeVisible();
  await expect(page.getByText("查看待授权操作", { exact: true })).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(dialog(page)).toHaveCount(0);
  await expect(page.getByText("查看待授权操作", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await expect(page.getByText("查看待授权操作", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "游览", exact: true }).click();
  await page.getByRole("button", { name: "安装任务", exact: true }).click();
  await page.getByRole("dialog", { name: "安装任务" }).getByRole("button", { name: work.title, exact: true }).click();
  await dialog(page).getByRole("button", { name: "拒绝", exact: true }).click();
  await expect(dialog(page).getByText("安装操作已拒绝", { exact: true })).toBeVisible();
  expect(await getCommands(page, "respond_approval")).toMatchObject([
    { conversationId, approvalId: "question", response: { approved: true } },
    { conversationId, approvalId: "write", response: { approved: false } },
  ]);
  expect(await getCommands(page, "start_turn")).toEqual([]);
});

test("模型的成功事件不会验证安装，重试仍可取消", async ({ page }) => {
  await startPage(page);
  await page.evaluate(() => (window as any).__BROWSE_INSTALL_SPEC__.skipQuestion());
  await openWork(page);
  await dialog(page).getByRole("button", { name: "开始安装", exact: true }).click();
  await expect(dialog(page).getByText("安装进行中", { exact: true })).toBeVisible();
  await page.evaluate(() => (window as any).__BROWSE_INSTALL_SPEC__.finishWithoutEvidence());
  await expect(dialog(page).getByText("未验证到安装结果", { exact: true })).toBeVisible();
  await expect(dialog(page).getByText("安装已验证", { exact: true })).toHaveCount(0);
  // A new browsing panel can make another attempt using the active search.
  await dialog(page).getByRole("button", { name: "重新尝试" }).click();
  await expect.poll(async () => (await getCommands(page, "start_browse_install")).length).toBe(2);
  await expect(dialog(page).getByText("安装进行中", { exact: true })).toBeVisible();
  await dialog(page).getByRole("button", { name: "取消", exact: true }).click();
  await expect(dialog(page).getByText("安装已取消", { exact: true })).toBeVisible();
  expect(await getCommands(page, "cancel_turn")).toEqual([{ conversationId: conversationId + "-2", turnId: turnId + "-2" }]);
});


test("页内批准后由后端事务回执确认安装", async ({ page }) => {
  await startPage(page);
  await openWork(page);
  await dialog(page).getByRole("button", { name: "开始安装", exact: true }).click();
  await dialog(page).getByText("稳定版", { exact: true }).click();
  await dialog(page).getByRole("button", { name: "提交回答" }).click();
  await dialog(page).getByRole("button", { name: "批准并执行" }).click();
  await expect(dialog(page).getByText("安装进行中", { exact: true })).toBeVisible();
  await expect(dialog(page).getByText("安装已验证", { exact: true })).toHaveCount(0);
  await page.evaluate(() => (window as any).__BROWSE_INSTALL_SPEC__.finishWithEvidence());
  await expect(dialog(page).getByText("安装已验证", { exact: true })).toBeVisible();
  expect((await getCommands(page, "respond_approval")).at(-1)).toMatchObject({ conversationId, approvalId: "write", response: { approved: true } });
});
