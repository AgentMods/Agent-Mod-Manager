import { memo, useMemo, type ReactNode } from 'react';
import { ArrowDownToLineIcon, ChevronDownIcon, Gamepad2Icon, PackageIcon, PackageMinusIcon, Trash2Icon } from 'lucide-react';
import { mcpSiteLogo } from '@/assets/mcp-sites';
import { Badge } from '@/components/ui/badge';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import type { AgentToolActivity } from '@/lib/agent-store';
import { useBackendQuery } from '@/lib/api';
import type { BrowseInstallRequest } from '@/lib/browse-install-request';
import { tr, useLocale } from '@/lib/i18n';
import type { Installed, LibraryMod } from '@/lib/mod-management';
import type { ModRequest } from '@/lib/mod-request';
import { cn } from '@/lib/utils';

const actionStyles = {
  install: { Icon: ArrowDownToLineIcon, color: 'bg-accent text-accent-foreground' },
  uninstall: { Icon: PackageMinusIcon, color: 'bg-warning/10 text-warning' },
  remove: { Icon: Trash2Icon, color: 'bg-destructive/10 text-destructive' },
};

function shortId(id: string) {
  return id.length > 16 ? `…${id.slice(-10)}` : id;
}

interface RequestContext { gameName?: string; messageId: string }

function RequestCard({ action, title, badge, messageId, gameName, gameId, identifiers, children }: RequestContext & {
  action: ModRequest['kind'] | 'browse-install'; title: string; badge: string; gameId: string; identifiers: ReactNode; children: ReactNode;
}) {
  const browse = action === 'browse-install';
  const { Icon, color } = actionStyles[browse ? 'install' : action];
  return <Card role="group" aria-label={title} data-testid="mod-request-card" data-message-id={messageId} className="w-full min-w-0 gap-0 overflow-hidden py-0">
    <CardHeader className="flex flex-row items-center gap-3 px-4 py-3.5">
      <span aria-hidden="true" className={cn('flex size-10 shrink-0 items-center justify-center rounded-lg', color)}><Icon className="size-5" /></span>
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <CardTitle className="text-sm">{title}</CardTitle>
        <CardDescription className="flex min-w-0 items-center gap-1.5 text-xs">
          <Gamepad2Icon aria-hidden="true" className="size-3.5 shrink-0" />
          <span className="wrap-anywhere">{gameName || tr('游戏 {id}', { id: shortId(gameId) })}</span>
        </CardDescription>
      </div>
      <Badge variant="secondary">{badge}</Badge>
    </CardHeader>
    <CardContent className="px-4">
      {children}
      <details className="group/ids">
        <summary className="flex cursor-pointer list-none items-center gap-1.5 rounded-sm py-2.5 text-xs text-muted-foreground hover:text-foreground focus-visible:outline-ring [&::-webkit-details-marker]:hidden">
          {tr('查看标识')}<ChevronDownIcon aria-hidden="true" className="size-3.5 group-open/ids:rotate-180" />
        </summary>
        <dl className={cn("mb-3 grid max-h-56 gap-2.5 overflow-y-auto rounded-md bg-muted px-3 py-2.5 text-xs",
          browse && "grid-cols-2 [&>div]:min-w-0 [&>div:first-child]:col-span-full")}>
          <Identifier label={tr('游戏 ID')} value={gameId} />
          {identifiers}
        </dl>
      </details>
    </CardContent>
  </Card>;
}

function Identifier({ label, value }: { label: string; value: string }) {
  return <div><dt className="text-muted-foreground">{label}</dt><dd className="mt-0.5 font-mono break-all">{value}</dd></div>;
}

