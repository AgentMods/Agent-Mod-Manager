import { tr } from "@/lib/i18n";
import { useCloudGameMatch } from '@/lib/cloud-game-match';
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { open } from "@tauri-apps/plugin-dialog";
import { EllipsisIcon, FolderOpenIcon, Gamepad2Icon, GlobeIcon, PencilIcon, PuzzleIcon, RefreshCwIcon, SearchIcon, SparklesIcon, Trash2Icon, XIcon } from "lucide-react";
import { api, backendAvailable, errorText, useBackendQuery } from "@/lib/api";
import { useUI } from "@/lib/store";
import { displayPath, gameDirectoryKey } from "@/lib/paths";
import { reconcileCandidates } from "@/lib/game-discovery";
import { invalidateGameQueries } from "@/lib/game-cache";
import type { BootstrapState, Game, GameCandidate, Plugin, ScanReport } from "@/lib/types";
import { Badge, EmptyState, ErrorState, IconButton, Modal, Page, PageHeader, PathText } from "@/components/ui";
import { GameConflictPanel, useGameConflicts } from "@/components/GameConflictPanel";
import { GameLaunchButton, type LaunchMethod } from "@/components/GameLaunchButton";
import { chooseGameProgram } from "@/lib/game-launch";
import { GameCover } from "@/components/GameCover";
import { ScanGameCard, ScanGameGrid } from "@/components/ScanGameCard";
import { ManagerPluginCatalog } from "@/components/ManagerPluginCatalog";
import { GameIdentityDialog } from "@/components/GameIdentityDialog";
import { AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Card, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Spinner } from "@/components/ui/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { stagger } from "@/lib/motion";

const storedLaunchPath = (game: Game) => typeof game.metadata?.launchPath === "string" ? game.metadata.launchPath : "";
const launchPathDraft = (raw: string) => ({ raw, text: displayPath(raw) });

function GameCard({ game, index, coverUrl, disabled, busy, writable, onOpen, onLaunch, onLocation, onEdit, onRemove, onChooseAdapter, onCreateAdapter }: {
  game: Game; index: number; coverUrl: string | null; disabled: boolean; busy: string | null;
  writable: boolean; onOpen: () => void; onLaunch: (method?: LaunchMethod) => void; onLocation: (target: "root" | "mods") => void; onEdit: () => void; onRemove: () => void;
  onChooseAdapter: () => void; onCreateAdapter: () => void;
}) {
  const working = busy === game.id;
  return <Card className="gap-0 overflow-hidden py-0 motion-safe:animate-rise-in motion-safe:transition-[box-shadow,border-color] motion-safe:duration-300 motion-safe:ease-out hover:border-primary/40 hover:shadow-md" style={stagger(index)}>
    <button type="button" onClick={onOpen} className="flex flex-col text-left outline-none transition-colors hover:bg-muted/40 focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:ring-inset">
      <GameCover name={game.name} url={coverUrl} decorative />
      <CardHeader className="gap-1 px-5 pt-4 pb-4">
        <CardTitle className="truncate text-base">{game.name}</CardTitle>
        <CardDescription>{tr("{platform} · {installedModCount} 个 Mod", { platform: game.platform, installedModCount: game.installedModCount })}</CardDescription>
      </CardHeader>
    </button>
    <CardFooter className="gap-2 border-t px-5 py-3">
      <GameLaunchButton game={game} disabled={disabled} working={working} writable={writable} onLaunch={onLaunch} />
      {!game.adapterSlug && <IconButton title={tr("创建适配包")} disabled={disabled} onClick={onCreateAdapter}>
        <SparklesIcon />
      </IconButton>}
      <DropdownMenu>
        <Tooltip>
          <TooltipTrigger asChild><DropdownMenuTrigger asChild><Button variant="ghost" size="icon-sm" aria-label={tr("更多操作")} disabled={disabled || working}><EllipsisIcon /></Button></DropdownMenuTrigger></TooltipTrigger>
          <TooltipContent>{tr("更多操作")}</TooltipContent>
        </Tooltip>
        <DropdownMenuContent align="end">
          <DropdownMenuGroup>
            <DropdownMenuItem onSelect={() => onLocation("root")}><FolderOpenIcon />{tr("打开目录")}</DropdownMenuItem>
            <DropdownMenuItem onSelect={() => onLocation("mods")}><PuzzleIcon />{tr("Mod 文件夹")}</DropdownMenuItem>
            <DropdownMenuItem onSelect={onEdit}><PencilIcon />{tr("编辑")}</DropdownMenuItem>
            <DropdownMenuItem onSelect={onChooseAdapter}><PuzzleIcon />{tr("选择适配包")}</DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem variant="destructive" onSelect={onRemove}><Trash2Icon />{tr("移除")}</DropdownMenuItem>
          </DropdownMenuGroup>
        </DropdownMenuContent>
      </DropdownMenu>
    </CardFooter>
  </Card>;
}

