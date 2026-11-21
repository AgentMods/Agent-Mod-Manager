import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

async function askAgent(page: Page) {
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: /^(Agent|Agent 对话)$/}).click();
  await page.getByRole("textbox", {name: "告诉 Agent 你想做什么"}).fill("帮我确认游戏适配信息");
  await page.getByRole("button", {name: "发送消息", exact: true}).click();
}

function responses(page: Page) {
  return page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "respond_approval"));
}

const title = "确认游戏适配信息";
const structured = {
  title,
  questions: [
    {question: "游戏在哪个平台？", options: ["Windows PC", "其他平台"]},
    {question: "从哪里确认识别标记？", options: ["参考已登记游戏", "参考本机文件"]},
    {question: "是否已核实 Mod 安装规则？"},
  ],
};

test("questionnaire walks steps, keeps draft answers, and submits ordered choices and skips", async ({page}, testInfo) => {
  const theme = testInfo.project.name === "desktop-1280" ? "light" : "dark";
  await page.emulateMedia({colorScheme: theme});
  await installMockTauri(page, {askUser: {preview: structured, deferResult: true}});
  await askAgent(page);
  await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
  const card = page.getByRole("region", {name: title});
  await expect(card).toBeVisible();
  await expect(page.getByRole("dialog", {name: "Agent 等待你的回答"})).toHaveCount(0);
  await expect(card.getByText("1 / 3", {exact: true})).toBeVisible();
  await expect(card.getByRole("button", {name: "下一步"})).toBeDisabled();
  await card.getByText("Windows PC", {exact: true}).click();
  await expect(card.getByRole("radio", {name: "Windows PC"})).toBeChecked();
  await card.getByRole("button", {name: "下一步"}).click();

  await expect(card.getByText("2 / 3", {exact: true})).toBeVisible();
  await card.getByRole("radio", {name: "自己填写"}).click();
  const input = card.getByRole("textbox", {name: "填写回答"});
  await input.fill("已验证 Game.exe");
  await card.getByRole("button", {name: "上一步"}).click();
  await expect(card.getByRole("radio", {name: "Windows PC"})).toBeChecked();
  await card.getByRole("button", {name: "下一步"}).click();
  await expect(input).toHaveValue("已验证 Game.exe");
  await card.getByRole("button", {name: "下一步"}).click();

  await expect(card.getByText("3 / 3", {exact: true})).toBeVisible();
  await expect(input).toBeVisible();
  expect(await responses(page)).toEqual([]);
  await card.getByRole("button", {name: "跳过", exact: true}).click();
  await expect.poll(() => responses(page)).toEqual([expect.objectContaining({args: expect.objectContaining({
    response: {approved: true, answer: JSON.stringify(["Windows PC", "已验证 Game.exe", null])},
  })})]);

  const trace = page.getByRole("button", {name: "已询问 3 个问题", exact: true});
  const details = page.locator("dl").filter({hasText: structured.questions[0].question});
  await expect(trace).toHaveAttribute("aria-expanded", "false");
  await expect(details).not.toBeVisible();
  await trace.focus();
  await trace.press("Enter");
  await expect(trace).toHaveAttribute("aria-expanded", "true");
  await expect(details.locator("dt")).toHaveText(structured.questions.map(({question}) => question));
  await expect(details.locator("dd")).toHaveText(["Windows PC", "已验证 Game.exe", "已跳过"]);

  await page.evaluate(() => (window as any).__AMM_TEST__.publishQuestionResult());
  await expect(page.getByText("回答已提交。")).toBeVisible();
  await expect(trace).toHaveAttribute("aria-expanded", "true");
  await page.reload();
  const conversation = page.getByRole("button", {name: /^(新对话|帮我确认游戏适配信息)$/});
  await conversation.click();
  await expect(trace).toHaveAttribute("aria-expanded", "false");
  await trace.click();
  await expect(details.locator("dt")).toHaveText(structured.questions.map(({question}) => question));
  await expect(details.locator("dd")).toHaveText(["Windows PC", "已验证 Game.exe", "已跳过"]);
  await expect(card).toHaveCount(0);
  await expect(details).toBeInViewport({ratio: 0.95});
});

