import { create } from "zustand";
import type { FilePlan, PageName } from "./types";

interface UIState {
  view: PageName;
  gameId: string | null;
  conversationId: string | null;
  theme: "light" | "dark";
  toast: string | null;
  planReview: { plan: FilePlan; onApplied?: () => void } | null;
  navigate: (view: PageName) => void;
  selectGame: (id: string) => void;
  openConversation: (id: string) => void;
  setConversation: (id: string | null) => void;
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

let toastTimer: ReturnType<typeof setTimeout>;
export const useUI = create<UIState>((set) => ({
  view: "library", gameId: null, conversationId: null, theme: initialTheme(), toast: null, planReview: null,
  navigate: (view) => set({view}),
  selectGame: (gameId) => set({gameId, view: "detail"}),
  openConversation: (conversationId) => set({conversationId, view: "agent"}),
  setConversation: (conversationId) => set({conversationId}),
  toggleTheme: () => set((state) => {
    const theme = state.theme === "light" ? "dark" : "light";
    try { localStorage.setItem("amm.theme", theme); } catch { /* Keep in-memory selection. */ }
    return {theme};
  }),
  notify: (toast) => {
    clearTimeout(toastTimer);
    set({toast});
    toastTimer = setTimeout(() => set({toast: null}), 6000);
  },
  reviewPlan: (plan, onApplied) => set({planReview: {plan, onApplied}}),
  closePlan: () => set({planReview: null}),
}));
