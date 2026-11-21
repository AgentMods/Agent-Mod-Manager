import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { open } from "@tauri-apps/plugin-dialog";
import { EllipsisIcon, FolderOpenIcon, FolderPlusIcon, Gamepad2Icon, GlobeIcon, PencilIcon, PlayIcon, PuzzleIcon, RefreshCwIcon, SearchIcon, SparklesIcon, Trash2Icon, XIcon } from "lucide-react";
import { api, backendAvailable, errorText, useBackendQuery } from "@/lib/api";
import { useUI } from "@/lib/store";
import type { BootstrapState, Game, GameCandidate, Plugin, ScanReport } from "@/lib/types";
import { Badge, EmptyState, ErrorState, IconButton, Modal, Page, PageHeader, PathText } from "@/components/ui";
import { GameCover } from "@/components/GameCover";
import { GameMatchBadge } from "@/components/GameMatchBadge";
import { GameIdentityDialog } from "@/components/GameIdentityDialog";
import { AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group";
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemMedia, ItemTitle } from "@/components/ui/item";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Spinner } from "@/components/ui/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

const storedLaunchPath = (game: Game) => typeof game.metadata?.launchPath === "string" ? game.metadata.launchPath : "";

function GameCard({ game, coverUrl, disabled, busy, onOpen, onLaunch, onLocation, onEdit, onRemove, onChooseAdapter, onCreateAdapter }: {
  game: Game; coverUrl: string | null; disabled: boolean; busy: string | null;
  onOpen: () => void; onLaunch: () => void; onLocation: (target: "root" | "mods") => void; onEdit: () => void; onRemove: () => void;
  onChooseAdapter: () => void; onCreateAdapter: () => void;
}) {
  const working = busy === game.id;
  return <Card className="gap-0 overflow-hidden py-0">
    <button type="button" onClick={onOpen} className="flex flex-col text-left outline-none transition-colors hover:bg-muted/40 focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:ring-inset">
      <GameCover name={game.name} url={coverUrl} />
      <CardHeader className="gap-1 px-5 pt-4 pb-4">
        <CardTitle className="truncate text-base">{game.name}</CardTitle>
        <CardDescription>{game.platform} · {game.installedModCount} 个 Mod</CardDescription>
      </CardHeader>
    </button>
    {!game.adapterSlug && <div className="px-5 pb-3">
      <Button variant="outline" size="sm" disabled={disabled} onClick={onCreateAdapter}><SparklesIcon data-icon="inline-start" />创建适配包</Button>
    </div>}
    <CardFooter className="gap-2 border-t px-5 py-3">
      <Button size="sm" className="flex-1" disabled={disabled || working} onClick={onLaunch}>
        {working ? <Spinner data-icon="inline-start" /> : <PlayIcon data-icon="inline-start" />}启动游戏
      </Button>
      <DropdownMenu>
        <Tooltip>
          <TooltipTrigger asChild><DropdownMenuTrigger asChild><Button variant="ghost" size="icon-sm" aria-label="更多操作" disabled={disabled || working}><EllipsisIcon /></Button></DropdownMenuTrigger></TooltipTrigger>
          <TooltipContent>更多操作</TooltipContent>
        </Tooltip>
        <DropdownMenuContent align="end">
          <DropdownMenuGroup>
            <DropdownMenuItem onSelect={() => onLocation("root")}><FolderOpenIcon />打开目录</DropdownMenuItem>
            <DropdownMenuItem onSelect={() => onLocation("mods")}><PuzzleIcon />Mod 文件夹</DropdownMenuItem>
            <DropdownMenuItem onSelect={onEdit}><PencilIcon />编辑</DropdownMenuItem>
            <DropdownMenuItem onSelect={onChooseAdapter}><PuzzleIcon />选择适配包</DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem variant="destructive" onSelect={onRemove}><Trash2Icon />移除</DropdownMenuItem>
          </DropdownMenuGroup>
        </DropdownMenuContent>
      </DropdownMenu>
    </CardFooter>
  </Card>;
}

