import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const onboardingHttpConfig = {
  id: "fixture-http",
  name: "Remote HTTP",
  transport: "streamable_http" as const,
  command: null,
  args: [],
  url: "https://mcp.example.test",
  env_secret_refs: {},
  bearer_secret_ref: null,
  enabled: false,
  builtin: false,
};

const onboardingStdioConfig = {
  id: "fixture-stdio",
  name: "Local Stdio",
  transport: "stdio" as const,
  command: "fixture-mcp",
  args: ["--stdio"],
  url: null,
  env_secret_refs: {},
  bearer_secret_ref: null,
  enabled: true,
  builtin: false,
};

async function openOnboardingWithMcp(page: Page, {active = []}: {active?: string[]} = {}) {
  await installMockTauri(page, {onboarding: true});
  await page.goto("/");
  await page.evaluate(({configs, active}) => {
    const internals = (window as any).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    const state = {configs: structuredClone(configs), active: [...active] as string[], calls: [] as Array<{command: string; args: any}>};
    (window as any).__ONBOARDING_MCP__ = state;
    internals.invoke = async (command: string, args: any = {}, ...rest: unknown[]) => {
      if (command === "mcp_list") return structuredClone(state.configs);
      if (command === "mcp_startup_requests") return [];
      if (command === "mcp_connections") return state.active.map((serverId) => ({serverId, tools: ["fixture_search"]}));
      if (command === "mcp_update") {
        state.calls.push({command, args: structuredClone(args)});
        state.configs = state.configs.map((config: any) => config.id === args.config.id ? args.config : config);
        return null;
      }
      if (command === "mcp_connect") {
        state.calls.push({command, args: structuredClone(args)});
        const serverId = args.expectedConfig.id;
        if (!state.active.includes(serverId)) state.active.push(serverId);
        return {serverId, tools: ["fixture_search"]};
      }
      if (command === "mcp_disconnect") {
        state.calls.push({command, args: structuredClone(args)});
        state.active = state.active.filter((serverId) => serverId !== args.id);
        return null;
      }
      return invoke(command, args, ...rest);
    };
  }, {configs: [onboardingHttpConfig, onboardingStdioConfig], active});
}

const onboardingMcpCalls = (page: Page) => page.evaluate(() => (window as any).__ONBOARDING_MCP__.calls);
const onboardingMcpRow = (page: Page, name: string) => page.getByRole("list", {name: "外部 MCP 连接", exact: true})
  .getByRole("listitem").filter({hasText: name});

const onboardingSteps = [
  "设置数据存储目录",
  "配置 AI 模型服务",
  "启用可用工具",
  "扫描游戏列表",
  "完成",
] as const;

async function expectOnboardingStep(page: Page, index: number) {
  await expect(page.getByRole("heading", {name: onboardingSteps[index], exact: true})).toBeVisible();
}

async function continueToModelSetup(page: Page) {
  await page.getByRole("button", {name: "下一步", exact: true}).click();
  await expectOnboardingStep(page, 1);
}

async function skipModelSetup(page: Page) {
  await page.getByRole("button", {name: "跳过配置", exact: true}).click();
  await expectOnboardingStep(page, 2);
}

async function continueToGameScan(page: Page) {
  await continueToModelSetup(page);
  await skipModelSetup(page);
  await page.getByRole("button", {name: "下一步", exact: true}).click();
  await expectOnboardingStep(page, 3);
}

const migrationCurrentRoot = "/sandbox/data";
const migrationParent = String.raw`D:\应用数据`;
const migrationNewRoot = migrationParent + String.raw`\Agent Mod Manager`;
const migrationFailureMessage = "所选位置已包含 Agent Mod Manager 文件夹";

async function installOnboardingMigrationMock(page: Page, {
  nextDataRoot = migrationCurrentRoot,
  nextMigrationError = null,
  rejectMigration = false,
  hold = false,
  onboarding = true,
}: {nextDataRoot?: string; nextMigrationError?: string | null; rejectMigration?: boolean; hold?: boolean; onboarding?: boolean}) {
  await installMockTauri(page, {
    onboarding,
    storage: {
      dataRoot: migrationCurrentRoot,
      parent: migrationParent,
      rejectFirstMigration: rejectMigration,
      holdMigration: hold,
      nextBootstrap: {dataRoot: nextDataRoot, storageMigrationError: nextMigrationError},
    },
  });
}

