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