function EditGameDialog({ game, onClose, onSaved }: { game: Game; onClose: () => void; onSaved: () => Promise<void> }) {
  const [name, setName] = useState(game.name);
  const [launchPath, setLaunchPath] = useState(storedLaunchPath(game));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const trimmed = name.trim();
  async function choose() {
    setError(null);
    try {
      const windows = navigator.userAgent.includes("Windows");
      const selected = await open({ multiple: false, directory: false, defaultPath: game.rootPath, title: "选择游戏启动程序", filters: windows ? [{ name: "可执行文件", extensions: ["exe"] }] : undefined });
      if (typeof selected === "string") setLaunchPath(selected);
    } catch (failure) { setError(failure); }
  }
  async function save() {
    if (busy || !trimmed) return;
    setBusy(true); setError(null);
    try {
      await api<Game>("update_game", { id: game.id, name: trimmed, launchPath: launchPath.trim() || null });
      await onSaved(); onClose();
    } catch (failure) { setError(failure); }
    finally { setBusy(false); }
  }
  return <Modal title="编辑游戏" description="修改显示名称，或指定一个游戏启动程序。" onClose={() => { if (!busy) onClose(); }} className="sm:max-w-lg"
    actions={<>
      <Button variant="outline" disabled={busy} onClick={onClose}>取消</Button>
      <Button disabled={busy || !trimmed} onClick={() => { void save(); }}>{busy && <Spinner data-icon="inline-start" />}保存</Button>
    </>}>
    <ErrorState error={error} />
    <FieldGroup>
      <Field>
        <FieldLabel htmlFor="game-name">游戏名称</FieldLabel>
        <InputGroup><InputGroupInput id="game-name" value={name} maxLength={80} disabled={busy} onChange={(event) => setName(event.target.value)} /></InputGroup>
      </Field>
      <Field>
        <FieldLabel htmlFor="game-launch-path">启动程序</FieldLabel>
        <InputGroup>
          <InputGroupInput id="game-launch-path" value={launchPath} placeholder={game.steamAppId ? "留空则通过 Steam 启动" : "选择游戏的可执行文件"} disabled={busy} onChange={(event) => setLaunchPath(event.target.value)} />
          <InputGroupAddon align="inline-end">
            {launchPath && <InputGroupButton size="icon-xs" aria-label="清除启动程序" disabled={busy} onClick={() => setLaunchPath("")}><XIcon /></InputGroupButton>}
            <InputGroupButton disabled={busy || !backendAvailable} onClick={() => { void choose(); }}><FolderOpenIcon />选择</InputGroupButton>
          </InputGroupAddon>
        </InputGroup>
        <FieldDescription>程序必须位于游戏目录内。留空时，已识别的 Steam 游戏会通过 Steam 启动。</FieldDescription>
      </Field>
    </FieldGroup>
  </Modal>;
}

