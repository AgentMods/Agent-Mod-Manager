import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { Gamepad2Icon, LibraryIcon, MessageSquareIcon, MoonIcon, PuzzleIcon, SettingsIcon, SunIcon, Trash2Icon } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { backendAvailable, useBackendQuery } from "./lib/api";
import { subscribeAgentToolResults, syncAgentEvents } from "./lib/agent-store";
import { invalidateAgentModQueries } from "./lib/agent-mod-cache";
import { syncModSwitches, withoutModSwitchWrites } from "./lib/mod-switch-store";
import { useUI } from "./lib/store";
import { t } from "./lib/i18n";
import type { BootstrapState, PageName } from "./lib/types";
import { BackgroundApprovalLink } from "./components/AgentConfirmation";
import { PlanDialog } from "./components/PlanDialog";
import { ErrorState, IconButton } from "./components/ui";
import { Breadcrumb, BreadcrumbItem, BreadcrumbList, BreadcrumbPage, BreadcrumbSeparator } from "@/components/ui/breadcrumb";
import { Sidebar, SidebarContent, SidebarFooter, SidebarGroup, SidebarGroupContent, SidebarHeader, SidebarInset, SidebarMenu, SidebarMenuButton, SidebarMenuItem, SidebarProvider, SidebarRail, SidebarTrigger } from "@/components/ui/sidebar";
import { Spinner } from "@/components/ui/spinner";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Separator } from "@/components/ui/separator";
import { cn } from "@/lib/utils";
import { stagger } from "@/lib/motion";

const LibraryPage = lazy(() => import("./pages/LibraryPage"));
const GameDetailPage = lazy(() => import("./pages/GameDetailPage"));
const AgentPage = lazy(() => import("./pages/AgentPage"));
const PluginsPage = lazy(() => import("./pages/PluginsPage"));
const TrashPage = lazy(() => import("./pages/TrashPage"));
const HistoryPage = lazy(() => import("./pages/HistoryPage"));
const SettingsPage = lazy(() => import("./pages/SettingsPage"));
const OnboardingPage = lazy(() => import("./pages/OnboardingPage"));

const navigation = [
  { id: "agent", label: "Agent ", icon: MessageSquareIcon },
  { id: "library", label: "游戏库", icon: LibraryIcon },
  { id: "plugins", label: "插件中心", icon: PuzzleIcon },
  { id: "trash", label: "回收站", icon: Trash2Icon },
  // { id: "history", label: "历史对话", icon: HistoryIcon },
] as const;
const settingsItem = { id: "settings", label: "设置", icon: SettingsIcon } as const;
const APP_VERSION = "v0.1.0";
const pages = { library: LibraryPage, detail: GameDetailPage, agent: AgentPage, plugins: PluginsPage, trash: TrashPage, history: HistoryPage, settings: SettingsPage };

/** The navigation rail collapses to icons below this width so content keeps its room. */
const WIDE_QUERY = "(min-width: 1100px)";
function useWideWindow() {
  const [wide, setWide] = useState(() => matchMedia(WIDE_QUERY).matches);
  useEffect(() => {
    const query = matchMedia(WIDE_QUERY);
    const sync = () => setWide(query.matches);
    query.addEventListener("change", sync);
    return () => query.removeEventListener("change", sync);
  }, []);
  return wide;
}

/** `null` means the user never chose, so the rail follows the window width. */
const RAIL_KEY = "amm.sidebar";
function storedRail(): boolean | null {
  try { const stored = localStorage.getItem(RAIL_KEY); if (stored === "open" || stored === "closed") return stored === "open"; }
  catch { /* Storage may be unavailable in a restricted webview. */ }
  return null;
}
function storeRail(open: boolean) {
  try { localStorage.setItem(RAIL_KEY, open ? "open" : "closed"); } catch { /* Keep the in-memory choice. */ }
}

function Loading({ children }: { children: string }) {
  return <div role="status" className="flex min-h-40 items-center justify-center gap-2 p-8 text-sm text-muted-foreground motion-safe:animate-soft-fade"><Spinner />{children}</div>;
}

function invalidateOrganizedModQueries(client: ReturnType<typeof useQueryClient>, gameId: string) {
  void client.invalidateQueries({ queryKey: ["installs", gameId] });
  void client.invalidateQueries({ queryKey: ["mod-tags", gameId] });
}

