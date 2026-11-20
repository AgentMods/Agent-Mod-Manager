import { createElement } from "react";
import { toast } from "sonner";
import { create } from "zustand";
import type { FilePlan, PageName } from "./types";

interface UIState {
  view: PageName;
  gameId: string | null;
  conversationId: string | null;
  adapterChat: { label: string; prompt: string; gameId: string | null } | null;
  theme: "light" | "dark";
  planReview: { plan: FilePlan; onApplied?: () => void } | null;
  navigate: (view: PageName) => void;
  selectGame: (id: string) => void;
  openConversation: (id: string) => void;
  startAdapterChat: (target?: { name: string; slug?: string; gameId?: string }) => void;
  setConversation: (id: string | null, keepAdapterChat?: boolean) => void;
  toggleTheme: () => void;
  notify: (message: string) => void;
  reviewPlan: (plan: FilePlan, onApplied?: () => void) => void;
  closePlan: () => void;
}

function initialTheme(): "light" | "dark" {
  try { const stored = localStorage.getItem("amm.theme"); if (stored === "light" || stored === "dark") return stored; }
  catch { /* Storage may be unavailable in a restricted webview. */ }
  return matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export const useUI = create<UIState>((set) => ({
  view: "agent", gameId: null, conversationId: null, adapterChat: null, theme: initialTheme(), planReview: null,
  navigate: (view) => set({view}),
  selectGame: (gameId) => set({gameId, view: "detail"}),
  openConversation: (conversationId) => set({conversationId, adapterChat: null, view: "agent"}),
  startAdapterChat: (target) => set({
    conversationId: null,
    view: "agent",
    adapterChat: {
      label: target?.name ?? "创建适配包",
      gameId: target?.gameId ?? null,
      prompt: target?.slug
        ? `我想修改「${target.name}」适配包（${target.slug}）。请先读取现有适配包，再和我确认要修改的内容。`
        : target
          ? `请为管理中的游戏「${target.name}」创建适配包，包含游戏封面和游戏 Skill。请先和我确认识别标记与 Mod 安装规则。`
          : "我想创建一个游戏适配包，包含游戏封面和游戏 Skill。请先和我确认游戏名称及适配需求。",
    },
  }),
  setConversation: (conversationId, keepAdapterChat = false) => set((state) => ({
    conversationId, adapterChat: keepAdapterChat ? state.adapterChat : null,
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