function EditGameDialog({ game, onClose, onSaved }: { game: Game; onClose: () => void; onSaved: () => Promise<void> }) {
  const [name, setName] = useState(game.name);
  const [launchPath, setLaunchPath] = useState(() => launchPathDraft(storedLaunchPath(game)));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const trimmed = name.trim();
  async function choose() {
    setError(null);
    try {
      const selected = await chooseGameProgram(game.rootPath);
      if (typeof selected === "string") setLaunchPath(launchPathDraft(selected));
    } catch (failure) { setError(failure); }
  }
  async function save() {
    if (busy || !trimmed) return;
    setBusy(true); setError(null);
    try {
      await api<Game>("update_game", { id: game.id, name: trimmed, launchPath: launchPath.raw.trim() ? launchPath.raw : null });
      await onSaved(); onClose();
    } catch (failure) { setError(failure); }
    finally { setBusy(false); }
  }
  return <Modal title={tr("编辑游戏")} description={tr("修改显示名称，或指定一个游戏启动程序。")} onClose={() => { if (!busy) onClose(); }} className="sm:max-w-lg"
    actions={<>
      <Button variant="outline" disabled={busy} onClick={onClose}>{tr("取消")}</Button>
      <Button disabled={busy || !trimmed} onClick={() => { void save(); }}>{busy && <Spinner data-icon="inline-start" />}{tr("保存")}</Button>
    </>}>
    <ErrorState error={error} />
    <FieldGroup>
      <Field>
        <FieldLabel htmlFor="game-name">{tr("游戏名称")}</FieldLabel>
        <InputGroup><InputGroupInput id="game-name" value={name} disabled={busy} onChange={(event) => setName(event.target.value)} /></InputGroup>
      </Field>
      <Field>
        <FieldLabel htmlFor="game-launch-path">{tr("启动程序")}</FieldLabel>
        <InputGroup>
          <InputGroupInput id="game-launch-path" value={launchPath.text} placeholder={game.steamAppId ? tr("留空则通过 Steam 启动") : tr("选择游戏的可执行文件")} disabled={busy} onChange={(event) => setLaunchPath({ raw: event.target.value, text: event.target.value })} />
          <InputGroupAddon align="inline-end">
            {launchPath.text && <InputGroupButton size="icon-xs" aria-label={tr("清除启动程序")} disabled={busy} onClick={() => setLaunchPath(launchPathDraft(""))}><XIcon /></InputGroupButton>}
            <InputGroupButton disabled={busy || !backendAvailable} onClick={() => { void choose(); }}><FolderOpenIcon />{tr("选择")}</InputGroupButton>
          </InputGroupAddon>
        </InputGroup>
        <FieldDescription>{tr("程序必须位于游戏目录内。留空时，已识别的 Steam 游戏会通过 Steam 启动。")}</FieldDescription>
      </Field>
    </FieldGroup>
  </Modal>;
}

