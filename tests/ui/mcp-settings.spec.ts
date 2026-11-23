import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

type McpFixture = {
  id: string;
  name: string;
  transport: "stdio" | "streamable_http";
  command: string | null;
  args: string[];
  url: string | null;
  credential_url?: string | null;
  env_secret_refs: Record<string, string>;
  bearer_secret_ref: string | null;
  description?: string;
  enabled: boolean;
  builtin: boolean;
  plugin_slug?: string;
};

const httpConfig: McpFixture = {
  id: "fixture-http",
  name: "Remote HTTP",
  transport: "streamable_http",
  command: null,
  args: [],
  url: "https://mcp.example.test",
  env_secret_refs: {},
  bearer_secret_ref: "mcp.remote_http.bearer",
  enabled: true,
  builtin: false,
};

const stdioConfig: McpFixture = {
  id: "fixture-stdio",
  name: "Local Stdio",
  transport: "stdio",
  command: "fixture-mcp",
  args: [],
  url: null,
  env_secret_refs: {
    GITHUB_TOKEN: "mcp.local_stdio.github_token",
    MAPS_API_KEY: "mcp.local_stdio.maps_key",
  },
  bearer_secret_ref: null,
  enabled: true,
  builtin: false,
};

const noCredentialsConfig: McpFixture = {
  id: "fixture-no-credentials",
  name: "Public MCP",
  transport: "streamable_http",
  command: null,
  args: [],
  url: "https://public.example.test",
  env_secret_refs: {},
  bearer_secret_ref: null,
  enabled: true,
  builtin: true,
};

async function openMcpSettings(page: Page) {
  await installMockTauri(page);
  await page.goto("/");

  // Keep the established mock for every command except the list fixture.
  // This lets the mock continue recording set_secret calls for exact assertions.
  await page.evaluate((configs) => {
    const internals = (window as any).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = (command: string, args: unknown, ...rest: unknown[]) =>
      command === "mcp_list" ? Promise.resolve(configs) : invoke(command, args, ...rest);
  }, [httpConfig, stdioConfig, noCredentialsConfig]);

  await page.getByRole("button", {name: "设置", exact: true}).click();
  await page.getByRole("tab", {name: "工具", exact: true}).click();
}

async function secretCalls(page: Page) {
  return page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "set_secret"));
}

test("MCP credentials stay out of the list and configure controls follow secret references", async ({page}) => {
  await openMcpSettings(page);

  await expect(page.getByRole("button", {name: "配置 Remote HTTP 凭据", exact: true})).toBeVisible();
  await expect(page.getByRole("button", {name: "配置 Local Stdio 凭据", exact: true})).toBeVisible();
  await expect(page.getByRole("button", {name: "配置 Public MCP 凭据", exact: true})).toHaveCount(0);
  await expect(page.locator('input[type="password"]')).toHaveCount(0);
  await expect(page.getByText("mcp.remote_http.bearer", {exact: true})).toHaveCount(0);
  await expect(page.getByText("mcp.local_stdio.github_token", {exact: true})).toHaveCount(0);
  expect(await page.locator("main").evaluate((main) => main.scrollWidth <= main.clientWidth + 1)).toBe(true);
});

test("HTTP credentials can be discarded or saved from the dialog footer", async ({page}) => {
  await openMcpSettings(page);

  const trigger = page.getByRole("button", {name: "配置 Remote HTTP 凭据", exact: true});
  const dialog = page.getByRole("dialog", {name: "配置 MCP 凭据", exact: true});
  await trigger.click();
  await expect(dialog).toContainText("Remote HTTP");

  const footer = dialog.locator('[data-slot="dialog-footer"]');
  const back = footer.getByRole("button").nth(0);
  const save = footer.getByRole("button").nth(1);
  await expect(footer.getByRole("button")).toHaveCount(2);
  await expect(back).toHaveAccessibleName("返回");
  await expect(save).toHaveAccessibleName("保存");
  const backBox = await back.boundingBox();
  const saveBox = await save.boundingBox();
  expect(backBox && saveBox).toBeTruthy();
  if (backBox && saveBox) {
    expect(saveBox.x).toBeGreaterThan(backBox.x);
    expect(Math.abs(saveBox.y - backBox.y)).toBeLessThan(1);
  }

  const bearer = dialog.getByLabel("Bearer 密钥", {exact: true});
  await expect(bearer).toHaveAttribute("type", "password");
  await expect(save).toBeDisabled();
  await bearer.fill("discard-on-close");
  await back.click();
  await expect(dialog).toBeHidden();
  await expect(trigger).toBeFocused();
  expect(await secretCalls(page)).toEqual([]);

  await trigger.click();
  await expect(bearer).toHaveValue("");
  await expect(bearer).toHaveAttribute("type", "password");
  await expect(save).toBeDisabled();
  await bearer.fill("http-secret-fixture");
  await expect(save).toBeEnabled();
  await save.click();
  await expect(dialog).toBeHidden();
  await expect(trigger).toBeFocused();

  await trigger.click();
  await expect(bearer).toHaveValue("");
  // Older configs omit credential_url; the credentials dialog remains unchanged for them.
  await expect(dialog.getByRole("link", {name: "获取 Key", exact: true})).toHaveCount(0);
  expect(await secretCalls(page)).toEqual([{
    command: "set_secret",
    args: {key: "mcp.remote_http.bearer", value: "http-secret-fixture"},
  }]);
});

