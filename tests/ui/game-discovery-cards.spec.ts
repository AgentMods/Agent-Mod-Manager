import { expect, test, type Locator, type Page } from "@playwright/test";
import type { GameCandidate } from "../../src/lib/types";
import { GAME_COVER_URL } from "./publication-fixture";
import { installMockTauri } from "./mock-tauri";

const cloudNames = ["Cloud Good", "Cloud Missing", "Cloud Failure", ...Array.from({ length: 6 }, (_, i) => `Cloud Lazy ${i + 1}`)];
const cloudCandidates = cloudNames.map((name) => candidate(`/sandbox/games/${name.toLowerCase().replaceAll(" ", "-")}`, name, {
  warnings: name === "Cloud Good" ? ["目录需要确认"] : [],
}));
const candidates = [
  candidate("/sandbox/games/local-cover", "Local Cover Game", { confidence: 1, status: "high_confidence", suggestedAdapterSlug: "minecraft-java" }),
  candidate("/sandbox/minecraft", "Minecraft Java Edition", { confidence: 1, status: "high_confidence", suggestedAdapterSlug: "minecraft-java", registeredGameId: "game-fixture" }),
  candidate("/sandbox/games/no-cover-source", "No Cover Source Game", { confidence: 1, status: "high_confidence", suggestedAdapterSlug: "adapter-not-installed" }),
  ...cloudCandidates,
  candidate("/sandbox/games/not-in-catalog", "Unmatched Local Game"),
];
const releases = cloudCandidates.map((item, i) => ({
  pluginId: `adapter-${i + 1}`, releaseId: `release-${i + 1}`, slug: `cloud-adapter-${i + 1}`, name: `${item.name} Adapter`,
}));

function candidate(rootPath: string, name: string, overrides: Partial<GameCandidate> = {}): GameCandidate {
  return { rootPath, name, confidence: 0.72, status: "candidate", suggestedAdapterSlug: null, registeredGameId: null, platform: "windows", warnings: [], ...overrides };
}

async function openDiscovery(page: Page) {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await installMockTauri(page, { withExternalAdapter: true });
  await page.goto("/");
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await page.evaluate(({ candidates, cloudCandidates, releases, coverUrl }) => {
    const host = window as any;
    const invoke = host.__TAURI_INTERNALS__.invoke;
    const cloudGames = cloudCandidates.map((item: GameCandidate, i: number) => ({
      id: `catalog-game-${i + 1}`, name: item.name, slug: item.name.toLowerCase().replaceAll(" ", "-"), aliases: [], steamAppId: "",
      plugins: [{ id: releases[i].pluginId, releaseId: releases[i].releaseId, slug: releases[i].slug, name: releases[i].name,
        version: "1.0.0", authorName: "Fixture Author", platforms: ["windows"], steamAppId: "", description: "Fixture adapter" }],
    }));
    host.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, any> = {}, ...rest: unknown[]) => {
      if (command === "scan_games" && !args.options?.page?.cancel) {
        host.__AMM_TEST__.calls.push({ command, args: structuredClone(args) });
        return { candidates: structuredClone(candidates), warnings: [], directoriesChecked: 12, entriesChecked: 48, truncated: false };
      }
      if (command === "list_manager_plugin_games") {
        host.__AMM_TEST__.calls.push({ command, args: structuredClone(args) });
        return { games: structuredClone(cloudGames), totalGames: cloudGames.length, skippedEntries: 0 };
      }
      if (command === "get_manager_plugin_display") {
        host.__AMM_TEST__.calls.push({ command, args: structuredClone(args) });
        if (args.releaseId === "release-2") return null;
        if (args.releaseId === "release-3") throw new Error("Fixture cover lookup failed");
        return { description: "Fixture cover", coverUrl };
      }
      return invoke(command, args, ...rest);
    };
  }, { candidates, cloudCandidates, releases, coverUrl: GAME_COVER_URL });

  await page.getByRole("button", { name: "扫描游戏", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "扫描游戏", exact: true });
  await expect(dialog).toBeVisible();
  const list = dialog.getByRole("list", { name: "扫描候选游戏", exact: true });
  await expect(list.getByRole("listitem")).toHaveCount(candidates.length);
  return { dialog, list };
}

function card(list: Locator, page: Page, name: string) {
  return list.getByRole("listitem").filter({ has: page.getByRole("heading", { name, exact: true }) });
}

function commandArgs(page: Page, command: string) {
  return page.evaluate((name) => (window as any).__AMM_TEST__.calls
    .filter((call: any) => call.command === name).map((call: any) => call.args), command);
}

