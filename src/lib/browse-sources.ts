import { useEffect, useEffectEvent, useState } from "react";
import { api, useBackendQuery } from "./api";
import { tr } from "./i18n";
import type { BrowseGrant, BrowseSource } from "./browse";

// The native window has one active grant. Keep an older IPC from replacing a newer visit's grant.
let authorizations: Promise<void> = Promise.resolve();
const DISCOVERY_TTL_MS = 3 * 60_000;
type SourceRequest = { ids: string[] | null; refresh: boolean };

export function useBrowseSources(initial: BrowseGrant | null, hasGames: boolean, onApplied: (grant: BrowseGrant) => void) {
  const sources = useBackendQuery<BrowseSource[]>(["browse-sources"], "browse_list_sources");
  const [grant, setGrant] = useState(initial);
  const [request, setRequest] = useState<SourceRequest | null>(initial ? null : { ids: null, refresh: false });
  const [error, setError] = useState<unknown>(null);
  const available = sources.data?.some((s) => s.status === "available") ?? false;
  const busy = !!request && !error && hasGames && available;

  const discover = useEffectEvent(async (refresh: boolean) => {
    // The native discovery snapshot expires after three minutes, even if the game picker stays open.
    if (refresh || sources.isError || sources.isFetching || Date.now() - sources.dataUpdatedAt >= DISCOVERY_TTL_MS) {
      return (await sources.refetch({ cancelRefetch: false, throwOnError: true })).data ?? [];
    }
    return sources.data ?? [];
  });
  const applied = useEffectEvent(onApplied);
  useEffect(() => {
    if (!busy || !request) return;
    let active = true;
    const authorize = async () => {
      if (!active) return;
      try {
        const catalog = await discover(request.refresh);
        if (!active) return;
        const availableIds = catalog.filter((s) => s.status === "available").map((s) => s.id);
        const ids = request.ids ?? availableIds;
        if (!ids.length) return; // The refreshed catalog will show the empty state.
        if (ids.some((id) => !availableIds.includes(id))) throw new Error(tr("所选来源已不可用，请重新选择。"));
        const next = await api<BrowseGrant>("browse_authorize", { sourceIds: ids });
        if (active) {
          setGrant(next);
          setRequest(null);
          applied(next);
        }
      } catch (cause) {
        if (active) setError(cause);
      }
    };
    authorizations = authorizations.then(authorize, authorize);
    return () => { active = false; };
  }, [busy, request]);

  function apply(ids: string[] | null) {
    setError(null);
    setRequest({ ids, refresh: true });
  }
  function retry() {
    apply(request ? request.ids : grant?.sources.map((s) => s.id) ?? null);
  }
  return { sources, grant, available, busy, error, apply, retry };
}