function BrowseRequestCard({ request, ...context }: RequestContext & { request: BrowseInstallRequest }) {
  const { sourceName, projectId, sourceId, catalogSourceId, providerGameId } = request;
  const logo = mcpSiteLogo(sourceId);
  return <RequestCard {...context} action="browse-install" title={tr('安装 Mod')} badge={tr('游览')} gameId={request.gameId} identifiers={<>
    <Identifier label={tr('来源')} value={sourceId} />
    <Identifier label={tr('来源游戏 ID')} value={providerGameId} />
    <Identifier label={tr('作品 ID')} value={projectId} />
    {catalogSourceId && <Identifier label={tr('目录来源 ID')} value={catalogSourceId} />}
  </>}>
    <div className="flex items-center gap-3 border-y py-3">
      {logo && <img src={logo} alt="" className="size-8 shrink-0 rounded-md object-contain" />}
      <div className="min-w-0">
        <div className="text-sm font-medium">{sourceName}</div>
        <div className="mt-0.5 wrap-anywhere text-xs text-muted-foreground">{tr('项目 ID：{projectId}', { projectId })}</div>
      </div>
    </div>
  </RequestCard>;
}

function LibraryRequestCard({ request, gameName, tools, messageId }: RequestContext & {
  request: ModRequest; tools: AgentToolActivity[];
}) {
  const { kind, batch, gameId, ids } = request;
  const installing = kind === 'install';
  const installs = useBackendQuery<Installed[]>(['installs', gameId], 'list_installs', { gameId }, !installing);
  const library = useBackendQuery<LibraryMod[]>(['mod-library', gameId], 'list_mod_library', { gameId }, installing);
  const names = useMemo(() => {
    const result = new Map<string, string>();
    if (installing) {
      for (const entry of library.data ?? []) result.set(entry.artifactId, entry.name);
    } else {
      // A recorded removal approval can still identify a Mod after it leaves the list.
      for (const { request: approval } of tools) {
        const preview = approval?.payload.preview;
        if (approval?.payload.title === 'forget_uninstalled_mod' && preview && typeof preview === 'object' &&
          'gameId' in preview && preview.gameId === gameId && 'installId' in preview && typeof preview.installId === 'string' &&
          'name' in preview && typeof preview.name === 'string') result.set(preview.installId, preview.name);
      }
      for (const entry of installs.data ?? []) result.set(entry.id, entry.name);
    }
    return result;
  }, [installing, library.data, installs.data, tools, gameId]);
  const title = batch ? {
    install: tr('批量安装 Mod'), uninstall: tr('批量卸载 Mod'), remove: tr('批量删除 Mod'),
  }[kind] : {
    install: tr('安装 Mod'), uninstall: tr('卸载 Mod'), remove: tr('删除 Mod'),
  }[kind];
  const recordIdLabel = installing ? tr('归档 ID') : tr('安装记录 ID');
  return <RequestCard action={kind} title={title} badge={tr('{length} 项', { length: ids.length })}
    gameId={gameId} gameName={gameName} messageId={messageId}
    identifiers={ids.map((id, index) => <Identifier key={id} label={`${index + 1}. ${recordIdLabel}`} value={id} />)}>
    <ol aria-label={tr('操作目标')} className="max-h-56 overflow-y-auto border-y divide-y">
      {ids.map((id, index) => <li key={id} className="flex items-center gap-2.5 py-2.5">
        <span aria-hidden="true" className="min-w-[2ch] text-xs tabular-nums text-muted-foreground">{index + 1}.</span>
        <PackageIcon aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
        <span className="min-w-0 wrap-anywhere text-sm">{names.get(id) || (installing ? tr('归档 {id}', { id: shortId(id) }) : tr('安装记录 {id}', { id: shortId(id) }))}</span>
      </li>)}
    </ol>
  </RequestCard>;
}

export const ModRequestCard = memo(function ModRequestCard({ request, tools, ...context }: RequestContext & {
  request: ModRequest | BrowseInstallRequest; tools: AgentToolActivity[];
}) {
  useLocale();
  return request.kind === 'browse-install' ? <BrowseRequestCard {...context} request={request} />
    : <LibraryRequestCard {...context} request={request} tools={tools} />;
});
