import { useMutation, useIsMutating, useQuery, useQueryClient } from '@tanstack/react-query';
import { api, backendAvailable } from './api';
import { pluginChangeKey } from './plugin-change';
import type { CatalogSearch, Game, GameCandidate, Plugin } from './types';

// Ignore case, spaces and punctuation; never use substring or fuzzy name matching.
const normalizeName = (s: string) => s.normalize('NFKC').toLowerCase().replace(/[^\p{L}\p{N}]/gu, '');
const staleTime = 5 * 60 * 1000;

function eligible(candidate: GameCandidate) {
  return !candidate.registeredGameId && !candidate.suggestedAdapterSlug
    && !candidate.sources?.some(source => source.completeInstall === false)
    && !(candidate.status === 'candidate' && candidate.confidence >= 0.8);
}

function matchRelease(candidate: GameCandidate, catalog: CatalogSearch, installed: Plugin[]) {
  if (!eligible(candidate)) return;
  const ids = new Set(candidate.sources?.filter(s => s.kind === 'steam').map(s => s.steamAppId).filter((id): id is string => !!id));
  if (ids.size > 1) return;
  const [steamId] = ids;
  const basename = candidate.rootPath.replace(/[\\/]+$/, '').split(/[\\/]/).pop() ?? '';
  const names = new Set([candidate.name, basename].map(normalizeName).filter(Boolean));
  const hits = catalog.games.filter(game => steamId
    ? game.steamAppId === steamId || (!game.steamAppId && game.plugins.some(p => p.steamAppId === steamId))
    : [game.name, game.slug, ...game.aliases].some(name => names.has(normalizeName(name))));
  if (hits.length !== 1) return;
  const [game] = hits;
  // Without an installed adapter, the payload layout is normally unknown. Don't
  // substitute the host OS (e.g. Proton); the desktop verifies the actual files.
  const releases = game.plugins.filter(p => (candidate.platform === 'unknown' || p.platforms.includes(candidate.platform))
    && (!steamId || (p.steamAppId || game.steamAppId) === steamId));
  if (releases.length !== 1) return;
  const [release] = releases;
  const local = installed.find(p => p.slug === release.slug);
  if (local && (!local.enabled || !local.validated || local.version !== release.version)) return;
  if (installed.some(p => p.slug !== release.slug && normalizeName(p.gameName) === normalizeName(game.name))) return;
  // This is only a suggestion: the backend verifies both hashes before reuse.
  return { ...release, reusable: !!local };
}

export function useCloudGameMatch(enabled: boolean) {
  const client = useQueryClient();
  const catalog = useQuery<CatalogSearch>({
    queryKey: ['manager-plugin-games'], queryFn: () => api('list_manager_plugin_games', {}), enabled: backendAvailable && enabled,
    networkMode: 'always', retry: false, staleTime, refetchOnWindowFocus: false, refetchOnReconnect: false,
  });
  const plugins = useQuery<Plugin[]>({
    queryKey: ['plugins'], queryFn: () => api('list_plugins'), enabled: backendAvailable && enabled,
    networkMode: 'always', retry: false, staleTime,
  });
  const pending = useIsMutating({ mutationKey: pluginChangeKey }) > 0;
  const waiting = (c: GameCandidate) => enabled && eligible(c) && (catalog.isLoading || plugins.isLoading);
  const match = (c: GameCandidate) => catalog.data && plugins.data && matchRelease(c, catalog.data, plugins.data);
  const mutation = useMutation({
    mutationKey: pluginChangeKey, networkMode: 'always',
    mutationFn: async ({ candidate, localOnly }: { action: 'register-game'; candidate: GameCandidate; localOnly: boolean }) => {
      if (!localOnly && waiting(candidate)) throw new Error('云目录正在加载，请稍后重试，或选择仅添加本地。');
      const release = localOnly ? undefined : match(candidate);
      const game = await api<Game>('register_game', {
        directory: candidate.rootPath, adapterSlug: candidate.suggestedAdapterSlug, fromDiscovery: true,
        ...(release && { cloudAdapter: { pluginId: release.id, releaseId: release.releaseId }, discoveryName: candidate.name }),
      });
      return { game, release };
    },
    onSuccess: ({ game, release }) => {
      client.setQueryData(['game', game.id], game);
      client.setQueryData<Game[]>(['games'], current => [...(current ?? []).filter(g => g.id !== game.id), game]);
      void client.invalidateQueries({ queryKey: ['games'] });
      if (release) {
        for (const key of ['plugins', 'skills', 'mcp']) void client.invalidateQueries({ queryKey: [key] });
      }
    },
  });
  async function register(candidate: GameCandidate, localOnly = false) {
    if (client.isMutating({ mutationKey: pluginChangeKey })) throw new Error('适配包操作正在进行，请稍后重试。');
    return (await mutation.mutateAsync({ action: 'register-game', candidate, localOnly })).game;
  }
  return { match, register, waiting, pending };
}
