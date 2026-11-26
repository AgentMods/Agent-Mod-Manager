import { expect, test, type Page } from "@playwright/test";
import type { McpConfig } from "../../src/lib/mcp-connection";
import { installMockTauri } from "./mock-tauri";

const stdioConfig: McpConfig = {
  id: "plugin_minecraft-java_local",
  name: "Fixture local tool",
  plugin_slug: "minecraft-java",
  transport: "stdio",
  command: "C:\\Program Files\\Fixture\\tool.exe",
  args: ["--stdio", "with  spaces", "", 'a"quote'],
  url: null,
  env_secret_refs: { FIXTURE_TOKEN: "mcp.fixture.local.token" },
  bearer_secret_ref: null,
  enabled: false,
  builtin: false,
};
const httpConfig: McpConfig = {
  id: "plugin_minecraft-java_remote",
  name: "Fixture remote tool",
  plugin_slug: "minecraft-java",
  transport: "streamable_http",
  command: null,
  args: [],
  url: "https://mcp.example.test/exact%2Fpath/name/a%20b",
  env_secret_refs: {},
  bearer_secret_ref: "mcp.fixture.remote.bearer",
  credential_url: "https://mcp.example.test/credentials",
  enabled: true,
  builtin: false,
};

const review = (page: Page) => page.getByRole("button", { name: "查看授权请求", exact: true });
const dialog = (page: Page) => page.getByRole("dialog", { name: "适配包工具授权", exact: true });
const calls = (page: Page) => page.evaluate(() => (window as any).__GAME_TOOL_MCP__.calls);

async function loadFixture(page: Page, pending: McpConfig[], { onboarding = false, rejectFirst = false } = {}) {
  await installMockTauri(page, { withExternalAdapter: true, onboarding, pendingGameTools: pending });
  await page.goto("/");
  await page.evaluate(({ configs, rejectFirst }) => {
    const fixture = {
      configs,
      pending: structuredClone(configs),
      active: [] as string[],
      calls: [] as Array<{ command: string; args: unknown }>,
      failReads: false,
      fail: undefined as (() => void) | undefined,
    };
    (window as any).__GAME_TOOL_MCP__ = fixture;
    const internals = (window as any).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = async (command: string, args: any = {}, ...rest: unknown[]) => {
      if (command === "mcp_pending_game_tools") {
        if (fixture.failReads) throw new Error("Fixture pending read failed");
        return structuredClone(fixture.pending);
      }
      if (command === "mcp_list") return structuredClone(fixture.configs);
      if (command === "mcp_connections") return fixture.active.map((serverId) => ({ serverId, tools: ["fixture_tool"] }));
      if (command === "mcp_approve_game_tool" || command === "mcp_decline_game_tool") {
        fixture.calls.push({ command, args: structuredClone(args) });
        if (rejectFirst && fixture.calls.length === 1) {
          await new Promise((_, reject) => { fixture.fail = () => reject(new Error("Fixture decision failed")); });
        }
        fixture.pending = fixture.pending.filter(({ id }) => id !== args.expectedConfig.id);
        return null;
      }
      if (command === "mcp_disconnect") {
        fixture.calls.push({ command, args: structuredClone(args) });
        fixture.active = fixture.active.filter((id) => id !== args.id);
        return null;
      }
      if (command === "mcp_connect" || command === "mcp_update") fixture.calls.push({ command, args: structuredClone(args) });
      return invoke(command, args, ...rest);
    };
  }, { configs: pending, rejectFirst });
  await expect.poll(() => page.evaluate(() => (window as any).__AMM_TEST__.calls.some(
    (call: any) => call.command === "plugin:event|listen" && call.args.event === "mcp://restore-progress",
  ))).toBe(true);
}

async function publishProgress(page: Page) {
  await page.evaluate(() => (window as any).__AMM_TEST__.emitEvent("mcp://restore-progress", null));
}

async function openTools(page: Page) {
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await page.getByRole("tab", { name: "工具", exact: true }).click();
}

test("reviews the exact startup config globally and refreshes automatically started and stopped tools", async ({ page }) => {
  await loadFixture(page, [stdioConfig]);
  await expect(page.getByText("适配包「Minecraft Java Edition」有工具待授权。", { exact: true })).toBeVisible();
  await review(page).click();
  const prompt = dialog(page);
  await expect(prompt).toContainText("Minecraft Java Edition (minecraft-java)");
  await expect(prompt).toContainText("此 stdio 进程将在沙箱外以当前系统用户的权限运行。");
  expect(await prompt.locator("pre").allTextContents()).toEqual(
    [stdioConfig.command, stdioConfig.args, stdioConfig.env_secret_refs].map((value) => JSON.stringify(value, null, 2)),
  );
  await page.keyboard.press("Escape");
  await expect(prompt).toHaveCount(0);
  await expect(review(page)).toBeFocused();
  expect(await calls(page)).toEqual([]);

  await review(page).click();
  await prompt.getByRole("button", { name: "允许并启动", exact: true }).click();
  await expect(review(page)).toHaveCount(0);
  await expect(prompt).toHaveCount(0);
  const approved = { command: "mcp_approve_game_tool", args: { expectedConfig: stdioConfig } };
  expect(await calls(page)).toEqual([approved]);

  await openTools(page);
  const row = page.getByRole("list", { name: "外部 MCP 连接", exact: true })
    .getByRole("listitem").filter({ hasText: stdioConfig.name });
  await expect(row).toContainText("未启动");
  await page.evaluate((id) => {
    (window as any).__GAME_TOOL_MCP__.active.push(id);
    (window as any).__AMM_TEST__.emitEvent("mcp://restore-progress", null);
  }, stdioConfig.id);
  await expect(row).toContainText("运行中");
  await row.getByRole("button", { name: `停止 ${stdioConfig.name}`, exact: true }).click();
  await expect(row).toContainText("未启动");
  await expect(review(page)).toHaveCount(0);
  expect(await calls(page)).toEqual([approved, { command: "mcp_disconnect", args: { id: stdioConfig.id } }]);
});

