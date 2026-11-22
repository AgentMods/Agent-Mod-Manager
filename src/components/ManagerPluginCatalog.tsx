import { useEffect, useId, useRef, useState } from "react";
import { ArrowLeftIcon, RefreshCwIcon, SearchIcon, XIcon } from "lucide-react";
import { api, backendAvailable, useBackendQuery } from "@/lib/api";
import { useUI } from "@/lib/store";
import type { FilePlan } from "@/lib/types";
import { Badge, EmptyState, ErrorState, IconButton } from "@/components/ui";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group";
import { Spinner } from "@/components/ui/spinner";

type Candidate = {
  id: string; releaseId: string; name: string; slug: string; version: string;
  authorName: string; platforms: string[]; steamAppId: string; description: string; tags?: string[];
};
type CatalogGame = { id: string; name: string; slug: string; aliases: string[]; steamAppId: string; plugins: Candidate[] };
type CatalogSearch = { totalGames: number; skippedEntries: number; games: CatalogGame[] };
type Hashes = { adapterSha256: string; skillSha256: string };
type ReleaseDisplay = { description: string; coverPng: string };
type ReleasePreview = {
  display?: ReleaseDisplay | null;
  gameName: string; plugin: Candidate; source: string; adapterJson: string; skillMarkdown: string;
  hashes: Hashes; installedVersion: string | null;
  mcpServers: { id: string; name: string; transport: string; command: string | null; args: string[]; url: string | null }[];
};
type PreparedImport = { preview: ReleasePreview; draftId: string; plan: FilePlan };
type CatalogProps = { writable: boolean; initialQuery?: string; onReturnToGame?: (slug: string) => void };

const platformNames: Record<string, string> = { windows: "Windows", macos: "macOS", linux: "Linux" };
function candidateByline(candidate: Candidate) {
  return [candidate.authorName, candidate.version, candidate.platforms.map(platform => platformNames[platform] ?? platform).join(" / ")].join(" · ");
}
function sameHashes(a: Hashes | null, b: Hashes) {
  return a?.adapterSha256 === b.adapterSha256 && a.skillSha256 === b.skillSha256;
}

function ReleaseCover({ png, className }: { png: string; className: string }) {
  return <img alt="插件封面" decoding="async" loading="lazy" className={className} src={`data:image/png;base64,${png}`} />;
}

// The public index contains metadata only. Fetch artwork for visible releases on demand.
function CatalogArtwork({ releaseId }: { releaseId: string }) {
  const container = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    if (!container.current) return;
    const observer = new IntersectionObserver(entries => {
      if (entries.some(entry => entry.isIntersecting)) {
        setVisible(true);
        observer.disconnect();
      }
    });
    observer.observe(container.current);
    return () => observer.disconnect();
  }, []);
  const display = useBackendQuery<ReleaseDisplay | null>(["manager-plugin-display", releaseId], "get_manager_plugin_display", {releaseId}, visible);
  return <div ref={container} className="min-h-px">{display.data?.coverPng && <ReleaseCover png={display.data.coverPng} className="mb-1 aspect-video w-32 rounded-md object-cover" />}</div>;
}

function RawFile({ name, text }: { name: string; text: string }) {
  return <details className="min-w-0 rounded-md border">
    <summary className="cursor-pointer rounded-md px-3 py-2 text-sm font-medium focus-visible:outline-ring">{name}</summary>
    <pre className="max-h-64 overflow-auto whitespace-pre-wrap wrap-anywhere border-t bg-muted/30 p-3 font-mono text-xs" tabIndex={0} aria-label={`${name} 原文`}>{text}</pre>
  </details>;
}

