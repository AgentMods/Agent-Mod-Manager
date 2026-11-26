import { lazy } from "react";
import { createHashRouter, generatePath, Navigate, useOutletContext, type RouteObject } from "react-router";
import { tr } from "./lib/i18n";
import type { BootstrapState, PageName } from "./lib/types";

const HomePage = lazy(() => import("./pages/HomePage"));
const OnboardingPage = lazy(() => import("./pages/OnboardingPage"));
export type ShellContext = { bootstrap?: BootstrapState; version: string };

function HomeRoute() {
  return <HomePage {...useOutletContext<ShellContext>()} />;
}

function OnboardingRoute() {
  const { bootstrap } = useOutletContext<ShellContext>();
  return bootstrap
    ? <OnboardingPage bootstrap={bootstrap} automatic={bootstrap.isFirstRun || bootstrap.resumeOnboarding} />
    : <Navigate to="/settings" replace />;
}

type StaticPage = Exclude<PageName, "detail">;
const paths = {
  home: "/", browse: "/browse", library: "/library", detail: "/games/:gameId", agent: "/agent",
  plugins: "/plugins", trash: "/trash", history: "/history", settings: "/settings", onboarding: "/onboarding",
} as const satisfies Record<PageName, string>;

function page(id: PageName, Component: RouteObject["Component"]): RouteObject {
  return id === "home" ? { id, index: true, Component } : { id, path: paths[id], Component };
}

// A fragment keeps every page on the same document in the packaged Tauri WebView.
export const router = createHashRouter([{
  path: "/",
  HydrateFallback: () => <div role="status" className="grid min-h-dvh place-items-center">{tr("正在加载...")}</div>,
  lazy: { Component: async () => (await import("./App")).default },
  children: [
    page("home", HomeRoute), page("onboarding", OnboardingRoute),
    page("library", lazy(() => import("./pages/LibraryPage"))),
    page("detail", lazy(() => import("./pages/GameDetailPage"))),
    page("browse", lazy(() => import("./pages/BrowsePage"))),
    page("agent", lazy(() => import("./pages/AgentPage"))),
    page("plugins", lazy(() => import("./pages/PluginsPage"))),
    page("trash", lazy(() => import("./pages/TrashPage"))),
    page("history", lazy(() => import("./pages/HistoryPage"))),
    page("settings", lazy(() => import("./pages/SettingsPage"))),
    { path: "*", element: <Navigate to="/" replace /> },
  ],
}]);

function go(path: string) {
  if (router.state.location.pathname !== path) void router.navigate(path);
}
export function navigatePage(id: StaticPage) { go(paths[id]); }
export function selectGame(gameId: string) { go(generatePath(paths.detail, { gameId })); }

// Native events and delayed actions must check the live route, not a captured render.
export function currentRoute() {
  const match = router.state.matches.at(-1);
  return { view: match?.route.id, gameId: match?.params.gameId };
}