test("invalidates a stale review and declines the new HTTP config unchanged", async ({ page }) => {
  await loadFixture(page, [httpConfig]);
  await review(page).click();
  const prompt = dialog(page);
  await expect(prompt).toContainText(httpConfig.url!);
  await expect(prompt).not.toContainText("当前系统用户");

  const updated = { ...httpConfig, url: "https://mcp.example.test/new%2Fpath/name/a%20b" };
  await page.evaluate((config) => {
    (window as any).__GAME_TOOL_MCP__.pending = [config];
    (window as any).__AMM_TEST__.emitEvent("mcp://restore-progress", null);
  }, updated);
  await expect(prompt).toHaveCount(0);
  expect(await calls(page)).toEqual([]);
  await review(page).click();
  expect(await prompt.locator("pre").allTextContents()).toEqual(
    [updated.url, updated.bearer_secret_ref, updated.credential_url].map((value) => JSON.stringify(value)),
  );
  await prompt.getByRole("button", { name: "拒绝", exact: true }).click();
  await expect(prompt).toHaveCount(0);
  await publishProgress(page);
  await expect(review(page)).toHaveCount(0);
  expect(await calls(page)).toEqual([{ command: "mcp_decline_game_tool", args: { expectedConfig: updated } }]);
});

test("shows pending tools on global invalidation and keeps a failed decision retryable", async ({ page }) => {
  await loadFixture(page, [], { rejectFirst: true });
  await page.getByRole("button", { name: "插件中心", exact: true }).click();
  await page.getByRole("button").filter({ hasText: "Minecraft Java Edition" }).click();
  const toggle = page.getByLabel("启用适配包 Minecraft Java Edition", { exact: true });
  await toggle.uncheck();
  await expect(review(page)).toHaveCount(0);
  await page.evaluate((config) => { (window as any).__GAME_TOOL_MCP__.pending = [config]; }, httpConfig);
  await toggle.check();
  await review(page).click();
  const prompt = dialog(page);
  const allow = prompt.getByRole("button", { name: "允许并启动", exact: true });
  const decline = prompt.getByRole("button", { name: "拒绝", exact: true });
  await allow.click();
  await expect(allow).toBeDisabled();
  await expect(decline).toBeDisabled();
  await page.keyboard.press("Escape");
  await expect(prompt).toBeVisible();
  expect(await calls(page)).toHaveLength(1);
  await page.evaluate(() => (window as any).__GAME_TOOL_MCP__.fail());
  await expect(prompt.getByRole("alert")).toContainText("Fixture decision failed");
  await decline.click();
  await expect(prompt).toHaveCount(0);
  expect(await calls(page)).toEqual([
    { command: "mcp_approve_game_tool", args: { expectedConfig: httpConfig } },
    { command: "mcp_decline_game_tool", args: { expectedConfig: httpConfig } },
  ]);
});

test("offers pending tools in onboarding and advances the review queue after a decision", async ({ page }) => {
  await loadFixture(page, [httpConfig, stdioConfig], { onboarding: true });
  await page.getByRole("button", { name: "开始设置", exact: true }).click();
  await expect(page.getByRole("heading", { name: "设置数据存储目录", exact: true })).toBeVisible();
  await review(page).click();
  const prompt = dialog(page);
  await expect(prompt).toContainText(httpConfig.name);
  await prompt.getByRole("button", { name: "拒绝", exact: true }).click();
  await expect(prompt).toHaveCount(0);
  await review(page).click();
  await expect(prompt).toContainText(stdioConfig.name);
});

test("keeps an open review read-only after refresh fails and permits retry", async ({ page }) => {
  await loadFixture(page, [httpConfig]);
  await review(page).click();
  const prompt = dialog(page);
  await page.evaluate(() => {
    (window as any).__GAME_TOOL_MCP__.failReads = true;
    (window as any).__AMM_TEST__.emitEvent("mcp://restore-progress", null);
  });
  await expect(prompt.getByRole("alert")).toContainText("Fixture pending read failed");
  await expect(prompt.getByRole("button", { name: "允许并启动", exact: true })).toBeDisabled();
  await expect(prompt.getByRole("button", { name: "拒绝", exact: true })).toBeDisabled();
  expect(await calls(page)).toEqual([]);

  await page.evaluate(() => { (window as any).__GAME_TOOL_MCP__.failReads = false; });
  await prompt.getByRole("button", { name: "重试", exact: true }).click();
  await expect(prompt.getByRole("button", { name: "允许并启动", exact: true })).toBeEnabled();
  await expect(prompt.getByRole("button", { name: "拒绝", exact: true })).toBeEnabled();
});
