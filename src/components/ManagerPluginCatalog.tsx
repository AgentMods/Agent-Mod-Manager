import { tr } from "@/lib/i18n";
import { GameCover } from "./GameCover";
import { useEffect, useId, useRef, useState } from "react";
import { ArrowLeftIcon, CheckIcon, DownloadIcon, RefreshCwIcon, SearchIcon, XIcon } from "lucide-react";
import { backendAvailable, useBackendQuery } from "@/lib/api";
import { usePluginChange } from "@/lib/plugin-change";
import type { CatalogGame, CatalogPluginRelease as Candidate, CatalogSearch, Plugin } from "@/lib/types";
import { Badge, EmptyState, ErrorState, IconButton } from "@/components/ui";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group";
import { CatalogGameCover, type ReleaseDisplay } from "@/components/CatalogGameCover";
import { Spinner } from "@/components/ui/spinner";

type ReleasePreview = {
  display?: ReleaseDisplay | null;
  gameName: string; plugin: Candidate; source: string; adapterJson: string; skillMarkdown: string;
  mcpServers: { id: string; name: string; transport: string; command: string | null; args: string[]; url: string | null }[];
};
type CatalogProps = { writable: boolean; initialQuery?: string; onReturnToGame?: (slug: string) => void };

const platformNames: Record<string, string> = { windows: "Windows", macos: "macOS", linux: "Linux" };
function candidateByline(candidate: Candidate) {
  return [candidate.authorName, candidate.version, candidate.platforms.map(platform => platformNames[platform] ?? platform).join(" / ")].join(" · ");
}

function CatalogCard({ game, plugin, onSelect, action }: {
  action: React.ReactNode;
  game: CatalogGame; plugin: Candidate; onSelect: (plugin: Candidate, trigger: HTMLButtonElement) => void;
}) {
  const steamId = plugin.steamAppId || game.steamAppId;
  return <Card className="h-full min-w-0 gap-4 p-4">
    <CardHeader className="grid-cols-[minmax(0,38%)_minmax(0,1fr)] grid-rows-[auto] items-center gap-3 p-0">
      <CatalogGameCover releaseId={plugin.releaseId} name={plugin.name} />
      <div className="flex min-w-0 flex-col items-start gap-1.5">
        {game.name !== plugin.name && <p className="wrap-anywhere text-xs text-muted-foreground">{game.name}</p>}
        <CardTitle className="wrap-anywhere text-base leading-snug">{plugin.name}</CardTitle>
        <CardDescription className="wrap-anywhere">{candidateByline(plugin)}</CardDescription>
        {steamId && <p className="wrap-anywhere text-xs text-muted-foreground">Steam App ID {steamId}</p>}
      </div>
    </CardHeader>
    <CardContent className="flex min-w-0 flex-col gap-3 border-t px-0 pt-3">
      {plugin.description && <p className="line-clamp-2 wrap-anywhere text-sm leading-relaxed">{plugin.description}</p>}
      {!!plugin.tags?.length && <div className="flex flex-wrap gap-1.5">{plugin.tags.map(tag => <Badge key={tag} className="max-w-full wrap-anywhere">{tag}</Badge>)}</div>}
    </CardContent>
    <CardFooter className="mt-auto flex-wrap justify-between gap-2 p-0">
      <Button size="sm" variant="ghost" aria-label={tr("查看详情 {name} · {authorName}", { name: plugin.name, authorName: plugin.authorName })} onClick={event => onSelect(plugin, event.currentTarget)}>{tr("查看详情")}</Button>
      {action}
    </CardFooter>
  </Card>;
}

function RawFile({ name, text }: { name: string; text: string }) {
  return <details className="min-w-0 rounded-md border">
    <summary className="cursor-pointer rounded-md px-3 py-2 text-sm font-medium focus-visible:outline-ring">{name}</summary>
    <pre className="max-h-64 overflow-auto whitespace-pre-wrap wrap-anywhere border-t bg-muted/30 p-3 font-mono text-xs" tabIndex={0} aria-label={tr("{name} 原文", { name })}>{text}</pre>
  </details>;
}

