import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

type McpFixture = {
  id: string;
  name: string;
  transport: "stdio" | "streamable_http";
  command: string | null;
  args: string[];
  url: string | null;
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

test("HTTP credentials use the bearer reference, remain masked, and reopen blank", async ({page}) => {
  await openMcpSettings(page);

  const trigger = page.getByRole("button", {name: "配置 Remote HTTP 凭据", exact: true});
  const dialog = page.getByRole("dialog", {name: "配置 MCP 凭据", exact: true});
  await trigger.click();
  await expect(dialog).toContainText("Remote HTTP");

  const bearer = dialog.getByLabel("Bearer 密钥", {exact: true});
  await expect(bearer).toHaveAttribute("type", "password");
  await bearer.fill("discard-on-close");
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(trigger).toBeFocused();

  await trigger.click();
  await expect(bearer).toHaveValue("");
  await expect(bearer).toHaveAttribute("type", "password");
  await bearer.fill("http-secret-fixture");
  await dialog.getByRole("button", {name: "保存Bearer 密钥", exact: true}).click();

  await expect(bearer).toHaveValue("");
  expect(await secretCalls(page)).toEqual([{
    command: "set_secret",
    args: {key: "mcp.remote_http.bearer", value: "http-secret-fixture"},
  }]);
});

test("stdio credentials expose one labeled field per reference and save each reference", async ({page}) => {
  await openMcpSettings(page);

  await page.getByRole("button", {name: "配置 Local Stdio 凭据", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "配置 MCP 凭据", exact: true});
  await expect(dialog).toContainText("Local Stdio");

  const githubToken = dialog.getByLabel("GITHUB_TOKEN", {exact: true});
  const mapsKey = dialog.getByLabel("MAPS_API_KEY", {exact: true});
  await expect(dialog.locator('input[type="password"]')).toHaveCount(2);
  await expect(githubToken).toHaveAttribute("type", "password");
  await expect(mapsKey).toHaveAttribute("type", "password");

  await githubToken.fill("stdio-github-fixture");
  await dialog.getByRole("button", {name: "保存GITHUB_TOKEN", exact: true}).click();
  await mapsKey.fill("stdio-maps-fixture");
  await dialog.getByRole("button", {name: "保存MAPS_API_KEY", exact: true}).click();

  expect(await secretCalls(page)).toEqual([
    {command: "set_secret", args: {key: "mcp.local_stdio.github_token", value: "stdio-github-fixture"}},
    {command: "set_secret", args: {key: "mcp.local_stdio.maps_key", value: "stdio-maps-fixture"}},
  ]);
});
