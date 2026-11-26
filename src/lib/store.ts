import { tr } from "@/lib/i18n";
import type { DraftAttachment } from "./agent-attachments";
import { createElement, type SetStateAction } from "react";
import { toast } from "sonner";
import { create } from "zustand";
import type { FilePlan, GameWorkspace, PageName, PermissionMode } from "./types";

type Draft = { text: string; attachments: DraftAttachment[] };
type EditDraft = (Draft & { conversationId: string; messageId: string }) | null;
export const emptyDraft = (text = ""): Draft => ({text, attachments: []});

interface UIState {
  composer: Draft;
  editDraft: EditDraft;
  setComposer: (update: SetStateAction<Draft>) => void;
  setEditDraft: (update: SetStateAction<EditDraft>) => void;
  view: PageName;
  composerPermission: PermissionMode | "" | undefined;
  setComposerPermission: (value: PermissionMode | "") => void;
  gameId: string | null;
  conversationId: string | null;
  draftWorkspace: GameWorkspace;
  setDraftWorkspace: (workspace: GameWorkspace) => void;
  adapterChat: { label: string; prompt: string; gameId: string | null } | null;
  theme: "light" | "dark";
  planReview: { plan: FilePlan; onApplied?: () => void } | null;
  navigate: (view: PageName) => void;
  selectGame: (id: string) => void;
  openConversation: (id: string, fromGameId?: string) => void;
  startAdapterChat: (target?: { name: string; slug?: string; gameId?: string }) => void;
  setConversation: (id: string | null, keepAdapterChat?: boolean) => void;
  toggleTheme: () => void;
  notify: (message: string) => void;
  reviewPlan: (plan: FilePlan, onApplied?: () => void) => void;
  closePlan: () => void;
}

// The WebView keeps this one-shot return point while the desktop restarts to migrate data.
const onboardingMigrationKey = "amm.onboarding.migrationOrigin";

export function pendingOnboardingMigration(): string | null {
  try { return localStorage.getItem(onboardingMigrationKey); }
  catch { return null; }
}

export function rememberOnboardingMigration(root: string | null) {
  try {
    if (root) localStorage.setItem(onboardingMigrationKey, root);
    else localStorage.removeItem(onboardingMigrationKey);
  } catch { /* First-run detection can still reopen the guide without WebView storage. */ }
}

function initialTheme(): "light" | "dark" {
  try { const stored = localStorage.getItem("amm.theme"); if (stored === "light" || stored === "dark") return stored; }
  catch { /* Storage may be unavailable in a restricted webview. */ }
  return matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

const adapterResearchPrompt = () => tr("请先使用已有的 Mod 平台 MCP，查找并获取该游戏不同安装类型的代表性 Mod。结合发布页的安装说明、依赖声明和压缩包目录结构，分析并核实各类型的安装规则。请自行核实游戏识别标记，只在信息无法核实或需要我决定偏好时向我提问。");

export const useUI = create<UIState>((set) => ({
  view: "home", gameId: null, conversationId: null, adapterChat: null, theme: initialTheme(), planReview: null,
  composer: emptyDraft(), editDraft: null,
  setComposer: (update) => set(state => ({composer: typeof update === "function" ? update(state.composer) : update})),
  setEditDraft: (update) => set(state => ({editDraft: typeof update === "function" ? update(state.editDraft) : update})),
  draftWorkspace: null,
  setDraftWorkspace: (draftWorkspace) => set({draftWorkspace}),
  composerPermission: undefined,
  setComposerPermission: (composerPermission) => set({composerPermission}),
  navigate: (view) => set({view}),
  selectGame: (gameId) => set({gameId, view: "detail"}),
  openConversation: (conversationId, fromGameId) => set(state => {
    // A native launch can finish after the user has navigated elsewhere.
    if (fromGameId !== undefined && (state.view !== "detail" || state.gameId !== fromGameId)) return state;
    return {conversationId, adapterChat: null, view: "agent", ...(state.conversationId !== conversationId && { composer: emptyDraft(), editDraft: null })};
  }),
  startAdapterChat: (target) => {
    const prompt = target?.slug
        ? tr("我想修改「{name}」适配包（{slug}）。请先读取现有适配包，再和我确认要修改的内容。", { name: target.name, slug: target.slug })
        : target
          ? tr("请为管理中的游戏「{name}」创建适配包，包含游戏封面和游戏 Skill。{adapterResearchPrompt}校验通过后安装并绑定到这款游戏。", { name: target.name, adapterResearchPrompt: adapterResearchPrompt() })
          : tr("我想创建一个游戏适配包，包含游戏封面和游戏 Skill。若目标游戏不明确，请先向我确认。{adapterResearchPrompt}", { adapterResearchPrompt: adapterResearchPrompt() });
    set({
      conversationId: null, draftWorkspace: target?.gameId ? [target.gameId] : null, view: "agent",
      adapterChat: { label: target?.name ?? tr("创建适配包"), gameId: target?.gameId ?? null, prompt },
      composer: emptyDraft(prompt), editDraft: null,
    });
  },
  setConversation: (conversationId, keepAdapterChat = false) => set((state) => ({
    conversationId, ...(conversationId === null && {draftWorkspace: null}), adapterChat: keepAdapterChat ? state.adapterChat : null,
  })),
  toggleTheme: () => set((state) => {
    const theme = state.theme === "light" ? "dark" : "light";
    try { localStorage.setItem("amm.theme", theme); } catch { /* Keep in-memory selection. */ }
    return {theme};
  }),
  // One shared id: a new notice replaces the previous one instead of stacking.
  notify: (message) => { toast(createElement("span", {role: "status"}, message), {id: "app-notice", duration: 6000}); },
  reviewPlan: (plan, onApplied) => set({planReview: {plan, onApplied}}),
  closePlan: () => set({planReview: null}),
}));