test("stdio credentials save all nonempty values and retry only failed references", async ({page}) => {
  await openMcpSettings(page);

  await page.getByRole("button", {name: "配置 Local Stdio 凭据", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "配置 MCP 凭据", exact: true});
  await expect(dialog).toContainText("Local Stdio");

  const githubToken = dialog.getByLabel("GITHUB_TOKEN", {exact: true});
  const mapsKey = dialog.getByLabel("MAPS_API_KEY", {exact: true});
  await expect(dialog.locator('input[type="password"]')).toHaveCount(2);
  await expect(githubToken).toHaveAttribute("type", "password");
  await expect(mapsKey).toHaveAttribute("type", "password");
  await expect(dialog.getByRole("link", {name: "获取 Key", exact: true})).toHaveCount(0);

  const save = dialog.locator('[data-slot="dialog-footer"]').getByRole("button", {name: "保存", exact: true});
  await expect(save).toBeDisabled();
  expect(await secretCalls(page)).toEqual([]);

  // Reject the first write for one reference after the mock records the attempted IPC call.
  await page.evaluate((failingKey) => {
    const internals = (window as any).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    let rejected = false;
    internals.invoke = (command: string, args: any, ...rest: unknown[]) => {
      const result = invoke(command, args, ...rest);
      if (command === "set_secret" && args.key === failingKey && !rejected) {
        rejected = true;
        return Promise.resolve(result).then(() => { throw new Error("模拟后端写入失败"); });
      }
      return result;
    };
  }, "mcp.local_stdio.maps_key");

  await githubToken.fill("stdio-github-fixture");
  await mapsKey.fill("stdio-maps-fixture");
  await save.click();

  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText("MAPS_API_KEY");
  await expect(dialog).toContainText("模拟后端写入失败");
  await expect(githubToken).toHaveValue("");
  await expect(mapsKey).toHaveValue("stdio-maps-fixture");
  await expect(save).toBeEnabled();

  await save.click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole("button", {name: "配置 Local Stdio 凭据", exact: true})).toBeFocused();

  expect(await secretCalls(page)).toEqual([
    {command: "set_secret", args: {key: "mcp.local_stdio.github_token", value: "stdio-github-fixture"}},
    {command: "set_secret", args: {key: "mcp.local_stdio.maps_key", value: "stdio-maps-fixture"}},
    {command: "set_secret", args: {key: "mcp.local_stdio.maps_key", value: "stdio-maps-fixture"}},
  ]);

  await page.getByRole("button", {name: "配置 Local Stdio 凭据", exact: true}).click();
  await expect(githubToken).toHaveValue("");
  await expect(mapsKey).toHaveValue("");
  await expect(save).toBeDisabled();
});

