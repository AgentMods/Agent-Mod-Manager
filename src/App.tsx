import { lazy, Suspense, useEffect } from "react";
import { Gamepad2, History, Library, MessageSquare, Moon, Puzzle, Search, Settings, Sun, Trash2 } from "lucide-react";
import { backendAvailable, useBackendQuery } from "./lib/api";
import { ensureAgentListeners } from "./lib/agent-store";
import { useUI } from "./lib/store";
import type { PageName } from "./lib/types";
import { ApprovalDialog } from "./components/ApprovalDialog";
import { PlanDialog } from "./components/PlanDialog";
import { ErrorState, IconButton } from "./components/ui";
const LibraryPage = lazy(() => import("./pages/LibraryPage"));
const GameDetailPage = lazy(() => import("./pages/GameDetailPage"));
const AgentPage = lazy(() => import("./pages/AgentPage"));
const DiscoverPage = lazy(() => import("./pages/DiscoverPage"));
const PluginsPage = lazy(() => import("./pages/PluginsPage"));
const TrashPage = lazy(() => import("./pages/TrashPage"));
const HistoryPage = lazy(() => import("./pages/HistoryPage"));
const SettingsPage = lazy(() => import("./pages/SettingsPage"));

const navigation = [
  {id: "library", label: "游戏库", icon: Library},
  {id: "discover", label: "Mod 发现", icon: Search},
  {id: "agent", label: "Agent 对话", icon: MessageSquare},
  {id: "plugins", label: "插件中心", icon: Puzzle},
  {id: "trash", label: "回收站", icon: Trash2},
  {id: "history", label: "历史对话", icon: History},
  {id: "settings", label: "设置", icon: Settings},
] as const;
const pages = {library: LibraryPage, detail: GameDetailPage, agent: AgentPage, discover: DiscoverPage, plugins: PluginsPage, trash: TrashPage, history: HistoryPage, settings: SettingsPage};

export default function App() {
  const view = useUI((s) => s.view);
  const navigate = useUI((s) => s.navigate);
  const theme = useUI((s) => s.theme);
  const toggleTheme = useUI((s) => s.toggleTheme);
  const toast = useUI((s) => s.toast);
  const notify = useUI((s) => s.notify);
  const bootstrap = useBackendQuery<{dataRoot: string; recoveredTransactions: string[]; recoveryError: string | null}>(["bootstrap"], "get_bootstrap_state");
  useEffect(() => {document.documentElement.dataset.theme = theme; document.documentElement.style.colorScheme = theme;}, [theme]);
  useEffect(() => {void ensureAgentListeners().catch((error) => notify(String(error)));}, [notify]);
  const Page = pages[view];
  return <div className={`app-shell ${backendAvailable && navigator.platform.startsWith("Mac") ? "native-macos" : ""}`}>
    <aside className="sidebar"><button className="brand" onClick={() => navigate("library")} title="Mod 管家"><img src="/favicon.png" width={36} height={36} alt="Mod 管家"/><span>Mod 管家</span></button>
      <nav aria-label="主导航">{navigation.map(({id, label, icon: Icon}) => <button key={id} title={label} className={`nav-item ${view === id || (id === "library" && view === "detail") ? "active" : ""}`} onClick={() => navigate(id as PageName)} aria-current={view === id ? "page" : undefined}><Icon size={20}/><span>{label}</span></button>)}</nav>
      <div className="sidebar-footer"><IconButton title={theme === "light" ? "切换暗色主题" : "切换亮色主题"} onClick={toggleTheme}>{theme === "light" ? <Moon size={19}/> : <Sun size={19}/>}</IconButton><span className="muted">v0.1.0</span></div>
    </aside>
    <div className="app-body"><header className="topbar" data-tauri-drag-region><div className="breadcrumb"><Gamepad2 size={17}/><span>Agent Mod Manager</span><span className="separator">/</span><span>{view === "detail" ? "游戏详情" : navigation.find((item) => item.id === view)?.label}</span></div><span className={`connection-status ${backendAvailable ? "connected" : ""}`}>{backendAvailable ? "本地服务" : "浏览器预览"}</span></header>
      {!backendAvailable && <div className="preview-notice" role="status">未连接桌面服务，游戏目录与文件操作不可用。</div>}
      {bootstrap.data?.recoveryError && <ErrorState error={`文件事务恢复需要处理：${bootstrap.data.recoveryError}`}/>}
      <ErrorState error={bootstrap.error}/><main className={`main view-${view}`}><Suspense fallback={<div className="page" role="status">正在加载...</div>}><Page/></Suspense></main>
    </div>
    <PlanDialog/><ApprovalDialog/>{toast && <div className="toast" role="status">{toast}</div>}
  </div>;
}
