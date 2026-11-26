import { tr } from "@/lib/i18n";
import { GameCover } from "./GameCover";
import { useEffect, useId, useRef, useState } from "react";
import { ArrowLeftIcon, CheckIcon, DownloadIcon, LayoutGridIcon, ListIcon, PackageIcon, RefreshCwIcon, SearchIcon, XIcon } from "lucide-react";
import { backendAvailable, useBackendQuery } from "@/lib/api";
import { usePluginChange } from "@/lib/plugin-change";
import { cn } from "@/lib/utils";
import type { CatalogGame, CatalogPluginRelease as Candidate, CatalogSearch, Game, Plugin } from "@/lib/types";
import { Badge, EmptyState, ErrorState, IconButton } from "@/components/ui";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group";
import { CatalogGameCover, type ReleaseDisplay } from "@/components/CatalogGameCover";
import { Spinner } from "@/components/ui/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

type ReleasePreview = {
  display?: ReleaseDisplay | null;
  gameName: string; plugin: Candidate; source: string; adapterJson: string; skillMarkdown: string;
  mcpServers: { id: string; name: string; transport: string; command: string | null; args: string[]; url: string | null }[];
};
type CatalogProps = { writable: boolean; initialQuery?: string; onReturnToGame?: (slug: string) => void };
type View = "grid" | "list";
type StatusFilter = "all" | "uninstalled" | "installed" | "library";
type LibraryMatch = { game: Game; bound: boolean };
type Entry = {
  game: CatalogGame; plugin: Candidate; steamId: string; gameName: string; aliases: string[]; tags: string[];
  local: Plugin | undefined; library: LibraryMatch | null;
};

const platformNames: Record<string, string> = { windows: "Windows", macos: "macOS", linux: "Linux" };
const platformOrder = ["windows", "macos", "linux"];
const VIEW_KEY = "amm.catalogView";
const TAG_LIMIT = { grid: 5, list: 4 } as const;

