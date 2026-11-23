import { createHash } from "node:crypto";
import type { Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";
import { GAME_COVER_BYTES, GAME_COVER_URL } from "./publication-fixture";

const adapter = {
  slug: "minecraft-java", name: "Minecraft Java Edition", version: "1.1.0", platforms: ["windows", "linux"],
  coverUrl: GAME_COVER_URL,
  mcpServers: [{ id: "catalog-helper", name: "清单工具", transport: "stdio", command: "catalog-helper", args: ["--mode", "list only"], url: null }],
};
export const adapterText = JSON.stringify(adapter, null, 2) + "\n";
export const skillText = "---\nname: minecraft-java\ndescription: 示例安装规则\n---\n\n# 安装规则\n保持原样 <strong>原文</strong>。\n";
export const release = {pluginId: "9007199254740993", releaseId: "9007199254740995"};
const digest = (text: string) => createHash("sha256").update(text).digest("hex");
type Options = { separateGames?: boolean; installed?: boolean; directory?: "empty" | "fail-once"; previewFails?: boolean; installFailsOnce?: boolean; removeFailsOnce?: boolean; missingCover?: boolean; onboarding?: boolean };

export async function openCatalogFixture(page: Page, options: Options = {}) {
  await installMockTauri(page, {
    onboarding: options.onboarding,
    gameWithoutAdapter: true, withExternalAdapter: options.installed, gameWithoutMods: true,
    failInstalledPluginRemoval: options.removeFailsOnce ? "once" : null,
  });
  await page.route(GAME_COVER_URL, route => route.fulfill({status: 200, contentType: "image/png", body: GAME_COVER_BYTES}));
  await page.goto("/");
  const fixtureAdapter = {...adapter};
  if (options.missingCover) delete fixtureAdapter.coverUrl;
  const fixtureAdapterText = JSON.stringify(fixtureAdapter, null, 2) + "\n";
  const hashes = { adapterSha256: digest(fixtureAdapterText), skillSha256: digest(skillText) };
  await page.evaluate(({ options, adapter, adapterText, skillText, hashes, release }) => {
    const host = window as any;
    const original = host.__TAURI_INTERNALS__.invoke;
    let failList = options.directory === "fail-once";
    const plugin = { slug: adapter.slug, name: adapter.name, version: adapter.version, platforms: adapter.platforms,
      id: release.pluginId, releaseId: release.releaseId, authorName: "山雀", steamAppId: "413150", description: "示例安装规则" };
    const other = { ...plugin, id: "21", releaseId: "22", name: "备用适配包", authorName: "远山", version: "2.0.0", platforms: ["macos"] };
    const catalog = [{id: "7", name: "我的世界", slug: "minecraft", aliases: ["Minecraft Java Edition", "方块世界"], steamAppId: "413150", plugins: [other, plugin]}];
    if (options.separateGames) {
      catalog[0].plugins = [plugin];
      catalog.push({id: "8", name: "另一款游戏", slug: "another-game", aliases: [], steamAppId: "900", plugins: [{...other, slug: "another-adapter", steamAppId: "900"}]});
    }
    host.__AMM_TEST__.managerPluginGames = structuredClone(catalog);
    const display = {description: "管理这款游戏已声明的模组安装规则。", coverUrl: options.missingCover ? null : adapter.coverUrl};
    const preview = { display, gameName: "我的世界", plugin, source: "https://www.glossmod.com/manager-plugins", adapterJson: adapterText, skillMarkdown: skillText, hashes, mcpServers: adapter.mcpServers, installedVersion: options.installed ? "1.0.0" : null };
    host.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      if (command === "get_manager_plugin_display") {
        host.__AMM_TEST__.calls.push({command, args});
        return args.releaseId === release.releaseId ? display : null;
      }
      if (command === "list_games") return (await original(command, args)).map((game: object) => ({...game, steamAppId: "413150"}));
      if (["list_manager_plugin_games", "preview_manager_plugin", "install_manager_plugin"].includes(command)) {
        host.__AMM_TEST__.calls.push({command, args});
        if (command === "list_manager_plugin_games") {
          if (failList) { failList = false; throw new Error("云端目录连接超时"); }
          return options.directory === "empty" ? {games: [], totalGames: 0, skippedEntries: 0} : {games: catalog, totalGames: catalog.length, skippedEntries: 1};
        }
        if (args.pluginId !== release.pluginId || args.releaseId !== release.releaseId) throw new Error("所选云插件版本已下架");
        if (command === "preview_manager_plugin") {
          if (options.previewFails) throw new Error("所选云插件版本已下架");
          return preview;
        }
        await new Promise(resolve => setTimeout(resolve, 180));
        if (options.installFailsOnce && !host.__AMM_TEST__.managerPluginInstallFailed) {
          host.__AMM_TEST__.managerPluginInstallFailed = true;
          throw new Error("模拟云端插件安装失败");
        }
        const installed = {
          slug: adapter.slug, name: adapter.name, gameName: adapter.name, version: adapter.version,
          description: "示例安装规则", source: "external", validated: true, enabled: true,
          installPath: `/sandbox/data/plugins/games/${adapter.slug}`, coverUrl: null, updatedAt: "2026-11-22T00:00:00Z",
          hasCloudEvidence: true,
        };
        const installedPlugins = host.__AMM_TEST__.installedPlugins as Record<string, unknown>[];
        const index = installedPlugins.findIndex((entry) => entry.slug === adapter.slug);
        if (index >= 0) installedPlugins.splice(index, 1, installed);
        else installedPlugins.push(installed);
        host.__AMM_TEST__.cloudPluginSlugs.add(adapter.slug);
        return null;
      }
      return original(command, args);
    };
  }, { options, adapter: fixtureAdapter, adapterText: fixtureAdapterText, skillText, hashes, release });
  return page.evaluate(() => structuredClone((window as any).__AMM_TEST__.managerPluginGames));
}