function ReleaseImport({ candidate, writable, onBack, onReturnToGame }: CatalogProps & { candidate: Candidate; onBack: () => void }) {
  const [prepared, setPrepared] = useState<PreparedImport | null>(null);
  const [phase, setPhase] = useState<"preview" | "draft" | "installed">("preview");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [changed, setChanged] = useState(false);
  const heading = useRef<HTMLHeadingElement>(null);
  const active = useRef(false);
  const reviewPlan = useUI(state => state.reviewPlan);
  const release = { pluginId: candidate.id, releaseId: candidate.releaseId };
  const query = useBackendQuery<ReleasePreview>(["manager-plugin-preview", candidate.id, candidate.releaseId], "preview_manager_plugin", release, !prepared);
  const preview = prepared?.preview ?? (query.isError ? undefined : query.data);
  useEffect(() => {
    active.current = true;
    heading.current?.focus();
    return () => { active.current = false; };
  }, []);

  async function stage() {
    if (!preview || busy || !writable) return;
    setBusy(true); setError(null);
    try {
      const result = await api<PreparedImport>("prepare_manager_plugin_import", release);
      if (!active.current) return;
      if (result.preview.plugin.id !== candidate.id || result.preview.plugin.releaseId !== candidate.releaseId) throw new Error("导入预览的发布 ID 与所选版本不一致，请返回目录重新选择。");
      setPrepared(result);
      setChanged(result.preview.adapterJson !== preview.adapterJson || result.preview.skillMarkdown !== preview.skillMarkdown || result.preview.installedVersion !== preview.installedVersion);
    } catch (failure) { setError(failure); }
    finally { setBusy(false); }
  }
  async function reviewInstallation() {
    if (!prepared || busy || !writable) return;
    setBusy(true); setError(null);
    try {
      const args = { draftId: prepared.draftId };
      const validation = await api<{ valid: boolean; issues: { message: string }[]; hashes: Hashes | null }>("validate_plugin", args);
      if (!validation.valid) throw new Error(validation.issues.map(issue => issue.message).join("；") || "草稿验证未通过");
      const changedDraft = "草稿内容与云端版本的预览不一致，请返回目录重新导入。";
      if (!sameHashes(validation.hashes, prepared.preview.hashes)) throw new Error(changedDraft);
      if (!active.current) return;
      const install = await api<{ plan: FilePlan; hashes: Hashes }>("install_plugin", args);
      if (!sameHashes(install.hashes, prepared.preview.hashes)) throw new Error(changedDraft);
      if (active.current) reviewPlan(install.plan, () => setPhase("installed"));
    } catch (failure) { setError(failure); }
    finally { setBusy(false); }
  }

  return <Card className="min-w-0 gap-4 shadow-none">
    <CardHeader className="gap-3">
      <Button variant="ghost" size="sm" className="w-fit" onClick={onBack} disabled={busy}><ArrowLeftIcon data-icon="inline-start" />返回目录</Button>
      <CardTitle ref={heading} tabIndex={-1} className="wrap-anywhere">{preview?.plugin.name ?? candidate.name}</CardTitle>
      <p className="wrap-anywhere text-sm text-muted-foreground">{candidateByline(preview?.plugin ?? candidate)}</p>
    </CardHeader>
    <CardContent className="flex min-w-0 flex-col gap-4">
      {query.isLoading && <p role="status" className="flex items-center gap-2 text-sm"><Spinner />正在读取版本文件…</p>}
      {query.isError && !prepared && <><ErrorState error={query.error} /><Button variant="outline" className="self-start" disabled={query.isFetching} onClick={() => void query.refetch()}>重试预览</Button></>}
      {preview && <>
        <dl className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-sm [&_dt]:text-muted-foreground [&_dd]:wrap-anywhere">
          <dt>游戏</dt><dd>{preview.gameName}</dd>
          <dt>来源</dt><dd>{preview.source}</dd>
          <dt>适配包</dt><dd>{preview.plugin.slug}</dd>
          <dt>发布 ID</dt><dd>{preview.plugin.releaseId}</dd>
        </dl>
        {preview.display && <ReleaseCover png={preview.display.coverPng} className="aspect-video w-full max-w-sm rounded-md object-cover" />}
        {preview.display && <p className="wrap-anywhere whitespace-pre-line text-sm">{preview.display.description}</p>}
        {preview.plugin.description && <p className="wrap-anywhere text-sm">{preview.plugin.description}</p>}
        {preview.installedVersion && phase !== "installed" && <Alert variant="warning"><AlertDescription>已安装同标识适配包 {preview.plugin.slug} {preview.installedVersion}。安装此版本将覆盖已安装的文件。</AlertDescription></Alert>}
        <section aria-label="声明的 MCP 服务" className="flex flex-col gap-1.5 text-sm">
          <h3 className="font-medium">声明的 MCP 服务</h3>
          {preview.mcpServers.length ? <ul className="flex flex-col gap-2">{preview.mcpServers.map(server => <li key={server.id} className="min-w-0 rounded-md border px-3 py-2">
            <span className="wrap-anywhere">{server.name} · {server.transport}</span>
            <code className="block whitespace-pre-wrap wrap-anywhere text-xs text-muted-foreground">{server.command ? [server.command, ...server.args.map(arg => JSON.stringify(arg))].join(" ") : server.url}</code>
          </li>)}</ul> : <p className="text-muted-foreground">此版本未声明 MCP 服务。</p>}
        </section>
        <div className="flex min-w-0 flex-col gap-2"><RawFile name="adapter.json" text={preview.adapterJson} /><RawFile name="SKILL.md" text={preview.skillMarkdown} /></div>
        {changed && <Alert variant="warning"><AlertDescription>待导入的内容或本地安装版本已变化。请核对上方更新的预览，再审核草稿。</AlertDescription></Alert>}
        <ErrorState error={error} />
        <div className="flex flex-wrap items-center gap-2">
          {phase === "preview" && (prepared
            ? <Button disabled={!writable} onClick={() => reviewPlan(prepared.plan, () => setPhase("draft"))}>审核导入草稿</Button>
            : <Button disabled={!writable || busy || query.isFetching} onClick={() => void stage()}>{busy && <Spinner data-icon="inline-start" />}准备导入草稿</Button>)}
          {phase === "draft" && <><span role="status"><Badge tone="success">已导入草稿</Badge></span><Button disabled={!writable || busy} onClick={() => void reviewInstallation()}>{busy && <Spinner data-icon="inline-start" />}验证并预览安装</Button></>}
          {phase === "installed" && <><span role="status"><Badge tone="success">已安装 {preview.plugin.version}</Badge></span>{onReturnToGame && <Button onClick={() => onReturnToGame(preview.plugin.slug)}>返回并选择此适配包</Button>}</>}
        </div>
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
  const needle = search.trim().toLowerCase();
  const games = (catalog.data?.games ?? []).filter(game => !needle || game.steamAppId === needle ||
    [game.name, game.slug, ...game.aliases].some(value => value.toLowerCase().includes(needle)) ||
    game.plugins.some(plugin => plugin.steamAppId === needle || [plugin.name, plugin.slug].some(value => value.toLowerCase().includes(needle))));
  return <div className="w-full min-w-0 max-w-xl">
    <div hidden={!!selected}><Card className="min-w-0 gap-4 shadow-none">
      <CardHeader className="flex flex-row items-center justify-between gap-2"><CardTitle className="text-lg">云端插件</CardTitle><IconButton title="刷新云端目录" disabled={!backendAvailable || catalog.isFetching} onClick={() => void catalog.refetch()}><RefreshCwIcon /></IconButton></CardHeader>
      <CardContent className="flex min-w-0 flex-col gap-4">
        <label htmlFor={searchId} className="sr-only">搜索云端插件</label>
        <InputGroup><InputGroupAddon><SearchIcon /></InputGroupAddon><InputGroupInput id={searchId} ref={input} value={search} maxLength={128} disabled={!backendAvailable} placeholder="游戏名、别名、slug、Steam App ID" onChange={event => setSearch(event.target.value)} />
          {search && <InputGroupAddon align="inline-end"><InputGroupButton aria-label="清空搜索" size="icon-xs" onClick={() => { setSearch(""); input.current?.focus(); }}><XIcon /></InputGroupButton></InputGroupAddon>}
        </InputGroup>
        {!!catalog.data?.skippedEntries && !catalog.isError && <p role="status" className="text-sm text-warning">目录中有 {catalog.data.skippedEntries} 个无效条目，已跳过。</p>}
        {!backendAvailable ? <EmptyState title="桌面能力未连接">请在桌面应用中查看云端目录。</EmptyState>
          : catalog.isLoading ? <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground"><Spinner />正在读取云端目录…</p>
          : catalog.isError ? <ErrorState error={catalog.error} />
          : !games.length ? <EmptyState title={catalog.data?.totalGames ? "没有匹配结果" : "暂无已发布插件"}>{catalog.data?.totalGames ? "可更换游戏名称、别名、标识或 Steam App ID 再试。" : undefined}</EmptyState>
          : games.map(game => <section key={game.id} aria-label={game.name} className="min-w-0">
            <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1 border-b pb-2"><h3 className="wrap-anywhere text-sm font-semibold">{game.name}</h3>{game.steamAppId && <span className="text-xs text-muted-foreground">Steam App ID {game.steamAppId}</span>}</div>
            <ul className="divide-y">{game.plugins.map(plugin => <li key={plugin.releaseId} className="flex min-w-0 flex-col items-start gap-2 py-3">
              <CatalogArtwork releaseId={plugin.releaseId} />
              <h4 className="wrap-anywhere font-medium">{plugin.name}</h4><p className="wrap-anywhere text-sm text-muted-foreground">{candidateByline(plugin)}</p>
              {plugin.description && <p className="wrap-anywhere text-sm">{plugin.description}</p>}
              {!!plugin.tags?.length && <div className="flex flex-wrap gap-1">{plugin.tags.map(tag => <Badge key={tag}>{tag}</Badge>)}</div>}
              {plugin.steamAppId && plugin.steamAppId !== game.steamAppId && <p className="text-xs text-muted-foreground">Steam App ID {plugin.steamAppId}</p>}
              <Button size="sm" variant="outline" aria-label={`查看并导入 ${plugin.name} · ${plugin.authorName} · ${plugin.version}`} onClick={event => { trigger.current = event.currentTarget; setSelected(plugin); }}>查看并导入</Button>
            </li>)}</ul>
          </section>)}
      </CardContent>
    </Card></div>
    {selected && <ReleaseImport key={selected.releaseId} candidate={selected} writable={writable} onReturnToGame={onReturnToGame} onBack={() => {
      setSelected(null);
      requestAnimationFrame(() => (trigger.current?.isConnected ? trigger.current : input.current)?.focus());
    }} />}
  </div>;
}
