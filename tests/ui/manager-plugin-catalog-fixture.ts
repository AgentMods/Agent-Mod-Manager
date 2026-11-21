import { createHash } from "node:crypto";
import type { Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const adapter = {
  slug: "minecraft-java", name: "Minecraft Java Edition", version: "1.1.0", platforms: ["windows", "linux"],
  mcpServers: [{ id: "catalog-helper", name: "清单工具", transport: "stdio", command: "catalog-helper", args: ["--mode", "list only"], url: null }],
};
export const adapterText = JSON.stringify(adapter, null, 2) + "\n";
export const skillText = "---\nname: minecraft-java\ndescription: 示例安装规则\n---\n\n# 安装规则\n保持原样 <strong>原文</strong>。\n";
export const release = {pluginId: "9007199254740993", releaseId: "9007199254740995"};
const digest = (text: string) => createHash("sha256").update(text).digest("hex");
type Options = { installed?: boolean; directory?: "empty" | "fail-once"; changeAtStage?: boolean; previewFails?: boolean; draftChanges?: boolean };

export async function openCatalogFixture(page: Page, options: Options = {}) {
  await installMockTauri(page, { gameWithoutAdapter: true, withExternalAdapter: options.installed, gameWithoutMods: true });
  await page.goto("/");
  const stagedSkill = options.changeAtStage ? skillText + "更新后的草稿。\n" : skillText;
  const hashes = { adapterSha256: digest(adapterText), skillSha256: digest(skillText) };
  await page.evaluate(({ options, adapter, adapterText, skillText, stagedSkill, hashes, stagedSkillHash, release }) => {
    const host = window as any;
    const original = host.__TAURI_INTERNALS__.invoke;
    let failList = options.directory === "fail-once";
    let draftId: string | undefined;
    const plugin = { ...adapter, id: release.pluginId, releaseId: release.releaseId, authorName: "山雀", steamAppId: "413150", description: "示例安装规则" };
    const other = { ...plugin, id: "21", releaseId: "22", name: "备用适配包", authorName: "远山", version: "2.0.0", platforms: ["macos"] };
    const catalog = [{id: "7", name: "我的世界", slug: "minecraft", aliases: ["Minecraft Java Edition", "方块世界"], steamAppId: "413150", plugins: [other, plugin]}];
    const preview = { gameName: "我的世界", plugin, source: "https://www.glossmod.com/manager-plugins", adapterJson: adapterText, skillMarkdown: skillText, hashes, mcpServers: adapter.mcpServers, installedVersion: options.installed ? "1.0.0" : null };
    const staged = {...preview, skillMarkdown: stagedSkill, hashes: {...hashes, skillSha256: stagedSkillHash}};
    host.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      if (command === "list_games") return (await original(command, args)).map((game: object) => ({...game, steamAppId: "413150"}));
      if (["list_manager_plugin_games", "preview_manager_plugin", "prepare_manager_plugin_import"].includes(command)) {
        host.__AMM_TEST__.calls.push({command, args});
        if (command === "list_manager_plugin_games") {
          if (failList) { failList = false; throw new Error("云端目录连接超时"); }
          return options.directory === "empty" ? {games: [], totalGames: 0, skippedEntries: 0} : {games: catalog, totalGames: 1, skippedEntries: 1};
        }
        if (args.pluginId !== release.pluginId || args.releaseId !== release.releaseId || options.previewFails) throw new Error("所选云插件版本已下架");
        if (command === "preview_manager_plugin") return preview;
        // Reuse the shared draft transaction model for the two texts returned by this release.
        const result = await original("write_plugin_draft", { slug: adapter.slug, adapterJson: adapterText, skillMarkdown: stagedSkill });
        draftId = result.draft.id;
        return { preview: staged, draftId, plan: result.plan };
      }
      const result = await original(command, args);
      if (draftId && (command === "install_plugin" || command === "validate_plugin") && args.draftId === draftId) {
        return {...result, hashes: options.draftChanges ? {...staged.hashes, skillSha256: "f".repeat(64)} : staged.hashes};
      }
      return result;
    };
  }, { options, adapter, adapterText, skillText, stagedSkill, hashes, stagedSkillHash: digest(stagedSkill), release });
}
