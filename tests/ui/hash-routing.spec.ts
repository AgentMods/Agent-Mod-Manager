import { expect, test } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const directRoutes = [
  {hash: "#/library", page: "[data-testid=library-page]", title: "游戏库"},
  {hash: "#/settings", page: null, title: "设置"},
  {hash: "#/games/game-fixture", page: "[data-testid=game-detail-page]", title: "Minecraft Java Edition"},
] as const;

for (const route of directRoutes) {
  test(`直接进入并刷新 ${route.hash} 保持在对应页面`, async ({page}) => {
    await installMockTauri(page);
    await page.goto(`/${route.hash}`);

    await expect(page).toHaveURL(new RegExp(`${route.hash.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`));
    if (route.page) await expect(page.locator(route.page)).toBeVisible();
    await expect(page.getByRole("heading", {name: route.title, exact: true})).toBeVisible();

    await page.reload();

    await expect(page).toHaveURL(new RegExp(`${route.hash.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`));
    if (route.page) await expect(page.locator(route.page)).toBeVisible();
    await expect(page.getByRole("heading", {name: route.title, exact: true})).toBeVisible();
  });
}

test("游戏库、设置和游戏详情支持浏览器后退与前进", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/#/library");
  await expect(page.locator("[data-testid=library-page]")).toBeVisible();

  await page.getByRole("button", {name: "设置", exact: true}).click();
  await expect(page).toHaveURL(/#\/settings$/);
  await expect(page.getByRole("heading", {name: "设置", exact: true})).toBeVisible();

  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await expect(page).toHaveURL(/#\/library$/);
  await page.locator("[data-testid=library-page]").getByRole("button", {name: /Minecraft Java Edition/}).click();
  await expect(page).toHaveURL(/#\/games\/game-fixture$/);
  await expect(page.locator("[data-testid=game-detail-page]")).toBeVisible();

  await page.goBack();
  await expect(page).toHaveURL(/#\/library$/);
  await expect(page.locator("[data-testid=library-page]")).toBeVisible();
  await page.goBack();
  await expect(page).toHaveURL(/#\/settings$/);
  await expect(page.getByRole("heading", {name: "设置", exact: true})).toBeVisible();

  await page.goForward();
  await expect(page).toHaveURL(/#\/library$/);
  await expect(page.locator("[data-testid=library-page]")).toBeVisible();
  await page.goForward();
  await expect(page).toHaveURL(/#\/games\/game-fixture$/);
  await expect(page.locator("[data-testid=game-detail-page]")).toBeVisible();
});

test("未知路由回到首页且不会困住后退操作", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/#/library");
  await expect(page.getByTestId("library-page")).toBeVisible();
  await page.goto("/#/not-a-page");
  await expect(page).toHaveURL(/#\/$/);
  await expect(page.getByTestId("home-page")).toBeVisible();
  await page.goBack();
  await expect(page.getByTestId("library-page")).toBeVisible();
});

test("没有桌面服务时引导地址回到设置", async ({page}) => {
  await page.goto("/#/onboarding");
  await expect(page).toHaveURL(/#\/settings$/);
  await expect(page.getByRole("heading", {name: "设置", exact: true})).toBeVisible();
});