// Stateful stand-in for the MCP lifecycle commands. Every call is recorded for exact assertions.
async function openLifecycle(page: Page, configs: McpFixture[], {failConnect, failConnectId, holdConnect = false, remembered = [], localRequested = false, localRunning = false, active = []}: {
  failConnect?: string; failConnectId?: string; holdConnect?: boolean; remembered?: string[]; localRequested?: boolean; localRunning?: boolean; active?: string[];
} = {}) {
  await installMockTauri(page);
  await page.goto("/");
  await page.evaluate(({configs, failConnect, failConnectId, holdConnect, remembered, localRequested, localRunning, active}) => {
    const internals = (window as any).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    const state = {
      configs: structuredClone(configs), remembered, localRequested, localRunning, failConnect, failConnectId,
      finishConnect: undefined as (() => void) | undefined,
      active: [...active] as string[], calls: [] as Array<{command: string; args: any}>,
    };
    (window as any).__MCP__ = state;
    internals.invoke = async (command: string, args: any, ...rest: unknown[]) => {
      if (command === "mcp_server_status") return {...await invoke(command, args, ...rest), startupRequested: state.localRequested, running: state.localRunning};
      if (!command.startsWith("mcp_")) return invoke(command, args, ...rest);
      state.calls.push({command, args: structuredClone(args)});
      if (command === "mcp_server_start") { state.localRequested = true; state.localRunning = true; return; }
      if (command === "mcp_server_stop") { state.localRequested = false; state.localRunning = false; return; }
      if (command === "mcp_list") return structuredClone(state.configs);
      if (command === "mcp_startup_requests") return [...state.remembered];
      if (command === "mcp_connections") return state.active.map((serverId) => ({serverId, tools: ["fixture_search", "fixture_read"]}));
      if (command === "mcp_update") { state.configs = state.configs.map((item) => item.id === args.config.id ? args.config : item); return null; }
      if (command === "mcp_create") { state.configs.push(args.config); return null; }
      if (command === "mcp_remove") { state.configs = state.configs.filter((item) => item.id !== args.id); return null; }
      if (command === "mcp_connect") {
        if (holdConnect) await new Promise<void>((resolve) => { state.finishConnect = resolve; });
        if (state.failConnect && (!state.failConnectId || state.failConnectId === args.expectedConfig.id)) throw new Error(state.failConnect);
        state.active.push(args.expectedConfig.id);
        return {serverId: args.expectedConfig.id, tools: ["fixture_search", "fixture_read"]};
      }
      if (command === "mcp_disconnect") { state.remembered = state.remembered.filter((id) => id !== args.id); state.active = state.active.filter((id) => id !== args.id); return null; }
      return invoke(command, args, ...rest);
    };
  }, {configs, failConnect, failConnectId, holdConnect, remembered, localRequested, localRunning, active});
  await page.getByRole("button", {name: "设置", exact: true}).click();
  await page.getByRole("tab", {name: "工具", exact: true}).click();
}

// Scoped to the MCP list: toasts are also list items and repeat the service name.
const mcpRow = (page: Page, name: string) => page.getByRole("list", {name: "外部 MCP 连接", exact: true}).getByRole("listitem").filter({hasText: name});

const lifecycleCalls = (page: Page) => page.evaluate(() => (window as any).__MCP__.calls
  .filter((call: any) => ["mcp_update", "mcp_connect", "mcp_disconnect"].includes(call.command))
  .map((call: any) => ({command: call.command, args: call.args})));

const mcpWriteCalls = (page: Page) => page.evaluate(() => (window as any).__MCP__.calls
  .filter((call: any) => ["mcp_create", "mcp_update"].includes(call.command)));

test("plugin details and draft validation list declared MCP services", async ({ page }) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.evaluate(() => {
    const ipc = (window as any).__TAURI_INTERNALS__;
    const invoke = ipc.invoke.bind(ipc);
    ipc.invoke = async (command: string, ...args: unknown[]) => {
      const result = await invoke(command, ...args);
      if (command === "list_plugins") return [{ slug: "catalog-game", gameName: "Catalog Game", name: "Catalog Game", version: "1.0.0", description: "Catalog Skill", source: "external", enabled: true, validated: true, installPath: "/fixtures/catalog-game", coverUrl: null, mcpServerNames: ["Local catalog"] }];
      if (command === "validate_plugin") result.package.adapter.mcpServers = [{id: "catalog", name: "Draft catalog"}];
      return result;
    };
  });
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await page.getByRole("button", { name: "插件中心", exact: true }).click();
  await page.getByRole("row").filter({hasText: "Catalog Game"}).getByRole("cell").first().getByRole("button").click();
  await expect(page.getByText("Local catalog", { exact: true })).toBeVisible();
  await page.getByRole("tab", { name: /^草稿/ }).click();
  await page.getByRole("button", { name: "验证", exact: true }).first().click();
  await expect(page.getByRole("dialog")).toContainText("Draft catalog");
});

for (const base of [stdioConfig, httpConfig]) {
  test(`a plugin ${base.transport} service starts from its row without rewriting the owned config`, async ({page}) => {
    const config = {...base, id: "plugin_example_catalog", name: "Catalog", plugin_slug: "example", enabled: false};
    await openLifecycle(page, [config]);
    const row = mcpRow(page, config.name);
    await expect(row).toContainText("来自适配包 · example");
    await expect(row.getByRole("button", {name: "编辑 Catalog", exact: true})).toHaveCount(0);
    await row.getByRole("button", {name: "启动 Catalog", exact: true}).click();
    await expect(row).toContainText("运行中");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(await lifecycleCalls(page)).toEqual([
      {command: "mcp_connect", args: {expectedConfig: {...config, enabled: true}, acceptsUnsandboxedStdio: base.transport === "stdio"}},
    ]);
  });
}

