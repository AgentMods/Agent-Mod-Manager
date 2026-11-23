import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const gameId = "game-fixture";

type ModTag = { id: string; name: string; color: string };
type InstallFixture = {
  id: string;
  gameId: string;
  name: string;
  version: string;
  source: string;
  status: string;
  installRoot: string;
  files: unknown[];
  metadata: Record<string, unknown>;
  installedAt: string;
};

const tags: ModTag[] = [
  { id: "tag-performance", name: "性能优化", color: "#2e7d32" },
  { id: "tag-visual", name: "视觉效果", color: "#6a1b9a" },
  { id: "tag-stability", name: "稳定性", color: "#1565c0" },
];

const additionalInstalls: InstallFixture[] = [
  {
    id: "install-stability-fixture", gameId, name: "稳定性修复包", version: "2.0.0", source: "local",
    status: "enabled", installRoot: "/sandbox/minecraft/mods", files: [], metadata: {}, installedAt: "2026-11-18T00:00:00Z",
  },
  {
    id: "install-untagged-fixture", gameId, name: "未分类工具包", version: "1.2.0", source: "local",
    status: "enabled", installRoot: "/sandbox/minecraft/mods", files: [], metadata: {}, installedAt: "2026-11-18T00:00:00Z",
  },
];

async function installTagFixture(page: Page, assignments: Record<string, string[]>) {
  await installMockTauri(page);
  await page.addInitScript(({ initialTags, initialAssignments, extraInstalls }) => {
    const target = window as any;
    const transport = target.__TAURI_INTERNALS__;
    const originalInvoke = transport.invoke;
    const state: { tags: ModTag[]; assignments: Record<string, string[]>; nextId: number } = {
      tags: structuredClone(initialTags),
      assignments: structuredClone(initialAssignments),
      nextId: 0,
    };
    const recordCall = (command: string, args: Record<string, unknown>) => target.__AMM_TEST__.calls.push({ command, args });

    transport.invoke = async (command: string, args: Record<string, any> = {}, options?: unknown) => {
      if (command === "list_mod_tags") {
        recordCall(command, args);
        if (args.gameId !== "game-fixture") throw new Error("unknown game for Mod tags");
        return structuredClone(state.tags);
      }
      if (command === "save_mod_tag") {
        recordCall(command, args);
        if (args.gameId !== "game-fixture") throw new Error("unknown game for Mod tags");
        const id = args.id ?? `tag-created-${++state.nextId}`;
        const saved = { id, name: args.name, color: args.color };
        const index = state.tags.findIndex(tag => tag.id === id);
        if (index < 0) state.tags.push(saved);
        else state.tags[index] = saved;
        return structuredClone(saved);
      }
      if (command === "delete_mod_tag") {
        recordCall(command, args);
        if (args.gameId !== "game-fixture") throw new Error("unknown game for Mod tags");
        state.tags = state.tags.filter(tag => tag.id !== args.id);
        for (const [installId, tagIds] of Object.entries(state.assignments)) {
          state.assignments[installId] = tagIds.filter(id => id !== args.id);
        }
        return null;
      }
      if (command === "set_mod_tags") {
        recordCall(command, args);
        if (args.gameId !== "game-fixture") throw new Error("unknown game for Mod tags");
        state.assignments[args.installId] = structuredClone(args.tagIds);
        return null;
      }

      const result = await originalInvoke(command, args, options);
      if (command !== "list_installs" || args.gameId !== "game-fixture") return result;
      const base = (result as InstallFixture[]).map(record => ({
        ...record,
        name: record.id === "install-fixture" ? "星辉优化包" : record.name,
        tagIds: state.assignments[record.id] ?? [],
      }));
      return [...base, ...extraInstalls.map(record => ({
        ...structuredClone(record), tagIds: state.assignments[record.id] ?? [],
      }))];
    };
    target.__MOD_TAG_TEST__ = state;
  }, { initialTags: tags, initialAssignments: assignments, extraInstalls: additionalInstalls });
}

async function openGameDetail(page: Page) {
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await page.getByRole("button").filter({ has: page.getByRole("heading", { name: "Minecraft Java Edition", exact: true }) }).click();
  await expect(page.getByTestId("game-detail-page")).toBeVisible();
  await page.getByRole("tab", { name: "Mod 管理", exact: true }).click();
}

function modTable(page: Page) {
  return page.getByTestId("game-detail-page").getByRole("table", { name: "Mod 管理", exact: true });
}

function installedRow(page: Page, name: string) {
  return modTable(page).getByRole("row").filter({ hasText: name });
}

function tagFilters(page: Page) {
  return page.getByRole("radiogroup", { name: "按标签筛选", exact: true });
}

