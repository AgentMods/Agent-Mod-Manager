import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const settings = {
  base_url: "https://provider.example/v1",
  model: "fixture-thinking",
  context_budget: 128000,
  permission_mode: "request_approval",
  api_key_ref: "model.api_key",
  onboarding_completed: true,
  models: [
    {display_name: "fixture-thinking", id: "fixture-thinking", context_budget: 128000, reasoning_levels: ["low", "high", "ultra"]},
    {display_name: "fixture-standard", id: "fixture-standard", context_budget: 64000, reasoning_levels: []},
  ],
};

async function open(page: Page, remembered: Record<string, unknown> | null) {
  await installMockTauri(page);
  await page.addInitScript(({settings, remembered}) => {
    const target = window as any;
    const originalInvoke = target.__TAURI_INTERNALS__.invoke;
    target.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      if (command === "get_settings") return structuredClone(settings);
      if (command === "get_composer_preferences" && remembered) return structuredClone(remembered);
      return originalInvoke(command, args);
    };
  }, {settings, remembered});
  await page.goto("/");
  await page.getByRole("button", {name: "Agent", exact: true}).click();
}

const calls = (page: Page, command: string) =>
  page.evaluate((name) => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === name), command);

test("发送后记住本轮模型、思考等级和权限策略", async ({page}) => {
  await open(page, null);
  await page.getByRole("button", {name: "本轮权限策略", exact: true}).click();
  await page.getByRole("menuitemradio", {name: "完全信任", exact: true}).click();
  await page.getByRole("button", {name: "本轮思考等级", exact: true}).click();
  const slider = page.getByRole("dialog", {name: "本轮思考等级", exact: true}).getByRole("slider", {name: "推理强度", exact: true});
  await slider.press("End");
  await page.keyboard.press("Escape");

  await page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true}).fill("记住我的选择");
  await page.getByRole("button", {name: "发送消息", exact: true}).click();

  await expect.poll(async () => (await calls(page, "set_composer_preferences")).length).toBe(1);
  expect((await calls(page, "set_composer_preferences"))[0].args.preferences).toEqual({
    model: "fixture-thinking", reasoning_effort: "ultra", permission_mode: "full_trust",
  });
});

test("重新打开应用后恢复上次的模型、思考等级和权限策略", async ({page}) => {
  await open(page, {model: "fixture-thinking", reasoning_effort: "high", permission_mode: "auto_approve"});
  await expect(page.getByRole("button", {name: "本轮权限策略", exact: true})).toContainText("游戏内自动批准");
  await expect(page.getByRole("button", {name: "本轮模型", exact: true})).toContainText("fixture-thinking");

  await page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true}).fill("沿用上次选择");
  await page.getByRole("button", {name: "发送消息", exact: true}).click();
  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(1);
  const request = (await calls(page, "start_turn"))[0].args.request;
  expect(request.permissionMode).toBe("auto_approve");
  expect(request.reasoningEffort).toBe("high");
});

test("已删除的模型或思考等级不会被恢复", async ({page}) => {
  await open(page, {model: "removed-model", reasoning_effort: "xhigh", permission_mode: null});
  await expect(page.getByRole("button", {name: "本轮模型", exact: true})).toContainText("fixture-thinking");
  await page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true}).fill("回退到默认");
  await page.getByRole("button", {name: "发送消息", exact: true}).click();
  await expect.poll(async () => (await calls(page, "start_turn")).length).toBe(1);
  const request = (await calls(page, "start_turn"))[0].args.request;
  expect(request.model ?? null).toBe(null);
  expect(request.reasoningEffort).toBe(null);
});

