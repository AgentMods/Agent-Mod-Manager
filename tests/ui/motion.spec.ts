import { expect, test, type Locator, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const MOTION_NAMES = ["page-in", "rise-in", "soft-fade", "pop-in", "float", "soft-ping", "dot-wave", "slide-in-left", "icon-swap", "tab-in", "check-pop"];
const animationName = (target: Locator) => target.evaluate((element) => getComputedStyle(element).animationName);

async function openLibrary(page: Page) {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  const library = page.getByTestId("library-page");
  const card = library.locator('[data-slot="card"]').filter({ has: page.getByRole("heading", { name: "Minecraft Java Edition", exact: true }) });
  await expect(card).toBeVisible();
  return { library, card };
}

test("pages and cards animate in by default", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "no-preference" });
  const { library, card } = await openLibrary(page);
  await expect.poll(() => animationName(library)).toBe("page-in");
  await expect.poll(() => animationName(card)).toBe("rise-in");
});

test("reduced motion switches every entrance effect off", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  const { library, card } = await openLibrary(page);
  expect(await animationName(library)).toBe("none");
  expect(await animationName(card)).toBe("none");
  const running = await page.evaluate((names) => document.getAnimations()
    .map((animation) => (animation as CSSAnimation).animationName)
    .filter((name) => names.includes(name)), MOTION_NAMES);
  expect(running).toEqual([]);
});

test("the active navigation item grows an indicator bar", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await openLibrary(page);
  const bar = (target: Locator) => target.evaluate((element) => getComputedStyle(element, "::before").transform);
  const active = page.locator('[data-slot="sidebar-menu-button"][data-active="true"]');
  const idle = page.locator('[data-slot="sidebar-menu-button"][data-active="false"]').first();
  await expect.poll(() => bar(active)).toBe("matrix(1, 0, 0, 1, 0, 0)");
  expect(await bar(idle)).toBe("matrix(1, 0, 0, 0, 0, 0)");
});

test("the active navigation bar stays visible under reduced motion", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await openLibrary(page);
  const active = page.locator('[data-slot="sidebar-menu-button"][data-active="true"]');
  expect(await active.evaluate((element) => getComputedStyle(element, "::before").transform)).toBe("matrix(1, 0, 0, 1, 0, 0)");
});

test("navigation items enter one after another", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await openLibrary(page);
  const delays = await page.locator('nav[aria-label="主导航"] [data-slot="sidebar-menu-item"]')
    .evaluateAll((items) => items.map((item) => getComputedStyle(item).animationDelay));
  expect(delays.slice(0, 3)).toEqual(["0s", "0.05s", "0.1s"]);
});

test("a theme switch fades colours, then removes its helper class", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await page.addInitScript(() => {
    (window as any).__sawThemeFade = false;
    // `document.documentElement` can still be null when init scripts run, so observe the document itself.
    new MutationObserver(() => {
      if (document.documentElement.classList.contains("theme-switching")) (window as any).__sawThemeFade = true;
    }).observe(document, { attributes: true, subtree: true, attributeFilter: ["class"] });
  });
  await installMockTauri(page);
  await page.goto("/");
  const html = page.locator("html");
  await expect(html).toHaveAttribute("data-theme", /light|dark/);
  expect(await page.evaluate(() => (window as any).__sawThemeFade), "first paint must not animate").toBe(false);

  const before = await html.getAttribute("data-theme");
  await page.getByRole("button", { name: before === "dark" ? "切换亮色主题" : "切换暗色主题", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__sawThemeFade)).toBe(true);
  await expect(html).not.toHaveClass(/theme-switching/);
  await expect(html).toHaveAttribute("data-theme", before === "dark" ? "light" : "dark");
});

test("stagger delays grow per row and are capped", async ({ page }) => {
  await installMockTauri(page);
  await page.goto("/");
  const delays = await page.evaluate(async () => {
    const { stagger } = await import("/src/lib/motion.ts");
    return [-2, 0, 1, 3, 100].map((index) => (stagger(index) as Record<string, string>)["--stagger"]);
  });
  expect(delays).toEqual(["0ms", "0ms", "45ms", "135ms", "360ms"]);
});
