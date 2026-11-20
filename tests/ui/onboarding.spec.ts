import { expect, test } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

test.describe("首次启动引导", () => {
  test("扫描候选不会自动添加，失败候选不影响其他候选", async ({page}) => {
    await installMockTauri(page, {onboarding: true, failRegistration: true});
    await page.goto("/");
    await expect(page.getByRole("heading", {name: "先找到你想管理的游戏"})).toBeVisible();
    await expect(page.getByRole("navigation", {name: "主导航"})).toHaveCount(0);

    await page.getByRole("button", {name: "扫描已安装游戏"}).click();
    const fixture = page.getByRole("listitem").filter({hasText: "Fixture Game"});
    const broken = page.getByRole("listitem").filter({hasText: "Broken Fixture"});
    await expect(fixture).toContainText("/sandbox/steam/Fixture");
    await expect(fixture).toContainText("92% 匹配");
    await expect(fixture).toContainText("这是一个测试扫描候选，请确认目录后再添加");
    await expect(page.getByText("已检查 3 个目录、18 个条目")).toBeVisible();

    const registrations = () => page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "register_game"));
    expect(await registrations()).toHaveLength(0);
    await expect(fixture.getByRole("button", {name: "添加"})).toBeDisabled();
    await broken.getByRole("checkbox", {name: "选择 Broken Fixture"}).check();
    await broken.getByRole("button", {name: "添加"}).click();
    await expect(broken.getByRole("alert")).toContainText("目录已变化，请重新扫描");
    await expect(fixture).toBeVisible();

    await fixture.getByRole("checkbox", {name: "选择 Fixture Game"}).check();
    await fixture.getByRole("button", {name: "添加"}).click();
    await expect(fixture.getByText("管理中", {exact: true})).toBeVisible();
    await expect(broken.getByRole("button", {name: "添加"})).toBeEnabled();
    const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls.map((call: any) => call.command));
    expect(calls.filter((command: string) => command === "start_onboarding")).toHaveLength(1);
    expect((await registrations()).map((call: any) => call.args.directory)).toEqual(["/sandbox/steam/Broken", "/sandbox/steam/Fixture"]);
  });

  test("配置 Agent 时密钥走 set_secret，完成导览后进入游戏库", async ({page}) => {
    await installMockTauri(page, {onboarding: true});
    await page.goto("/");
    await page.getByRole("button", {name: "稍后再扫"}).click();
    await expect(page.getByRole("heading", {name: "配置你的 AI Agent"})).toBeVisible();
    await expect(page.getByRole("link", {name: "没有AI？从这里获取"})).toHaveAttribute("href", "https://one.gloscai.com/keys");
    await page.getByLabel("API 地址", {exact: true}).fill("https://provider.example/v1");
    await page.getByLabel("API 密钥").fill("sk-test-secret-never-logged");
    await page.getByRole("button", {name: "获取模型列表"}).click();
    await page.getByRole("checkbox", {name: "fixture-thinking"}).check();
    await page.getByRole("checkbox", {name: "fixture-standard"}).check();
    await page.getByRole("button", {name: "添加所选（2）"}).click();
    await expect(page.getByLabel("fixture-thinking 上下文预算")).toHaveValue("128000");
    await expect(page.getByLabel("fixture-standard 上下文预算")).toHaveValue("64000");
    await expect(page.getByRole("button", {name: "fixture-standard 思考等级"})).toHaveText("服务默认");
    await page.getByRole("button", {name: "fixture-thinking 思考等级"}).click();
    const thinking = page.getByRole("menu");
    await expect(thinking.getByRole("menuitemcheckbox", {name: "低（low）"})).toBeChecked();
    await expect(thinking.getByRole("menuitemcheckbox", {name: "高（high）"})).toBeChecked();
    await thinking.getByRole("menuitemcheckbox", {name: "中（medium）"}).click();
    await expect(thinking.getByRole("menuitemcheckbox", {name: "中（medium）"})).not.toBeChecked();
    await page.keyboard.press("Escape");
    await page.getByRole("combobox", {name: "默认模型", exact: true}).selectOption("fixture-thinking");
    await page.getByRole("button", {name: "保存并继续"}).click();
    await expect(page.getByRole("heading", {name: "认识 Agent Mod Manager"})).toBeVisible();
    await expect(page.getByRole("tab", {name: "Mod 发现", exact: true})).toHaveCount(0);
    await page.getByRole("tab", {name: "Agent 对话", exact: true}).click();
    await expect(page.getByRole("tabpanel")).toContainText("搜索、下载 Mod");

    const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
    const secret = calls.find((call: any) => call.command === "set_secret");
    const settings = calls.find((call: any) => call.command === "set_settings");
    expect(secret?.args.value).toBe("sk-test-secret-never-logged");
    expect(JSON.stringify(settings?.args)).not.toContain("sk-test-secret-never-logged");
    expect(calls.find((call: any) => call.command === "list_agent_models")?.args).toEqual({baseUrl: "https://provider.example/v1", authenticated: true});
    expect(calls.find((call: any) => call.command === "get_agent_model_metadata")?.args).toEqual({baseUrl: "https://provider.example/v1", modelIds: ["fixture-standard", "fixture-thinking"]});
    expect(settings?.args.settings.model).toBe("fixture-thinking");
    expect(settings?.args.settings.models).toEqual([
      {display_name: "fixture-thinking", id: "fixture-thinking", context_budget: 128000, reasoning_levels: ["low", "high"]},
      {display_name: "fixture-standard", id: "fixture-standard", context_budget: 64000, reasoning_levels: []},
    ]);

    await page.getByRole("button", {name: "完成并进入游戏库"}).click();
    await expect(page.getByTestId("library-page")).toBeVisible();
    await expect(page.getByRole("navigation", {name: "主导航"})).toBeVisible();
    const completed = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "set_onboarding_completed"));
    expect(completed.at(-1)?.args.completed).toBe(true);
  });

  test("Agent composer supports per-turn model and reasoning controls", async ({page}) => {
    const configuredSettings = {
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
    await installMockTauri(page);
    await page.addInitScript((settings) => {
      const target = window as any;
      const originalInvoke = target.__TAURI_INTERNALS__.invoke;
      let seeded = false;
      target.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, unknown> = {}) => {
        if (command === "get_settings") {
          if (!seeded) {
            seeded = true;
            await originalInvoke("set_settings", {settings});
          }
          return structuredClone(settings);
        }
        return originalInvoke(command, args);
      };
    }, configuredSettings);
    await page.goto("/");
    await expect(page.getByTestId("library-page")).toBeVisible();
    await page.getByRole("button", {name: /^Agent( 对话)?$/}).click();

    const composer = page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true});
    const send = page.getByRole("button", {name: "发送消息", exact: true});
    const newConversation = async () => {
      await page.getByRole("button", {name: "新建对话", exact: true}).click();
      await expect(composer).toHaveValue("");
      await expect(send).toBeVisible();
    };
    const modelTrigger = page.getByRole("button", {name: "本轮模型", exact: true});
    const startTurns = () => page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "start_turn"));
    const finishCurrentTurn = async () => {
      const approval = page.getByRole("dialog", {name: "操作审批", exact: true});
      await expect(approval).toBeVisible();
      await approval.getByRole("button", {name: "拒绝", exact: true}).click();
      await expect(approval).toBeHidden();
      await expect(send).toBeVisible();
    };

    await expect(composer).toBeVisible();
    await expect(send).toBeVisible();
    await expect(modelTrigger).toContainText("fixture-thinking");
    await expect(page.getByRole("button", {name: "本轮思考等级", exact: true})).toBeVisible();

    await modelTrigger.click();
    const initialModelMenu = page.getByRole("menu");
    await expect(initialModelMenu).toHaveAttribute("data-side", "top");
    await expect(initialModelMenu.getByRole("menuitemradio")).toHaveCount(2);
    await expect(initialModelMenu.getByRole("menuitemradio", {name: "fixture-thinking", exact: true})).toHaveAttribute("aria-checked", "true");
    await expect(initialModelMenu.getByRole("menuitemradio", {name: "fixture-standard", exact: true})).toHaveAttribute("aria-checked", "false");
    await page.keyboard.press("Escape");
    await expect(initialModelMenu).toBeHidden();

    await composer.fill("默认请求");
    await send.click();
    await expect.poll(async () => (await startTurns()).length).toBe(1);
    const untouchedRequest = (await startTurns())[0].args.request;
    expect(untouchedRequest.model ?? null).toBe(null);
    expect(untouchedRequest.reasoningEffort).toBe(null);
    await finishCurrentTurn();
    await newConversation();

    const reasoningTrigger = page.getByRole("button", {name: "本轮思考等级", exact: true});
    await reasoningTrigger.click();
    const reasoningDialog = page.getByRole("dialog", {name: "本轮思考等级", exact: true});
    const slider = reasoningDialog.getByRole("slider", {name: "推理强度", exact: true});
    await expect(reasoningDialog).toBeVisible();
    await expect(slider).toHaveAttribute("aria-valuetext", "服务默认");
    await slider.press("ArrowRight");
    await expect(slider).toHaveAttribute("aria-valuetext", "低（low）");
    await slider.press("Home");
    await expect(slider).toHaveAttribute("aria-valuetext", "服务默认");
    await page.emulateMedia({reducedMotion: "no-preference"});
    await slider.press("End");
    await expect(slider).toHaveAttribute("aria-valuetext", "超高（ultra）");
    await expect(reasoningDialog.getByText("Ultra", {exact: true})).toBeVisible();
    const ultraRange = reasoningDialog.locator(".agent-ultra-range");
    const ultraAnimations = () => ultraRange.evaluate((range) => [
      getComputedStyle(range, "::before").animationName,
      getComputedStyle(range, "::after").animationName,
    ]);
    await expect.poll(ultraAnimations).toEqual(["agent-ultra-flow", "agent-ultra-stars"]);
    await page.emulateMedia({reducedMotion: "reduce"});
    await expect.poll(ultraAnimations).toEqual(["none", "none"]);
    await expect(ultraRange).toHaveCSS("background-image", /linear-gradient/);
    await slider.press("ArrowLeft");
    await expect(slider).toHaveAttribute("aria-valuetext", "高（high）");
    await expect(ultraRange).toHaveCount(0);
    await slider.press("End");
    await page.keyboard.press("Escape");
    await expect(reasoningDialog).toBeHidden();

    await composer.fill("Ultra 等级请求");
    await send.click();
    await expect.poll(async () => (await startTurns()).length).toBe(2);
    const ultraRequest = (await startTurns())[1].args.request;
    expect(ultraRequest.model ?? null).toBe(null);
    expect(ultraRequest.reasoningEffort).toBe("ultra");
    await finishCurrentTurn();
    await newConversation();

    await modelTrigger.click();
    const switchedModelMenu = page.getByRole("menu");
    await switchedModelMenu.getByRole("menuitemradio", {name: "fixture-standard", exact: true}).click();
    await expect(modelTrigger).toContainText("fixture-standard");
    await expect(page.getByRole("button", {name: "本轮思考等级", exact: true})).toHaveCount(0);

    await composer.fill("标准模型请求");
    await send.click();
    await expect.poll(async () => (await startTurns()).length).toBe(3);
    const standardRequest = (await startTurns())[2].args.request;
    expect(standardRequest.model).toBe("fixture-standard");
    expect(standardRequest.reasoningEffort).toBe(null);
    await finishCurrentTurn();

    const settingsCalls = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "set_settings"));
    expect(settingsCalls).toHaveLength(1);
    expect(settingsCalls[0].args.settings.model).toBe("fixture-thinking");
  });

  test("BaseLLM 不可用时仍可手动添加模型和填写上下文", async ({page}) => {
    await installMockTauri(page, {onboarding: true});
    await page.goto("/");
    await page.getByRole("button", {name: "稍后再扫"}).click();
    await page.getByLabel("API 地址", {exact: true}).fill("https://offline.example/v1");
    await page.getByRole("button", {name: "手动添加", exact: true}).click();
    await page.getByLabel("新模型 实际请求模型", {exact: true}).fill("local-model");
    await page.getByLabel("local-model 实际请求模型").press("Tab");
    await expect(page.getByText(/暂时无法读取 BaseLLM/)).toBeVisible();
    await page.getByLabel("local-model 上下文预算").fill("20480");
    await page.getByRole("button", {name: "保存并继续"}).click();
    await expect(page.getByRole("heading", {name: "认识 Agent Mod Manager"})).toBeVisible();
    const models = await page.evaluate(() => (window as any).__AMM_TEST__.calls.find((call: any) => call.command === "set_settings").args.settings.models);
    expect(models).toEqual([{display_name: "local-model", id: "local-model", context_budget: 20480, reasoning_levels: []}]);
  });

  test("退出引导会持久化完成状态并解除阻断", async ({page}) => {
    await installMockTauri(page, {onboarding: true});
    await page.goto("/");
    await page.getByRole("button", {name: "退出引导"}).click();
    await expect(page.getByTestId("library-page")).toBeVisible();
    const completed = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "set_onboarding_completed"));
    expect(completed.at(-1)?.args.completed).toBe(true);
  });

  test("添加后中途退出会恢复引导，已有游戏的旧用户不会被阻断", async ({page}) => {
    await installMockTauri(page, {onboarding: true, onboardingStarted: true});
    await page.goto("/");
    await expect(page.getByRole("heading", {name: "先找到你想管理的游戏"})).toBeVisible();
    await expect(page.getByRole("navigation", {name: "主导航"})).toHaveCount(0);
  });

  test("已完成引导的用户可以从设置重新查看引导", async ({page}) => {
    await installMockTauri(page);
    await page.goto("/");
    await expect(page.getByTestId("library-page")).toBeVisible();
    await page.getByRole("button", {name: "设置", exact: true}).click();
    await page.getByRole("tab", {name: "外观与存储"}).click();
    await page.getByRole("button", {name: "重新查看新手引导"}).click();
    await expect(page.getByRole("heading", {name: "先找到你想管理的游戏"})).toBeVisible();
    await page.getByRole("button", {name: "退出引导"}).click();
    await expect(page.getByTestId("library-page")).toBeVisible();
  });

  test("引导页在亮色和暗色主题下没有横向溢出", async ({page}, info) => {
    await installMockTauri(page, {onboarding: true});
    await page.goto("/");
    for (const theme of ["light", "dark"]) {
      if (await page.locator("html").getAttribute("data-theme") !== theme) {
        await page.getByRole("button", {name: theme === "dark" ? "切换暗色主题" : "切换亮色主题"}).click();
      }
      await page.getByRole("button", {name: /扫描已安装游戏|重新扫描/}).click();
      await expect(page.getByText("Fixture Game", {exact: true})).toBeVisible();
      for (const name of ["稍后再扫", "继续配置 Agent"]) {
        const next = page.getByRole("button", {name});
        if (await next.count()) { await next.click(); break; }
      }
      await expect(page.getByRole("heading", {name: "配置你的 AI Agent"})).toBeVisible();
      await page.getByRole("button", {name: "跳过配置"}).click();
      await expect(page.getByRole("tab", {name: "设置"})).toBeVisible();
      await page.getByRole("tab", {name: "设置"}).click();
      await expect(page.getByText(/外部 stdio MCP 进程拥有当前用户的系统权限/)).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1), `${theme}/${info.project.name}`).toBe(false);
      await page.screenshot({path: `output/playwright/screenshots/onboarding-${info.project.name}-${theme}.png`, fullPage: true});
      await page.getByRole("button", {name: "上一步"}).click();
      await page.getByRole("button", {name: "上一步"}).click();
    }
  });
});