test("an HTTP service starts and stops from one button and shows its state", async ({page}) => {
  await openLifecycle(page, [{...httpConfig, enabled: false}]);
  const row = mcpRow(page, "Remote HTTP");
  await expect(row).toContainText("未启动");

  await row.getByRole("button", {name: "启动 Remote HTTP", exact: true}).click();
  await expect(row).toContainText("运行中");
  await expect(row.getByRole("button", {name: "停止 Remote HTTP", exact: true})).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  // A disabled saved config is switched on as part of starting, then connected with that exact config.
  expect(await lifecycleCalls(page)).toEqual([
    {command: "mcp_update", args: {config: {...httpConfig, enabled: true}}},
    {command: "mcp_connect", args: {expectedConfig: {...httpConfig, enabled: true}, acceptsUnsandboxedStdio: false}},
  ]);

  await row.getByRole("button", {name: "停止 Remote HTTP", exact: true}).click();
  await expect(row).toContainText("未启动");
  expect((await lifecycleCalls(page)).at(-1)).toEqual({command: "mcp_disconnect", args: {id: "fixture-http"}});
});

test("an already enabled HTTP service connects without rewriting its config", async ({page}) => {
  await openLifecycle(page, [httpConfig]);
  await page.getByRole("button", {name: "启动 Remote HTTP", exact: true}).click();
  await expect(mcpRow(page, "Remote HTTP")).toContainText("运行中");
  expect((await lifecycleCalls(page)).map((call: any) => call.command)).toEqual(["mcp_connect"]);
});

test("a stdio service starts directly with its exact config and prevents concurrent starts", async ({page}) => {
  const config = {...stdioConfig, args: ["--stdio"]};
  await openLifecycle(page, [config, httpConfig], {holdConnect: true});
  const row = mcpRow(page, config.name);
  const start = row.getByRole("button", {name: "启动 Local Stdio", exact: true});
  await expect(row).toContainText("fixture-mcp --stdio");
  expect(await lifecycleCalls(page)).toEqual([]);

  await start.click();
  await expect(row).toContainText("启动中");
  await expect(start).toBeDisabled();
  await expect(page.getByRole("button", {name: "启动 Remote HTTP", exact: true})).toBeDisabled();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect.poll(() => lifecycleCalls(page)).toEqual([
    {command: "mcp_connect", args: {expectedConfig: config, acceptsUnsandboxedStdio: true}},
  ]);

  await page.evaluate(() => (window as any).__MCP__.finishConnect());
  await expect(row).toContainText("运行中");
  await expect(page.getByRole("button", {name: "启动 Remote HTTP", exact: true})).toBeEnabled();
});

test("a failed start stays visible with its reason and can be retried", async ({page}) => {
  await openLifecycle(page, [stdioConfig], {failConnect: "连接被拒绝：fixture"});
  const row = mcpRow(page, stdioConfig.name);
  const start = row.getByRole("button", {name: "启动 Local Stdio", exact: true});
  await start.click();
  await expect(row).toContainText("启动失败");
  await expect(row).toContainText("连接被拒绝：fixture");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(start).toBeEnabled();

  await page.evaluate(() => { (window as any).__MCP__.failConnect = undefined; });
  await start.click();
  await expect(row).toContainText("运行中");
  await expect(row).not.toContainText("连接被拒绝：fixture");
  expect(await lifecycleCalls(page)).toHaveLength(2);
});

test("bulk enable starts remaining MCPs, retries a partial failure, then offers bulk disable", async ({page}) => {
  await openLifecycle(page, [httpConfig, stdioConfig], {failConnect: "连接被拒绝：fixture", failConnectId: httpConfig.id});
  const httpRow = mcpRow(page, httpConfig.name);
  const stdioRow = mcpRow(page, stdioConfig.name);
  const availableTools = page.locator('[data-slot="card"]').filter({has: page.getByRole("heading", {name: "可用工具", exact: true})});
  const enableAll = availableTools.getByRole("button", {name: "全部启用", exact: true});
  const batchMenu = availableTools.getByRole("button", {name: "选择批量操作", exact: true});
  await expect(enableAll).toBeVisible();
  await expect(batchMenu).toBeVisible();

  await enableAll.click();
  await expect(httpRow).toContainText("启动失败");
  await expect(httpRow).toContainText("连接被拒绝：fixture");
  await expect(stdioRow).toContainText("运行中");
  expect((await lifecycleCalls(page)).filter((call: any) => call.command === "mcp_connect").map((call: any) => call.args.expectedConfig.id).sort()).toEqual([
    httpConfig.id,
    stdioConfig.id,
  ].sort());
  await expect(enableAll).toBeEnabled();

  await page.evaluate(() => { (window as any).__MCP__.failConnect = undefined; });
  await enableAll.click();
  await expect(httpRow).toContainText("运行中");
  await expect(stdioRow).toContainText("运行中");
  await expect(page.getByRole("button", {name: "全部禁用", exact: true})).toBeVisible();
  expect((await lifecycleCalls(page)).filter((call: any) => call.command === "mcp_connect").map((call: any) => call.args.expectedConfig.id).sort()).toEqual([
    httpConfig.id,
    httpConfig.id,
    stdioConfig.id,
  ].sort());
});