async function chooseAndConfirmStorageMigration(page: Page) {
  await page.getByRole("button", {name: "更改存储目录", exact: true}).click();
  const dialog = page.getByRole("alertdialog", {name: "迁移存储目录", exact: true});
  await expect(dialog.getByText(migrationParent + String.raw`\Agent Mod Manager`, {exact: true})).toBeVisible();
  await dialog.getByRole("button", {name: "迁移并重启", exact: true}).click();
  return dialog;
}

test.describe("首次启动引导", () => {
  test("五步标题、桌面标签和前后往返一致", async ({page}, info) => {
    await installMockTauri(page, {onboarding: true});
    await page.goto("/");

    const expectCurrent = async (index: number) => {
      await expectOnboardingStep(page, index);
      if (info.project.name === "narrow-390") {
        await expect(page.getByText(`步骤 ${index + 1} / 5`, {exact: true})).toBeVisible();
      }
      if (info.project.name.startsWith("desktop-")) {
        const steps = page.getByRole("list", {name: "引导步骤", exact: true});
        const labels = steps.getByRole("listitem");
        await expect(labels).toHaveCount(onboardingSteps.length);
        for (const [labelIndex, label] of onboardingSteps.entries()) {
          await expect(labels.nth(labelIndex)).toContainText(label);
        }
        await expect(steps.getByRole("listitem").nth(index)).toHaveAttribute("aria-current", "step");
      }
    };

    await expectCurrent(0);
    await continueToModelSetup(page);
    await expectCurrent(1);
    await skipModelSetup(page);
    await expectCurrent(2);
    await page.getByRole("button", {name: "下一步", exact: true}).click();
    await expectCurrent(3);
    await page.getByRole("button", {name: "扫描已安装游戏", exact: true}).click();
    await expect(page.getByRole("button", {name: "下一步", exact: true})).toBeVisible();
    await page.getByRole("button", {name: "下一步", exact: true}).click();
    await expectCurrent(4);

    for (const index of [3, 2, 1, 0]) {
      await page.getByRole("button", {name: "上一步", exact: true}).click();
      await expectCurrent(index);
    }
  });

  test("保留当前存储目录后可跳过扫描并完成引导", async ({page}) => {
    await installMockTauri(page, {onboarding: true});
    await page.goto("/");
    await expectOnboardingStep(page, 0);
    await expect(page.getByText("/sandbox/data", {exact: true})).toBeVisible();

    await continueToGameScan(page);
    await page.getByRole("button", {name: "稍后再扫", exact: true}).click();
    await expectOnboardingStep(page, 4);
    await expect(page.getByText("/sandbox/data", {exact: true})).toBeVisible();
    await expect(page.getByText("尚未配置", {exact: true})).toBeVisible();
    await expect(page.getByText("0 款游戏", {exact: true})).toBeVisible();
    const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
    expect(calls.filter((call: any) => call.command === "migrate_storage")).toEqual([]);

    await page.getByRole("button", {name: "完成并进入游戏库", exact: true}).click();
    await expect(page.getByTestId("library-page")).toBeVisible();
    const completed = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "set_onboarding_completed"));
    expect(completed.at(-1)?.args.completed).toBe(true);
  });

  test("存储迁移成功时锁定引导，设置页重进后刷新到第 2 步", async ({page}) => {
    await installOnboardingMigrationMock(page, {nextDataRoot: migrationNewRoot, hold: true, onboarding: false});
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    await page.getByRole("button", {name: "设置", exact: true}).click();
    await page.getByRole("tab", {name: "外观与存储", exact: true}).click();
    await page.getByRole("button", {name: "重新查看新手引导", exact: true}).click();
    await expectOnboardingStep(page, 0);

    const dialog = await chooseAndConfirmStorageMigration(page);
    await expect(dialog.getByRole("button", {name: "正在重启…", exact: true})).toBeDisabled();
    await expect(dialog.getByRole("button", {name: "取消", exact: true})).toBeDisabled();
    await expect.poll(() => page.evaluate(() => (window as any).__AMM_TEST__.calls.filter(
      (call: any) => call.command === "migrate_storage",
    ))).toEqual([{command: "migrate_storage", args: {parent: migrationParent}}]);

    await page.evaluate(() => (window as any).__AMM_TEST__.finishStorageMigration());
    await page.reload();
    await expectOnboardingStep(page, 1);
    await expect(page.getByText(/迁移失败/)).toHaveCount(0);
    await page.getByRole("button", {name: "上一步", exact: true}).click();
    await expectOnboardingStep(page, 0);
    await expect(page.getByText(migrationNewRoot, {exact: true})).toBeVisible();
  });

  test("迁移后目录未变时刷新仍回到第 1 步", async ({page}) => {
    await installOnboardingMigrationMock(page, {nextDataRoot: migrationCurrentRoot, onboarding: true});
    await page.goto("/");
    await expectOnboardingStep(page, 0);
    await chooseAndConfirmStorageMigration(page);
    await expect(page.getByRole("button", {name: "正在重启…", exact: true})).toBeDisabled();
    await page.reload();
    await expectOnboardingStep(page, 0);
    await expect(page.getByText(migrationCurrentRoot, {exact: true})).toBeVisible();
  });

  test("存储迁移即时拒绝后可重试", async ({page}) => {
    await installOnboardingMigrationMock(page, {
      nextDataRoot: migrationNewRoot,
      rejectMigration: true,
      onboarding: true,
    });
    await page.goto("/");
    await expectOnboardingStep(page, 0);
    const dialog = await chooseAndConfirmStorageMigration(page);
    await expect(dialog.getByText(migrationFailureMessage, {exact: true})).toBeVisible();
    await expect(dialog.getByRole("button", {name: "迁移并重启", exact: true})).toBeEnabled();
    await expect(dialog.getByRole("button", {name: "取消", exact: true})).toBeEnabled();
    await dialog.getByRole("button", {name: "取消", exact: true}).click();
    await expect(dialog).toBeHidden();
    await expectOnboardingStep(page, 0);

    const retryDialog = await chooseAndConfirmStorageMigration(page);
    await expect(retryDialog.getByRole("button", {name: "正在重启…", exact: true})).toBeDisabled();
    await expect.poll(() => page.evaluate(() => (window as any).__AMM_TEST__.calls.filter(
      (call: any) => call.command === "migrate_storage",
    ))).toEqual([
      {command: "migrate_storage", args: {parent: migrationParent}},
      {command: "migrate_storage", args: {parent: migrationParent}},
    ]);

    await page.reload();
    await expectOnboardingStep(page, 1);
  });

  test("重启后 bootstrap 报告存储迁移失败时回到第 1 步", async ({page}) => {
    await installOnboardingMigrationMock(page, {
      nextDataRoot: migrationCurrentRoot,
      nextMigrationError: migrationFailureMessage,
      onboarding: true,
    });
    await page.goto("/");
    await expectOnboardingStep(page, 0);
    const dialog = await chooseAndConfirmStorageMigration(page);
    await expect(dialog.getByRole("button", {name: "正在重启…", exact: true})).toBeDisabled();
    await expect.poll(() => page.evaluate(() => (window as any).__AMM_TEST__.calls.filter(
      (call: any) => call.command === "migrate_storage",
    ))).toEqual([{command: "migrate_storage", args: {parent: migrationParent}}]);

    await page.reload();
    await expectOnboardingStep(page, 0);
    await expect(page.getByText(/迁移失败，仍在使用原目录/)).toBeVisible();
  });

  test("扫描候选不会自动添加，失败候选不影响其他候选", async ({page}) => {
    await installMockTauri(page, {onboarding: true, failRegistration: true, withExternalAdapter: true});
    await page.goto("/");
    await continueToGameScan(page);
    await expect(page.getByRole("navigation", {name: "主导航"})).toHaveCount(0);

    await page.getByRole("button", {name: "扫描已安装游戏"}).click();
    const fixture = page.getByRole("listitem").filter({hasText: "Fixture Game"});
    const broken = page.getByRole("listitem").filter({hasText: "Broken Fixture"});
    await expect(fixture).toContainText("/sandbox/steam/Fixture");
    await expect(fixture).toContainText("92% 匹配");
    await expect(broken.getByText("未匹配", {exact: true})).toBeVisible();
    await expect(broken).toContainText("未知平台 · 建议适配包：无");
    await expect(broken).not.toContainText("60% 匹配");
    await expect(fixture).toContainText("这是一个测试扫描候选，请确认目录后再添加");
    await expect(page.getByText("已检查 3 个目录、18 个条目")).toBeVisible();

    const registrations = () => page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "register_game"));
    expect(await registrations()).toHaveLength(0);
    await expect(fixture.getByRole("button", {name: "添加", exact: true})).toBeEnabled();
    await expect(broken.getByRole("button", {name: "添加", exact: true})).toBeEnabled();
    await broken.getByRole("button", {name: "添加", exact: true}).click();
    await expect(broken.getByRole("alert")).toContainText("目录已变化，请重新扫描");
    await expect(fixture).toBeVisible();
    await expect(fixture.getByText("管理中", {exact: true})).toHaveCount(0);

    await fixture.getByRole("button", {name: "添加", exact: true}).click();
    await expect(fixture.getByText("管理中", {exact: true})).toBeVisible();
    await expect(broken.getByText("管理中", {exact: true})).toHaveCount(0);
    await expect(broken.getByRole("button", {name: "添加", exact: true})).toBeEnabled();
    const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls.map((call: any) => call.command));
    expect(calls.filter((command: string) => command === "start_onboarding")).toHaveLength(1);
    expect((await registrations()).map((call: any) => call.args)).toEqual([
      {directory: "/sandbox/steam/Broken", adapterSlug: null, fromDiscovery: true},
      {directory: "/sandbox/steam/Fixture", adapterSlug: "minecraft-java", fromDiscovery: true},
    ]);
  });

  test("引导页添加无建议的扫描候选时不绑定适配包", async ({page}) => {
    await installMockTauri(page, {onboarding: true});
    await page.goto("/");
    await continueToGameScan(page);
    await page.getByRole("button", {name: "扫描已安装游戏"}).click();
    const directory = "/sandbox/steam/Broken";
    const rows = page.getByRole("list", {name: "扫描候选游戏", exact: true});
    // Registration can replace the discovery title with the library name; the directory stays the same.
    const broken = rows.getByRole("listitem").filter({has: page.getByText(directory, {exact: true})});
    const add = broken.getByRole("button", {name: "添加", exact: true});
    await expect(add).toBeEnabled();
    await add.click();
    await expect(broken.getByText("管理中", {exact: true})).toBeVisible();
    await expect(broken.getByText("Broken", {exact: true})).toBeVisible();
    await expect(broken.getByText("未关联适配包", {exact: true})).toBeVisible();
    await expect(broken.getByRole("button", {name: "添加", exact: true})).toHaveCount(0);
    const fixture = rows.getByRole("listitem").filter({has: page.getByText("/sandbox/steam/Fixture", {exact: true})});
    await expect(fixture.getByRole("button", {name: "添加", exact: true})).toBeEnabled();
    await expect(fixture.getByText("管理中", {exact: true})).toHaveCount(0);
    const registrations = await page.evaluate(() => (window as any).__AMM_TEST__.calls
      .filter((call: any) => call.command === "register_game").map((call: any) => call.args));
    expect(registrations).toEqual([{directory, adapterSlug: null, fromDiscovery: true}]);
    await page.getByRole("button", {name: "下一步", exact: true}).click();
    await expectOnboardingStep(page, 4);
    await expect(page.getByText("1 款游戏", {exact: true})).toBeVisible();
    await page.getByRole("button", {name: "完成并进入游戏库", exact: true}).click();
    const game = page.locator('[data-slot="card"]').filter({has: page.getByText("Broken", {exact: true})});
    await expect(game.getByRole("button", {name: "创建适配包"})).toBeVisible();
  });

  test("配置模型时密钥走 set_secret，完成五步引导后进入游戏库", async ({page}) => {
    await openOnboardingWithMcp(page);
    await continueToModelSetup(page);
    await expect(page.getByRole("link", {name: "没有AI？从这里获取"})).toHaveAttribute("href", "https://one.gloscai.com/keys");
    await page.getByLabel("API 地址", {exact: true}).fill("https://provider.example/v1");
    await page.getByLabel("API 密钥").fill("sk-test-secret-never-logged");
    await page.getByRole("button", {name: "获取模型列表"}).click();
    await page.getByRole("checkbox", {name: "fixture-thinking"}).check();
    await page.getByRole("checkbox", {name: "fixture-standard"}).check();
    await page.getByRole("button", {name: "添加所选（2）"}).click();
    await expect(page.getByRole("spinbutton", {name: /上下文预算/})).toHaveCount(0);
    await expect(page.getByRole("button", {name: "fixture-standard 思考等级"})).toHaveText("服务默认");
    await page.getByRole("button", {name: "fixture-thinking 思考等级"}).click();
    const thinking = page.getByRole("menu");
    await expect(thinking.getByRole("menuitemcheckbox", {name: "低（low）"})).toBeChecked();
    await expect(thinking.getByRole("menuitemcheckbox", {name: "高（high）"})).toBeChecked();
    await thinking.getByRole("menuitemcheckbox", {name: "中（medium）"}).click();
    await expect(thinking.getByRole("menuitemcheckbox", {name: "中（medium）"})).not.toBeChecked();
    await page.keyboard.press("Escape");
    await expect(thinking).toHaveCount(0);
    await expect(page.getByRole("button", {name: "fixture-thinking 思考等级"})).toBeFocused();
    await page.getByRole("combobox", {name: "默认模型", exact: true}).selectOption("fixture-thinking");
    await page.getByRole("button", {name: "保存并继续"}).click();
    await expectOnboardingStep(page, 2);
    await expect(onboardingMcpRow(page, "Remote HTTP")).toContainText("未启动");
    await expect(onboardingMcpRow(page, "Local Stdio")).toContainText("未启动");
    expect(await onboardingMcpCalls(page)).toEqual([]);

    const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
    const secret = calls.find((call: any) => call.command === "set_secret");
    const settings = calls.find((call: any) => call.command === "set_settings");
    expect(secret?.args.value).toBe("sk-test-secret-never-logged");
    expect(JSON.stringify(settings?.args)).not.toContain("sk-test-secret-never-logged");
    expect(calls.find((call: any) => call.command === "list_agent_models")?.args).toEqual({baseUrl: "https://provider.example/v1", authenticated: true});
    expect(calls.find((call: any) => call.command === "get_agent_model_metadata")?.args).toEqual({baseUrl: "https://provider.example/v1", modelIds: ["fixture-standard", "fixture-thinking"]});
    expect(settings?.args.settings.model).toBe("fixture-thinking");
    expect(settings?.args.settings.models).toEqual([
      {display_name: "fixture-thinking", id: "fixture-thinking", reasoning_levels: ["low", "high"]},
      {display_name: "fixture-standard", id: "fixture-standard", reasoning_levels: []},
    ]);

    await page.getByRole("button", {name: "下一步", exact: true}).click();
    await expectOnboardingStep(page, 3);
    await expect(page.getByRole("button", {name: "稍后再扫", exact: true})).toBeVisible();
    await page.getByRole("button", {name: "稍后再扫", exact: true}).click();
    await expectOnboardingStep(page, 4);
    await expect(page.getByText("fixture-thinking", {exact: true})).toBeVisible();
    await expect(page.getByText("0 款游戏", {exact: true})).toBeVisible();
    await page.getByRole("button", {name: "完成并进入游戏库", exact: true}).click();
    await expect(page.getByTestId("library-page")).toBeVisible();
    await expect(page.getByRole("navigation", {name: "主导航"})).toBeVisible();
    const completed = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "set_onboarding_completed"));
    expect(completed.at(-1)?.args.completed).toBe(true);
  });

  test("进入或跳过 Agent 配置不会自动连接 MCP，主动启动与返回后状态正确", async ({page}) => {
    await openOnboardingWithMcp(page);
    await continueToModelSetup(page);
    await skipModelSetup(page);

    const mcpHeading = page.getByRole("heading", {name: "启用可用工具", exact: true});
    const httpRow = onboardingMcpRow(page, onboardingHttpConfig.name);
    const stdioRow = onboardingMcpRow(page, onboardingStdioConfig.name);
    await expect(mcpHeading).toBeVisible();
    await expect(httpRow).toContainText("未启动");
    await expect(stdioRow).toContainText("未启动");
    expect(await onboardingMcpCalls(page)).toEqual([]);

    await page.getByRole("button", {name: "上一步", exact: true}).click();
    await expectOnboardingStep(page, 1);
    await page.getByRole("button", {name: "上一步", exact: true}).click();
    await expectOnboardingStep(page, 0);
    await continueToModelSetup(page);
    await skipModelSetup(page);
    await expect(mcpHeading).toBeVisible();
    expect(await onboardingMcpCalls(page)).toEqual([]);

    await httpRow.getByRole("button", {name: `启动 ${onboardingHttpConfig.name}`, exact: true}).click();
    await expect(httpRow).toContainText("运行中");
    await expect(httpRow.getByRole("button", {name: `停止 ${onboardingHttpConfig.name}`, exact: true})).toBeVisible();
    const httpCalls = [
      {command: "mcp_update", args: {config: {...onboardingHttpConfig, enabled: true}}},
      {command: "mcp_connect", args: {expectedConfig: {...onboardingHttpConfig, enabled: true}, acceptsUnsandboxedStdio: false}},
    ];
    expect(await onboardingMcpCalls(page)).toEqual(httpCalls);

    await expect(stdioRow).toContainText("fixture-mcp --stdio");
    await stdioRow.getByRole("button", {name: `启动 ${onboardingStdioConfig.name}`, exact: true}).click();
    await expect(stdioRow).toContainText("运行中");
    const connectedCalls = [...httpCalls, {
      command: "mcp_connect",
      args: {expectedConfig: onboardingStdioConfig, acceptsUnsandboxedStdio: true},
    }];
    expect(await onboardingMcpCalls(page)).toEqual(connectedCalls);

    await page.getByRole("button", {name: "下一步", exact: true}).click();
    await expectOnboardingStep(page, 3);
    await page.getByRole("button", {name: "稍后再扫", exact: true}).click();
    await expectOnboardingStep(page, 4);
    await page.getByRole("button", {name: "上一步", exact: true}).click();
    await expectOnboardingStep(page, 3);
    await page.getByRole("button", {name: "上一步", exact: true}).click();
    await expect(mcpHeading).toBeVisible();
    await expect(httpRow).toContainText("运行中");
    await expect(stdioRow).toContainText("运行中");
    expect(await onboardingMcpCalls(page)).toEqual(connectedCalls);
  });

  test("全部启用只连接尚未运行的 MCP，全部运行后禁用批量按钮", async ({page}) => {
    await openOnboardingWithMcp(page, {active: [onboardingStdioConfig.id]});
    await continueToModelSetup(page);
    await skipModelSetup(page);

    const httpRow = onboardingMcpRow(page, onboardingHttpConfig.name);
    const stdioRow = onboardingMcpRow(page, onboardingStdioConfig.name);
    const enableAll = page.getByRole("button", {name: "全部启用", exact: true});
    await expect(stdioRow).toContainText("运行中");
    await expect(httpRow).toContainText("未启动");
    await enableAll.click();

    await expect(httpRow).toContainText("运行中");
    await expect(stdioRow).toContainText("运行中");
    await expect(enableAll).toBeDisabled();
    expect(await onboardingMcpCalls(page)).toEqual([
      {command: "mcp_update", args: {config: {...onboardingHttpConfig, enabled: true}}},
      {command: "mcp_connect", args: {expectedConfig: {...onboardingHttpConfig, enabled: true}, acceptsUnsandboxedStdio: false}},
    ]);
  });

  test("Agent composer supports per-turn model and reasoning controls", async ({page}) => {
    const configuredSettings = {
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
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
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
      const approval = page.getByRole("group", {name: "操作审批", exact: true});
      await expect(page.getByRole("dialog", {name: "操作审批", exact: true})).toHaveCount(0);
      await expect(approval).toBeVisible();
      const reject = approval.getByRole("button", {name: "拒绝", exact: true});
      await reject.click();
      await expect(reject).toHaveCount(0);
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

  test("BaseLLM 不可用时仍可手动添加模型并保存", async ({page}) => {
    await installMockTauri(page, {onboarding: true});
    await page.goto("/");
    await continueToModelSetup(page);
    await page.getByLabel("API 地址", {exact: true}).fill("https://offline.example/v1");
    await page.getByRole("button", {name: "手动添加", exact: true}).click();
    await page.getByLabel("新模型 实际请求模型", {exact: true}).fill("local-model");
    await page.getByLabel("local-model 实际请求模型").press("Tab");
    await expect(page.getByText(/暂时无法读取 BaseLLM/)).toBeVisible();
    await page.getByRole("button", {name: "保存并继续"}).click();
    await expectOnboardingStep(page, 2);
    const models = await page.evaluate(() => (window as any).__AMM_TEST__.calls.find((call: any) => call.command === "set_settings").args.settings.models);
    expect(models).toEqual([{display_name: "local-model", id: "local-model", reasoning_levels: []}]);
    await page.getByRole("button", {name: "下一步", exact: true}).click();
    await expectOnboardingStep(page, 3);
    await page.getByRole("button", {name: "稍后再扫", exact: true}).click();
    await expectOnboardingStep(page, 4);
  });

  test("在模型步骤退出失败时显示错误并允许重试", async ({page}) => {
    await installMockTauri(page, {onboarding: true});
    await page.goto("/");
    await continueToModelSetup(page);
    await page.evaluate(() => {
      const transport = (window as any).__TAURI_INTERNALS__;
      const invoke = transport.invoke.bind(transport);
      transport.invoke = async (command: string, ...args: unknown[]) => {
        if (command === "set_onboarding_completed") {
          transport.invoke = invoke;
          throw new Error("无法保存引导状态");
        }
        return invoke(command, ...args);
      };
    });
    const exit = page.getByRole("button", {name: "退出引导", exact: true});
    await exit.click();
    await expect(page.getByRole("alert")).toContainText("无法保存引导状态");
    await expectOnboardingStep(page, 1);
    await exit.click();
    await expect(page.getByTestId("library-page")).toBeVisible();
  });

  test("退出引导会持久化完成状态并解除阻断", async ({page}) => {
    await installMockTauri(page, {onboarding: true});
    await page.goto("/");
    await page.getByRole("button", {name: "退出引导", exact: true}).click();
    await expect(page.getByTestId("library-page")).toBeVisible();
    const completed = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "set_onboarding_completed"));
    expect(completed.at(-1)?.args.completed).toBe(true);
  });

  test("添加后中途退出会恢复引导，已有游戏的旧用户不会被阻断", async ({page}) => {
    await installMockTauri(page, {onboarding: true, onboardingStarted: true});
    await page.goto("/");
    await expectOnboardingStep(page, 0);
    await expect(page.getByRole("navigation", {name: "主导航"})).toHaveCount(0);
  });

  test("已完成引导的用户可以从设置重新查看引导", async ({page}) => {
    await installMockTauri(page);
    await page.goto("/");
    await page.getByRole("button", {name: "游戏库", exact: true}).click();
    await expect(page.getByTestId("library-page")).toBeVisible();
    await page.getByRole("button", {name: "设置", exact: true}).click();
    await page.getByRole("tab", {name: "外观与存储"}).click();
    await page.getByRole("button", {name: "重新查看新手引导"}).click();
    await expectOnboardingStep(page, 0);
    await continueToGameScan(page);
    await page.getByRole("button", {name: "稍后再扫", exact: true}).click();
    await expectOnboardingStep(page, 4);
    await page.getByRole("button", {name: "完成并进入游戏库", exact: true}).click();
    await expect(page.getByTestId("library-page")).toBeVisible();
  });

  test("引导页在亮色和暗色主题下没有横向溢出", async ({page}, info) => {
    await openOnboardingWithMcp(page);
    for (const theme of ["light", "dark"]) {
      if (await page.locator("html").getAttribute("data-theme") !== theme) {
        await page.getByRole("button", {name: theme === "dark" ? "切换暗色主题" : "切换亮色主题"}).click();
      }
      await continueToModelSetup(page);
      await skipModelSetup(page);
      await expectOnboardingStep(page, 2);
      if (info.project.name === "narrow-390") {
        await expect(page.getByText("步骤 3 / 5", {exact: true})).toBeVisible();
      }
      if (info.project.name.startsWith("desktop-")) {
        const navigation = page.getByRole("list", {name: "引导步骤", exact: true});
        await expect(navigation).toBeVisible();
        const labels = navigation.getByRole("listitem");
        await expect(labels).toHaveCount(onboardingSteps.length);
        for (const [index, label] of onboardingSteps.entries()) {
          await expect(labels.nth(index)).toContainText(label);
        }
        await expect(navigation.getByRole("listitem").nth(2)).toHaveAttribute("aria-current", "step");
      }
      expect(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1), `${theme}/${info.project.name}`).toBe(false);
      await page.screenshot({path: `output/playwright/screenshots/onboarding-tools-${info.project.name}-${theme}.png`, fullPage: true, animations: "disabled"});

      await page.getByRole("button", {name: "下一步", exact: true}).click();
      await expectOnboardingStep(page, 3);
      expect(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1), `${theme}/${info.project.name}/scan`).toBe(false);
      await page.getByRole("button", {name: "上一步", exact: true}).click();
      await expectOnboardingStep(page, 2);
      await page.getByRole("button", {name: "上一步", exact: true}).click();
      await expectOnboardingStep(page, 1);
      await page.getByRole("button", {name: "上一步", exact: true}).click();
      await expectOnboardingStep(page, 0);
    }
  });
});
