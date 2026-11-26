import { expect, test } from "@playwright/test";
import { createRequire } from "node:module";
import { installMockTauri } from "./mock-tauri";

const {version} = createRequire(import.meta.url)("../../package.json") as {version: string};

test.describe("首页", () => {
  test("首次运行即使没有游戏也先进入首页，可显式开始设置", async ({page}) => {
    await installMockTauri(page, {onboarding: true, dashboardStats: {managedGameCount: 0, installedModCount: 0, totalModCount: 0}});
    await page.goto("/");

    await expect(page.getByRole("navigation", {name: "主导航", exact: true})
      .getByRole("button", {name: "首页", exact: true})).toHaveAttribute("aria-current", "page");
    await expect(page.getByRole("heading", {name: "欢迎来到 Agent Mod Manager", exact: true, level: 1})).toBeVisible();
    await expect(page.locator('main header img')).toHaveCount(0);
    await expect(page.getByRole("heading", {name: "管理的游戏", exact: true, level: 2})).toBeVisible();
    await expect(page.getByRole("heading", {name: "已安装的 Mod / 全部 Mod", exact: true, level: 2})).toBeVisible();
    await expect(page.getByRole("button", {name: "Minecraft Java Edition", exact: true})).toHaveCount(0);

    const setup = page.getByRole("button", {name: "开始设置", exact: true});
    await expect(setup).toBeVisible();

    await setup.click();
    await expect(page.getByRole("heading", {name: "设置数据存储目录", exact: true})).toBeVisible();
  });

  test("未管理游戏的用户可添加第一款游戏", async ({page}) => {
    await installMockTauri(page, {emptyLibrary: true, dashboardStats: {managedGameCount: 0, installedModCount: 0, totalModCount: 0}});
    await page.goto("/");

    await expect(page.getByRole("heading", {name: "欢迎来到 Agent Mod Manager", exact: true, level: 1})).toBeVisible();
    await expect(page.locator('main header img')).toHaveCount(0);
    await expect(page.getByRole("heading", {name: "管理的游戏", exact: true, level: 2})).toBeVisible();
    await expect(page.getByRole("heading", {name: "已安装的 Mod / 全部 Mod", exact: true, level: 2})).toBeVisible();
    await expect(page.getByRole("button", {name: "添加第一款游戏", exact: true})).toBeVisible();
    await expect(page.getByRole("button", {name: "打开游戏库", exact: true})).toHaveCount(0);
    await expect(page.getByRole("button", {name: "浏览 Mod", exact: true})).toHaveCount(0);
    await page.getByRole("button", {name: "添加第一款游戏", exact: true}).click();
    await expect(page.getByTestId("library-page")).toBeVisible();
  });

  test("未完成设置返回首页后可继续进入引导", async ({page}) => {
    await installMockTauri(page, {onboarding: true, onboardingStarted: true});
    await page.goto("/");

    const resume = page.getByRole("button", {name: "继续设置", exact: true});
    await expect(resume).toBeVisible();
    await expect(page.getByRole("heading", {name: "设置数据存储目录", exact: true})).toHaveCount(0);
    await resume.click();
    await expect(page.getByRole("heading", {name: "设置数据存储目录", exact: true})).toBeVisible();
  });

  test("有游戏时首页提供游戏库和 Mod 浏览入口，版本及赞助名单可用", async ({page}) => {
    await installMockTauri(page, {
      dashboardSponsors: ["Fixture Sponsor", "Second Fixture"],
      dashboardStats: {managedGameCount: 1, installedModCount: 2, totalModCount: 5},
    });
    await page.goto("/");

    await expect(page.getByRole("heading", {name: "欢迎来到 Agent Mod Manager", exact: true, level: 1})).toBeVisible();
    await expect(page.locator('main header img')).toHaveCount(0);
    await expect(page.getByRole("heading", {name: "管理的游戏", exact: true, level: 2})).toBeVisible();
    await expect(page.getByText("1", {exact: true})).toBeVisible();
    await expect(page.getByRole("heading", {name: "已安装的 Mod / 全部 Mod", exact: true, level: 2})).toBeVisible();
    await expect(page.getByText("2 / 5", {exact: true})).toBeVisible();
    const game = page.getByRole("button", {name: "Minecraft Java Edition", exact: true});
    await expect(game).toBeVisible();
    await expect(page.getByRole("button", {name: "打开游戏库", exact: true})).toBeVisible();
    await expect(page.getByRole("button", {name: "浏览 Mod", exact: true})).toBeVisible();
    await expect(page.locator("main").getByText(new RegExp(`当前版本\\s*v${version}`))).toBeVisible();

    await game.click();
    await expect(page.getByTestId("game-detail-page")).toBeVisible();
    await page.getByRole("button", {name: "首页", exact: true}).click();

    await page.getByRole("button", {name: "打开游戏库", exact: true}).click();
    await expect(page.getByTestId("library-page")).toBeVisible();
    await page.getByRole("button", {name: "首页", exact: true}).click();
    await page.getByRole("button", {name: "浏览 Mod", exact: true}).click();
    await expect(page.getByRole("navigation", {name: "主导航", exact: true})
      .getByRole("button", {name: "游览", exact: true})).toHaveAttribute("aria-current", "page");
    await page.getByRole("button", {name: "首页", exact: true}).click();

    const sponsor = page.getByRole("link", {name: "前往赞助", exact: true});
    await expect(sponsor).toHaveAttribute("href", "https://sponsor.aoe.top/");
    await expect(sponsor).toHaveAttribute("target", "_blank");
    const supporters = page.getByRole("link", {name: "查看完整名单", exact: true});
    await expect(supporters).toHaveAttribute("href", "https://sponsor.aoe.top/");
    await expect(supporters).toHaveAttribute("target", "_blank");
    await expect(page.getByText("Fixture Sponsor", {exact: true})).toBeVisible();
    await expect(page.getByText("Second Fixture", {exact: true})).toBeVisible();
    await expect.poll(() => page.evaluate(() => (window as any).__AMM_TEST__.calls.some(
      (call: any) => call.command === "list_dashboard_sponsors",
    ))).toBe(true);
    await expect.poll(() => page.evaluate(() => (window as any).__AMM_TEST__.calls.some(
      (call: any) => call.command === "get_dashboard_stats",
    ))).toBe(true);
  });
});
