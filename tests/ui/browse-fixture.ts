import type { Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

export type BrowseSourceFixture = {
  id: string;
  name: string;
  status: "available" | "notConnected" | "permissionDenied" | "unsupported";
};

export type BrowseItemFixture = {
  id: string;
  sourceId: string;
  gameIds: string[];
  title: string;
  originalTitle: string;
  author: string | null;
  summary: string | null;
  typeKey: "visual" | "localization" | "gameplay" | "audio" | "framework" | "other" | "unknown";
  tags: { key: string; label: string }[];
  coverUrl: string | null;
  canOpenProject: boolean;
};

export type BrowsePageFixture = {
  items: BrowseItemFixture[];
  issues?: { gameId: string; sourceId: string; code: string }[];
  gameChoices?: { gameId: string; sourceId: string; options: { id: string; name: string }[] }[];
  hasNext?: boolean;
  isEnriching?: boolean;
  loadingPolls?: number;
  enrichedItems?: BrowseItemFixture[];
};

export const BROWSE_SOURCES: BrowseSourceFixture[] = [
  { id: "glossmod_http", name: "GlossMod", status: "available" },
  { id: "thunderstore", name: "Thunderstore", status: "available" },
  { id: "nexusmods", name: "Nexus Mods", status: "notConnected" },
];

// mock-tauri always registers Minecraft first; add two games to exercise a
// multi-selection while leaving Valheim available as an unrelated game.
export const BROWSE_GAMES = [
  { id: "game-stardew", name: "Stardew Valley" },
  { id: "game-valheim", name: "Valheim" },
];

export const BROWSE_ITEMS: BrowseItemFixture[] = [
  {
    id: "work-starlit-fields",
    sourceId: "glossmod_http",
    gameIds: ["game-fixture", "game-stardew"],
    title: "星光农场",
    originalTitle: "Starlit Fields",
    author: "Northstar",
    summary: "为农场增加新的季节景观。",
    typeKey: "visual",
    tags: [{ key: "environment", label: "环境" }],
    coverUrl: "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSI0MDAiIGhlaWdodD0iMjAwIiB2aWV3Qm94PSIwIDAgNDAwIDIwMCI+PHJlY3Qgd2lkdGg9IjQwMCIgaGVpZ2h0PSIyMDAiIGZpbGw9IiM0Nzg3OGMiLz48dGV4dCB4PSIyMDAiIHk9IjExMCIgdGV4dC1hbmNob3I9Im1pZGRsZSIgZmlsbD0id2hpdGUiIGZvbnQtc2l6ZT0iMjYiPkZpeHR1cmUgY292ZXI8L3RleHQ+PC9zdmc+",
    canOpenProject: true,
  },
  {
    id: "work-gentle-combat",
    sourceId: "thunderstore",
    gameIds: ["game-stardew"],
    title: "温和战斗",
    originalTitle: "Gentle Combat",
    author: "Willow",
    summary: "调整战斗节奏和敌人强度。",
    typeKey: "gameplay",
    tags: [{ key: "balance", label: "平衡" }],
    coverUrl: null,
    canOpenProject: true,
  },
  {
    id: "work-quiet-harvest",
    sourceId: "glossmod_http",
    gameIds: ["game-stardew"],
    title: "宁静丰收",
    originalTitle: "Quiet Harvest",
    author: "Cedar",
    summary: "加入新的农作物和收获音效。",
    typeKey: "localization",
    tags: [{ key: "translation", label: "汉化" }],
    coverUrl: null,
    canOpenProject: true,
  },
  {
    id: "work-valheim-only",
    sourceId: "glossmod_http",
    gameIds: ["game-valheim"],
    title: "静谧群岛",
    originalTitle: "Quiet Archipelago",
    author: "Birch",
    summary: "为群岛增添新的风景。",
    typeKey: "visual",
    tags: [{ key: "environment", label: "环境" }],
    coverUrl: null,
    canOpenProject: true,
  },
];

export const BROWSE_PAGES: BrowsePageFixture[] = [
  // The fixture deliberately supplies a different game's result. The mocked
  // backend must exclude it using the selected game IDs before returning data.
  { items: [BROWSE_ITEMS[0], BROWSE_ITEMS[1], BROWSE_ITEMS[3]], hasNext: true, isEnriching: false },
  { items: [BROWSE_ITEMS[2]], hasNext: false, isEnriching: false },
];

type BrowseFixtureOptions = {
  sources?: BrowseSourceFixture[];
  pages?: BrowsePageFixture[];
  workspaceGames?: { id: string; name: string }[];
};

/** Installs deterministic browse IPC responses over the shared Tauri test transport. */
export async function installBrowseFixture(page: Page, options: BrowseFixtureOptions = {}) {
  await installMockTauri(page, { workspaceGames: options.workspaceGames ?? BROWSE_GAMES });
  await page.addInitScript(({ sources, pages }) => {
    const target = window as any;
    const state = {
      calls: [] as { command: string; args: Record<string, unknown> }[],
      sources: structuredClone(sources),
      pages: structuredClone(pages),
      pageReads: new Map<string, number>(),
      nextSearch: 0,
      activeSearchId: "" as string,
      activeGameIds: [] as string[],
      activeSourceIds: null as string[] | null,
      authorizedSourceIds: [] as string[],
      authorizationId: "browse-authorization-fixture",
      sourceListReadAt: 0,
    };

    const wrap = (internals: any) => {
      if (!internals || internals.__browseFixtureWrapped) return;
      const originalInvoke = internals.invoke;
      internals.invoke = async (command: string, args: Record<string, any> = {}) => {
        if (!command.startsWith("browse_")) return originalInvoke(command, args);
        const call = { command, args: structuredClone(args) };
        state.calls.push(call);
        target.__AMM_TEST__?.calls.push(call);

        switch (command) {
          case "browse_list_sources":
            state.sourceListReadAt = Date.now();
            return structuredClone(state.sources);
          case "browse_authorize": {
            if (!state.sourceListReadAt || Date.now() - state.sourceListReadAt > 180_000) throw new Error("browse sources must be refreshed before authorization");
            const sourceIds = Array.isArray(args.sourceIds) ? args.sourceIds : [];
            if (sourceIds.some((id: string) => !state.sources.some((source: BrowseSourceFixture) => source.id === id && source.status === "available"))) {
              throw new Error("browse authorization contains an unavailable source");
            }
            state.authorizedSourceIds = [...sourceIds];
            return {
              authorizationId: state.authorizationId,
              sources: state.sources.filter((source: BrowseSourceFixture) => sourceIds.includes(source.id)),
            };
          }
          case "browse_start_search":
            if (args.authorizationId !== state.authorizationId) throw new Error("browse authorization missing");
            if (!Array.isArray(args.gameIds) || args.gameIds.length === 0) throw new Error("browse search requires at least one selected game");
            if (Array.isArray(args.sourceIds) && args.sourceIds.some((id: string) => !state.authorizedSourceIds.includes(id))) {
              throw new Error("browse search requested a source outside the authorization");
            }
            state.nextSearch += 1;
            state.activeSearchId = "browse-search-" + state.nextSearch;
            state.activeGameIds = Array.isArray(args.gameIds) ? [...args.gameIds] : [];
            state.activeSourceIds = Array.isArray(args.sourceIds) ? [...args.sourceIds] : [...state.authorizedSourceIds];
            state.pageReads.clear();
            return { searchId: state.activeSearchId };
          case "browse_get_page": {
            if (args.searchId !== state.activeSearchId) throw new Error("browse search expired");
            const pageNumber = Number(args.page);
            const fixture = state.pages[pageNumber] ?? { items: [], hasNext: false, isEnriching: false };
            const readKey = state.activeSearchId + ":" + pageNumber;
            const reads = state.pageReads.get(readKey) ?? 0;
            state.pageReads.set(readKey, reads + 1);
            if (reads < (fixture.loadingPolls ?? 0)) return { status: "loading", page: pageNumber, items: [], issues: [], gameChoices: [], hasNext: false, isEnriching: false };
            return {
              status: "ready",
              page: pageNumber,
              items: structuredClone((fixture.enrichedItems && reads > (fixture.loadingPolls ?? 0) ? fixture.enrichedItems : fixture.items)
                .filter((item: BrowseItemFixture) => item.gameIds.some(id => state.activeGameIds.includes(id)))
                .filter((item: BrowseItemFixture) => state.activeSourceIds.includes(item.sourceId))),
              issues: structuredClone(fixture.issues ?? []),
              gameChoices: structuredClone(fixture.gameChoices ?? []),
              hasNext: fixture.hasNext ?? false,
              isEnriching: fixture.isEnriching ?? false,
            };
          }
          case "browse_select_game_match":
          case "browse_open_project":
          case "browse_cancel_search":
            return null;
          case "browse_install_task":
            return null;
          default:
            throw new Error("Unimplemented browse test IPC: " + command);
        }
      };
      internals.__browseFixtureWrapped = true;
    };

    let internals = target.__TAURI_INTERNALS__;
    Object.defineProperty(target, "__TAURI_INTERNALS__", {
      configurable: true,
      enumerable: true,
      get: () => internals,
      set: (value: any) => {
        internals = value;
        wrap(internals);
      },
    });
    wrap(internals);
    target.__BROWSE_TEST__ = state;
  }, { sources: options.sources ?? BROWSE_SOURCES, pages: options.pages ?? BROWSE_PAGES });
}

export async function browseCalls(page: Page, command?: string) {
  return page.evaluate((requested) => {
    const calls = (window as any).__BROWSE_TEST__.calls as { command: string; args: Record<string, unknown> }[];
    return calls.filter((call) => !requested || call.command === requested);
  }, command);
}
