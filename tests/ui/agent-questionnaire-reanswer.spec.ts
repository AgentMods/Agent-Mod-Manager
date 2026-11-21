import { expect, test, type Locator, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const title = "确认游戏适配信息";
const preview = {
  title,
  questions: [
    { question: "游戏在哪个平台？", options: ["Windows PC", "其他平台"] },
    { question: "从哪里确认识别标记？", options: ["参考已登记游戏", "参考本机文件"] },
  ],
};

async function stubReanswerStart(page: Page, rejectFirst: boolean) {
  await page.evaluate((rejectFirst) => {
    const fixture = (window as any).__AMM_TEST__;
    const tauri = (window as any).__TAURI_INTERNALS__;
    const invoke = tauri.invoke;
    let attempts = 0;
    tauri.invoke = async (command: string, args: Record<string, any> = {}) => {
      if (command === "start_turn" && String(args.request?.content).startsWith("补充回答：")) {
        attempts += 1;
        fixture.calls.push({ command, args });
        if (rejectFirst && attempts === 1) throw new Error("测试模拟 start_turn 首次拒绝");
        return `turn-reanswer-fixture-${attempts}`;
      }
      return invoke(command, args);
    };
  }, rejectFirst);
}

async function openHistoryPage(page: Page) {
  // HistoryPage currently has no primary-navigation entry; exercise the page
  // through the app's navigation store, then use its actual controls below.
  await page.evaluate(async () => {
    const { useUI } = await import("/src/lib/store.ts");
    useUI.getState().navigate("history");
  });
  await expect(page.getByTestId("history-page")).toBeVisible();
}

async function expectQuestionnaireFitsViewport(page: Page, form: Locator) {
  const viewport = page.viewportSize();
  const bounds = await form.boundingBox();
  expect(viewport).not.toBeNull();
  expect(bounds).not.toBeNull();
  expect(bounds!.x).toBeGreaterThanOrEqual(0);
  expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(viewport!.width);
  expect(viewport!.height - (bounds!.y + bounds!.height)).toBeLessThanOrEqual(24);
  await expect(form).toBeInViewport({ ratio: 0.99 });
}

async function startQuestionnaire(page: Page, terminal: "cancelled" | "expired" | "skipped" | "rejected") {
  const questionPreview = terminal === "rejected" ? { question: "请输入安装目录。" } : preview;
  const questionTitle = terminal === "rejected" ? "Agent 等待你的回答" : title;
  const count = terminal === "rejected" ? 1 : 2;
  await installMockTauri(page, { askUser: { preview: questionPreview } });
  await page.goto("/");
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await page.getByRole("button", { name: /^Agent( 对话)?$/ }).click();
  await page.getByRole("textbox", { name: "告诉 Agent 你想做什么" }).fill("帮我确认游戏适配信息");
  await page.getByRole("button", { name: "发送消息", exact: true }).click();
  const form = page.getByRole("region", { name: questionTitle });
  await expect(form).toBeVisible();
  if (terminal === "cancelled") {
    await page.getByRole("button", { name: "停止生成", exact: true }).click();
  } else if (terminal === "skipped") {
    await form.getByRole("button", { name: "跳过", exact: true }).click();
    await expect(form.getByText("2 / 2", { exact: true })).toBeVisible();
    await form.getByRole("button", { name: "跳过", exact: true }).click();
  } else if (terminal === "rejected") {
    await form.getByRole("button", { name: "跳过", exact: true }).click();
  } else {
    await page.evaluate(() => {
      const fixture = (window as any).__AMM_TEST__;
      fixture.emit("approval-resolved", { approvalId: "approval-fixture", status: "cancelled" });
      fixture.emit("done", { status: "cancelled" });
    });
  }

  const summary = page.getByRole("button", { name: new RegExp(`已询问 ${count} 个问题`) });
  await expect(summary).toBeVisible();
  await expect(form).toHaveCount(0);
  await summary.click();
  await expect(page.locator("dl dd")).toHaveText(
    terminal === "skipped" ? ["已跳过", "已跳过"] : terminal === "rejected" ? ["已跳过"] : ["未回答", "未回答"],
  );
  await expect(page.getByRole("button", { name: "停止生成", exact: true })).toHaveCount(0);
  return { summary, questionTitle, terminal };
}

const terminalLabels = { cancelled: "已取消", expired: "已过期", skipped: "全部跳过", rejected: "已拒绝" } as const;
for (const terminal of ["cancelled", "expired", "skipped", "rejected"] as const) {
  test(`${terminalLabels[terminal]}问卷可在原会话重新回答并以新用户消息开启一轮`, async ({ page }) => {
    const { summary, questionTitle } = await startQuestionnaire(page, terminal);
    const oldApprovalBefore = await page.evaluate(() => (window as any).__AMM_TEST__.events
      .filter((event: any) => event.kind === "approval-resolved" && event.payload.approvalId === "approval-fixture")
      .map((event: any) => event.payload.status));
    const reanswer = page.getByRole("button", { name: "重新回答", exact: true });
    await expect(reanswer).toBeVisible();
    expect((await reanswer.boundingBox())!.height).toBeGreaterThanOrEqual(36);
    const composer = page.getByRole("textbox", { name: "告诉 Agent 你想做什么" });
    await expect(composer).toBeVisible();
    await composer.fill("保留在输入框里的草稿");
    await reanswer.click();

    const form = page.getByRole("region", { name: questionTitle });
    await expect(form).toBeVisible();
    await expectQuestionnaireFitsViewport(page, form);
    await expect(composer).toHaveCount(0);
    const rejectFirst = terminal === "cancelled";
    await stubReanswerStart(page, rejectFirst);
    if (terminal === "rejected") {
      await form.getByRole("textbox", { name: "填写回答" }).fill("D:/Games/Test");
    } else {
      await form.getByRole("radio", { name: "Windows PC" }).click();
      await form.getByRole("button", { name: "下一步", exact: true }).click();
      await form.getByRole("radio", { name: "参考本机文件" }).click();
    }
    await form.getByRole("button", { name: "提交回答", exact: true }).click();
    if (rejectFirst) {
      await expect(form).toBeVisible();
      await expect(form.getByRole("radio", { name: "参考本机文件" })).toBeChecked();
      await expect(composer).toHaveCount(0);
      await expect(form.getByRole("button", { name: "提交回答", exact: true })).toBeEnabled();
      await form.getByRole("button", { name: "提交回答", exact: true }).click();
    }
    await expect(form).toHaveCount(0);
    await expect(composer).toBeVisible();
    await expect(composer).toHaveValue("保留在输入框里的草稿");
    const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
    const turns = calls.filter((call: any) => call.command === "start_turn");
    const restarts = calls.filter((call: any) => call.command === "restart_turn");
    const staleResponses = calls.filter((call: any) => call.command === "respond_approval");
    expect(turns).toHaveLength(rejectFirst ? 3 : 2);
    const reanswerTurns = turns.slice(1);
    expect(reanswerTurns.every((call: any) => call.args.request.conversationId === turns[0].args.request.conversationId)).toBe(true);
    expect(reanswerTurns.every((call: any) => call.args.request.content === reanswerTurns[0].args.request.content)).toBe(true);
    expect(reanswerTurns[0].args.request.content).toContain(terminal === "rejected" ? "请输入安装目录。" : "游戏在哪个平台？");
    expect(reanswerTurns[0].args.request.content).toContain(terminal === "rejected" ? "D:/Games/Test" : "Windows PC");
    if (terminal !== "rejected") {
      expect(reanswerTurns[0].args.request.content).toContain("从哪里确认识别标记？");
      expect(reanswerTurns[0].args.request.content).toContain("参考本机文件");
    }
    expect(restarts).toEqual([]);
    expect(staleResponses).toHaveLength(terminal === "skipped" || terminal === "rejected" ? 1 : 0);

    await expect(summary).toBeVisible();
    if (await summary.getAttribute("aria-expanded") === "false") await summary.click();
    await expect(page.locator("dl dd")).toHaveText(
      terminal === "skipped" ? ["已跳过", "已跳过"] : terminal === "rejected" ? ["已跳过"] : ["未回答", "未回答"],
    );
    expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "respond_approval")))
      .toHaveLength(terminal === "skipped" || terminal === "rejected" ? 1 : 0);
    expect(await page.evaluate(() => (window as any).__AMM_TEST__.events
      .filter((event: any) => event.kind === "approval-resolved" && event.payload.approvalId === "approval-fixture")
      .map((event: any) => event.payload.status))).toEqual(oldApprovalBefore);
  });
}