test("legacy question uses a single text step and submits a plain answer", async ({page}) => {
  await installMockTauri(page, {askUser: {preview: {question: "请输入安装目录。"}}});
  await askAgent(page);
  const card = page.getByRole("region", {name: "Agent 等待你的回答"});
  await expect(card.getByText("1 / 1", {exact: true})).toBeVisible();
  await expect(card.getByRole("radio")).toHaveCount(0);
  await card.getByRole("textbox", {name: "填写回答"}).fill("D:/Games/Test");
  await card.getByRole("button", {name: "提交回答"}).click();
  const trace = page.getByRole("button", {name: "已询问 1 个问题", exact: true});
  await expect(trace).toHaveAttribute("aria-expanded", "false");
  await trace.click();
  await expect(page.locator("dt")).toHaveText("请输入安装目录。");
  await expect(page.locator("dd")).toHaveText("D:/Games/Test");
  await expect.poll(() => responses(page)).toEqual([expect.objectContaining({args: expect.objectContaining({
    response: {approved: true, answer: "D:/Games/Test"},
  })})]);
});

test("live questionnaire occupies the bottom input area and stays with its conversation", async ({page}) => {
  const longOptions = Array.from({length: 5}, (_, i) => `${i + 1}. ${"其他平台及其支持的游戏版本。".repeat(12)}`);
  const questions = [{...structured.questions[0], options: ["Windows PC", ...longOptions]}, ...structured.questions.slice(1)];
  await installMockTauri(page, {askUser: {preview: {title, questions}}});
  await askAgent(page);

  const form = page.getByRole("region", {name: title});
  const composer = page.getByRole("textbox", {name: "告诉 Agent 你想做什么"});
  await expect(form).toBeVisible();
  await expect(composer).toHaveCount(0);
  await expect(page.getByRole("button", {name: "停止生成", exact: true})).toBeVisible();
  await expect(form.getByRole("radio", {name: "Windows PC"})).toBeVisible();
  await expect(form.getByRole("button", {name: "下一步", exact: true})).toBeDisabled();
  await expect(form.getByRole("button", {name: "跳过", exact: true})).toBeVisible();

  const viewport = page.viewportSize();
  const bounds = await form.boundingBox();
  expect(viewport).not.toBeNull();
  expect(bounds).not.toBeNull();
  expect(bounds!.x).toBeGreaterThanOrEqual(0);
  expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(viewport!.width);
  await expect(form.getByRole("button", {name: "跳过", exact: true})).toBeInViewport();
  await expect(form.getByRole("button", {name: "下一步", exact: true})).toBeInViewport();

  const nextButton = form.getByRole("button", {name: "下一步", exact: true});
  const beforeScroll = await nextButton.boundingBox();
  await form.getByRole("radio", {name: "自己填写"}).scrollIntoViewIfNeeded();
  expect(await nextButton.boundingBox()).toEqual(beforeScroll);

  const conversationTitle = await page.evaluate(() => {
    const fixture = (window as any).__AMM_TEST__;
    const request = fixture.calls.find((call: any) => call.command === "start_turn");
    return fixture.conversations.find((conversation: any) => conversation.id === request.args.request.conversationId).title;
  });
  await page.getByRole("button", {name: "新建对话", exact: true}).click();
  await expect(form).toHaveCount(0);
  await expect(composer).toBeVisible();
  await page.getByRole("button", {name: conversationTitle, exact: true}).click();
  await expect(form).toBeVisible();
  await expect(form.getByText("1 / 3", {exact: true})).toBeVisible();
  await expect(composer).toHaveCount(0);
});

test("cancelling a turn disables its unfinished questionnaire", async ({page}) => {
  await installMockTauri(page, {askUser: {preview: structured}});
  await askAgent(page);
  const card = page.getByRole("region", {name: title});
  await expect(card.getByRole("button", {name: "跳过"})).toBeVisible();
  await page.getByRole("button", {name: "停止生成"}).click();
  const trace = page.getByRole("button", {name: /已询问 3 个问题.*已取消/});
  await expect(page.getByRole("textbox", {name: "告诉 Agent 你想做什么"})).toBeVisible();
  await expect(trace).toHaveAttribute("aria-expanded", "false");
  await trace.click();
  await expect(page.locator("dd")).toHaveText(["未回答", "未回答", "未回答"]);
  await expect(card).toHaveCount(0);
  expect(await responses(page)).toEqual([]);
});
