import type { Page } from "@playwright/test";
import type { GameCandidate } from "../../src/lib/types";
import { openCatalogFixture, release } from "./manager-plugin-catalog-fixture";

type CatalogPlugin = { id: string; releaseId: string; slug: string; name: string; gameName?: string; version: string; platforms: string[]; steamAppId: string };
type CatalogGame = { id: string; name: string; slug: string; aliases: string[]; steamAppId: string; plugins: CatalogPlugin[] };
type CloudCatalog = { games: CatalogGame[]; totalGames: number; skippedEntries: number };

export async function openGameAdapterMatchingFixture(page: Page, {
  onboarding = false,
  candidates,
  catalogStatus = "available",
  failCloudRegistrationOnce = false,
  installedPlugin = false,
  catalogGroupName,
  pluginTitle,
  releaseGameName,
  deferCatalog = false,
}: {
  onboarding?: boolean;
  candidates: GameCandidate[];
  catalogStatus?: "available" | "empty" | "offline";
  failCloudRegistrationOnce?: boolean;
  installedPlugin?: boolean;
  catalogGroupName?: string;
  pluginTitle?: string;
  releaseGameName?: string | null;
  deferCatalog?: boolean;
}) {
  const fixtureGames = await openCatalogFixture(page, { onboarding, installed: installedPlugin, separateGames: true }) as CatalogGame[];
  const matchedGame = fixtureGames.find((game) => game.plugins.some((plugin) => plugin.id === release.pluginId && plugin.releaseId === release.releaseId));
  const matchedRelease = matchedGame?.plugins.find((plugin) => plugin.id === release.pluginId && plugin.releaseId === release.releaseId);
  if (matchedGame && catalogGroupName !== undefined) matchedGame.name = catalogGroupName;
  if (matchedRelease && pluginTitle !== undefined) matchedRelease.name = pluginTitle;
  if (matchedRelease && releaseGameName !== undefined) {
    if (releaseGameName === null) delete matchedRelease.gameName;
    else matchedRelease.gameName = releaseGameName;
  }
  const catalog: CloudCatalog = {
    games: catalogStatus === "available" ? fixtureGames : [],
    totalGames: catalogStatus === "available" ? fixtureGames.length : 0,
    skippedEntries: 0,
  };

  await page.addInitScript(({ catalog, candidates, catalogStatus, failCloudRegistrationOnce, deferCatalog }) => {
    const host = window as any;
    const wrapped = new WeakSet<object>();
    let releaseCatalog: (() => void) | undefined;
    const catalogGate = new Promise<void>((resolve) => { releaseCatalog = resolve; });
    host.__AMM_TEST__.releaseManagerPluginCatalog = () => releaseCatalog?.();
    const attach = (internals: any) => {
      if (!internals || wrapped.has(internals)) return;
      wrapped.add(internals);
      const original = internals.invoke;
      let failNextRegistration = failCloudRegistrationOnce;
      let registeredCount = 0;
      internals.invoke = async (command: string, args: Record<string, any> = {}, ...rest: unknown[]) => {
        if (command === "list_manager_plugin_games") {
          host.__AMM_TEST__.calls.push({ command, args: structuredClone(args) });
          if (catalogStatus === "offline") throw new Error("模拟云端目录离线");
          if (deferCatalog) await catalogGate;
          return structuredClone(catalog);
        }
        if (command === "scan_games" && !args.options?.page?.cancel) {
          host.__AMM_TEST__.calls.push({ command, args: structuredClone(args) });
          return { candidates: structuredClone(candidates), warnings: [], directoriesChecked: 3, entriesChecked: 18, truncated: false };
        }
        if (command === "register_game") {
          host.__AMM_TEST__.calls.push({ command, args: structuredClone(args) });
          if (failNextRegistration && args.cloudAdapter) {
            failNextRegistration = false;
            throw new Error("模拟云端适配包安装失败");
          }
          const candidate = candidates.find((item) => item.rootPath === args.directory);
          if (!candidate) throw new Error("unknown candidate");
          const cloudAdapter = args.cloudAdapter as { pluginId?: string; releaseId?: string } | undefined;
          const release = cloudAdapter && catalog.games.flatMap((game) => game.plugins).find((plugin) =>
            plugin.id === cloudAdapter.pluginId && plugin.releaseId === cloudAdapter.releaseId,
          );
          if (cloudAdapter && !release) throw new Error("unknown cloud release");
          const id = `game-match-fixture-${++registeredCount}`;
          const now = "2026-11-23T00:00:00Z";
          const game = {
            id, name: args.discoveryName ?? candidate.name, slug: release?.slug ?? args.adapterSlug ?? "unknown", rootPath: candidate.rootPath,
            platform: candidate.platform, adapterSlug: release?.slug ?? args.adapterSlug ?? null,
            adapterVersion: release?.version ?? null, confidence: candidate.confidence, coverPath: null,
            steamAppId: release?.steamAppId ?? candidate.sources?.find((source) => source.kind === "steam")?.steamAppId ?? null,
            installedModCount: 0, enabledModCount: 0, metadata: {}, createdAt: now, updatedAt: now,
          };
          host.__AMM_TEST__.games.push(game);
          host.__AMM_TEST__.registerResults ??= [];
          host.__AMM_TEST__.registerResults.push(structuredClone(game));
          if (release) {
            const plugins = host.__AMM_TEST__.installedPlugins as Record<string, unknown>[];
            if (!plugins.some((plugin) => plugin.slug === release.slug)) {
              plugins.push({ slug: release.slug, name: release.name, gameName: release.name, version: release.version,
                description: "Playwright cloud adapter", source: "external", validated: true, enabled: true,
                installPath: `/sandbox/data/plugins/games/${release.slug}`, coverUrl: null, updatedAt: now, hasCloudEvidence: true });
            }
            host.__AMM_TEST__.cloudPluginSlugs.add(release.slug);
          }
          return structuredClone(game);
        }
        return original(command, args, ...rest);
      };
    };

    const key = "__TAURI_INTERNALS__";
    let current = host[key];
    Object.defineProperty(host, key, {
      configurable: true,
      enumerable: true,
      get: () => current,
      set: (value) => { current = value; attach(value); },
    });
    attach(current);
  }, { catalog, candidates, catalogStatus, failCloudRegistrationOnce, deferCatalog });

  await page.reload();
  return catalog;
}