async function expectCoverAboveDetails(row: Locator, name: string, rootPath: string) {
  const image = row.locator("img").first();
  await expect(image).toHaveAttribute("src", GAME_COVER_URL);
  await expect(image).toHaveAttribute("alt", `${name}的游戏封面`);
  await expect.poll(() => image.evaluate((element) => (element as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
  const boxes = await row.evaluate((element, path) => {
    const rect = (target: Element | null) => {
      if (!target) throw new Error("扫描卡片缺少布局元素");
      const { top, bottom, left, right } = target.getBoundingClientRect();
      return { top, bottom, left, right };
    };
    const media = element.firstElementChild;
    const title = element.querySelector("h1,h2,h3,h4,h5,h6");
    const pathText = Array.from(element.querySelectorAll("span")).find((span) => span.textContent?.trim() === path);
    return { media: rect(media), title: rect(title), path: rect(pathText), footer: rect(element.querySelector('[data-slot="item-footer"]')) };
  }, rootPath);
  expect(boxes.media.bottom).toBeLessThanOrEqual(boxes.title.top + 1);
  expect(boxes.title.bottom).toBeLessThanOrEqual(boxes.path.top + 1);
  expect(boxes.path.bottom).toBeLessThanOrEqual(boxes.footer.top + 1);
}

test("扫描卡片封面置顶、宽屏换行窄屏单列，匹配信息和更多操作保留", async ({ page }) => {
  const { list } = await openDiscovery(page);
  const rows = list.getByRole("listitem");
  const geometry = await rows.evaluateAll((elements) => elements.map((element) => {
    const { left, top } = element.getBoundingClientRect();
    return { left: Math.round(left), top: Math.round(top) };
  }));
  const columns = new Set(geometry.map(({ left }) => left));
  const visualRows = new Set(geometry.map(({ top }) => top));
  if ((page.viewportSize()?.width ?? 0) <= 500) {
    expect(columns.size).toBe(1);
    expect(visualRows.size).toBe(candidates.length);
  } else {
    expect(columns.size).toBeGreaterThan(1);
    expect(visualRows.size).toBeGreaterThan(1);
  }

  const lazy = card(list, page, "Cloud Lazy 6");
  expect(await lazy.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return rect.bottom > 0 && rect.top < window.innerHeight && rect.right > 0 && rect.left < window.innerWidth;
  })).toBe(false);
  expect(await commandArgs(page, "get_manager_plugin_display")).not.toContainEqual({ releaseId: "release-9" });

  const local = card(list, page, "Local Cover Game");
  await local.scrollIntoViewIfNeeded();
  await expectCoverAboveDetails(local, "Local Cover Game", "/sandbox/games/local-cover");
  await expect(local.getByRole("button", { name: "添加", exact: true })).toBeEnabled();
  await expect(card(list, page, "Minecraft Java Edition").getByRole("button", { name: "管理中", exact: true })).toBeDisabled();

  const cloud = card(list, page, "Cloud Good");
  await cloud.scrollIntoViewIfNeeded();
  await expect(cloud.getByText("云端已匹配", { exact: true })).toBeVisible();
  const adapterDetails = cloud.locator("p").filter({ hasText: "Cloud Good Adapter" });
  await expect(adapterDetails).toContainText("适配包将随添加安装");
  await expect(cloud.getByText("目录需要确认", { exact: true })).toBeVisible();
  await expectCoverAboveDetails(cloud, "Cloud Good", "/sandbox/games/cloud-good");
  await expect(cloud.locator('[data-slot="item-footer"]').getByText("云端已匹配", { exact: true })).toBeVisible();
  const detailBoxes = await cloud.evaluate((element) => {
    const path = Array.from(element.querySelectorAll("span")).find((span) => span.textContent?.trim() === "/sandbox/games/cloud-good")!;
    const footer = element.querySelector('[data-slot="item-footer"]')!;
    return {
      pathBottom: path.getBoundingClientRect().bottom,
      footerTop: footer.getBoundingClientRect().top,
      paragraphs: Array.from(element.querySelectorAll("p")).map((paragraph) => {
        const { top, bottom } = paragraph.getBoundingClientRect();
        return { top, bottom };
      }),
    };
  });
  expect(detailBoxes.paragraphs).toHaveLength(2);
  for (const paragraph of detailBoxes.paragraphs) {
    expect(paragraph.top).toBeGreaterThanOrEqual(detailBoxes.pathBottom - 1);
    expect(paragraph.bottom).toBeLessThanOrEqual(detailBoxes.footerTop + 1);
  }

  await lazy.scrollIntoViewIfNeeded();
  await expect.poll(async () => commandArgs(page, "get_manager_plugin_display")).toContainEqual({ releaseId: "release-9" });
  await expectCoverAboveDetails(lazy, "Cloud Lazy 6", "/sandbox/games/cloud-lazy-6");
  for (const [name, releaseId] of [["Cloud Missing", "release-2"], ["Cloud Failure", "release-3"]] as const) {
    const row = card(list, page, name);
    await row.scrollIntoViewIfNeeded();
    await expect.poll(async () => commandArgs(page, "get_manager_plugin_display")).toContainEqual({ releaseId });
    await expect(row.locator("img")).toHaveCount(0);
    await expect(row.getByText("无游戏封面", { exact: true })).toBeVisible();
  }
  const noSource = card(list, page, "No Cover Source Game");
  await expect(noSource.locator("img")).toHaveCount(0);
  await expect(noSource.getByText("无游戏封面", { exact: true })).toBeVisible();

  for (const row of await rows.all()) await expect(row.getByRole("button", { name: "更多操作", exact: true })).toBeVisible();
  const unmatched = card(list, page, "Unmatched Local Game");
  await unmatched.getByRole("button", { name: "更多操作", exact: true }).click();
  await expect(page.getByRole("menuitem", { name: "复制游戏目录", exact: true })).toBeVisible();
  await expect(page.getByRole("menuitem", { name: "联网识别", exact: true })).toBeEnabled();
  await page.keyboard.press("Escape");
  await page.evaluate(() => {
    Object.defineProperty(navigator.clipboard, "writeText", {
      configurable: true,
      value: async (text: string) => { (window as any).__AMM_TEST__.clipboardText = text; },
    });
  });
  await local.getByRole("button", { name: "更多操作", exact: true }).click();
  await page.getByRole("menuitem", { name: "复制游戏目录", exact: true }).click();
  await expect(page.getByText("已复制", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.clipboardText)).toBe("/sandbox/games/local-cover");
});