export default function App() {
  const view = useUI((state) => state.view);
  const navigate = useUI((state) => state.navigate);
  const theme = useUI((state) => state.theme);
  const toggleTheme = useUI((state) => state.toggleTheme);
  const notify = useUI((state) => state.notify);
  const visibleConversation = useUI((state) => state.view === "agent" ? state.conversationId : null);
  const client = useQueryClient();
  const bootstrap = useBackendQuery<BootstrapState>(["bootstrap"], "get_bootstrap_state");
  const shownTheme = useRef<string | null>(null);
  useEffect(() => {
    const root = document.documentElement;
    const switching = shownTheme.current !== null && shownTheme.current !== theme;
    shownTheme.current = theme;
    root.dataset.theme = theme; root.style.colorScheme = theme;
    if (!switching) return;
    // motion.css fades colours while this class is present; the first paint never animates.
    root.classList.add("theme-switching");
    const timer = setTimeout(() => root.classList.remove("theme-switching"), 400);
    return () => { clearTimeout(timer); root.classList.remove("theme-switching"); };
  }, [theme]);
  useEffect(() => subscribeAgentToolResults((results) => {
    // Read back durable changes as soon as the tool finishes; the Agent may
    // keep working in this turn. An unknown publication is not a receipt.
    const published = results.some(({ payload: { name, status, result } }) =>
      name === "publish_plugin" && status === "done" && result &&
      typeof result === "object" && "state" in result && result.state === "completed");
    const installed = results.some(({ payload: { name, status } }) => name === "install_plugin" && status === "done");
    if (installed) {
      // An updated adapter may also change the default name of its bound games.
      for (const key of ["games", "game", "mod-library"]) void client.invalidateQueries({ queryKey: [key] });
    }
    if (published || installed) {
      void client.invalidateQueries({ queryKey: ["plugin-drafts"] });
    }
    if (published) void client.invalidateQueries({ queryKey: ["plugins"] });
  }), [client]);
  useEffect(() => {
    if (!backendAvailable) return;
    const refresh = () => {
      for (const key of ["plugins", "skills", "mcp", "games", "game", "mod-library", "manager-plugin-preview"]) {
        void client.invalidateQueries({ queryKey: [key] });
      }
    };
    let disposed = false;
    const subscription = listen<string>("plugin://catalog-updated", refresh).then(stop => {
      if (!disposed) refresh(); // A native update can finish while the webview is starting.
      return stop;
    }).catch(error => {
      if (!disposed) notify(`无法订阅适配包更新：${String(error)}`);
    });
    return () => { disposed = true; void subscription.then(stop => stop?.()); };
  }, [client, notify]);
  useEffect(() => syncModSwitches(client, notify), [client, notify]);
  useEffect(() => {
    if (!backendAvailable) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<{ gameId: string; installId: string }>("mod://organized", ({ payload }) => {
      if (typeof payload?.gameId !== "string" || !payload.gameId ||
          typeof payload.installId !== "string" || !payload.installId) return;
      invalidateOrganizedModQueries(client, payload.gameId);
    }).then((stop) => {
      if (disposed) {
        stop();
        return;
      }
      unlisten = stop;
      // A query may have completed before Tauri finished registering this listener.
      // Refresh the currently open game once registration is ready to cover that gap.
      const current = useUI.getState();
      if (current.view === "detail" && current.gameId) {
        invalidateOrganizedModQueries(client, current.gameId);
      }
    }).catch((error) => {
      if (!disposed) notify(`无法订阅 Mod 整理更新：${String(error)}`);
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [client, notify]);
  useEffect(() => syncAgentEvents(visibleConversation, (id, events) => {
    void client.invalidateQueries({ queryKey: ["conversation", id] });
    void client.invalidateQueries({ queryKey: ["conversations"] });
    const boundGameIds = new Set(events.flatMap((event) => {
      if (event.kind !== "tool-result" || event.payload.name !== "set_game_adapter" || event.payload.status !== "done") return [];
      const result = event.payload.result;
      if (!result || typeof result !== "object" || !("gameId" in result)) return [];
      const gameId = result.gameId;
      return typeof gameId === "string" && gameId.length > 0 ? [gameId] : [];
    }));
    if (boundGameIds.size) {
      void client.invalidateQueries({ queryKey: ["games"] });
      for (const gameId of boundGameIds) {
        void client.invalidateQueries({ queryKey: ["game", gameId] });
        void client.invalidateQueries({ queryKey: ["mod-library", gameId] });
      }
    }
    const successfulTools = new Set(events.filter(event => event.kind === "tool-result" && event.payload.status === "done")
      .map(event => event.payload.name));
    const createdDraft = successfulTools.has("write_plugin_draft") || successfulTools.has("import_cloud_plugin");
    if (createdDraft) void client.invalidateQueries({ queryKey: ["plugin-drafts"] });
    if (createdDraft || successfulTools.has("install_plugin")) {
      for (const key of ["plugins", "skills"]) {
        void client.invalidateQueries({ queryKey: [key] });
      }
    }
    invalidateAgentModQueries(client, withoutModSwitchWrites(events));
    if (successfulTools.has("publish_plugin")) {
      for (const key of ["manager-publications", "manager-plugin-games"]) {
        void client.invalidateQueries({ queryKey: [key] });
      }
    }
  }, (error) => notify(String(error))), [client, visibleConversation, notify]);
  const appName = t("app.name");
  const wide = useWideWindow();
  const [railOpen, setRailOpen] = useState<boolean | null>(storedRail);
  const changeRail = (open: boolean) => { setRailOpen(open); storeRail(open); };

  if (backendAvailable && bootstrap.isLoading && !bootstrap.data) return <div className="flex min-h-dvh items-center justify-center bg-background"><Loading>正在准备桌面服务…</Loading></div>;
  const automaticOnboarding = !!(bootstrap.data?.isFirstRun || bootstrap.data?.resumeOnboarding);
  if (backendAvailable && bootstrap.data && (view === "onboarding" || automaticOnboarding)) {
    return <Suspense fallback={<main className="flex min-h-dvh items-center justify-center bg-background"><Loading>正在加载引导…</Loading></main>}>
      <OnboardingPage automatic={automaticOnboarding} bootstrap={bootstrap.data} />
    </Suspense>;
  }

  const shellView = view === "onboarding" ? "settings" : view;
  const Page = pages[shellView];
  const current = shellView === "detail" ? "游戏详情" : [...navigation, settingsItem].find((item) => item.id === shellView)?.label;
  const macos = backendAvailable && navigator.platform.startsWith("Mac");
  return <SidebarProvider open={railOpen ?? wide} onOpenChange={changeRail} className="h-dvh min-h-0 overflow-hidden" style={{ "--sidebar-width": "13.5rem" } as React.CSSProperties}>
    <Sidebar collapsible="icon" className="[&_[data-slot=sidebar-inner]]:bg-sidebar">
      <SidebarHeader className={cn("gap-0 p-3", macos && "pt-10")}>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton size="lg" tooltip={appName} aria-label={appName} onClick={() => navigate("agent")}>
              <img src="/favicon.png" width={32} height={32} alt="" className="size-8 rounded-lg" />
              <span className="truncate text-sm font-semibold">{appName}</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>
      <SidebarContent>
        <SidebarGroup>
          <SidebarGroupContent>
            <nav aria-label="主导航">
              <SidebarMenu>
                {navigation.map(({ id, label, icon: Icon }, index) => {
                  const active = view === id || (id === "library" && view === "detail");
                  return <SidebarMenuItem key={id} className="motion-safe:animate-slide-in-left" style={stagger(index, 50)}>
                    <SidebarMenuButton tooltip={label} isActive={active} aria-current={view === id ? "page" : undefined} onClick={() => navigate(id as PageName)}>
                      <Icon /><span>{label}</span>
                    </SidebarMenuButton>
                  </SidebarMenuItem>;
                })}
              </SidebarMenu>
            </nav>
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>
      <SidebarFooter className="p-3">
        <div className="flex items-center justify-between gap-2 group-data-[collapsible=icon]:flex-col">
          <IconButton title={settingsItem.label} variant={view === "settings" ? "secondary" : "ghost"} aria-current={view === "settings" ? "page" : undefined} onClick={() => navigate("settings")}><settingsItem.icon /></IconButton>
          <IconButton title={theme === "light" ? "切换暗色主题" : "切换亮色主题"} onClick={toggleTheme}>{theme === "light" ? <MoonIcon className="motion-safe:animate-icon-swap" /> : <SunIcon className="motion-safe:animate-icon-swap" />}</IconButton>
        </div>
      </SidebarFooter>
      <SidebarRail />
    </Sidebar>
    <SidebarInset className="min-h-0 min-w-0">
      <header data-tauri-drag-region className="flex h-14 shrink-0 items-center justify-between gap-4 border-b px-4">
        <div className="flex min-w-0 items-center gap-2">
          <SidebarTrigger className="-ml-1" />
          <Separator orientation="vertical" className="mr-1 h-4" />
          <Breadcrumb className="min-w-0">
            <BreadcrumbList className="flex-nowrap">
              <BreadcrumbItem className="hidden sm:inline-flex"><Gamepad2Icon className="size-4 text-muted-foreground" /><span className="font-medium">{appName}</span><span className="text-xs text-muted-foreground">{APP_VERSION}</span></BreadcrumbItem>
              <BreadcrumbSeparator className="hidden sm:block" />
              <BreadcrumbItem className="min-w-0"><BreadcrumbPage className="truncate">{current}</BreadcrumbPage></BreadcrumbItem>
            </BreadcrumbList>
          </Breadcrumb>
        </div>
        <BackgroundApprovalLink visibleConversation={visibleConversation} />
      </header>
      {!backendAvailable && <Alert role="status" className="rounded-none border-x-0 border-t-0 bg-muted/50"><AlertDescription>未连接桌面服务，游戏目录与文件操作不可用。</AlertDescription></Alert>}
      {bootstrap.data?.recoveryError && <ErrorState className="rounded-none border-x-0 border-t-0" error={`文件事务恢复需要处理：${bootstrap.data.recoveryError}`} />}
      <ErrorState className="rounded-none border-x-0 border-t-0" error={bootstrap.error} />
      <main className="min-h-0 min-w-0 flex-1 overflow-auto">
        <Suspense fallback={<Loading>正在加载...</Loading>}><Page /></Suspense>
      </main>
    </SidebarInset>
    <PlanDialog />
  </SidebarProvider>;
}
