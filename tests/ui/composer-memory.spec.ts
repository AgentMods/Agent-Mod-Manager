import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const settings = {
  base_url: "https://provider.example/v1",
  model: "fixture-thinking",
  permission_mode: "request_approval",
  api_key_ref: "model.api_key",
  onboarding_completed: true,
  models: [
    {display_name: "fixture-thinking", id: "fixture-thinking", reasoning_levels: ["low", "high", "ultra"]},
    {display_name: "fixture-standard", id: "fixture-standard", reasoning_levels: []},
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


for (const explicit of [false, true]) {
  test(`retry waits for preferences and recovers${explicit ? " with explicit selection" : " with remembered policy"}`, async ({page}) => {
    await installMockTauri(page);
    await page.addInitScript(() => {
      const target = window as any;
      const invoke = target.__TAURI_INTERNALS__.invoke;
      target.preferenceReads = 0;
      target.preferenceState = "delay";
      target.__TAURI_INTERNALS__.invoke = async (command: string, args: any = {}) => {
        if (command === "get_composer_preferences") {
          target.preferenceReads++;
          await new Promise<void>((resolve, reject) => {
            target.releasePreferences = () => target.preferenceState === "fail" ? reject(new Error("preference read failed")) : resolve();
            if (target.preferenceState !== "delay") target.releasePreferences();
          });
          return {model: null, reasoning_effort: null, permission_mode: "auto_approve"};
        }
        if (command === "get_conversation") {
          const detail = await invoke(command, args);
          return {...detail, messages: [
            {id: "retry-user", role: "user", content: "检查模组", reasoning: null, tool_calls: null, tool_call_id: null},
            {id: "retry-assistant", role: "assistant", content: "历史回复", reasoning: null, tool_calls: null, tool_call_id: null},
          ]};
        }
        if (command === "restart_turn") {
          target.__AMM_TEST__.calls.push({command, args});
          return "fixture-retry";
        }
        return invoke(command, args);
      };
    });
    await page.goto("/");
    await page.getByRole("button", {name: "检查 Minecraft 模组", exact: true}).click();
    const retry = page.getByRole("button", {name: "重试", exact: true}).first();
    await retry.click();
    await expect.poll(() => page.evaluate(() => (window as any).preferenceReads)).toBe(1);
    expect(await calls(page, "start_turn")).toHaveLength(0);
    expect(await calls(page, "restart_turn")).toHaveLength(0);
    await page.evaluate(() => { const t = window as any; t.preferenceState = "fail"; t.releasePreferences(); });
    await expect(page.getByText("preference read failed", {exact: false})).toBeVisible();
    expect(await calls(page, "restart_turn")).toHaveLength(0);
    if (explicit) {
      await page.getByRole("button", {name: "本轮权限策略", exact: true}).click();
      await page.getByRole("menuitemradio", {name: "逐次审批", exact: true}).click();
    } else {
      await page.evaluate(() => { (window as any).preferenceState = "success"; });
    }
    await retry.click();
    await expect.poll(async () => (await calls(page, "restart_turn")).length).toBe(1);
    expect((await calls(page, "restart_turn"))[0].args.request.permissionMode).toBe(explicit ? "request_approval" : "auto_approve");
    expect(await calls(page, "start_turn")).toHaveLength(0);
    if (explicit) expect(await page.evaluate(() => (window as any).preferenceReads)).toBe(1);
  });
}

for (const pending of ["get_conversation", "get_settings"]) {
  test(`inherited permission stays neutral while ${pending} is pending`, async ({page}) => {
    await installMockTauri(page);
    await page.addInitScript((pending) => {
      const t = window as any;
      const invoke = t.__TAURI_INTERNALS__.invoke;
      t.__TAURI_INTERNALS__.invoke = async (command: string, args: any = {}) => {
        if (command === "get_composer_preferences") return {model: null, reasoning_effort: null, permission_mode: null};
        if (command === pending) await new Promise<void>(resolve => {t.releasePermissionSource = resolve;});
        const result = await invoke(command, args);
        if (command === "get_settings") return {...result, permission_mode: "full_trust"};
        if (command === "get_conversation") return {...result, conversation: {...result.conversation, permission_mode: null}};
        return result;
      };
    }, pending);
    await page.goto("/");
    await page.getByRole("button", {name: "检查 Minecraft 模组", exact: true}).click();
    const permission = page.getByRole("button", {name: "本轮权限策略", exact: true});
    await expect(permission).toContainText("会话设置 · 读取中");
    await expect(permission).not.toContainText("逐次审批");
    await expect(permission).not.toContainText("完全信任");
    await expect(permission).not.toContainText("权限未记录");
    await page.evaluate(() => (window as any).releasePermissionSource());
    await expect(permission).toContainText("完全信任");
  });
}
