import { lazy, Suspense, useEffect, useState } from "react";
import { Gamepad2Icon, HistoryIcon, LibraryIcon, MessageSquareIcon, MoonIcon, PuzzleIcon, SearchIcon, SettingsIcon, SunIcon, Trash2Icon } from "lucide-react";
import { backendAvailable, useBackendQuery } from "./lib/api";
import { ensureAgentListeners } from "./lib/agent-store";
import { useUI } from "./lib/store";
import { t } from "./lib/i18n";
import type { BootstrapState, PageName } from "./lib/types";
import { ApprovalDialog } from "./components/ApprovalDialog";
import { PlanDialog } from "./components/PlanDialog";
import { ErrorState, IconButton } from "./components/ui";
import { Badge } from "@/components/ui/badge";
import { Breadcrumb, BreadcrumbItem, BreadcrumbList, BreadcrumbPage, BreadcrumbSeparator } from "@/components/ui/breadcrumb";
import { Sidebar, SidebarContent, SidebarFooter, SidebarGroup, SidebarGroupContent, SidebarHeader, SidebarInset, SidebarMenu, SidebarMenuButton, SidebarMenuItem, SidebarProvider, SidebarRail, SidebarTrigger } from "@/components/ui/sidebar";
import { Spinner } from "@/components/ui/spinner";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Separator } from "@/components/ui/separator";
import { cn } from "@/lib/utils";

const LibraryPage = lazy(() => import("./pages/LibraryPage"));
const GameDetailPage = lazy(() => import("./pages/GameDetailPage"));
const AgentPage = lazy(() => import("./pages/AgentPage"));
const DiscoverPage = lazy(() => import("./pages/DiscoverPage"));
const PluginsPage = lazy(() => import("./pages/PluginsPage"));
const TrashPage = lazy(() => import("./pages/TrashPage"));
const HistoryPage = lazy(() => import("./pages/HistoryPage"));
const SettingsPage = lazy(() => import("./pages/SettingsPage"));
const OnboardingPage = lazy(() => import("./pages/OnboardingPage"));

const navigation = [
  {id: "library", label: "游戏库", icon: LibraryIcon},
  {id: "discover", label: "Mod 发现", icon: SearchIcon},
  {id: "agent", label: "Agent 对话", icon: MessageSquareIcon},
  {id: "plugins", label: "插件中心", icon: PuzzleIcon},
  {id: "trash", label: "回收站", icon: Trash2Icon},
  {id: "history", label: "历史对话", icon: HistoryIcon},
  {id: "settings", label: "设置", icon: SettingsIcon},
] as const;
const pages = {library: LibraryPage, detail: GameDetailPage, agent: AgentPage, discover: DiscoverPage, plugins: PluginsPage, trash: TrashPage, history: HistoryPage, settings: SettingsPage, onboarding: OnboardingPage};

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

function Loading({children}: {children: string}) {
  return <div role="status" className="flex min-h-40 items-center justify-center gap-2 p-8 text-sm text-muted-foreground"><Spinner/>{children}</div>;
}