function storedView(): View {
  try { return localStorage.getItem(VIEW_KEY) === "list" ? "list" : "grid"; } catch { return "grid"; }
}
function candidateByline(candidate: Candidate) {
  return [candidate.authorName, candidate.version, candidate.platforms.map(platform => platformNames[platform] ?? platform).join(" / ")].join(" · ");
}
// Case-insensitive unique values; anything in `skip` (already shown elsewhere on the card) is dropped.
function distinct(values: readonly string[], skip: readonly string[]) {
  const seen = new Set(skip.map(value => value.trim().toLowerCase()));
  return values.map(value => value.trim()).filter(value => {
    const key = value.toLowerCase();
    if (!key || seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}
function matchLibraryGame(steamId: string, plugin: Candidate, games: Game[]): LibraryMatch | null {
  const bound = games.find(game => game.adapterSlug === plugin.slug);
  if (bound) return { game: bound, bound: true };
  const sameSteam = steamId ? games.find(game => game.steamAppId === steamId) : undefined;
  return sameSteam ? { game: sameSteam, bound: false } : null;
}

function TagList({ tags, limit }: { tags: string[]; limit: number }) {
  if (!tags.length) return null;
  const rest = tags.slice(limit);
  return <div className="flex flex-wrap items-center gap-1">
    {tags.slice(0, limit).map(tag => <Badge key={tag} className="min-h-0 max-w-full px-1.5 py-0 text-[11px] leading-[1.15rem] font-normal wrap-anywhere">{tag}</Badge>)}
    {!!rest.length && <Tooltip>
      <TooltipTrigger asChild><span tabIndex={0} className="inline-flex items-center rounded-full border px-1.5 text-[11px] leading-[1.15rem] text-muted-foreground outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50">+{rest.length}</span></TooltipTrigger>
      <TooltipContent className="max-w-64 wrap-anywhere">{rest.join(" · ")}</TooltipContent>
    </Tooltip>}
  </div>;
}

function LibraryBadge({ match }: { match: LibraryMatch | null }) {
  return match ? <span className="shrink-0" title={match.game.name}><Badge tone={match.bound ? "success" : "neutral"} className="min-h-0 px-1.5 py-0 text-[10px] leading-4">{tr("游戏库中")}</Badge></span> : null;
}

function Meta({ entry, className }: { entry: Entry; className?: string }) {
  const { plugin, steamId, aliases } = entry;
  return <div className={cn("flex min-w-0 flex-wrap items-center gap-x-2.5 gap-y-0.5 text-[11px] leading-tight text-muted-foreground", className)}>
    <span className="inline-flex max-w-full min-w-0 items-center gap-1 font-mono" title={plugin.slug}>
      <PackageIcon className="size-3 shrink-0 opacity-70" aria-hidden="true" /><span className="truncate">{plugin.slug}</span>
    </span>
    {steamId && <span className="tabular-nums wrap-anywhere">Steam App ID {steamId}</span>}
    {!!aliases.length && <span className="max-w-full min-w-0 truncate" title={aliases.join(" / ")}>{tr("别名")}：{aliases.slice(0, 2).join(" / ")}</span>}
  </div>;
}

function CatalogCard({ entry, onSelect, action }: {
  action: React.ReactNode; entry: Entry; onSelect: (plugin: Candidate, trigger: HTMLButtonElement) => void;
}) {
  const { plugin, gameName, tags, library } = entry;
  const detail = <Button size="xs" variant="ghost" className="-ms-1.5 h-7 px-2 text-muted-foreground hover:text-foreground" aria-label={tr("查看详情 {name} · {authorName}", { name: plugin.name, authorName: plugin.authorName })} onClick={event => onSelect(plugin, event.currentTarget)}>{tr("查看详情")}</Button>;
  return <Card className="h-full min-w-0 gap-2 p-3 transition-colors hover:border-primary/40">
    <CardHeader className="grid-cols-[6.25rem_minmax(0,1fr)] grid-rows-[auto] items-start gap-2.5 p-0">
      <CatalogGameCover releaseId={plugin.releaseId} name={plugin.name} className="rounded-md" />
      <div className="flex min-w-0 flex-col gap-0.5">
        {(gameName !== plugin.name || library) && <div className="flex min-w-0 items-center gap-1.5">
          {gameName !== plugin.name && <p className="min-w-0 truncate text-[11px] leading-tight text-muted-foreground" title={gameName}>{gameName}</p>}
          <span className="ms-auto"><LibraryBadge match={library} /></span>
        </div>}
        <CardTitle className="line-clamp-2 text-sm leading-snug wrap-anywhere" title={plugin.name}>{plugin.name}</CardTitle>
        <CardDescription className="text-xs leading-snug wrap-anywhere">{candidateByline(plugin)}</CardDescription>
      </div>
    </CardHeader>
    <CardContent className="flex min-w-0 flex-1 flex-col gap-1.5 border-t px-0 pt-2">
      <Meta entry={entry} />
      {plugin.description && <p className="line-clamp-2 text-xs leading-relaxed wrap-anywhere text-foreground/85" title={plugin.description}>{plugin.description}</p>}
      <TagList tags={tags} limit={TAG_LIMIT.grid} />
    </CardContent>
    <CardFooter className="mt-auto flex-wrap items-center justify-between gap-1.5 border-t px-0 pt-2">
      {detail}
      <div className="flex flex-wrap items-center justify-end gap-1.5">{action}</div>
    </CardFooter>
  </Card>;
}

function CatalogRow({ entry, onSelect, action }: {
  action: React.ReactNode; entry: Entry; onSelect: (plugin: Candidate, trigger: HTMLButtonElement) => void;
}) {
  const { plugin, gameName, tags, library } = entry;
  return <Card className="min-w-0 gap-0 p-2.5 transition-colors hover:border-primary/40">
    <div className="grid grid-cols-[5.5rem_minmax(0,1fr)] items-start gap-2.5 @2xl/catalog:grid-cols-[6rem_minmax(0,1fr)_auto] @2xl/catalog:items-center">
      <CatalogGameCover releaseId={plugin.releaseId} name={plugin.name} className="rounded-md" />
      <div className="flex min-w-0 flex-col gap-1">
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5">
          <CardTitle className="text-sm leading-snug wrap-anywhere" title={plugin.name}>{plugin.name}</CardTitle>
          {gameName !== plugin.name && <p className="min-w-0 truncate text-[11px] leading-tight text-muted-foreground" title={gameName}>{gameName}</p>}
          <LibraryBadge match={library} />
          <CardDescription className="text-xs wrap-anywhere">{candidateByline(plugin)}</CardDescription>
        </div>
        {plugin.description && <p className="line-clamp-1 text-xs wrap-anywhere text-foreground/85" title={plugin.description}>{plugin.description}</p>}
        <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
          <Meta entry={entry} className="gap-y-1" />
          <TagList tags={tags} limit={TAG_LIMIT.list} />
        </div>
      </div>
      <div className="col-span-full flex flex-wrap items-center justify-between gap-1.5 border-t pt-2 @2xl/catalog:col-span-1 @2xl/catalog:justify-end @2xl/catalog:border-t-0 @2xl/catalog:pt-0">
        <Button size="xs" variant="ghost" className="-ms-1.5 h-7 px-2 text-muted-foreground hover:text-foreground @2xl/catalog:ms-0" aria-label={tr("查看详情 {name} · {authorName}", { name: plugin.name, authorName: plugin.authorName })} onClick={event => onSelect(plugin, event.currentTarget)}>{tr("查看详情")}</Button>
        <div className="flex flex-wrap items-center justify-end gap-1.5">{action}</div>
      </div>
    </div>
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
  const [view, setView] = useState<View>(storedView);
  const [status, setStatus] = useState<StatusFilter>("all");
  const [platform, setPlatform] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const trigger = useRef<HTMLButtonElement | null>(null);
  const searchId = useId();
  const inGameDialog = !!onReturnToGame;
  useEffect(() => { if (inGameDialog) input.current?.focus(); }, [inGameDialog]);
  const catalog = useBackendQuery<CatalogSearch>(["manager-plugin-games"], "list_manager_plugin_games", {}, !selected);
  const installed = useBackendQuery<Plugin[]>(["plugins"], "list_plugins");
  // Library matching is a best-effort hint; a failed games query must never block the catalog.
  const libraryGames = useBackendQuery<Game[]>(["games"], "list_games");
  const bySlug = new Map(installed.data?.map(plugin => [plugin.slug, plugin]));
  const change = usePluginChange();
  function chooseView(next: View) {
    setView(next);
    try { localStorage.setItem(VIEW_KEY, next); } catch { /* Keep the in-memory choice. */ }
  }
  function installAction(plugin: Candidate, compact = false) {
    const local = bySlug.get(plugin.slug);
    const installing = change.pending?.action === "install" && change.pending.releaseId === plugin.releaseId;
    const chip = compact ? "min-h-0 px-1.5 py-0.5 text-[11px]" : undefined;
    const buttonSize = compact ? "xs" : "sm";
    const buttonClass = compact ? "h-7 px-2.5" : undefined;
    return <>
      {change.failed?.action === "install" && change.failed.releaseId === plugin.releaseId && <ErrorState className="basis-full" error={change.error} />}
      {local ? <>
        {local.version !== plugin.version && <p className={cn("basis-full text-xs text-muted-foreground", compact && "text-[11px] leading-snug text-warning")}>{tr("本地版本 {version}，云端版本 {value2}。", { version: local.version, value2: plugin.version })}</p>}
        {compact && !local.enabled && <Badge tone="neutral" className={chip}>{tr("已停用")}</Badge>}
        <Badge tone="success" className={chip}><CheckIcon className="size-3" />{tr("已安装")}</Badge>
        {onReturnToGame && <Button size={buttonSize} className={buttonClass} variant="outline" onClick={() => onReturnToGame(local.slug)}>{tr("返回并选择此适配包")}</Button>}
      </> : <Button size={buttonSize} className={buttonClass} aria-label={tr("安装 {name} · {authorName} · {version}", { name: plugin.name, authorName: plugin.authorName, version: plugin.version })} disabled={!writable || !installed.isSuccess || !!change.pending}
        onClick={() => change.run({ action: "install", slug: plugin.slug, name: plugin.name, pluginId: plugin.id, releaseId: plugin.releaseId })}>
        {installing ? <Spinner data-icon="inline-start" /> : <DownloadIcon data-icon="inline-start" />}{installing ? tr("安装中…") : tr("安装")}
      </Button>}
    </>;
  }
  const needle = search.trim().toLowerCase();
  const localGames = libraryGames.data ?? [];
  const entries: Entry[] = (catalog.data?.games ?? []).flatMap(game => game.plugins.map(plugin => {
    const steamId = plugin.steamAppId || game.steamAppId;
    const gameName = plugin.gameName || game.name;
    return {
      game, plugin, steamId, gameName,
      aliases: distinct(game.aliases, [gameName, plugin.name]),
      tags: distinct(plugin.tags ?? [], [gameName, game.name, plugin.name, plugin.slug, ...plugin.platforms, ...plugin.platforms.map(key => platformNames[key] ?? key)]),
      local: bySlug.get(plugin.slug), library: matchLibraryGame(steamId, plugin, localGames),
    };
  }));
  const searchable = (entry: Entry) => !needle || entry.game.steamAppId === needle || entry.plugin.steamAppId === needle ||
    [entry.game.name, entry.game.slug, ...entry.game.aliases, entry.plugin.name, entry.plugin.slug, entry.plugin.gameName ?? "", entry.plugin.authorName, entry.plugin.description, ...(entry.plugin.tags ?? [])]
      .some(value => value.toLowerCase().includes(needle));
  const shown = entries.filter(entry => searchable(entry)
    && (status === "all" || (status === "installed" ? !!entry.local : status === "uninstalled" ? !entry.local : !!entry.library))
    && (!platform || entry.plugin.platforms.includes(platform)));
  const counts = {
    all: entries.length, installed: entries.filter(entry => entry.local).length,
    uninstalled: entries.filter(entry => !entry.local).length, library: entries.filter(entry => entry.library).length,
  };
  const platforms = [...new Set(entries.flatMap(entry => entry.plugin.platforms))]
    .sort((a, b) => (platformOrder.indexOf(a) + 1 || 99) - (platformOrder.indexOf(b) + 1 || 99));
  const filtered = status !== "all" || platform !== null;
  const hasEntries = catalog.isSuccess && entries.length > 0;
  function resetFilters() { setStatus("all"); setPlatform(null); }
  function selectPlugin(plugin: Candidate, button: HTMLButtonElement) {
    trigger.current = button;
    setSelected(plugin);
  }
  const statusOptions: { id: StatusFilter; label: string; count: number }[] = [
    { id: "all", label: tr("全部"), count: counts.all },
    { id: "uninstalled", label: tr("未安装"), count: counts.uninstalled },
    ...(counts.installed ? [{ id: "installed" as const, label: tr("已安装"), count: counts.installed }] : []),
    ...(counts.library ? [{ id: "library" as const, label: tr("游戏库匹配"), count: counts.library }] : []),
  ];
  const chipClass = "h-6 gap-1 px-2 text-xs";
  return <div className="@container/catalog w-full min-w-0">
    <div hidden={!!selected}>
      <section aria-label={tr("云端插件目录")} className="flex flex-col gap-3">
        <header className="flex flex-col gap-2 @xl/catalog:flex-row @xl/catalog:items-center @xl/catalog:justify-between">
          <div className="flex min-w-0 flex-wrap items-baseline gap-x-2.5 gap-y-0.5">
            <h2 className="text-base font-semibold">{tr("云端插件")}</h2>
            {hasEntries && <p className="text-xs text-muted-foreground tabular-nums">
              {tr("{length} 个插件", { length: counts.all })} · {tr("{length} 款游戏", { length: catalog.data?.games.length ?? 0 })}
              {counts.installed > 0 && <> · {tr("已安装")} {counts.installed}</>}
              {shown.length !== counts.all && <> · <span className="font-medium text-foreground/80">{tr("显示 {shown} / {total}", { shown: shown.length, total: counts.all })}</span></>}
            </p>}
          </div>
          <div className="flex w-full min-w-0 items-center gap-1.5 @xl/catalog:max-w-md">
            <label htmlFor={searchId} className="sr-only">{tr("搜索云端插件")}</label>
            <InputGroup className="h-8"><InputGroupAddon><SearchIcon /></InputGroupAddon><InputGroupInput id={searchId} ref={input} value={search} maxLength={128} disabled={!backendAvailable} placeholder={tr("游戏名、别名、slug、标签、Steam App ID")} onChange={event => setSearch(event.target.value)} />
              {search && <InputGroupAddon align="inline-end"><InputGroupButton aria-label={tr("清空搜索")} size="icon-xs" onClick={() => { setSearch(""); input.current?.focus(); }}><XIcon /></InputGroupButton></InputGroupAddon>}
            </InputGroup>
            <div role="group" className="flex shrink-0 items-center rounded-md border p-0.5">
              <IconButton title={tr("卡片视图")} size="icon-xs" variant={view === "grid" ? "secondary" : "ghost"} aria-pressed={view === "grid"} onClick={() => chooseView("grid")}><LayoutGridIcon /></IconButton>
              <IconButton title={tr("列表视图")} size="icon-xs" variant={view === "list" ? "secondary" : "ghost"} aria-pressed={view === "list"} onClick={() => chooseView("list")}><ListIcon /></IconButton>
            </div>
            <IconButton title={tr("刷新云端目录")} size="icon-sm" variant="outline" disabled={!backendAvailable || catalog.isFetching} onClick={() => void catalog.refetch()}><RefreshCwIcon /></IconButton>
          </div>
        </header>
        {hasEntries && <div className="flex flex-wrap items-center gap-1.5">
          {statusOptions.map(option => <Button key={option.id} size="xs" variant={status === option.id ? "secondary" : "ghost"} className={chipClass} aria-pressed={status === option.id} onClick={() => setStatus(option.id)}>
            {option.label}<span className="tabular-nums text-muted-foreground">{option.count}</span>
          </Button>)}
          {platforms.length >= 2 && <>
            <span aria-hidden="true" className="mx-0.5 h-3.5 w-px bg-border" />
            {platforms.map(key => <Button key={key} size="xs" variant={platform === key ? "secondary" : "ghost"} className={chipClass} aria-pressed={platform === key} onClick={() => setPlatform(platform === key ? null : key)}>{platformNames[key] ?? key}</Button>)}
          </>}
          {filtered && <Button size="xs" variant="ghost" className={cn(chipClass, "text-muted-foreground")} onClick={resetFilters}>{tr("重置筛选")}</Button>}
        </div>}
        {installed.isError && <><ErrorState error={installed.error} /><Button className="self-start" size="sm" variant="outline" onClick={() => void installed.refetch()}>{tr("重试读取已安装插件")}</Button></>}
        {!!catalog.data?.skippedEntries && !catalog.isError && <p role="status" className="text-sm text-warning">{tr("目录中有 {skippedEntries} 个无效条目，已跳过。", { skippedEntries: catalog.data.skippedEntries })}</p>}
        {!backendAvailable ? <EmptyState title={tr("桌面能力未连接")}>{tr("请在桌面应用中查看云端目录。")}</EmptyState>
          : catalog.isLoading ? <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground"><Spinner />{tr("正在读取云端目录…")}</p>
          : catalog.isError ? <ErrorState error={catalog.error} />
          : !shown.length ? <EmptyState title={catalog.data?.totalGames ? tr("没有匹配结果") : tr("暂无已发布插件")} action={filtered ? <Button size="sm" variant="outline" onClick={resetFilters}>{tr("重置筛选")}</Button> : undefined}>{catalog.data?.totalGames ? tr("可更换游戏名称、别名、标识或 Steam App ID 再试。") : undefined}</EmptyState>
          : view === "list" ? <ul className="flex flex-col gap-2">
            {shown.map(entry => <li key={entry.plugin.releaseId} className="min-w-0">
              <CatalogRow entry={entry} onSelect={selectPlugin} action={installAction(entry.plugin, true)} />
            </li>)}
          </ul>
          : <ul className="grid grid-cols-[repeat(auto-fill,minmax(min(100%,18.5rem),1fr))] gap-3">
            {shown.map(entry => <li key={entry.plugin.releaseId} className="min-w-0">
              <CatalogCard entry={entry} onSelect={selectPlugin} action={installAction(entry.plugin, true)} />
            </li>)}
          </ul>}
      </section>
    </div>
    {selected && <ReleaseDetails key={selected.releaseId} candidate={selected} action={installAction(selected)} onBack={() => {
      setSelected(null);
      requestAnimationFrame(() => (trigger.current?.isConnected ? trigger.current : input.current)?.focus());
    }} />}
  </div>;
}
