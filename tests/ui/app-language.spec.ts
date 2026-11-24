import { expect, test, type Page } from "@playwright/test";
import { installMockTauri, PLUGIN_UPDATED_AT_FIXTURE } from "./mock-tauri";

const locales = [
  ["zh-CN", "设置"], ["en", "Settings"], ["ja", "設定"], ["ko", "설정"], ["de", "Einstellungen"],
  ["fr", "Paramètres"], ["es", "Ajustes"], ["pt-BR", "Configurações"], ["ru", "Настройки"], ["it", "Impostazioni"],
] as const;

async function openLanguageSettings(page: Page, initialLocale = "zh-CN") {
  await page.goto("/");
  await expect(page.locator("html")).toHaveAttribute("lang", initialLocale);
  await page.locator("button:has(svg.lucide-settings)").click();
  const heading = locales.find(([code]) => code === initialLocale)?.[1];
  if (heading) await expect(page.getByRole("heading", { level: 1 })).toHaveText(heading, { timeout: 15_000 });
  const selector = page.locator("#app-locale");
  await expect(selector).toBeVisible({ timeout: 15_000 });
  return selector;
}

async function expectNoHorizontalOverflow(page: Page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true);
}

test("the single language selector exposes and applies all ten locales", async ({ page }) => {
  await installMockTauri(page);
  const selector = await openLanguageSettings(page);
  expect(await selector.locator("option").evaluateAll(options => options.map(option => (option as HTMLOptionElement).value)))
    .toEqual(["", ...locales.map(([code]) => code)]);

  for (const [code, heading] of locales) {
    await selector.selectOption(code);
    await expect(page.locator("html")).toHaveAttribute("lang", code);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText(heading);
    await expect(selector).toHaveValue(code);
    await expectNoHorizontalOverflow(page);
  }

  const saved = await page.evaluate(() => (window as any).__AMM_TEST__.calls
    .filter((call: any) => call.command === "set_app_locale")
    .map((call: any) => call.args.locale));
  expect(saved).toEqual(locales.map(([code]) => code));
  expect(await page.evaluate(() => (window as any).__TAURI_INTERNALS__.invoke("get_app_locale")))
    .toEqual({ locale: "it", effective_locale: "it" });
});

test("following unsupported or missing system language falls back to English", async ({ page }) => {
  await installMockTauri(page, { systemLocale: "ar-SA" });
  const selector = await openLanguageSettings(page, "en");
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Settings");
  await expect(selector).toHaveValue("");

  await page.evaluate(() => {
    (window as any).__AMM_TEST__.setSystemLocale(null);
    window.dispatchEvent(new Event("languagechange"));
  });
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect(selector).toHaveValue("");
});

test("manual locale persists across reload and resetting returns to the simulated system locale", async ({ page }) => {
  await installMockTauri(page, { systemLocale: "fr-FR" });
  const selector = await openLanguageSettings(page, "fr");
  await selector.selectOption("ja");
  await expect(page.locator("html")).toHaveAttribute("lang", "ja");
  await expect.poll(() => page.evaluate(() => sessionStorage.getItem("amm.test.appLocale"))).toBe("ja");

  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("lang", "ja");
  await expect(page.getByRole("button").filter({ has: page.locator("svg.lucide-settings") })).toBeVisible();
  await page.getByRole("button").filter({ has: page.locator("svg.lucide-settings") }).click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("設定", { timeout: 15_000 });
  const afterReload = page.locator("#app-locale");
  await expect(afterReload).toHaveValue("ja", { timeout: 15_000 });
  await afterReload.selectOption("");
  await expect(page.locator("html")).toHaveAttribute("lang", "fr");
  await expect(afterReload).toHaveValue("");
  await expect.poll(() => page.evaluate(() => sessionStorage.getItem("amm.test.appLocale"))).toBeNull();
});

test("chat draft survives navigating to settings, switching language, and returning to chat", async ({ page }) => {
  await installMockTauri(page);
  await page.goto("/");
  await expect(page.locator("html")).toHaveAttribute("lang", "zh-CN");
  const draft = "这段未发送的聊天草稿应在手动切换语言后保留。";
  await page.getByRole("textbox", { name: "告诉 Agent 你想做什么", exact: true }).fill(draft);

  await page.locator("button:has(svg.lucide-settings)").click();
  const selector = page.locator("#app-locale");
  await expect(selector).toBeVisible();
  await selector.selectOption("ja");
  await expect(page.locator("html")).toHaveAttribute("lang", "ja");

  await page.locator("button:has(svg.lucide-message-square)").click();
  await expect(page.locator("html")).toHaveAttribute("lang", "ja");
  await expect(page.locator("main textarea").last()).toHaveValue(draft);
  await expect.poll(() => page.evaluate(() => (window as any).__AMM_TEST__.calls
    .filter((call: any) => call.command === "set_app_locale")
    .map((call: any) => call.args.locale))).toEqual(["ja"]);
});

test("system language updates keep an open dialog and Agent draft", async ({ page }) => {
  await installMockTauri(page);
  await page.goto("/");
  await expect(page.locator("html")).toHaveAttribute("lang", "zh-CN");
  const draft = "切换系统语言时保留这段草稿";
  await page.getByRole("textbox", { name: "告诉 Agent 你想做什么", exact: true }).fill(draft);
  await page.getByRole("button", { name: "切换工作区：全部游戏", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();

  await page.evaluate(() => {
    (window as any).__AMM_TEST__.setSystemLocale("en-US");
    window.dispatchEvent(new Event("languagechange"));
  });
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect(dialog).toBeVisible();
  await expect(page.locator("main textarea").last()).toHaveValue(draft);
});

test("browser preview saves its choice in the preview-only preference", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "languages", { configurable: true, value: ["zh-CN"] });
    Object.defineProperty(navigator, "language", { configurable: true, value: "zh-CN" });
  });
  await page.goto("/");
  await expect(page.locator("html")).toHaveAttribute("lang", "zh-CN");
  await page.locator("button:has(svg.lucide-settings)").click();
  const selector = page.locator("#app-locale");
  await selector.selectOption("en");
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect.poll(() => page.evaluate(() => localStorage.getItem("amm.language"))).toBe("en");
  expect(await page.evaluate(() => sessionStorage.getItem("amm.test.appLocale"))).toBeNull();

  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
});

test("dates use the selected app language", async ({ page }) => {
  await installMockTauri(page, { withExternalAdapter: true });
  const selector = await openLanguageSettings(page);
  await selector.selectOption("en");
  await page.locator("button:has(svg.lucide-puzzle)").click();
  const pageRoot = page.getByTestId("plugins-page");
  await expect(pageRoot).toBeVisible();
  const timestamp = pageRoot.locator("time").first();
  await expect(timestamp).toHaveAttribute("datetime", PLUGIN_UPDATED_AT_FIXTURE);
  const expected = await page.evaluate(value => new Date(value).toLocaleString("en", { dateStyle: "short", timeStyle: "short" }), PLUGIN_UPDATED_AT_FIXTURE);
  await expect(timestamp).toHaveText(expected);
});