async function expectTagColor(row: ReturnType<typeof installedRow>, name: string, color: string) {
  const badge = row.locator(".mod-tag-color").filter({ hasText: name });
  await expect(badge).toBeVisible();
  await expect(badge).toHaveCSS("--mod-tag-color", color);
  const computed = await badge.evaluate(element => {
    const style = getComputedStyle(element);
    const parent = element.parentElement ? getComputedStyle(element.parentElement) : null;
    return {
      background: style.backgroundColor,
      border: style.borderTopColor,
      text: style.color,
      parentBackground: parent?.backgroundColor,
      parentBorder: parent?.borderTopColor,
      parentText: parent?.color,
    };
  });
  expect(computed.background).not.toBe("rgba(0, 0, 0, 0)");
  expect(computed.background).not.toBe(computed.parentBackground);
  expect(computed.border).not.toBe(computed.parentBorder);
  expect(computed.text).not.toBe(computed.parentText);
}

async function expectNoHorizontalOverflow(page: Page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true);
}

async function expectTagsBeforeNamesWithoutDots(page: Page) {
  const validOrder = await modTable(page).getByRole("row").evaluateAll(rows => rows.every(row => {
    const name = row.querySelector("strong");
    const badges = [...row.querySelectorAll(".mod-tag-color")];
    return badges.every(badge => !!name
      && !!(badge.compareDocumentPosition(name) & Node.DOCUMENT_POSITION_FOLLOWING)
      && badge.querySelector('[aria-hidden="true"]') === null);
  }));
  expect(validOrder).toBe(true);
}

test("installed records show their display name and colored tags, with single-tag and all-tag filters", async ({ page }, testInfo) => {
  await installTagFixture(page, {
    "install-fixture": ["tag-performance", "tag-visual"],
    "install-stability-fixture": ["tag-stability"],
  });
  await page.goto("/");
  await openGameDetail(page);

  const featured = installedRow(page, "星辉优化包");
  await expect(featured).toContainText("星辉优化包");
  await expectTagColor(featured, "性能优化", "#2e7d32");
  await expectTagColor(featured, "视觉效果", "#6a1b9a");
  await expect(installedRow(page, "稳定性修复包")).toContainText("稳定性");
  await expectTagsBeforeNamesWithoutDots(page);
  await expect(modTable(page).getByRole("row")).toHaveCount(4); // Header plus three installed records.

  const filters = tagFilters(page);
  await filters.getByRole("radio", { name: "性能优化", exact: true }).click();
  await expect(installedRow(page, "星辉优化包")).toBeVisible();
  await expect(installedRow(page, "稳定性修复包")).toHaveCount(0);
  await expect(installedRow(page, "未分类工具包")).toHaveCount(0);

  await filters.getByRole("radio", { name: "全部标签", exact: true }).click();
  await expect(installedRow(page, "星辉优化包")).toBeVisible();
  await expect(installedRow(page, "稳定性修复包")).toBeVisible();
  await expect(installedRow(page, "未分类工具包")).toBeVisible();
  await expectTagsBeforeNamesWithoutDots(page);
  if (testInfo.project.name === "desktop-1280") {
    const filterBounds = await tagFilters(page).boundingBox();
    const tableBounds = await modTable(page).boundingBox();
    const manageBounds = await page.getByRole("button", { name: "管理标签", exact: true }).boundingBox();
    const rows = await modTable(page).getByRole("row").all();
    const thirdRecordBounds = await rows[3].boundingBox();
    if (!filterBounds || !tableBounds || !manageBounds || !thirdRecordBounds) {
      throw new Error("Unable to measure the Mod tag list screenshot region");
    }
    const x = Math.floor(Math.min(filterBounds.x, tableBounds.x) - 8);
    const y = Math.floor(Math.min(filterBounds.y, tableBounds.y) - 8);
    const right = Math.ceil(Math.max(manageBounds.x + manageBounds.width, tableBounds.x + tableBounds.width) + 8);
    const bottom = Math.ceil(thirdRecordBounds.y + thirdRecordBounds.height + 8);
    await page.screenshot({
      path: "output/playwright/mod-tag-list.png",
      clip: { x, y, width: right - x, height: bottom - y },
    });
  }
  if (testInfo.project.name === "narrow-390") {
    await page.screenshot({ path: "output/playwright/mod-organization-narrow-390.png", fullPage: true });
  }
});

