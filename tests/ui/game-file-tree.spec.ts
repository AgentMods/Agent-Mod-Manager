import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

type Entry = { name: string; path: string; is_dir: boolean; size: number; modified: string };

const root = "/sandbox/minecraft";
const config = `${root}/config`;
const ui = `${config}/ui`;
const mods = `${root}/mods`;
const modified = "2026-11-21T00:00:00Z";

const folder = (name: string, path: string): Entry => ({ name, path, is_dir: true, size: 0, modified });
const file = (name: string, path: string, size = 128): Entry => ({ name, path, is_dir: false, size, modified });

const fixtureDirectories: Record<string, Entry[]> = {
  [root]: [
    folder("config", config),
    folder("mods", mods),
    file("options.txt", `${root}/options.txt`),
    file("README.md", `${root}/README.md`),
  ],
  [config]: [folder("ui", ui), file("settings.ini", `${config}/settings.ini`)],
  [ui]: [file("layout.json", `${ui}/layout.json`)],
  [mods]: [file("unexpanded-only.jar", `${mods}/unexpanded-only.jar`)],
  "/sandbox/shared": [file("shared-config.ini", "/sandbox/shared/shared-config.ini")],
};

async function installFileTreeFixture(page: Page, withExtraRoot = false) {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", { name: "游戏库", exact: true }).click();

  await page.evaluate(({ directories, withExtraRoot }) => {
    const target = window as any;
    const internals = target.__TAURI_INTERNALS__;
    const originalInvoke = internals.invoke;
    const entriesByPath = new Map(Object.entries(directories).map(([path, entries]) => [path, structuredClone(entries)]));
    internals.invoke = async (command: string, args: Record<string, any> = {}, options?: unknown) => {
      if (command === "read_dir") {
        await originalInvoke(command, args, options);
        return structuredClone(entriesByPath.get(args.path) ?? []);
      }
      if (command === "read_file") {
        await originalInvoke(command, args, options);
        return { lines: [{ line: 1, text: `fixture preview: ${args.path}` }], truncated: false };
      }
      if (command === "list_user_roots" && withExtraRoot) {
        await originalInvoke(command, args, options);
        return { shared: "/sandbox/shared" };
      }
      return originalInvoke(command, args, options);
    };
    target.__FILE_TREE_TEST__ = {
      replaceDirectory(path: string, entries: unknown[]) {
        entriesByPath.set(path, structuredClone(entries));
      },
    };
  }, { directories: fixtureDirectories, withExtraRoot });

  const gameCard = page.locator('[data-slot="card"]').filter({
    has: page.getByRole("heading", { name: "Minecraft Java Edition", exact: true }),
  });
  await gameCard.getByRole("button", { name: /Minecraft Java Edition/ }).click();
  const detail = page.getByTestId("game-detail-page");
  await detail.getByRole("tab", { name: "文件", exact: true }).click();
  await expect(detail.getByRole("button", { name: "游戏目录", exact: true })).toBeVisible();
  return detail;
}

async function commandCalls(page: Page, command: string) {
  return page.evaluate((name) => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === name), command);
}

test("游戏根目录默认展开，子目录可用键盘原位展开折叠，文件按钮选择预览", async ({ page }) => {
  const detail = await installFileTreeFixture(page);
  await expect(detail.getByRole("button", { name: "config", exact: true })).toBeVisible();
  await expect(detail.getByRole("button", { name: "options.txt", exact: true })).toBeVisible();
  await expect(detail.getByRole("combobox", { name: "浏览根目录", exact: true })).toHaveCount(0);
  expect((await commandCalls(page, "read_dir")).map((call: any) => call.args.path)).toEqual([root]);

  const configButton = detail.getByRole("button", { name: "config", exact: true });
  await configButton.focus();
  await page.keyboard.press("Enter");
  const settings = detail.getByRole("button", { name: "settings.ini", exact: true });
  await expect(settings).toBeVisible();
  expect((await commandCalls(page, "read_dir")).map((call: any) => call.args.path)).toEqual([root, config]);

  await page.keyboard.press("Space");
  await expect(settings).toBeHidden();
  await expect(configButton).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(settings).toBeVisible();

  await settings.focus();
  await page.keyboard.press("Enter");
  await expect(settings).toHaveAttribute("aria-current", "true");
  await expect(detail.locator(".file-viewer")).toContainText(`fixture preview: ${config}/settings.ini`);
  expect((await commandCalls(page, "read_file")).map((call: any) => call.args.path)).toEqual([`${config}/settings.ini`]);
  expect((await commandCalls(page, "read_dir")).some((call: any) => call.args.path === mods)).toBe(false);
});

