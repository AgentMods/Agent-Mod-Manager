import { expect, test } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const toggle = (page: import("@playwright/test").Page) => page.getByRole("button", {name: "切换侧边栏"}).first();
const rail = (page: import("@playwright/test").Page) => page.locator('[data-slot="sidebar"]').first();

test("手动折叠左侧菜单栏后重新打开仍保持折叠", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  const initial = await rail(page).getAttribute("data-state");
  await toggle(page).click();
  const changed = initial === "expanded" ? "collapsed" : "expanded";
  await expect(rail(page)).toHaveAttribute("data-state", changed);
  expect(await page.evaluate(() => localStorage.getItem("amm.sidebar"))).toBe(changed === "expanded" ? "open" : "closed");

  await page.reload();
  await expect(rail(page)).toHaveAttribute("data-state", changed);
});

test("没有手动选择时左侧菜单栏仍随窗口宽度自动决定", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  expect(await page.evaluate(() => localStorage.getItem("amm.sidebar"))).toBeNull();
});

test("损坏的侧边栏记录被忽略", async ({page}) => {
  await installMockTauri(page);
  await page.addInitScript(() => localStorage.setItem("amm.sidebar", "garbage"));
  await page.goto("/");
  await expect(rail(page)).toBeVisible();
});
