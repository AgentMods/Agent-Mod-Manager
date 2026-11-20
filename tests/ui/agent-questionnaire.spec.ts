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

test("questionnaire walks steps, keeps draft answers, and submits ordered choices and skips", async ({page}) => {
  await installMockTauri(page, {askUser: {preview: structured}});
  await askAgent(page);
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
  await expect(card.getByRole("status")).toHaveText("已提交回答");
  await expect.poll(() => responses(page)).toEqual([expect.objectContaining({args: expect.objectContaining({
    response: {approved: true, answer: JSON.stringify(["Windows PC", "已验证 Game.exe", null])},
  })})]);

  const conversation = page.getByRole("button", {name: /^(新对话|帮我确认游戏适配信息)$/});
  await page.getByRole("button", {name: "新建对话", exact: true}).click();
  await expect(card).toHaveCount(0);
  await conversation.click();
  await expect(card.getByRole("status")).toHaveText("已提交回答");
  await expect(card.getByRole("radio")).toHaveCount(0);
});

test("legacy question uses a single inline text step and submits a plain answer", async ({page}) => {
  await installMockTauri(page, {askUser: {preview: {question: "请输入安装目录。"}}});
  await askAgent(page);
  const card = page.getByRole("region", {name: "Agent 等待你的回答"});
  await expect(card.getByText("1 / 1", {exact: true})).toBeVisible();
  await expect(card.getByRole("radio")).toHaveCount(0);
  await card.getByRole("textbox", {name: "填写回答"}).fill("D:/Games/Test");
  await card.getByRole("button", {name: "提交回答"}).click();
  await expect(card.getByRole("status")).toHaveText("已提交回答");
  await expect.poll(() => responses(page)).toEqual([expect.objectContaining({args: expect.objectContaining({
    response: {approved: true, answer: "D:/Games/Test"},
  })})]);
});

test("cancelling a turn disables its unfinished questionnaire", async ({page}) => {
  await installMockTauri(page, {askUser: {preview: structured}});
  await askAgent(page);
  const card = page.getByRole("region", {name: title});
  await expect(card.getByRole("button", {name: "跳过"})).toBeVisible();
  await page.getByRole("button", {name: "停止生成"}).click();
  await expect(card.getByRole("status")).toHaveText(/已结束|已取消/);
  await expect(card.getByRole("button", {name: "跳过"})).toHaveCount(0);
  expect(await responses(page)).toEqual([]);
});