export default function App() {
  const view = useUI((state) => state.view);
  const navigate = useUI((state) => state.navigate);
  const theme = useUI((state) => state.theme);
  const toggleTheme = useUI((state) => state.toggleTheme);
  const notify = useUI((state) => state.notify);
  const bootstrap = useBackendQuery<BootstrapState>(["bootstrap"], "get_bootstrap_state");
  useEffect(() => {document.documentElement.dataset.theme = theme; document.documentElement.style.colorScheme = theme;}, [theme]);
  useEffect(() => {void ensureAgentListeners().catch((error) => notify(String(error)));}, [notify]);
  const appName = t("app.name");
  const wide = useWideWindow();
  const [railOpen, setRailOpen] = useState<boolean | null>(null);

  if (backendAvailable && bootstrap.isLoading && !bootstrap.data) return <div className="flex min-h-dvh items-center justify-center bg-background"><Loading>正在准备桌面服务…</Loading></div>;
  const automaticOnboarding = !!(bootstrap.data?.isFirstRun || bootstrap.data?.resumeOnboarding);
  if (backendAvailable && bootstrap.data && (view === "onboarding" || automaticOnboarding)) {
    return <Suspense fallback={<main className="flex min-h-dvh items-center justify-center bg-background"><Loading>正在加载引导…</Loading></main>}>
      <OnboardingPage automatic={automaticOnboarding} recoveryError={bootstrap.data.recoveryError}/>
    </Suspense>;
  }

  const Page = pages[view];
  const current = view === "detail" ? "游戏详情" : navigation.find((item) => item.id === view)?.label;
  const macos = backendAvailable && navigator.platform.startsWith("Mac");
  return <SidebarProvider open={railOpen ?? wide} onOpenChange={setRailOpen} className="h-dvh min-h-0 overflow-hidden" style={{"--sidebar-width": "13.5rem"} as React.CSSProperties}>
    <Sidebar collapsible="icon" className="[&_[data-slot=sidebar-inner]]:bg-sidebar">
      <SidebarHeader className={cn("gap-0 p-3", macos && "pt-10")}>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton size="lg" tooltip={appName} aria-label={appName} onClick={() => navigate("library")}>
              <img src="/favicon.png" width={32} height={32} alt="" className="size-8 rounded-lg"/>
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
                {navigation.map(({id, label, icon: Icon}) => {
                  const active = view === id || (id === "library" && view === "detail");
                  return <SidebarMenuItem key={id}>
                    <SidebarMenuButton tooltip={label} isActive={active} aria-current={view === id ? "page" : undefined} onClick={() => navigate(id as PageName)}>
                      <Icon/><span>{label}</span>
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
          <IconButton title={theme === "light" ? "切换暗色主题" : "切换亮色主题"} onClick={toggleTheme}>{theme === "light" ? <MoonIcon/> : <SunIcon/>}</IconButton>
          <span className="text-xs text-muted-foreground group-data-[collapsible=icon]:hidden">v0.1.0</span>
        </div>
      </SidebarFooter>
      <SidebarRail/>
    </Sidebar>
    <SidebarInset className="min-h-0 min-w-0">
      <header data-tauri-drag-region className="flex h-14 shrink-0 items-center justify-between gap-4 border-b px-4">
        <div className="flex min-w-0 items-center gap-2">
          <SidebarTrigger className="-ml-1"/>
          <Separator orientation="vertical" className="mr-1 h-4"/>
          <Breadcrumb className="min-w-0">
            <BreadcrumbList className="flex-nowrap">
              <BreadcrumbItem className="hidden sm:inline-flex"><Gamepad2Icon className="size-4 text-muted-foreground"/><span className="font-medium">{appName}</span></BreadcrumbItem>
              <BreadcrumbSeparator className="hidden sm:block"/>
              <BreadcrumbItem className="min-w-0"><BreadcrumbPage className="truncate">{current}</BreadcrumbPage></BreadcrumbItem>
            </BreadcrumbList>
          </Breadcrumb>
        </div>
        <Badge variant="outline" className="gap-1.5 font-normal text-muted-foreground"><span className={cn("size-1.5 rounded-full", backendAvailable ? "bg-success" : "bg-muted-foreground")}/>{backendAvailable ? "本地服务" : "浏览器预览"}</Badge>
      </header>
      {!backendAvailable && <Alert role="status" className="rounded-none border-x-0 border-t-0 bg-muted/50"><AlertDescription>未连接桌面服务，游戏目录与文件操作不可用。</AlertDescription></Alert>}
      {bootstrap.data?.recoveryError && <ErrorState className="rounded-none border-x-0 border-t-0" error={`文件事务恢复需要处理：${bootstrap.data.recoveryError}`}/>}
      <ErrorState className="rounded-none border-x-0 border-t-0" error={bootstrap.error}/>
      <main className="min-h-0 min-w-0 flex-1 overflow-auto">
        <Suspense fallback={<Loading>正在加载...</Loading>}><Page/></Suspense>
      </main>
    </SidebarInset>
    <PlanDialog/><ApprovalDialog/>
  </SidebarProvider>;
}