function ChooseAdapterDialog({ game, plugins, writable, onClose, onSaved }: {
  game: Game; plugins: Plugin[]; writable: boolean; onClose: () => void; onSaved: (updated: Game) => Promise<void>;
}) {
  const choices = plugins.filter((plugin) => plugin.enabled && plugin.validated);
  const currentSlug = game.adapterSlug ?? "";
  const [slug, setSlug] = useState(currentSlug);
  const [showCatalog, setShowCatalog] = useState(false);
  const [importedSlug, setImportedSlug] = useState<string | null>(null);
  const catalogButton = useRef<HTMLButtonElement>(null);
  const adapterSelect = useRef<HTMLSelectElement>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const conflicts = useGameConflicts(game.id, "select_adapter");
  const working = busy || !!conflicts.resolving;
  const removing = !!currentSlug && slug === "";
  const canApply = slug !== currentSlug && (removing || choices.some((plugin) => plugin.slug === slug)) && conflicts.canContinue;
  async function apply() {
    setBusy(true); setError(null);
    try {
      const updated = removing
        ? await api<Game>("remove_game_adapter", { id: game.id })
        : await api<Game>("set_game_adapter", { id: game.id, adapterSlug: slug });
      await onSaved(updated); onClose();
    } catch (failure) {
      const latest = await conflicts.query.refetch();
      setError(latest.data?.length ? null : failure);
    }
    finally { setBusy(false); }
  }
  function returnToSelection(installedSlug?: string) {
    setShowCatalog(false);
    if (installedSlug) {
      setImportedSlug(installedSlug);
      if (choices.some((plugin) => plugin.slug === installedSlug)) setSlug(installedSlug);
    }
    requestAnimationFrame(() => (installedSlug && !adapterSelect.current?.disabled ? adapterSelect.current : catalogButton.current)?.focus());
  }
  return <Modal title={tr("选择适配包")} description={showCatalog ? tr("为「{name}」查找云端适配包。", { name: game.name }) : removing
    ? tr("将解除「{name}」与适配包的绑定，保留游戏和已安装的 Mod。", { name: game.name })
    : tr("为「{name}」选择已启用的适配包。应用时会核对游戏目录中的识别标记。", { name: game.name })} onClose={() => { if (!working) onClose(); }}
    actions={showCatalog ? <Button variant="outline" onClick={() => returnToSelection()}>{tr("返回选择适配包")}</Button> : <>
      <Button variant="outline" disabled={working} onClick={onClose}>{tr("取消")}</Button>
      <Button disabled={!writable || !canApply || working} onClick={() => { void apply(); }}>{busy && <Spinner data-icon="inline-start" />}{removing ? tr("移除适配包") : tr("应用适配包")}</Button>
    </>}>
    <ErrorState error={error} />
    {showCatalog ? <ManagerPluginCatalog writable={writable} initialQuery={game.steamAppId || game.name} onReturnToGame={returnToSelection} /> : <>
      <GameConflictPanel state={conflicts} busy={busy} />
      <Field>
        <FieldLabel htmlFor="game-adapter">{tr("游戏适配包")}</FieldLabel>
        <NativeSelect ref={adapterSelect} id="game-adapter" value={slug} disabled={working || (!currentSlug && !choices.length)} onChange={(event) => setSlug(event.target.value)}>
          <NativeSelectOption value="" disabled={!currentSlug}>{currentSlug ? tr("移除适配包") : choices.length ? tr("请选择适配包") : tr("暂无已启用的适配包")}</NativeSelectOption>
          {currentSlug && !choices.some((plugin) => plugin.slug === currentSlug) && <NativeSelectOption value={currentSlug} disabled>{tr("{currentSlug}（当前适配包不可用）", { currentSlug })}</NativeSelectOption>}
          {choices.map((plugin) => <NativeSelectOption key={plugin.slug} value={plugin.slug}>{plugin.name} · {plugin.slug}</NativeSelectOption>)}
        </NativeSelect>
      </Field>
      {importedSlug && !choices.some((plugin) => plugin.slug === importedSlug) && <p role="status" className="text-sm text-muted-foreground">{tr("请在插件中心确认 {importedSlug} 已启用并通过验证，再绑定到游戏。", { importedSlug })}</p>}
      <Button ref={catalogButton} className="self-start" variant="outline" disabled={!backendAvailable || working} onClick={() => { setError(null); setShowCatalog(true); }}><GlobeIcon data-icon="inline-start" />{tr("查找云端插件")}</Button>
    </>}
  </Modal>;
}

