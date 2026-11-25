import { useEffect, useMemo, useRef, useState } from "react";
import { useQueries } from "@tanstack/react-query";
import { api } from "./api";
import { tr, useLocale } from "./i18n";

export type BrowseSource = {
  id: string;
  name: string;
  status: "available" | "notConnected" | "permissionDenied" | "unsupported";
};
export type BrowseGrant = { authorizationId: string; sources: BrowseSource[] };
export type BrowseType = "visual" | "localization" | "gameplay" | "audio" | "framework" | "other" | "unknown";
export interface BrowseItem {
  id: string;
  sourceId: string;
  gameIds: string[];
  title: string;
  originalTitle: string;
  author: string | null;
  summary: string | null;
  typeKey: BrowseType;
  tags: { key: string; label: string }[];
  coverUrl: string | null;
  canOpenProject: boolean;
}
export type BrowseIssue = {
  sourceId: string;
  gameId: string;
  code: "noGameMatch" | "permissionDenied" | "queryFailed" | "limitReached";
};
export type BrowseChoice = { sourceId: string; gameId: string; options: { id: string; name: string }[] };
interface BrowsePageData {
  status: "loading" | "ready";
  page: number;
  items: BrowseItem[];
  issues: BrowseIssue[];
  gameChoices: BrowseChoice[];
  hasNext: boolean;
  isEnriching: boolean;
}
export const browseTypes = (): Record<BrowseType, string> => ({
  visual: tr("视觉增强"),
  localization: tr("本地化"),
  gameplay: tr("玩法"),
  audio: tr("音频"),
  framework: tr("框架"),
  other: tr("其他"),
  unknown: tr("未分类"),
});
export const sourceStatus = (status: BrowseSource["status"]) =>
  ({
    available: tr("已连接"),
    notConnected: tr("未连接"),
    unsupported: tr("不支持目录查询"),
    permissionDenied: tr("权限不足"),
  })[status];
export const issueText = (code: BrowseIssue["code"]) =>
  ({
    noGameMatch: tr("未找到对应游戏"),
    permissionDenied: tr("权限不足"),
    queryFailed: tr("目录查询失败"),
    limitReached: tr("已达到本次查询上限"),
  })[code];
export const MAX_BROWSE_GAMES = 8;
const GAMES_KEY = "amm.browse.games";
export function storedBrowseGames(): string[] | null {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(GAMES_KEY) ?? "null");
    return Array.isArray(value) && value.every((v) => typeof v === "string")
      ? value.slice(0, MAX_BROWSE_GAMES)
      : null;
  } catch {
    return null;
  }
}
export function saveBrowseGames(ids: string[]) {
  try {
    localStorage.setItem(GAMES_KEY, JSON.stringify(ids));
  } catch {
    /* In-memory selection still works. */
  }
}

// An opaque search owns server-side cursors. Never resend caller-built cursors, source URLs or
// project IDs. Serialize starts even across unmount/remount so an older IPC cannot replace a newer search.
let starts: Promise<unknown> = Promise.resolve();
export function useBrowseSearch(
  grant: BrowseGrant | null,
  games: string[],
  source: string,
  query: string,
  revision: number,
) {
  const locale = useLocale();
  const key = grant && games.length ? JSON.stringify([grant.authorizationId, games, source, query, revision]) : "";
  const [result, setResult] = useState<{ key: string; id?: string; error?: unknown } | null>(null);
  const [end, setEnd] = useState({ key: "", index: 0 });
  const args = useMemo(
    () =>
      key
        ? {
            authorizationId: grant!.authorizationId,
            gameIds: [...games],
            query,
            sourceIds: source ? [source] : null,
          }
        : null,
    [key],
  );
  useEffect(() => {
    if (!args) return;
    let active = true,
      searchId: string | null = null;
    const dispose = (id: string) => {
      void api("browse_cancel_search", { searchId: id }).catch(() => {});
    };
    const start = async () => {
      if (!active) return;
      try {
        const res = await api<{ searchId: string }>("browse_start_search", args);
        searchId = res.searchId;
        if (active) setResult({ key, id: searchId });
        else dispose(searchId);
      } catch (error) {
        if (active) setResult({ key, error });
      }
    };
    starts = starts.then(start, start);
    return () => {
      active = false;
      if (searchId) dispose(searchId);
    };
  }, [key, args]);
  const id = key && result?.key === key ? (result.id ?? "") : "";
  const lastIndex = end.key === id ? end.index : 0;
  const frames = useQueries({
    queries: Array.from({ length: id ? lastIndex + 1 : 0 }, (_, page) => ({
      queryKey: ["browse-page", id, locale, page],
      queryFn: () => api<BrowsePageData>("browse_get_page", { searchId: id, page }),
      staleTime: Infinity,
      gcTime: 60_000,
      retry: false,
      refetchInterval: (q: { state: { data?: BrowsePageData; error: unknown } }) =>
        q.state.error || (q.state.data?.status === "ready" && !q.state.data.isEnriching) ? (false as const) : 450,
    })),
  });
  const ready = frames.flatMap((frame) => (frame.data?.status === "ready" ? [frame.data] : []));
  const last = frames.at(-1);
  const error = (key && result?.key === key ? result.error : null) || frames.find((frame) => frame.error)?.error;
  const lock = useRef("");
  function loadNext() {
    if (
      !id ||
      last?.data?.status !== "ready" ||
      !last.data.hasNext ||
      error ||
      lock.current === `${id}/${lastIndex}`
    )
      return;
    lock.current = `${id}/${lastIndex}`;
    setEnd({ key: id, index: lastIndex + 1 });
  }
  return {
    id,
    ready,
    lastIndex,
    loadNext,
    error,
    busy: !!key && !error && (!last?.data || last.data.status === "loading"),
    hasNext: !error && last?.data?.status === "ready" && last.data.hasNext,
    enriching: ready.some((p) => p.isEnriching),
  };
}