function ChooseAdapterDialog({ game, plugins, onClose, onSaved }: {
  game: Game; plugins: Plugin[]; onClose: () => void; onSaved: () => Promise<void>;
}) {
  const choices = plugins.filter((plugin) => plugin.enabled && plugin.validated);
  const [slug, setSlug] = useState(game.adapterSlug ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const canApply = slug !== game.adapterSlug && choices.some((plugin) => plugin.slug === slug);
  async function apply() {
    setBusy(true); setError(null);
    try {
      await api<Game>("set_game_adapter", { id: game.id, adapterSlug: slug });
      await onSaved(); onClose();
    } catch (failure) { setError(failure); }
    finally { setBusy(false); }
  }
  return <Modal title="选择适配包" description={`为「${game.name}」选择已启用的适配包。应用时会核对游戏目录中的识别标记。`} onClose={() => { if (!busy) onClose(); }}
    actions={<>
      <Button variant="outline" disabled={busy} onClick={onClose}>取消</Button>
      <Button disabled={!canApply || busy} onClick={() => { void apply(); }}>{busy && <Spinner data-icon="inline-start" />}应用适配包</Button>
    </>}>
    <ErrorState error={error} />
    <Field>
      <FieldLabel htmlFor="game-adapter">游戏适配包</FieldLabel>
      <NativeSelect id="game-adapter" value={choices.some((plugin) => plugin.slug === slug) ? slug : ""} disabled={busy || !choices.length} onChange={(event) => setSlug(event.target.value)}>
        <NativeSelectOption value="">{choices.length ? "请选择适配包" : "暂无已启用的适配包"}</NativeSelectOption>
        {choices.map((plugin) => <NativeSelectOption key={plugin.slug} value={plugin.slug}>{plugin.name} · {plugin.slug}</NativeSelectOption>)}
      </NativeSelect>
    </Field>
  </Modal>;
}

function RemoveGameDialog({ game, onClose, onRemoved }: { game: Game; onClose: () => void; onRemoved: () => Promise<void> }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  async function remove() {
    setBusy(true); setError(null);
    try { await api("remove_game", { id: game.id }); await onRemoved(); onClose(); }
    catch (failure) { setError(failure); }
    finally { setBusy(false); }
  }
  return <AlertDialog open onOpenChange={(next) => { if (!next && !busy) onClose(); }}>
    <AlertDialogContent>
      <AlertDialogHeader>
        <AlertDialogTitle>移除游戏</AlertDialogTitle>
        <AlertDialogDescription>将「{game.name}」从游戏库移除。只移出管理，不会删除游戏文件、Mod 或存档。仍有已安装的 Mod 时，需要先在游戏详情中卸载。</AlertDialogDescription>
      </AlertDialogHeader>
      <ErrorState error={error} />
      <AlertDialogFooter>
        <AlertDialogCancel disabled={busy}>取消</AlertDialogCancel>
        <Button variant="destructive" disabled={busy} onClick={() => { void remove(); }}>{busy && <Spinner data-icon="inline-start" />}移除</Button>
      </AlertDialogFooter>
    </AlertDialogContent>
  </AlertDialog>;
}

export default function LibraryPage() {
  const client = useQueryClient();
  const games = useBackendQuery<Game[]>(["games"], "list_games");
  const plugins = useBackendQuery<Plugin[]>(["plugins"], "list_plugins");
  const bootstrap = useBackendQuery<BootstrapState>(["bootstrap"], "get_bootstrap_state");
  const writable = backendAvailable && !!bootstrap.data && !bootstrap.data.recoveryError;
  const selectGame = useUI((state) => state.selectGame);
  const startAdapterChat = useUI((state) => state.startAdapterChat);
  const notify = useUI((state) => state.notify);
  const [search, setSearch] = useState("");
  const [directory, setDirectory] = useState("");
  const [candidates, setCandidates] = useState<GameCandidate[]>([]);
  const [report, setReport] = useState<ScanReport | null>(null);
  const [scanRoot, setScanRoot] = useState<string | null>(null);
  const [scanning, setScanning] = useState(false);
  const [discoveryError, setDiscoveryError] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  const [actionBusy, setActionBusy] = useState<string | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [identityRoot, setIdentityRoot] = useState<string | null>(null);
  const [editing, setEditing] = useState<Game | null>(null);
  const [choosingAdapter, setChoosingAdapter] = useState<Game | null>(null);
  const [removing, setRemoving] = useState<Game | null>(null);

  async function discover(action: () => Promise<void>) {
    if (!backendAvailable || busy) return;
    setBusy(true); setError(null); setDiscoveryError(null);
    try { await action(); } catch (failure) { setDiscoveryError(failure); } finally { setBusy(false); setScanning(false); }
  }
  async function scan(root?: string) {
    if (root === "") return;
    setReport(null); setCandidates([]); setScanRoot(root ?? null); setScanning(true);
    const result = await api<ScanReport>("scan_games", { options: { userRoots: root ? [root] : [], includeCommonRoots: !root } });
    setReport(result); setCandidates(result.candidates);
    notify(`${result.truncated ? "扫描达到上限" : "扫描完成"}，发现 ${result.candidates.length} 个候选目录`);
  }
  async function identify() {
    setReport(null); setCandidates([]);
    setCandidates([await api<GameCandidate>("identify_game", { directory: directory.trim() })]);
  }
  async function chooseDirectory() {
    const selected = await open({ directory: true, multiple: false, title: "选择游戏目录" });
    if (typeof selected === "string" && selected) {
      setDirectory(selected);
      await scan(selected);
    }
  }
  async function register(candidate: GameCandidate) {
    if (!writable || busy) return;
    setBusy(true); setError(null);
    try {
      const game = await api<Game>("register_game", { directory: candidate.rootPath, adapterSlug: candidate.suggestedAdapterSlug });
      await games.refetch(); setCandidates((items) => items.filter((item) => item.rootPath !== candidate.rootPath)); selectGame(game.id);
    } catch (failure) { setError(failure); } finally { setBusy(false); }
  }
  /** Runs one card action. Failures are shown on the page; the native error text is already user-facing Chinese. */
  async function perform(game: Game, command: string, args: Record<string, unknown>, done?: string) {
    if (actionBusy) return;
    setActionBusy(game.id); setError(null);
    try { await api(command, args); if (done) notify(done); }
    catch (failure) { setError(errorText(failure)); }
    finally { setActionBusy(null); }
  }

  const rows = (games.data ?? []).filter((game) => game.name.toLowerCase().includes(search.toLowerCase()));
  const actionsDisabled = !backendAvailable || !!actionBusy;
  return <Page data-testid="library-page">
    <PageHeader title="游戏库" subtitle={backendAvailable ? `${games.data?.length ?? 0} 个管理中的游戏` : "桌面能力未连接"}
      actions={<Button variant="outline" disabled={!backendAvailable || busy} onClick={() => { void discover(() => scan()); }}><RefreshCwIcon data-icon="inline-start" className={scanning && !scanRoot ? "animate-spin" : undefined} />扫描游戏</Button>} />
    <div className="flex flex-wrap items-center gap-3">
      <InputGroup className="max-w-sm"><InputGroupAddon><SearchIcon /></InputGroupAddon><InputGroupInput aria-label="搜索游戏" placeholder="搜索游戏…" value={search} onChange={(event) => setSearch(event.target.value)} /></InputGroup>
      <Badge className="ml-auto">{backendAvailable ? "本机游戏" : "浏览器预览"}</Badge>
    </div>
    {games.error && <ErrorState error={games.error} />}
    {error != null && <ErrorState error={error} />}
    {rows.length > 0 && <div className="grid grid-cols-[repeat(auto-fill,minmax(min(100%,17rem),1fr))] gap-4">
      {rows.map((game) => <GameCard key={game.id} game={game} disabled={actionsDisabled} busy={actionBusy}
        coverUrl={plugins.data?.find((plugin) => plugin.slug === game.adapterSlug && plugin.enabled && plugin.validated)?.coverUrl ?? null}
        onCreateAdapter={() => startAdapterChat({ name: game.name, gameId: game.id })}
        onChooseAdapter={() => setChoosingAdapter(game)}
        onOpen={() => selectGame(game.id)}
        onLaunch={() => { void perform(game, "launch_game", { id: game.id }, "已发出启动请求"); }}
        onLocation={(target) => { void perform(game, "open_game_location", { id: game.id, target }); }}
        onEdit={() => setEditing(game)} onRemove={() => setRemoving(game)} />)}
    </div>}
    {!rows.length && !games.error && <EmptyState icon={<Gamepad2Icon />} title={!backendAvailable ? "桌面能力未连接" : games.isLoading ? "正在读取游戏库…" : search ? "没有匹配的游戏" : "尚无管理中的游戏"} />}

    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2 text-base"><FolderPlusIcon className="size-4" />添加</CardTitle>
        <CardDescription>选择存放多个游戏的文件夹即可自动扫描，也可以输入路径后按回车。扫描不会自动添加游戏。</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <form className="flex flex-wrap items-center gap-2" onSubmit={(event) => { event.preventDefault(); void discover(() => scan(directory.trim())); }}>
          <InputGroup className="min-w-40 flex-1"><InputGroupInput aria-label="游戏目录" placeholder="输入游戏或游戏库目录的完整路径" value={directory} disabled={busy} onChange={(event) => setDirectory(event.target.value)} /></InputGroup>
          <IconButton type="button" title="选择游戏目录" variant="outline" size="icon" disabled={!backendAvailable || busy} onClick={() => { void discover(chooseDirectory); }}><FolderOpenIcon /></IconButton>
          <Button type="submit" disabled={!backendAvailable || !directory.trim() || busy}>{scanning && scanRoot ? <><Spinner data-icon="inline-start" />扫描中…</> : "扫描此目录"}</Button>
          <Button type="button" variant="outline" disabled={!backendAvailable || !directory.trim() || busy} onClick={() => { void discover(identify); }}>检查单个游戏</Button>
        </form>
        <ErrorState error={discoveryError} />
        {report && <div role="status" className="flex flex-col gap-1 text-sm text-muted-foreground">
          {scanRoot && <PathText>扫描目录：{scanRoot}</PathText>}
          <p>已检查 {report.directoriesChecked} 个目录、{report.entriesChecked} 个条目{report.truncated ? " · 已达到扫描上限" : ""}</p>
          {!report.candidates.length && <p>{scanRoot ? "未发现候选游戏。若已选择单个游戏的安装目录，可使用“检查单个游戏”。" : "未在常用位置发现候选游戏，可指定目录扫描。"}</p>}
          {report.warnings.map((warning, index) => <PathText className="text-warning" key={index}>{warning.path}：{warning.message}</PathText>)}
        </div>}
        {candidates.length > 0 && <ItemGroup className="gap-2">
          {candidates.map((candidate) => <Item variant="outline" key={candidate.rootPath}>
            <ItemMedia variant="icon"><Gamepad2Icon /></ItemMedia>
            <ItemContent>
              <ItemTitle>{candidate.name}</ItemTitle>
              <ItemDescription className="line-clamp-none"><PathText>{candidate.rootPath}</PathText></ItemDescription>
              {candidate.warnings.map((warning) => <p key={warning} className="text-xs text-warning">{warning}</p>)}
            </ItemContent>
            <ItemActions className="flex-wrap">
              <GameMatchBadge candidate={candidate} />
              {candidate.confidence < 0.85 && !candidate.registeredGameId && <Button variant="outline" size="sm" disabled={!backendAvailable || busy} onClick={() => setIdentityRoot(candidate.rootPath)}><GlobeIcon data-icon="inline-start" />联网识别</Button>}
              <Button size="sm" disabled={!writable || busy || !!candidate.registeredGameId} onClick={() => { void register(candidate); }}>{candidate.registeredGameId ? "管理中" : "添加"}</Button>
            </ItemActions>
          </Item>)}
        </ItemGroup>}
      </CardContent>
    </Card>

    {identityRoot && <GameIdentityDialog key={identityRoot} directory={identityRoot} writable={writable} onClose={() => setIdentityRoot(null)} onConfirmed={async () => {
      await games.refetch(); setCandidates((items) => items.filter((candidate) => candidate.rootPath !== identityRoot)); notify("已确认游戏身份并添加");
    }} />}
    {editing && <EditGameDialog key={editing.id} game={editing} onClose={() => setEditing(null)} onSaved={async () => { await games.refetch(); notify("已保存游戏设置"); }} />}
    {choosingAdapter && <ChooseAdapterDialog key={choosingAdapter.id} game={choosingAdapter} plugins={plugins.data ?? []} onClose={() => setChoosingAdapter(null)} onSaved={async () => {
      await Promise.all([
        games.refetch(),
        client.invalidateQueries({ queryKey: ["game", choosingAdapter.id] }),
        client.invalidateQueries({ queryKey: ["game-adapter", choosingAdapter.id] }),
      ]);
      notify("已应用游戏适配包");
    }} />}
    {removing && <RemoveGameDialog key={removing.id} game={removing} onClose={() => setRemoving(null)} onRemoved={async () => { await games.refetch(); notify("已从游戏库移除"); }} />}
  </Page>;
}