test("bulk disable clears a remembered disconnected MCP without stopping the separate local server", async ({page}) => {
  await openLifecycle(page, [httpConfig, stdioConfig], {
    active: [httpConfig.id],
    remembered: [stdioConfig.id],
    localRequested: true,
    localRunning: true,
  });
  const httpRow = mcpRow(page, httpConfig.name);
  const stdioRow = mcpRow(page, stdioConfig.name);
  await expect(httpRow).toContainText("运行中");
  await expect(stdioRow).toContainText("未启动");
  await expect(stdioRow.getByRole("button", {name: "取消 Local Stdio 自动启动", exact: true})).toBeVisible();

  const availableTools = page.locator('[data-slot="card"]').filter({has: page.getByRole("heading", {name: "可用工具", exact: true})});
  await expect(availableTools.getByRole("button", {name: "全部启用", exact: true})).toBeVisible();
  await availableTools.getByRole("button", {name: "选择批量操作", exact: true}).click();
  const menu = page.getByRole("menu");
  await expect(menu.getByRole("menuitem", {name: "全部启用", exact: true})).toBeVisible();
  await menu.getByRole("menuitem", {name: "全部禁用", exact: true}).click();

  await expect(httpRow).toContainText("未启动");
  await expect(stdioRow).toContainText("未启动");
  await expect(stdioRow.getByRole("button", {name: "取消 Local Stdio 自动启动", exact: true})).toHaveCount(0);
  const localCard = page.locator('[data-slot="card"]').filter({has: page.getByRole("heading", {name: "本地工具", exact: true})});
  await expect(localCard).toContainText("运行中");
  await expect(localCard.getByRole("button", {name: "停止工具", exact: true})).toBeVisible();
  expect((await lifecycleCalls(page)).filter((call: any) => call.command === "mcp_disconnect").map((call: any) => call.args.id).sort()).toEqual([
    httpConfig.id,
    stdioConfig.id,
  ].sort());
  expect(await page.evaluate(() => (window as any).__MCP__.calls.filter((call: any) => call.command === "mcp_server_stop" || call.command === "mcp_server_start"))).toEqual([]);
  expect(await page.evaluate(() => (window as any).__MCP__.remembered)).toEqual([]);
});

test("starting inbound local tools still requires confirmation", async ({page}) => {
  await openLifecycle(page, []);
  const start = page.getByRole("button", {name: "开启工具", exact: true});
  const dialog = page.getByRole("dialog", {name: "开启本地工具", exact: true});
  const localStarts = () => page.evaluate(() => (window as any).__MCP__.calls.filter((call: any) => call.command === "mcp_server_start"));
  await start.click();
  await expect(dialog).toBeVisible();
  expect(await localStarts()).toEqual([]);
  await dialog.getByRole("button", {name: "取消", exact: true}).click();
  await expect(dialog).toBeHidden();
  expect(await localStarts()).toEqual([]);

  await start.click();
  await dialog.getByRole("button", {name: "开启工具", exact: true}).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole("button", {name: "停止工具", exact: true})).toBeVisible();
  expect(await localStarts()).toEqual([{command: "mcp_server_start", args: {confirmation: "START_LOCAL_MCP"}}]);
});

test("removing a saved external connection still requires confirmation", async ({page}) => {
  await openLifecycle(page, [httpConfig]);
  const row = mcpRow(page, httpConfig.name);
  await row.getByRole("button", {name: "编辑 Remote HTTP", exact: true}).click();
  await page.getByRole("dialog", {name: "编辑 MCP 连接", exact: true}).getByRole("button", {name: "移除配置", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "移除 MCP 配置", exact: true});
  await expect(dialog).toBeVisible();
  const removals = () => page.evaluate(() => (window as any).__MCP__.calls.filter((call: any) => call.command === "mcp_remove"));
  expect(await removals()).toEqual([]);
  await dialog.getByRole("button", {name: "确认移除", exact: true}).click();
  await expect(dialog).toBeHidden();
  await expect(row).toHaveCount(0);
  expect(await removals()).toEqual([{command: "mcp_remove", args: {id: httpConfig.id}}]);
});

test("the editor has no separate enable switch", async ({page}) => {
  await openLifecycle(page, [httpConfig]);
  await page.getByRole("button", {name: "编辑 Remote HTTP", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "编辑 MCP 连接", exact: true});
  await expect(dialog.getByLabel("名称", {exact: true})).toBeVisible();
  await expect(dialog.getByRole("checkbox")).toHaveCount(0);
  await expect(dialog.getByRole("button", {name: "移除配置", exact: true})).toBeVisible();
});