test("tag management creates, renames, recolors, and deletes a tag through scoped IPC", async ({ page }) => {
  await installTagFixture(page, { "install-fixture": ["tag-performance"] });
  await page.goto("/");
  await openGameDetail(page);

  await page.getByRole("button", { name: "管理标签", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "管理标签", exact: true });
  await expect(dialog).toBeVisible();
  await dialog.getByLabel("标签名称", { exact: true }).fill("沉浸体验");
  await dialog.getByLabel("标签颜色", { exact: true }).fill("#ad1457");
  await dialog.getByRole("button", { name: "创建标签", exact: true }).click();
  await expect(dialog.getByRole("button", { name: "编辑 沉浸体验", exact: true })).toBeVisible();
  await expect.poll(async () => {
    const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
    return calls.filter((call: any) => call.command === "save_mod_tag").length;
  }).toBe(1);
  let saves = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "save_mod_tag"));
  expect(saves[0].args).toEqual({ gameId, id: null, name: "沉浸体验", color: "#ad1457" });

  await dialog.getByRole("button", { name: "编辑 沉浸体验", exact: true }).click();
  await dialog.getByLabel("标签名称", { exact: true }).fill("沉浸冒险");
  await dialog.getByLabel("标签颜色", { exact: true }).fill("#00838f");
  await dialog.getByRole("button", { name: "保存修改", exact: true }).click();
  await expect(dialog.getByRole("button", { name: "编辑 沉浸冒险", exact: true })).toBeVisible();
  saves = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "save_mod_tag"));
  expect(saves[1].args).toEqual({ gameId, id: "tag-created-1", name: "沉浸冒险", color: "#00838f" });

  await dialog.getByRole("button", { name: "删除 沉浸冒险", exact: true }).click();
  await expect(dialog.getByText("从此游戏的所有 Mod 移除此标签？", { exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: "确认删除", exact: true }).click();
  await expect(dialog.getByRole("button", { name: "编辑 沉浸冒险", exact: true })).toHaveCount(0);
  const deletes = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "delete_mod_tag"));
  expect(deletes.map((call: any) => call.args)).toEqual([{ gameId, id: "tag-created-1" }]);
});

test("an installed record can replace its assigned tags using install-scoped IPC", async ({ page }) => {
  await installTagFixture(page, {
    "install-fixture": ["tag-performance", "tag-visual"],
    "install-stability-fixture": ["tag-stability"],
  });
  await page.goto("/");
  await openGameDetail(page);

  const row = installedRow(page, "星辉优化包");
  await row.getByRole("button", { name: "编辑 星辉优化包 的标签", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "编辑 Mod 标签", exact: true });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("checkbox", { name: "性能优化", exact: true })).toBeChecked();
  await expect(dialog.getByRole("checkbox", { name: "视觉效果", exact: true })).toBeChecked();
  await dialog.getByRole("checkbox", { name: "性能优化", exact: true }).uncheck();
  await dialog.getByRole("checkbox", { name: "视觉效果", exact: true }).uncheck();
  await dialog.getByRole("checkbox", { name: "稳定性", exact: true }).check();
  await dialog.getByRole("button", { name: "保存标签", exact: true }).click();

  const assigned = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === "set_mod_tags"));
  expect(assigned.map((call: any) => call.args)).toEqual([{ gameId, installId: "install-fixture", tagIds: ["tag-stability"] }]);
  await expect(row.getByText("稳定性", { exact: true })).toBeVisible();
  await expect(row.getByText("性能优化", { exact: true })).toHaveCount(0);
  await expect(row.getByText("视觉效果", { exact: true })).toHaveCount(0);
});

test("tag controls remain usable without horizontal overflow on a narrow screen in both themes", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await installTagFixture(page, {
    "install-fixture": ["tag-performance", "tag-visual"],
    "install-stability-fixture": ["tag-stability"],
  });
  await page.goto("/");
  await openGameDetail(page);

  for (const theme of ["light", "dark"] as const) {
    const currentTheme = await page.locator("html").getAttribute("data-theme");
    if (currentTheme !== theme) {
      await page.getByRole("button", { name: theme === "dark" ? "切换暗色主题" : "切换亮色主题", exact: true }).click();
    }
    await expectTagColor(installedRow(page, "星辉优化包"), "视觉效果", "#6a1b9a");
    await expectNoHorizontalOverflow(page);

    await page.getByRole("button", { name: "管理标签", exact: true }).click();
    await expect(page.getByRole("dialog", { name: "管理标签", exact: true })).toBeVisible();
    await expectNoHorizontalOverflow(page);
    await page.keyboard.press("Escape");

    await installedRow(page, "星辉优化包")
      .getByRole("button", { name: "编辑 星辉优化包 的标签", exact: true }).click();
    await expect(page.getByRole("dialog", { name: "编辑 Mod 标签", exact: true })).toBeVisible();
    await expectNoHorizontalOverflow(page);
    await page.keyboard.press("Escape");
  }
});

test("Mod organization language can be saved and switched back to follow the system", async ({ page }) => {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await page.getByRole("tab", { name: "外观与存储", exact: true }).click();

  const locale = page.getByRole("combobox", { name: "Mod 整理目标语言", exact: true });
  await expect(locale).toHaveValue("");
  await expect(locale).toBeEnabled();
  await locale.selectOption("ja");
  await expect(locale).toHaveValue("ja");
  await expect.poll(async () => page.evaluate(() => (window as any).__AMM_TEST__.calls
    .filter((call: any) => call.command === "set_mod_organization_locale")
    .map((call: any) => call.args))).toEqual([{ locale: "ja" }]);

  await locale.selectOption("");
  await expect(locale).toHaveValue("");
  await expect(locale.locator("option:checked")).toHaveText("跟随系统（简体中文）");
  await expect.poll(async () => page.evaluate(() => (window as any).__AMM_TEST__.calls
    .filter((call: any) => call.command === "set_mod_organization_locale")
    .map((call: any) => call.args))).toEqual([{ locale: "ja" }, { locale: null }]);
});