function ReleaseDetails({ candidate, onBack, action }: { candidate: Candidate; onBack: () => void; action: React.ReactNode }) {
  const heading = useRef<HTMLHeadingElement>(null);
  const release = { pluginId: candidate.id, releaseId: candidate.releaseId };
  const query = useBackendQuery<ReleasePreview>(["manager-plugin-preview", candidate.id, candidate.releaseId], "preview_manager_plugin", release);
  const preview = query.isError ? undefined : query.data;
  useEffect(() => { heading.current?.focus(); }, []);
  return <Card className="min-w-0 max-w-xl gap-4 shadow-none">
    <CardHeader className="gap-3">
      <Button variant="ghost" size="sm" className="w-fit" onClick={onBack}><ArrowLeftIcon data-icon="inline-start" />{tr("返回目录")}</Button>
      <CardTitle ref={heading} tabIndex={-1} className="wrap-anywhere">{preview?.plugin.name ?? candidate.name}</CardTitle>
      <p className="wrap-anywhere text-sm text-muted-foreground">{candidateByline(preview?.plugin ?? candidate)}</p>
    </CardHeader>
    <CardContent className="flex min-w-0 flex-col gap-4">
      {query.isLoading && <p role="status" className="flex items-center gap-2 text-sm"><Spinner />{tr("正在读取版本文件…")}</p>}
      {query.isError && <><ErrorState error={query.error} /><Button variant="outline" className="self-start" disabled={query.isFetching} onClick={() => void query.refetch()}>{tr("重试预览")}</Button></>}
      {preview && <>
        <dl className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-sm [&_dt]:text-muted-foreground [&_dd]:wrap-anywhere">
          <dt>{tr("游戏")}</dt><dd>{preview.gameName}</dd>
          <dt>{tr("来源")}</dt><dd>{preview.source}</dd>
          <dt>{tr("适配包")}</dt><dd>{preview.plugin.slug}</dd>
          <dt>{tr("发布 ID")}</dt><dd>{preview.plugin.releaseId}</dd>
        </dl>
        <GameCover name={preview.plugin.name} url={preview.display?.coverUrl} className="aspect-video w-full max-w-sm rounded-md" />
        {preview.display?.description && <p className="wrap-anywhere whitespace-pre-line text-sm">{preview.display.description}</p>}
        {preview.plugin.description && <p className="wrap-anywhere text-sm">{preview.plugin.description}</p>}
        <section aria-label={tr("声明的 MCP 服务")} className="flex flex-col gap-1.5 text-sm">
          <h3 className="font-medium">{tr("声明的 MCP 服务")}</h3>
          {preview.mcpServers.length ? <ul className="flex flex-col gap-2">{preview.mcpServers.map(server => <li key={server.id} className="min-w-0 rounded-md border px-3 py-2">
            <span className="wrap-anywhere">{server.name} · {server.transport}</span>
            <code className="block whitespace-pre-wrap wrap-anywhere text-xs text-muted-foreground">{server.command ? [server.command, ...server.args.map(arg => JSON.stringify(arg))].join(" ") : server.url}</code>
          </li>)}</ul> : <p className="text-muted-foreground">{tr("此版本未声明 MCP 服务。")}</p>}
        </section>
        <div className="flex min-w-0 flex-col gap-2"><RawFile name="adapter.json" text={preview.adapterJson} /><RawFile name="SKILL.md" text={preview.skillMarkdown} /></div>
        <div className="flex flex-wrap items-center gap-2">{action}</div>
      </>}
    </CardContent>
  </Card>;
}