test("MCP descriptions stay on one line and reveal their full text on hover and focus", async ({page}) => {
  const description = "A long service description that explains the purpose, supported operations, and expected behavior of this remote MCP connection. The complete text remains available when the inline summary is shortened.";
  const config = {...httpConfig, name: "Described MCP", description};
  const noDescriptionConfig = {...httpConfig, id: "fixture-no-description", name: "No description"};
  await openLifecycle(page, [config, noDescriptionConfig]);

  const row = mcpRow(page, config.name);
  const descriptionText = row.getByText(description, {exact: true});
  await expect(descriptionText).toBeVisible();
  await expect(row.getByText(config.name, {exact: true})).toBeVisible();
  const nameBox = await row.getByText(config.name, {exact: true}).boundingBox();
  const descriptionBox = await descriptionText.boundingBox();
  expect(nameBox && descriptionBox).toBeTruthy();
  if (nameBox && descriptionBox) {
    expect(Math.abs(nameBox.y - descriptionBox.y)).toBeLessThanOrEqual(3);
  }

  const overflow = await descriptionText.evaluate((element) => {
    const node = element as HTMLElement;
    return {clientWidth: node.clientWidth, scrollWidth: node.scrollWidth, clientHeight: node.clientHeight, scrollHeight: node.scrollHeight};
  });
  expect(overflow.clientWidth).toBeGreaterThan(0);
  expect(overflow.scrollWidth).toBeGreaterThan(overflow.clientWidth);
  expect(overflow.clientHeight).toBeGreaterThan(0);
  expect(overflow.scrollHeight).toBeLessThanOrEqual(overflow.clientHeight + 1);

  const tooltip = page.getByRole("tooltip", {name: description, exact: true});
  await descriptionText.hover();
  await expect(tooltip).toHaveText(description);
  const bounds = await tooltip.boundingBox();
  expect(bounds).toBeTruthy();
  expect(bounds!.x).toBeGreaterThanOrEqual(0);
  expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  await page.mouse.move(0, 0);
  await expect(tooltip).toBeHidden();

  await row.getByRole("button", {name: `配置 ${config.name} 凭据`, exact: true}).focus();
  await page.keyboard.press("Shift+Tab");
  await expect(descriptionText).toBeFocused();
  await expect(tooltip).toHaveText(description);

  const emptyDescriptionRow = mcpRow(page, noDescriptionConfig.name);
  await expect(emptyDescriptionRow).toBeVisible();
  await expect(emptyDescriptionRow).not.toContainText("undefined");
  await expect(emptyDescriptionRow).not.toContainText("null");
});

test("MCP descriptions can be added and edited, then appear in the connection list", async ({page}) => {
  await openLifecycle(page, []);
  const name = "Described HTTP service";
  const initialDescription = "Created service description";
  const updatedDescription = "Updated service description";
  const editor = page.getByRole("dialog", {name: "添加 MCP 连接", exact: true});

  await page.getByRole("button", {name: "添加连接", exact: true}).click();
  await expect(editor.getByLabel("描述（可选）", {exact: true})).toHaveValue("");
  await editor.getByLabel("名称", {exact: true}).fill(name);
  await editor.getByLabel("描述（可选）", {exact: true}).fill(initialDescription);
  await editor.getByLabel("URL", {exact: true}).fill("https://described.example.test/mcp");
  await editor.getByRole("button", {name: "保存配置", exact: true}).click();
  await expect(editor).toBeHidden();

  const created = (await mcpWriteCalls(page)).findLast((call: any) => call.command === "mcp_create")?.args.config;
  expect(created).toMatchObject({name, description: initialDescription});
  const row = mcpRow(page, name);
  await expect(row.getByText(initialDescription, {exact: true})).toBeVisible();

  await row.getByRole("button", {name: `编辑 ${name}`, exact: true}).click();
  const editDialog = page.getByRole("dialog", {name: "编辑 MCP 连接", exact: true});
  await expect(editDialog.getByLabel("描述（可选）", {exact: true})).toHaveValue(initialDescription);
  await editDialog.getByLabel("描述（可选）", {exact: true}).fill(updatedDescription);
  await editDialog.getByRole("button", {name: "保存配置", exact: true}).click();
  await expect(editDialog).toBeHidden();

  const updated = (await mcpWriteCalls(page)).findLast((call: any) => call.command === "mcp_update")?.args.config;
  expect(updated).toMatchObject({name, description: updatedDescription});
  await expect(row.getByText(updatedDescription, {exact: true})).toBeVisible();
  await expect(row.getByText(initialDescription, {exact: true})).toHaveCount(0);
});

