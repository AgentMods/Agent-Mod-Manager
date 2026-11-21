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
  enabled: boolean;
  builtin: boolean;
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
  await page.getByRole("tab", {name: "MCP 服务", exact: true}).click();
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
async function openLifecycle(page: Page, configs: McpFixture[], {failConnect, remembered = [], localRequested = false}: {failConnect?: string; remembered?: string[]; localRequested?: boolean} = {}) {
  await installMockTauri(page);
  await page.goto("/");
  await page.evaluate(({configs, failConnect, remembered, localRequested}) => {
    const internals = (window as any).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    const state = {configs: structuredClone(configs), remembered, localRequested, active: [] as string[], calls: [] as Array<{command: string; args: any}>};
    (window as any).__MCP__ = state;
    internals.invoke = async (command: string, args: any, ...rest: unknown[]) => {
      if (command === "mcp_server_status") return {...await invoke(command, args, ...rest), startupRequested: state.localRequested};
      if (command === "mcp_server_stop") { state.localRequested = false; return; }
      if (!command.startsWith("mcp_") || command.startsWith("mcp_server")) return invoke(command, args, ...rest);
      state.calls.push({command, args: structuredClone(args)});
      if (command === "mcp_list") return structuredClone(state.configs);
      if (command === "mcp_startup_requests") return [...state.remembered];
      if (command === "mcp_connections") return state.active.map((serverId) => ({serverId, tools: ["fixture_search", "fixture_read"]}));
      if (command === "mcp_update") { state.configs = state.configs.map((item) => item.id === args.config.id ? args.config : item); return null; }
      if (command === "mcp_create") { state.configs.push(args.config); return null; }
      if (command === "mcp_connect") {
        if (failConnect) throw new Error(failConnect);
        state.active.push(args.expectedConfig.id);
        return {serverId: args.expectedConfig.id, tools: ["fixture_search", "fixture_read"]};
      }
      if (command === "mcp_disconnect") { state.remembered = state.remembered.filter((id) => id !== args.id); state.active = state.active.filter((id) => id !== args.id); return null; }
      return invoke(command, args, ...rest);
    };
  }, {configs, failConnect, remembered, localRequested});
  await page.getByRole("button", {name: "设置", exact: true}).click();
  await page.getByRole("tab", {name: "MCP 服务", exact: true}).click();
}

// Scoped to the MCP list: toasts are also list items and repeat the service name.
const mcpRow = (page: Page, name: string) => page.getByRole("list", {name: "外部 MCP 连接", exact: true}).getByRole("listitem").filter({hasText: name});

const lifecycleCalls = (page: Page) => page.evaluate(() => (window as any).__MCP__.calls
  .filter((call: any) => ["mcp_update", "mcp_connect", "mcp_disconnect"].includes(call.command))
  .map((call: any) => ({command: call.command, args: call.args})));

const mcpWriteCalls = (page: Page) => page.evaluate(() => (window as any).__MCP__.calls
  .filter((call: any) => ["mcp_create", "mcp_update"].includes(call.command)));

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

test("a stdio service asks once, with its command line, before it starts", async ({page}) => {
  await openLifecycle(page, [{...stdioConfig, command: "fixture-mcp", args: ["--stdio"]}]);
  const row = mcpRow(page, "Local Stdio");

  await row.getByRole("button", {name: "启动 Local Stdio", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "启动 Local Stdio", exact: true});
  await expect(dialog).toContainText("fixture-mcp --stdio");
  await expect(dialog.getByText("首次准备 Python", {exact: false})).toHaveCount(0);
  await expect(dialog.getByText("--help", {exact: true})).toHaveCount(0);
  await expect(dialog.getByRole("checkbox")).toHaveCount(0);
  await dialog.getByRole("button", {name: "取消", exact: true}).click();
  await expect(dialog).toBeHidden();
  expect(await lifecycleCalls(page)).toEqual([]);
  await expect(row).toContainText("未启动");

  await row.getByRole("button", {name: "启动 Local Stdio", exact: true}).click();
  await dialog.getByRole("button", {name: "启动", exact: true}).click();
  await expect(dialog).toBeHidden();
  await expect(row).toContainText("运行中");
  expect(await lifecycleCalls(page)).toEqual([
    {command: "mcp_connect", args: {expectedConfig: {...stdioConfig, command: "fixture-mcp", args: ["--stdio"]}, acceptsUnsandboxedStdio: true}},
  ]);
});

for (const [command, mirror] of [
  ["uvx", "清华 PyPI"], ["UVX.EXE", "清华 PyPI"], ["npx", "npmmirror"], ["npx.cmd", "npmmirror"], ["NPX.EXE", "npmmirror"],
  ["/opt/uv/bin/uvx", ""], [String.raw`C:\Program Files\nodejs\npx.cmd`, ""], ["/opt/tools/not-uvx.exe", ""],
]) {
  test(`package mirror disclosure follows the configured launcher: ${command}`, async ({page}) => {
    const config = {...stdioConfig, command, args: ["fixture-package"]};
    await openLifecycle(page, [config]);
    await mcpRow(page, config.name).getByRole("button", {name: `启动 ${config.name}`, exact: true}).click();
    const dialog = page.getByRole("dialog", {name: `启动 ${config.name}`, exact: true});
    await expect(dialog).toContainText(`${command} fixture-package`);
    if (mirror) await expect(dialog).toContainText(mirror);
    else await expect(dialog).not.toContainText("使用应用内置的");
    await dialog.getByRole("button", {name: "取消", exact: true}).click();
    expect(await lifecycleCalls(page)).toEqual([]);
  });
}

test("a failed start stays visible with its reason and can be retried", async ({page}) => {
  await openLifecycle(page, [httpConfig], {failConnect: "连接被拒绝：fixture"});
  const row = mcpRow(page, "Remote HTTP");
  await row.getByRole("button", {name: "启动 Remote HTTP", exact: true}).click();
  await expect(row).toContainText("启动失败");
  await expect(row).toContainText("连接被拒绝：fixture");
  await expect(row.getByRole("button", {name: "启动 Remote HTTP", exact: true})).toBeEnabled();
});

test("the editor has no separate enable switch", async ({page}) => {
  await openLifecycle(page, [httpConfig]);
  await page.getByRole("button", {name: "编辑 Remote HTTP", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "编辑 MCP 连接", exact: true});
  await expect(dialog.getByLabel("名称", {exact: true})).toBeVisible();
  await expect(dialog.getByRole("checkbox")).toHaveCount(0);
  await expect(dialog.getByRole("button", {name: "移除配置", exact: true})).toBeVisible();
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
  const cancelLocal = page.getByRole("button", {name: "取消本地 MCP 自动启动", exact: true});
  await cancelLocal.click();
  await expect(cancelLocal).toHaveCount(0);
  await page.evaluate(() => {
    (window as any).__MCP__.active = ["fixture-stdio"];
    (window as any).__AMM_TEST__.emitEvent("mcp://restore-progress", null);
  });
  await expect(mcpRow(page, stdioConfig.name)).toContainText("运行中");
  await expect(page.getByRole("button", {name: "停止 Local Stdio"})).toBeVisible();
});