test("搜索已展开目录中的条目并显示祖先，不读取未展开目录", async ({ page }) => {
  const detail = await installFileTreeFixture(page);
  await detail.getByRole("button", { name: "config", exact: true }).click();
  await detail.getByRole("button", { name: "ui", exact: true }).click();
  await expect(detail.getByRole("button", { name: "layout.json", exact: true })).toBeVisible();

  const search = detail.getByRole("searchbox", { name: "搜索已展开目录", exact: true });
  await search.fill("layout");
  await expect(detail.getByRole("button", { name: "config", exact: true })).toBeVisible();
  await expect(detail.getByRole("button", { name: "ui", exact: true })).toBeVisible();
  await expect(detail.getByRole("button", { name: "layout.json", exact: true })).toBeVisible();
  await expect(detail.getByRole("button", { name: "settings.ini", exact: true })).toBeHidden();

  await search.fill("unexpanded-only");
  await expect(detail.getByRole("button", { name: "unexpanded-only.jar", exact: true })).toHaveCount(0);
  expect((await commandCalls(page, "read_dir")).map((call: any) => call.args.path)).toEqual([root, config, ui]);
});

test("同页存在附加根时显示选择器并按需切换根目录", async ({ page }) => {
  const detail = await installFileTreeFixture(page, true);
  const rootPicker = detail.getByRole("combobox", { name: "浏览根目录", exact: true });
  await expect(rootPicker).toBeVisible();
  await expect(rootPicker.locator("option").nth(0)).toHaveText("游戏目录");
  await expect(rootPicker.locator("option").nth(1)).toHaveText("shared");
  expect((await commandCalls(page, "read_dir")).map((call: any) => call.args.path)).toEqual([root]);

  await rootPicker.selectOption("/sandbox/shared");
  await expect(detail.getByRole("button", { name: "shared", exact: true })).toBeVisible();
  await expect(detail.getByRole("button", { name: "shared-config.ini", exact: true })).toBeVisible();
  expect((await commandCalls(page, "read_dir")).map((call: any) => call.args.path)).toEqual([root, "/sandbox/shared"]);
});

test("刷新同步已展开分支并清除失效文件预览", async ({ page }) => {
  const detail = await installFileTreeFixture(page);
  await detail.getByRole("button", { name: "config", exact: true }).click();
  const settings = detail.getByRole("button", { name: "settings.ini", exact: true });
  await settings.click();
  await expect(detail.locator(".file-viewer")).toContainText(`fixture preview: ${config}/settings.ini`);

  await page.evaluate(({ path, timestamp }) => {
    (window as any).__FILE_TREE_TEST__.replaceDirectory(path, [
      { name: "ui", path: `${path}/ui`, is_dir: true, size: 0, modified: timestamp },
      { name: "fresh.ini", path: `${path}/fresh.ini`, is_dir: false, size: 96, modified: timestamp },
    ]);
  }, { path: config, timestamp: modified });
  await detail.getByRole("button", { name: "刷新目录", exact: true }).click();

  await expect(detail.getByRole("button", { name: "fresh.ini", exact: true })).toBeVisible();
  await expect(detail.getByRole("button", { name: "settings.ini", exact: true })).toHaveCount(0);
  await expect(detail.getByText("未选择文件", { exact: true })).toBeVisible();
  await expect(detail.locator(".file-viewer")).toHaveCount(0);
  const readDirs = await commandCalls(page, "read_dir");
  expect(readDirs.filter((call: any) => call.args.path === root)).toHaveLength(2);
  expect(readDirs.filter((call: any) => call.args.path === config)).toHaveLength(2);
});

test("窄屏文件树与预览不产生横向溢出", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name !== "narrow-390", "仅在 narrow-390 项目验证窄屏布局");
  const detail = await installFileTreeFixture(page, true);
  await expect(detail.getByRole("button", { name: "options.txt", exact: true })).toBeVisible();
  const options = detail.getByRole("button", { name: "options.txt", exact: true });
  await options.click();
  await expect(options).toHaveAttribute("aria-current", "true");
  await expect(detail.locator(".file-viewer")).toContainText(`fixture preview: ${root}/options.txt`);

  const dimensions = await page.evaluate(() => {
    const page = document.querySelector('[data-testid="game-detail-page"]')!;
    return {
      viewport: window.innerWidth,
      document: document.documentElement.scrollWidth,
      detail: page.clientWidth,
      detailContent: page.scrollWidth,
    };
  });
  expect(dimensions.viewport).toBe(390);
  expect(dimensions.document).toBeLessThanOrEqual(dimensions.viewport);
  expect(dimensions.detailContent).toBeLessThanOrEqual(dimensions.detail);
});