test("credential URLs can be set and cleared when adding and editing either transport", async ({page}) => {
  await openLifecycle(page, []);
  const editor = page.getByRole("dialog", {name: /MCP 连接$/, exact: false});
  const credentialUrlField = () => editor.getByLabel("获取 Key 链接（可选）", {exact: true});
  const endpoint = "https://mcp.example.test/v1/mcp";
  const bearerSecretRef = "mcp.credential_url.http_api_key";
  const stdioSecretRef = "mcp.credential_url.stdio_api_key";

  for (const transport of ["streamable_http", "stdio"] as const) {
    const name = `Credential URL ${transport}`;
    const connectionConfig = transport === "streamable_http"
      ? {url: endpoint, bearer_secret_ref: bearerSecretRef, env_secret_refs: {}}
      : {command: "fixture-mcp", url: null, bearer_secret_ref: null, env_secret_refs: {FIXTURE_API_KEY: stdioSecretRef}};
    await page.getByRole("button", {name: "添加连接", exact: true}).click();
    await expect(editor).toBeVisible();
    await editor.getByLabel("名称", {exact: true}).fill(name);
    if (transport === "stdio") {
      await editor.getByLabel("传输", {exact: true}).selectOption("stdio");
      await editor.getByLabel("命令", {exact: true}).fill("fixture-mcp");
      await editor.getByLabel("环境变量与密钥引用 JSON", {exact: true}).fill(JSON.stringify({FIXTURE_API_KEY: stdioSecretRef}));
    } else {
      await editor.getByLabel("URL", {exact: true}).fill(endpoint);
      await editor.getByLabel("Bearer 密钥引用", {exact: true}).fill(bearerSecretRef);
    }
    await credentialUrlField().fill(`https://keys.example.test/${transport}`);
    await editor.getByRole("button", {name: "保存配置", exact: true}).click();
    await expect(editor).toBeHidden();

    const created = await page.evaluate((configName) => (window as any).__MCP__.calls
      .findLast((call: any) => call.command === "mcp_create" && call.args.config.name === configName), name);
    expect(created.args.config).toMatchObject({
      name,
      transport,
      credential_url: `https://keys.example.test/${transport}`,
      ...connectionConfig,
    });

    const editedUrl = `https://keys.example.test/${transport}/edited`;
    await mcpRow(page, name).getByRole("button", {name: `编辑 ${name}`, exact: true}).click();
    await expect(credentialUrlField()).toHaveValue(`https://keys.example.test/${transport}`);
    await credentialUrlField().fill(editedUrl);
    await editor.getByRole("button", {name: "保存配置", exact: true}).click();
    await expect(editor).toBeHidden();
    const updated = await page.evaluate((configName) => (window as any).__MCP__.calls
      .filter((call: any) => call.command === "mcp_update" && call.args.config.name === configName).at(-1), name);
    expect(updated.args.config).toMatchObject({
      name,
      transport,
      credential_url: editedUrl,
      ...connectionConfig,
    });

    await mcpRow(page, name).getByRole("button", {name: `编辑 ${name}`, exact: true}).click();
    await expect(credentialUrlField()).toHaveValue(editedUrl);
    await credentialUrlField().fill("");
    await editor.getByRole("button", {name: "保存配置", exact: true}).click();
    await expect(editor).toBeHidden();
    const cleared = await page.evaluate((configName) => (window as any).__MCP__.calls
      .filter((call: any) => call.command === "mcp_update" && call.args.config.name === configName).at(-1), name);
    expect(cleared.args.config).toMatchObject({name, transport, credential_url: null});
  }
});

test("invalid credential URLs do not create or update MCP configs", async ({page}) => {
  await openLifecycle(page, [httpConfig]);
  const editor = page.getByRole("dialog", {name: /MCP 连接$/, exact: false});
  const invalidCredentialUrl = "ftp://keys.example.test/account";
  const validationError = "获取 Key 链接必须是有效的 HTTP 或 HTTPS 网址";

  await page.getByRole("button", {name: "添加连接", exact: true}).click();
  await editor.getByLabel("名称", {exact: true}).fill("Invalid credential URL");
  await editor.getByLabel("URL", {exact: true}).fill("https://mcp.example.test/v1/mcp");
  await editor.getByLabel("Bearer 密钥引用", {exact: true}).fill("mcp.invalid_credential_url.api_key");
  await editor.getByLabel("获取 Key 链接（可选）", {exact: true}).fill(invalidCredentialUrl);
  await editor.getByRole("button", {name: "保存配置", exact: true}).click();
  await expect(editor).toContainText(validationError);
  expect(await mcpWriteCalls(page)).toEqual([]);
  await editor.getByRole("button", {name: "取消", exact: true}).click();

  await page.getByRole("button", {name: "编辑 Remote HTTP", exact: true}).click();
  await editor.getByLabel("获取 Key 链接（可选）", {exact: true}).fill(invalidCredentialUrl);
  await editor.getByRole("button", {name: "保存配置", exact: true}).click();
  await expect(editor).toContainText(validationError);
  expect(await mcpWriteCalls(page)).toEqual([]);
});