test("取消历史问卷重新回答会恢复普通输入区和原草稿", async ({ page }) => {
  const { summary, questionTitle } = await startQuestionnaire(page, "cancelled");
  const composer = page.getByRole("textbox", { name: "告诉 Agent 你想做什么" });
  await expect(composer).toBeVisible();
  await composer.fill("取消后仍保留这段草稿");

  const reanswer = page.getByRole("button", { name: "重新回答", exact: true });
  await expect(reanswer).toBeVisible();
  await reanswer.click();
  const form = page.getByRole("region", { name: questionTitle });
  await expect(form).toBeVisible();
  await expect(composer).toHaveCount(0);
  await expect(form.getByRole("button", { name: "取消重新回答", exact: true })).toBeVisible();
  await form.getByRole("button", { name: "取消重新回答", exact: true }).click();

  await expect(form).toHaveCount(0);
  await expect(summary).toBeVisible();
  await expect(composer).toBeVisible();
  await expect(composer).toHaveValue("取消后仍保留这段草稿");
  const starts = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "start_turn"));
  expect(starts).toHaveLength(1);
});

test("已回答问卷作为历史记录只读，归档会话不能重新回答或发送消息", async ({ page }) => {
  await installMockTauri(page, { askUser: { preview } });
  await page.goto("/");
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await page.getByRole("button", { name: /^Agent( 对话)?$/ }).click();
  await page.getByRole("textbox", { name: "告诉 Agent 你想做什么" }).fill("帮我确认游戏适配信息");
  await page.getByRole("button", { name: "发送消息", exact: true }).click();
  const form = page.getByRole("region", { name: title });
  await expect(form).toBeVisible();
  await form.getByRole("radio", { name: "Windows PC" }).click();
  await form.getByRole("button", { name: "下一步", exact: true }).click();
  await form.getByRole("radio", { name: "参考已登记游戏" }).click();
  await form.getByRole("button", { name: "提交回答", exact: true }).click();
  await expect(page.getByText("回答已提交。")).toBeVisible();

  const conversationTitle = await page.evaluate(() => {
    const fixture = (window as any).__AMM_TEST__;
    const firstTurn = fixture.calls.find((call: any) => call.command === "start_turn");
    return fixture.conversations.find((conversation: any) => conversation.id === firstTurn.args.request.conversationId).title;
  });

  const summary = page.getByRole("button", { name: "已询问 2 个问题", exact: true });
  await summary.click();
  await expect(page.getByRole("button", { name: "重新回答", exact: true })).toHaveCount(0);
  await expect(page.locator("dl dd")).toHaveText(["Windows PC", "参考已登记游戏"]);

  await openHistoryPage(page);
  await page.getByRole("button", { name: `打开对话：${conversationTitle}`, exact: true }).click();
  await expect(page.getByRole("button", { name: "重新回答", exact: true })).toHaveCount(0);

  await openHistoryPage(page);
  await page.getByTitle(`归档：${conversationTitle}`).click();
  await page.getByRole("tab", { name: "已归档", exact: true }).click();
  await page.getByRole("button", { name: `打开对话：${conversationTitle}`, exact: true }).click();

  await expect(page.getByRole("textbox", { name: "告诉 Agent 你想做什么" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "重新回答", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "发送消息", exact: true })).toBeDisabled();
  const messageActions = page.getByRole("group", { name: "消息操作" });
  await expect(messageActions.getByRole("button", { name: "编辑消息", exact: true }).first()).toBeDisabled();
  await expect(messageActions.getByRole("button", { name: "重试", exact: true }).first()).toBeDisabled();
});
