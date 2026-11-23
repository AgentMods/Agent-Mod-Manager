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

test("侧栏赞助链接通过系统浏览器打开赞助页", async ({page}) => {
  await installMockTauri(page);
  await page.goto("/");
  const sponsor = page.getByRole("link", {name: "赞助"});
  await expect(sponsor).toHaveAttribute("href", "https://sponsor.aoe.top/");
  await expect(sponsor).toHaveText("");
  const theme = page.getByRole("button", {name: /切换(亮|暗)色主题/});
  const sponsorBox = (await sponsor.boundingBox())!;
  const themeBox = (await theme.boundingBox())!;
  expect(sponsorBox.width).toBe(themeBox.width);
  expect(sponsorBox.height).toBe(themeBox.height);
  if (await rail(page).getAttribute("data-state") === "expanded") {
    expect(sponsorBox.x + sponsorBox.width).toBe(themeBox.x);
    expect(sponsorBox.y).toBe(themeBox.y);
  } else {
    expect(sponsorBox.y + sponsorBox.height).toBe(themeBox.y);
    expect(sponsorBox.x).toBe(themeBox.x);
  }
  await sponsor.focus();
  await sponsor.press("Enter");
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls)).toContainEqual({
    command: "plugin:opener|open_url", args: {url: "https://sponsor.aoe.top/"},
  });
});