test("the 3DM HTTPS endpoint labels its API key and allows an anonymous configuration", async ({page}) => {
  const config = {...httpConfig, url: "HTTPS://MOD.3DMGAME.COM:443/mcp/", bearer_secret_ref: "mcp.custom.api_key"};
  await openLifecycle(page, [config]);
  const row = mcpRow(page, config.name);
  const credentials = page.getByRole("dialog", {name: "配置 MCP 凭据", exact: true});
  await row.getByRole("button", {name: "配置 Remote HTTP 凭据", exact: true}).click();
  await expect(credentials.getByLabel("3DM API Key", {exact: true})).toHaveAttribute("type", "password");
  await credentials.getByRole("button", {name: "返回", exact: true}).click();

  await row.getByRole("button", {name: "编辑 Remote HTTP", exact: true}).click();
  const editor = page.getByRole("dialog", {name: "编辑 MCP 连接", exact: true});
  const url = editor.getByLabel("URL", {exact: true});
  const keyRef = editor.getByLabel("3DM API Key 引用", {exact: true});
  await expect(keyRef).toHaveValue(config.bearer_secret_ref);
  for (const other of [
    "https://mod.3dmgame.com.example.test/mcp",
    "https://mod.3dmgame.com:8443/mcp",
    "https://mod.3dmgame.com/mcp/other",
    "http://mod.3dmgame.com/mcp",
  ]) {
    await url.fill(other);
    await expect(editor.getByLabel("Bearer 密钥引用", {exact: true})).toBeVisible();
  }
  await url.fill("https://mod.3dmgame.com/mcp");
  await expect(keyRef).toHaveAccessibleDescription(/留空可匿名连接/);
  await keyRef.fill("");
  await editor.getByRole("button", {name: "保存配置", exact: true}).click();
  await expect(editor).toBeHidden();
  expect((await mcpWriteCalls(page)).at(-1)?.args.config).toMatchObject({url: "https://mod.3dmgame.com/mcp", bearer_secret_ref: null});
  await expect(row.getByRole("button", {name: "配置 Remote HTTP 凭据", exact: true})).toHaveCount(0);
});

test("a configured credential URL is offered in the credentials dialog and opens externally", async ({page}) => {
  const credentialUrl = "https://keys.example.test/account";
  await openLifecycle(page, [{...httpConfig, credential_url: credentialUrl}]);

  const dialog = page.getByRole("dialog", {name: "配置 MCP 凭据", exact: true});
  await page.getByRole("button", {name: "配置 Remote HTTP 凭据", exact: true}).click();
  const link = dialog.getByRole("link", {name: "获取 Key", exact: true});
  await expect(link).toHaveAttribute("href", credentialUrl);
  await link.click();

  await expect.poll(() => page.evaluate(() => (window as any).__AMM_TEST__.calls
    .filter((call: any) => call.command === "plugin:opener|open_url")
    .map((call: any) => call.args))).toEqual([{url: credentialUrl}]);
  await expect(dialog).toBeVisible();
});

test("remembered MCP services refresh after background restore and can be forgotten while offline", async ({page}) => {
  await openLifecycle(page, [httpConfig, stdioConfig, {...httpConfig, id: "amm_local", name: "Managed local entry"}], {
    remembered: [httpConfig.id, stdioConfig.id], localRequested: true,
  });
  await expect(mcpRow(page, "Managed local entry")).toHaveCount(0);
  const cancel = page.getByRole("button", {name: "取消 Remote HTTP 自动启动", exact: true});
  await cancel.click();
  await expect(cancel).toHaveCount(0);
  expect(await lifecycleCalls(page)).toEqual([{command: "mcp_disconnect", args: {id: httpConfig.id}}]);
  const cancelLocal = page.getByRole("button", {name: "取消本地工具自动启动", exact: true});
  await cancelLocal.click();
  await expect(cancelLocal).toHaveCount(0);
  await page.evaluate(() => {
    (window as any).__MCP__.active = ["fixture-stdio"];
    (window as any).__AMM_TEST__.emitEvent("mcp://restore-progress", null);
  });
  await expect(mcpRow(page, stdioConfig.name)).toContainText("运行中");
  await expect(page.getByRole("button", {name: "停止 Local Stdio"})).toBeVisible();
});