function RemoveGameDialog({ game, onClose, onRemoved }: { game: Game; onClose: () => void; onRemoved: () => Promise<void> }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const conflicts = useGameConflicts(game.id, "remove");
  const working = busy || !!conflicts.resolving;
  async function remove() {
    setBusy(true); setError(null);
    try { await api("remove_game", { id: game.id }); await onRemoved(); onClose(); }
    catch (failure) {
      const latest = await conflicts.query.refetch();
      setError(latest.data?.length ? null : failure);
    }
    finally { setBusy(false); }
  }
  return <AlertDialog open onOpenChange={(next) => { if (!next && !working) onClose(); }}>
    <AlertDialogContent className="max-h-[calc(100dvh-2rem)] overflow-y-auto">
      <AlertDialogHeader>
        <AlertDialogTitle>{tr("移除游戏")}</AlertDialogTitle>
        <AlertDialogDescription>{tr("将「{name}」从游戏库移除。只移出管理，不会删除游戏文件、Mod 或存档。仍有已安装的 Mod 时，需要先在游戏详情中卸载。", { name: game.name })}</AlertDialogDescription>
      </AlertDialogHeader>
      <ErrorState error={error} />
      <GameConflictPanel state={conflicts} busy={busy} />
      <AlertDialogFooter>
        <AlertDialogCancel disabled={working}>{tr("取消")}</AlertDialogCancel>
        <Button variant="destructive" disabled={working || !conflicts.canContinue} onClick={() => { void remove(); }}>{busy && <Spinner data-icon="inline-start" />}{tr("移除")}</Button>
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
  const scanButton = useRef<HTMLButtonElement>(null);
  const scanRequest = useRef<string | null>(null);
  useEffect(() => () => {
    const requestId = scanRequest.current;
    scanRequest.current = null;
    if (requestId) void api("scan_games", { options: { page: { requestId, cancel: true } } }).catch(() => { });
  }, []);
  const [scanOpen, setScanOpen] = useState(false);
  const cloud = useCloudGameMatch(scanOpen);
  const [failedRegistrations, setFailedRegistrations] = useState<Record<string, boolean>>({});
  const [directory, setDirectory] = useState("");
  const [candidates, setCandidates] = useState<GameCandidate[]>([]);
  const [report, setReport] = useState<ScanReport | null>(null);
  const [scanRoot, setScanRoot] = useState<string | null>(null);
  const [scanning, setScanning] = useState(false);
  const [discoveryError, setDiscoveryError] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  const [actionBusy, setActionBusy] = useState<string | null>(null);
  const [error, setError] = useState<unknown>(null);
  const directoryInput = useRef<HTMLInputElement>(null);
  const [identity, setIdentity] = useState<{ directory: string; trigger: HTMLButtonElement } | null>(null);
  const [editing, setEditing] = useState<Game | null>(null);
  const [choosingAdapter, setChoosingAdapter] = useState<Game | null>(null);
  const [removing, setRemoving] = useState<Game | null>(null);

  async function discover(action: () => Promise<void>) {
    if (!backendAvailable || busy) return;
    setBusy(true); setDiscoveryError(null);
    try { await action(); } catch (failure) { setDiscoveryError(failure); } finally { setBusy(false); setScanning(false); }
  }
  function stopScan() {
    const requestId = scanRequest.current;
    scanRequest.current = null;
    if (requestId) void api("scan_games", { options: { page: { requestId, cancel: true } } }).catch(() => { });
    setReport((current) => current && { ...current, nextCursor: null });
  }
  async function scan(root?: string, cursor?: string) {
    if (root === "") return;
    if (!cursor) {
      stopScan();
      setReport(null); setCandidates([]); setScanRoot(root ?? null);
    }
    const requestId = cursor ?? crypto.randomUUID();
    scanRequest.current = requestId;
    setScanning(true);
    try {
      const result = await api<ScanReport>("scan_games", { options: { userRoots: root ? [root] : [], includeCommonRoots: !root, page: cursor ? { cursor } : { requestId } } });
      if (scanRequest.current !== requestId) return;
      scanRequest.current = result.nextCursor ?? null;
      setReport(result); setCandidates(reconcileCandidates(result.candidates));
    } catch (error) {
      if (scanRequest.current === requestId) { stopScan(); throw error; }
    }
  }
  async function identify() {
    stopScan();
    setReport(null); setCandidates([]);
    setCandidates([await api<GameCandidate>("identify_game", { directory })]);
  }
  async function chooseDirectory() {
    const selected = await open({ directory: true, multiple: false, title: tr("选择游戏目录") });
    if (typeof selected === "string" && selected) {
      setDirectory(selected);
      await scan(selected);
    }
  }
  function finishRegistration(directory: string, game: Game) {
    const key = gameDirectoryKey(directory);
    setCandidates((items) => items.map((item) => gameDirectoryKey(item.rootPath) === key
      ? { ...item, name: game.name, registeredGameId: game.id, suggestedAdapterSlug: game.adapterSlug } : item));
    notify(tr("已将「{name}」添加到游戏库", { name: game.name }));
  }
  async function register(candidate: GameCandidate, localOnly = false) {
    if (!writable || candidate.registeredGameId) return;
    await discover(async () => {
      setFailedRegistrations(previous => ({ ...previous, [candidate.rootPath]: false }));
      try {
        finishRegistration(candidate.rootPath, await cloud.register(candidate, localOnly));
      } catch (failure) {
        setFailedRegistrations(previous => ({ ...previous, [candidate.rootPath]: true }));
        throw failure;
      }
    });
  }
  /** Runs one card action. Failures are shown on the page; the native error text is already user-facing Chinese. */
  async function perform(game: Game, command: string, args: Record<string, unknown>, done?: string) {
    if (actionBusy) return;
    setActionBusy(game.id); setError(null);
    try { await api(command, args); if (done) notify(done); }
    catch (failure) { setError(errorText(failure)); }
    finally { setActionBusy(null); }
  }

  const discoveryRows = reconcileCandidates(candidates, games.data);
  const rows = (games.data ?? []).filter((game) => game.name.toLowerCase().includes(search.toLowerCase()));
  const actionsDisabled = !backendAvailable || !!actionBusy;
  return <Page data-testid="library-page">
    <PageHeader title={tr("游戏库")} subtitle={backendAvailable ? tr("{value1} 个管理中的游戏", { value1: games.data?.length ?? 0 }) : tr("桌面能力未连接")}
      actions={<Button ref={scanButton} variant="outline" aria-haspopup="dialog" disabled={!backendAvailable} onClick={() => {
        setScanOpen(true);
        void discover(() => scan());
      }}><RefreshCwIcon data-icon="inline-start" />{tr("扫描游戏")}</Button>} />
    <div className="flex flex-wrap items-center gap-3">
      <InputGroup className="max-w-sm"><InputGroupAddon><SearchIcon /></InputGroupAddon><InputGroupInput aria-label={tr("搜索游戏")} placeholder={tr("搜索游戏…")} value={search} onChange={(event) => setSearch(event.target.value)} /></InputGroup>
      <Badge className="ml-auto">{backendAvailable ? tr("本机游戏") : tr("浏览器预览")}</Badge>
    </div>
    {games.error && <ErrorState error={games.error} />}
    {error != null && <ErrorState error={error} />}
    {rows.length > 0 && <div className="grid grid-cols-[repeat(auto-fill,minmax(min(100%,17rem),1fr))] gap-4">
      {rows.map((game, index) => <GameCard key={game.id} game={game} index={index} disabled={actionsDisabled} busy={actionBusy} writable={writable}
        coverUrl={game.adapterSlug ? cloud.coversByAdapter.get(game.adapterSlug) ?? null : null}
        onCreateAdapter={() => startAdapterChat({ name: game.name, gameId: game.id })}
        onChooseAdapter={() => setChoosingAdapter(game)}
        onOpen={() => selectGame(game.id)}
        onLaunch={method => { void perform(game, "launch_game", method ? { id: game.id, method } : { id: game.id }, tr("已发出启动请求")); }}
        onLocation={(target) => { void perform(game, "open_game_location", { id: game.id, target }); }}
        onEdit={() => setEditing(game)} onRemove={() => setRemoving(game)} />)}
    </div>}
    {!rows.length && !games.error && <EmptyState icon={<Gamepad2Icon />} title={!backendAvailable ? tr("桌面能力未连接") : games.isLoading ? tr("正在读取游戏库…") : search ? tr("没有匹配的游戏") : tr("尚无管理中的游戏")} />}

    {scanOpen && <Modal title={tr("扫描游戏")} description={tr("扫描常用位置或指定目录，然后选择要添加的游戏。")} className="sm:w-[calc(100%-2rem)] sm:max-w-[68rem]"
      onClose={() => { stopScan(); setScanOpen(false); }} onCloseAutoFocus={(event) => { event.preventDefault(); scanButton.current?.focus(); }}>
      <form className="flex flex-col gap-2" onSubmit={(event) => { event.preventDefault(); if (directory.trim()) void discover(() => scan(directory)); }}>
        <div className="flex items-center gap-2">
          <InputGroup className="min-w-0 flex-1"><InputGroupInput ref={directoryInput} aria-label={tr("游戏目录")} placeholder={tr("输入游戏或游戏库目录的完整路径")} value={directory} disabled={busy} onChange={(event) => setDirectory(event.target.value)} /></InputGroup>
          <IconButton type="button" title={tr("选择游戏目录")} variant="outline" size="icon" disabled={busy} onClick={() => { void discover(chooseDirectory); }}><FolderOpenIcon /></IconButton>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button type="submit" disabled={!directory.trim() || busy}>{tr("扫描此目录")}</Button>
          <Button type="button" variant="outline" disabled={!directory.trim() || busy} onClick={() => { void discover(identify); }}>{tr("检查单个游戏")}</Button>
          <Button type="button" variant="ghost" disabled={busy} onClick={() => { void discover(() => scan()); }}>{tr("扫描常用位置")}</Button>
        </div>
      </form>
      {scanning && <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground"><Spinner />{scanRoot ? tr("正在扫描指定目录…") : tr("正在扫描常用位置…")}</p>}
      {(scanning || report?.nextCursor) && <div className="flex flex-wrap gap-2">
        {report?.nextCursor && !scanning && <Button type="button" variant="outline" disabled={busy} onClick={() => void discover(() => scan(scanRoot ?? undefined, report.nextCursor!))}>{tr("下一批候选")}</Button>}
        <Button type="button" variant="ghost" onClick={stopScan}>{tr("停止扫描")}</Button>
      </div>}
      <ErrorState error={discoveryError} />
      {report && <div role="status" className="flex flex-col gap-1 text-sm text-muted-foreground">
        {scanRoot && <PathText>{tr("扫描目录：{value1}", { value1: displayPath(scanRoot) })}</PathText>}
        <p>{tr("已检查 {directoriesChecked} 个目录、{entriesChecked} 个条目，本批 {length} 个候选目录{value4}", { directoriesChecked: report.directoriesChecked, entriesChecked: report.entriesChecked, length: discoveryRows.length, value4: report.truncated ? tr(" · 已达到扫描上限") : "" })}</p>
        {!report.candidates.length && !report.nextCursor && <p>{scanRoot ? tr("未发现候选游戏。若已选择单个游戏的安装目录，可使用“检查单个游戏”。") : tr("未在常用位置发现候选游戏，可指定目录扫描。")}</p>}
      </div>}
      {!!report?.warnings.length && <details className="min-w-0 text-sm">
        <summary className="w-fit cursor-pointer text-muted-foreground">{tr("扫描警告（{length}）", { length: report.warnings.length })}</summary>
        <ul className="mt-2 flex flex-col gap-1">
          {report.warnings.map((warning, index) => <li key={index}><PathText className="text-warning">{displayPath(warning.path)}：{errorText(warning)}</PathText></li>)}
        </ul>
      </details>}
      {discoveryRows.length > 0 && <ScanGameGrid>
        {discoveryRows.map((candidate, index) => <ScanGameCard key={gameDirectoryKey(candidate.rootPath)} candidate={candidate} index={index}
          coverUrl={candidate.suggestedAdapterSlug ? cloud.coversByAdapter.get(candidate.suggestedAdapterSlug) : null}
          matched={cloud.match(candidate)} waiting={cloud.waiting(candidate)} failed={!!failedRegistrations[candidate.rootPath]}
          disableRegister={!writable || busy || cloud.pending || games.isLoading} disableIdentify={!backendAvailable || busy || games.isLoading}
          onRegister={localOnly => void register(candidate, localOnly)} onIdentify={trigger => setIdentity({ directory: candidate.rootPath, trigger })} />)}
      </ScanGameGrid>}
      {identity && <GameIdentityDialog key={identity.directory} directory={identity.directory} writable={writable} onClose={() => setIdentity(null)}
        onCloseAutoFocus={(event) => {
          event.preventDefault();
          (identity.trigger.isConnected ? identity.trigger : directoryInput.current)?.focus();
        }}
        onConfirmed={async (game) => {
          finishRegistration(identity.directory, game);
          await invalidateGameQueries(client, { gameIds: [game.id] });
        }} />}
    </Modal>}

    {editing && <EditGameDialog key={editing.id} game={editing} onClose={() => setEditing(null)} onSaved={async () => {
      await invalidateGameQueries(client, { gameIds: [editing.id] });
      notify(tr("已保存游戏设置"));
    }} />}
    {choosingAdapter && <ChooseAdapterDialog key={choosingAdapter.id} game={choosingAdapter} writable={writable} plugins={plugins.data ?? []} onClose={() => setChoosingAdapter(null)} onSaved={async (updated) => {
      client.setQueryData(["game", updated.id], updated);
      await invalidateGameQueries(client, { gameIds: [updated.id] });
      notify(updated.adapterSlug ? tr("已应用游戏适配包") : tr("已移除游戏适配包"));
    }} />}
    {removing && <RemoveGameDialog key={removing.id} game={removing} onClose={() => setRemoving(null)} onRemoved={async () => {
      await invalidateGameQueries(client, { gameIds: [removing.id], workspaceChanged: true });
      notify(tr("已从游戏库移除"));
    }} />}
  </Page>;
}
