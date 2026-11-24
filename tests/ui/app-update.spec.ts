import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

async function openSettings(page: Page, appUpdate?: Parameters<typeof installMockTauri>[1]["appUpdate"]) {
  await installMockTauri(page, {appUpdate});
  await page.goto("/");
  await page.getByRole("button", {name: "设置", exact: true}).click();
}

function updateCalls(page: Page) {
  return page.evaluate(() => (window as typeof window & {
    __AMM_TEST__: {calls: {command: string; args: Record<string, unknown>}[]};
  }).__AMM_TEST__.calls.filter(({command}) => command === "get_app_update_status" || command === "check_app_update"));
}

function ipcCalls(page: Page) {
  return page.evaluate(() => (window as typeof window & {
    __AMM_TEST__: {calls: {command: string; args: Record<string, unknown>}[]};
  }).__AMM_TEST__.calls);
}

async function updateRouteSelect(page: Page) {
  const route = page.getByLabel("下载线路", {exact: true});
  await expect(route).toBeEnabled();
  return route;
}

test("更新线路默认使用 cdn.gh-proxy.org，并提供六条线路", async ({page}) => {
  await openSettings(page);
  const route = await updateRouteSelect(page);
  const values = await route.locator("option").evaluateAll(options => options.map(option => (option as HTMLOptionElement).value));

  expect(values).toEqual(["cdn.gh-proxy.org", "gh-proxy.org", "ghfast.top", "gh-proxy.com", "ghproxy.net", "direct"]);
  await expect(route).toHaveValue("cdn.gh-proxy.org");
  await expect(route.locator('option[value="direct"]')).toHaveText("GitHub 直连");
  await expect.poll(async () => (await ipcCalls(page)).some(({command}) => command === "get_app_update_route")).toBe(true);
});

test("选择更新线路后离开并返回设置仍保留", async ({page}) => {
  await openSettings(page);
  const route = await updateRouteSelect(page);
  const hostRoute = "ghfast.top";

  await route.selectOption(hostRoute);
  await expect.poll(async () => (await ipcCalls(page)).some(({command, args}) => command === "set_app_update_route" && args.route === hostRoute)).toBe(true);
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "设置", exact: true}).click();

  await expect(await updateRouteSelect(page)).toHaveValue(hostRoute);
  await expect.poll(async () => (await ipcCalls(page)).some(({command}) => command === "get_app_update_route")).toBe(true);
});

test("先保存所选线路，再使用该线路手动检查更新", async ({page}) => {
  await openSettings(page, {checkResult: {status: "upToDate"}});
  const route = await updateRouteSelect(page);
  const hostRoute = "ghfast.top";
  await route.selectOption(hostRoute);
  await expect.poll(async () => (await ipcCalls(page)).some(({command, args}) => command === "set_app_update_route" && args.route === hostRoute)).toBe(true);

  await page.getByRole("button", {name: "检查更新", exact: true}).click();
  await expect(page.getByRole("status")).toHaveText("当前已是最新版本。");
  const commands = (await ipcCalls(page)).map(({command}) => command);
  expect(commands.indexOf("set_app_update_route")).toBeGreaterThan(-1);
  expect(commands.indexOf("check_app_update")).toBeGreaterThan(commands.indexOf("set_app_update_route"));
});

test("保存更新线路失败时显示错误且可继续操作", async ({page}) => {
  await installMockTauri(page, {failAppUpdateRoute: true});
  await page.goto("/");
  await page.getByRole("button", {name: "设置", exact: true}).click();
  const route = await updateRouteSelect(page);
  const hostRoute = "ghfast.top";

  await route.selectOption(hostRoute);
  await expect(page.getByText("线路保存失败", {exact: false})).toBeVisible();
  await expect(route).toHaveValue("cdn.gh-proxy.org");
});

test("设置中可手动检查更新，并显示最新版本结果", async ({page}) => {
  await openSettings(page, {checkResult: {status: "upToDate"}});
  const check = page.getByRole("button", {name: "检查更新", exact: true});
  await expect(check).toBeVisible();
  await check.click();

  await expect(page.getByRole("status")).toHaveText("当前已是最新版本。");
  await expect.poll(async () => (await updateCalls(page)).some(({command}) => command === "check_app_update")).toBe(true);
});

test("ready 状态显示版本并提示退出时安装", async ({page}) => {
  await openSettings(page, {status: "ready", version: "9.8.7"});

  await expect(page.getByRole("status")).toHaveText("新版本 9.8.7 已下载，关闭程序后将自动安装。");
  await expect(page.getByRole("button", {name: "检查更新", exact: true})).toBeDisabled();
});

test("更新下载忙碌时禁用重复检查", async ({page}) => {
  await openSettings(page, {status: "downloading"});
  await expect(page.getByRole("status")).toHaveText("正在后台下载新版本…");
  await expect(page.getByRole("button", {name: "检查更新", exact: true})).toBeDisabled();
});

test("手动检查返回 checking 后轮询到 ready", async ({page}) => {
  await openSettings(page, {
    checkResult: {status: "checking"},
    pollStatuses: [{status: "ready", version: "9.8.7"}],
  });

  await page.getByRole("button", {name: "检查更新", exact: true}).click();
  await expect(page.getByRole("status")).toHaveText("正在检查更新…");
  await expect(page.getByRole("status")).toHaveText("新版本 9.8.7 已下载，关闭程序后将自动安装。", {timeout: 10_000});

  await expect.poll(async () => (await updateCalls(page)).filter(({command}) => command === "get_app_update_status").length).toBeGreaterThanOrEqual(2);
});

test("错误状态显示后端消息，并允许重试", async ({page}) => {
  await openSettings(page, {status: "error", message: "更新源暂时不可用"});
  await expect(page.getByRole("status")).toHaveText("更新源暂时不可用");
  await expect(page.getByRole("button", {name: "检查更新", exact: true})).toBeEnabled();
});