export function ManagerPluginCatalog({ writable, initialQuery = "", onReturnToGame }: CatalogProps) {
  const [search, setSearch] = useState(initialQuery);
  const [selected, setSelected] = useState<Candidate | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const trigger = useRef<HTMLButtonElement | null>(null);
  const searchId = useId();
  const inGameDialog = !!onReturnToGame;
  useEffect(() => { if (inGameDialog) input.current?.focus(); }, [inGameDialog]);
  const catalog = useBackendQuery<CatalogSearch>(["manager-plugin-games"], "list_manager_plugin_games", {}, !selected);
  const installed = useBackendQuery<Plugin[]>(["plugins"], "list_plugins");
  const bySlug = new Map(installed.data?.map(plugin => [plugin.slug, plugin]));
  const change = usePluginChange();
  function installAction(plugin: Candidate) {
    const local = bySlug.get(plugin.slug);
    const installing = change.pending?.action === "install" && change.pending.releaseId === plugin.releaseId;
    return <>
      {change.failed?.action === "install" && change.failed.releaseId === plugin.releaseId && <ErrorState className="basis-full" error={change.error} />}
      {local ? <>
        {local.version !== plugin.version && <p className="basis-full text-xs text-muted-foreground">{tr("本地版本 {version}，云端版本 {value2}。", { version: local.version, value2: plugin.version })}</p>}
        <Badge tone="success"><CheckIcon className="size-3" />{tr("已安装")}</Badge>
        {onReturnToGame && <Button size="sm" variant="outline" onClick={() => onReturnToGame(local.slug)}>{tr("返回并选择此适配包")}</Button>}
      </> : <Button size="sm" aria-label={tr("安装 {name} · {authorName} · {version}", { name: plugin.name, authorName: plugin.authorName, version: plugin.version })} disabled={!writable || !installed.isSuccess || !!change.pending}
        onClick={() => change.run({ action: "install", slug: plugin.slug, name: plugin.name, pluginId: plugin.id, releaseId: plugin.releaseId })}>
        {installing ? <Spinner data-icon="inline-start" /> : <DownloadIcon data-icon="inline-start" />}{installing ? tr("安装中…") : tr("安装")}
      </Button>}
    </>;
  }
  const needle = search.trim().toLowerCase();
  const games = (catalog.data?.games ?? []).filter(game => !needle || game.steamAppId === needle ||
    [game.name, game.slug, ...game.aliases].some(value => value.toLowerCase().includes(needle)) ||
    game.plugins.some(plugin => plugin.steamAppId === needle || [plugin.name, plugin.slug].some(value => value.toLowerCase().includes(needle))));
  function selectPlugin(plugin: Candidate, button: HTMLButtonElement) {
    trigger.current = button;
    setSelected(plugin);
  }
  return <div className="@container/catalog w-full min-w-0">
    <div hidden={!!selected}>
      <section aria-label={tr("云端插件目录")} className="flex flex-col gap-4">
        <header className="flex flex-col gap-3 @xl/catalog:flex-row @xl/catalog:items-center @xl/catalog:justify-between">
          <h2 className="text-lg font-semibold">{tr("云端插件")}</h2>
          <div className="flex w-full min-w-0 items-center gap-2 @xl/catalog:max-w-sm">
            <label htmlFor={searchId} className="sr-only">{tr("搜索云端插件")}</label>
            <InputGroup><InputGroupAddon><SearchIcon /></InputGroupAddon><InputGroupInput id={searchId} ref={input} value={search} maxLength={128} disabled={!backendAvailable} placeholder={tr("游戏名、别名、slug、Steam App ID")} onChange={event => setSearch(event.target.value)} />
              {search && <InputGroupAddon align="inline-end"><InputGroupButton aria-label={tr("清空搜索")} size="icon-xs" onClick={() => { setSearch(""); input.current?.focus(); }}><XIcon /></InputGroupButton></InputGroupAddon>}
            </InputGroup>
            <IconButton title={tr("刷新云端目录")} size="icon" variant="outline" disabled={!backendAvailable || catalog.isFetching} onClick={() => void catalog.refetch()}><RefreshCwIcon /></IconButton>
          </div>
        </header>
        {installed.isError && <><ErrorState error={installed.error} /><Button className="self-start" size="sm" variant="outline" onClick={() => void installed.refetch()}>{tr("重试读取已安装插件")}</Button></>}
        {!!catalog.data?.skippedEntries && !catalog.isError && <p role="status" className="text-sm text-warning">{tr("目录中有 {skippedEntries} 个无效条目，已跳过。", { skippedEntries: catalog.data.skippedEntries })}</p>}
        {!backendAvailable ? <EmptyState title={tr("桌面能力未连接")}>{tr("请在桌面应用中查看云端目录。")}</EmptyState>
          : catalog.isLoading ? <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground"><Spinner />{tr("正在读取云端目录…")}</p>
          : catalog.isError ? <ErrorState error={catalog.error} />
          : !games.length ? <EmptyState title={catalog.data?.totalGames ? tr("没有匹配结果") : tr("暂无已发布插件")}>{catalog.data?.totalGames ? tr("可更换游戏名称、别名、标识或 Steam App ID 再试。") : undefined}</EmptyState>
          : <ul className="grid grid-cols-[repeat(auto-fill,minmax(min(100%,26rem),1fr))] gap-4">
            {games.flatMap(game => game.plugins.map(plugin => <li key={plugin.releaseId} className="min-w-0">
              <CatalogCard game={game} plugin={plugin} onSelect={selectPlugin} action={installAction(plugin)} />
            </li>))}
          </ul>}
      </section>
    </div>
    {selected && <ReleaseDetails key={selected.releaseId} candidate={selected} action={installAction(selected)} onBack={() => {
      setSelected(null);
      requestAnimationFrame(() => (trigger.current?.isConnected ? trigger.current : input.current)?.focus());
    }} />}
  </div>;
}
