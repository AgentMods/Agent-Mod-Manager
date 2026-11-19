import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { listen } from '@tauri-apps/api/event';
import { openUrl } from '@tauri-apps/plugin-opener';
import { ArrowLeftIcon, ArrowRightIcon, DownloadIcon, ExternalLinkIcon, ListChecksIcon, PackageIcon, PackagePlusIcon, RefreshCwIcon, SearchIcon, TriangleAlertIcon, XIcon } from 'lucide-react';
import { FomodOptions } from '@/components/FomodOptions';
import { ModrinthGraphReview } from '@/components/ModrinthGraphReview';
import { Badge, bytes, date, EmptyState, ErrorState, IconButton, Modal, Page, PageHeader, PathText } from '@/components/ui';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Field, FieldGroup, FieldLabel } from '@/components/ui/field';
import { InputGroup, InputGroupAddon, InputGroupInput } from '@/components/ui/input-group';
import { Item, ItemActions, ItemContent, ItemDescription, ItemMedia, ItemTitle } from '@/components/ui/item';
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select';
import { Progress as ProgressBar } from '@/components/ui/progress';
import { Skeleton } from '@/components/ui/skeleton';
import { Spinner } from '@/components/ui/spinner';
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group';
import { api, backendAvailable, useBackendQuery } from '@/lib/api';
import { initialFomodSelections, selectedFomodMappings, validFomodSelection, type FomodPreview, type FomodSelections } from '@/lib/fomod';
import { dependencyLabels, dependencyOutcome, dependencyReasons, type Dependency, type DependencyCheck } from '@/lib/mod-dependencies';
import { useUI } from '@/lib/store';
import type { FilePlan, Game } from '@/lib/types';
type Source = { source_id: string; state: string; message: string };
type Mod = { source_id: string; project_id: string; game_id: string | null; name: string; summary: string; author: string | null; homepage_url: string | null; icon_url: string | null; downloads: number | null };
type Resolved = { source_id: string; project_id: string; version_id: string; name: string; version: string; filename: string; size: number | null; resolution_id: string; hashes: { sha256?: string | null; sha512?: string | null; sha1?: string | null; md5?: string | null }; homepage_url: string | null; warnings: string[] };
type Resolution = { status: Source; download: Resolved | null; homepage_url: string | null; dependencies: Dependency[] | null };
type ProjectRequest = Pick<Mod, 'source_id' | 'project_id' | 'game_id'> & { game_version: string | null; loader: string | null };
type SourceVersion = {
  version_id: string; version: string; name: string; release_channel: 'release' | 'beta' | 'alpha';
  published_at: string; game_versions: string[]; loaders: string[];
};
type VersionList = { status: Source; versions: SourceVersion[]; has_more: boolean };
type VersionPage = { request: ProjectRequest; offset: number } & (
  | { state: 'loading' }
  | { state: 'ready'; result: VersionList }
  | { state: 'error'; error: unknown }
);
type ResolutionState = { request: ProjectRequest; versionId: string | null } & (
  | { state: 'loading' }
  | { state: 'ready' | 'consumed'; result: Resolution; sequence: number }
  | { state: 'error'; error: unknown }
);
type DependencyReport = { gameId: string; excludedInstallId: string | null; checks: DependencyCheck[] | null };
type DependencyReviewState =
  | { status: 'idle' | 'checking' }
  | { status: 'ready'; report: DependencyReport }
  | { status: 'error'; error: unknown };
type Artifact = { id: string; path: string; filename: string; size: number; sha256: string; integrity_verified: boolean; requires_review: boolean; warnings: string[] };
type ArtifactRecord = { artifact: Artifact; sourceId: string; projectId: string; versionId: string; name: string; version: string; createdAt: string };
type InstalledVersion = {
  id: string; gameId: string; name: string; version: string | null; status: string; installedAt: string;
  downloadOrigin?: Pick<ArtifactRecord, 'sourceId' | 'projectId' | 'versionId'> | null;
  metadata?: { modType?: unknown } | null;
};
type Progress = { phase: string; downloaded: number; total: number | null; attempt: number };
type Adapter = { modTypes: { id: string; name: string; notes: string }[] };
const sourceNames: Record<string, string> = { modrinth: 'Modrinth', thunderstore: 'Thunderstore', curseforge: 'CurseForge', nexus: 'Nexus Mods', '3dm': '3DM' };
const sourceStates: Record<string, string> = { available: '可用', missing_key: '缺少密钥', invalid_key: '密钥无效', unsupported: '不支持', needs_configuration: '待配置', manual_download: '手动下载', limited: '受限', unavailable: '不可用' };
const phases: Record<string, string> = { connecting: '连接中', downloading: '下载中', retrying: '重试中', verifying: '校验中', completed: '已完成', cancelled: '已取消', failed: '失败' };
const versionPageSize = 20;
const channelLabels: Record<SourceVersion['release_channel'], string> = { release: '正式版', beta: 'Beta 测试版', alpha: 'Alpha 预览版' };
function https(value: string | null) { try { return value && new URL(value).protocol === 'https:' ? value : null; } catch { return null; } }

function sourceListSummary(values: string[]) {
  const visible = values.slice(0, 3);
  const remaining = values.length - visible.length;
  return visible.length ? `${visible.join('、')}${remaining ? ` +${remaining}` : ''}` : '未声明';
}

function nexusModNumber(projectId: string) {
  return /^(?:[^/]+\/)?(\d+)$/.exec(projectId)?.[1].replace(/^0+(?=\d)/, '');
}

function installOptions(installs: InstalledVersion[], artifact: ArtifactRecord | null, gameId: string) {
  const activeGame = installs.filter(mod => mod.gameId === gameId &&
    (mod.status === 'enabled' || mod.status === 'installed' || mod.status === 'partially_restored'));
  const active = activeGame.filter(mod => artifact && mod.downloadOrigin?.sourceId === artifact.sourceId &&
    mod.downloadOrigin.projectId === artifact.projectId);
  const nexusProject = artifact?.sourceId === 'nexus' ? artifact.projectId : null;
  const nexusNumber = nexusProject && nexusModNumber(nexusProject);
  const ambiguousNexus = !!nexusProject && !!nexusNumber && activeGame.some(({ downloadOrigin }) =>
    downloadOrigin?.sourceId === 'nexus' && (!nexusProject.includes('/') || !downloadOrigin.projectId.includes('/')) &&
    nexusModNumber(downloadOrigin.projectId) === nexusNumber);
  const sameVersion = active.some(mod => mod.downloadOrigin?.versionId === artifact?.versionId);
  const duplicateProject = active.length > 1;
  const ordinary = !ambiguousNexus && active.length === 0;
  const candidate = !ambiguousNexus && active.length === 1 && !sameVersion && active[0].status === 'enabled' ? active[0] : null;
  const originalType = typeof candidate?.metadata?.modType === 'string' && candidate.metadata.modType ? candidate.metadata.modType : null;
  return {
    sameVersion, duplicateProject, ambiguousNexus, ordinary, candidate, originalType,
    canPlan: (replaceInstallId: string, modType: string) => replaceInstallId
      ? candidate?.id === replaceInstallId && originalType === modType
      : ordinary,
  };
}

function DependencyReference({ dependency }: { dependency: Dependency }) {
  return <div className="flex min-w-0 flex-col gap-1">
    {dependency.project_id && <PathText>项目 {dependency.project_id}</PathText>}
    {dependency.version_id && <PathText>版本 {dependency.version_id}</PathText>}
    {dependency.file_name && <PathText>{dependency.file_name}</PathText>}
  </div>;
}

function DependencyDetails({ dependencies }: { dependencies: Dependency[] | null }) {
  if (!dependencies?.length) {
    return <p className="source-dependencies min-w-0 border-t pt-3 text-sm text-muted-foreground">
      {dependencies ? '来源未声明直接依赖' : '来源依赖未知'}
    </p>;
  }
  return <details className="source-dependencies min-w-0 border-t pt-3 text-sm">
    <summary className="cursor-pointer font-medium">来源声明的直接依赖（{dependencies.length}）</summary>
    <ul role="list" className="mt-3 flex list-none flex-col gap-2">
      {dependencies.map((dependency, index) => <Item asChild variant="outline" size="sm" className="items-start" key={index}>
        <li>
          <Badge tone={dependency.dependency_type === 'incompatible' ? 'error' : dependency.dependency_type === 'required' ? 'warning' : undefined}>{dependencyLabels[dependency.dependency_type]}</Badge>
          <ItemContent className="min-w-0"><DependencyReference dependency={dependency}/></ItemContent>
        </li>
      </Item>)}
    </ul>
  </details>;
}
function DependencyReview({ install, gameId, replaceInstallId, disabled }: {
  install: ArtifactRecord; gameId: string; replaceInstallId: string | null; disabled: boolean;
}) {
  const scopeId = useId();
  const [state, setState] = useState<DependencyReviewState>({ status: 'idle' });
  const active = useRef(false);
  const checking = useRef(false);
  // Retire the keyed review on commit, before a late resolution can start a local check.
  useLayoutEffect(() => {
    active.current = true;
    return () => { active.current = false; };
  }, []);

  async function check() {
    if (disabled || checking.current || !active.current) return;
    checking.current = true;
    setState({ status: 'checking' });
    try {
      const resolution = await api<Resolution>('resolve_mod', { request: {
        source_id: 'modrinth', project_id: install.projectId, version_id: install.versionId,
      } });
      if (!active.current) return;
      const download = resolution.download;
      if (!download) throw new Error('无法解析此 Modrinth 下载的确切版本。');
      if (download.source_id !== install.sourceId || download.project_id !== install.projectId || download.version_id !== install.versionId) {
        throw new Error('来源返回的下载与所选归档不一致。');
      }
      const report = await api<DependencyReport>('check_modrinth_dependencies', { request: {
        gameId, dependencies: resolution.dependencies, replaceInstallId,
      } });
      if (!active.current) return;
      if (report.gameId !== gameId || report.excludedInstallId !== (report.checks?.length ? replaceInstallId : null)) {
        throw new Error('核对结果与所选游戏或待替换记录不一致。');
      }
      setState({ status: 'ready', report });
    } catch (error) {
      if (active.current) setState({ status: 'error', error });
    } finally { checking.current = false; }
  }

  const pending = state.status === 'checking';
  const report = state.status === 'ready' ? state.report : null;
  const checks = report?.checks;
  return <section className="source-dependencies flex min-w-0 flex-col gap-3 rounded-lg border p-3 text-sm" aria-label="已装依赖记录核对">
    <Button variant="outline" className="self-start" disabled={disabled || pending} aria-busy={pending}
      aria-describedby={scopeId} onClick={() => { void check(); }}>
      {pending ? <Spinner data-icon="inline-start"/> : <ListChecksIcon data-icon="inline-start" aria-hidden="true"/>}
      {state.status === 'error' ? '重试核对已装依赖' : '核对已装依赖'}
    </Button>
    <p className="text-muted-foreground" id={scopeId}>仅核对目标游戏的管理器 Modrinth 安装记录，不含其他来源或本地安装。不查文件是否存在、校验和或加载器兼容性；仅供参考，不阻止安装。</p>
    <p className="text-muted-foreground" role="status">
      {pending ? '正在核对已装依赖记录…' : checks === null ? '该版本的直接依赖声明未知。' : checks?.length === 0 ? '该版本未声明直接依赖。'
        : checks && `已核对 ${checks.length} 条直接依赖声明。${report?.excludedInstallId ? '本次核对已排除所选待替换记录。' : ''}`}
    </p>
    <ErrorState error={state.status === 'error' ? state.error : null}/>
    {!!checks?.length && <ul role="list" aria-label="直接依赖核对结果" className="flex min-w-0 list-none flex-col gap-2">
      {checks.map((entry, index) => {
        const { dependency, reason, matches, matchCount } = entry;
        const outcome = dependencyOutcome(entry);
        return <Item asChild variant="outline" size="sm" className="items-start" key={index}>
          <li>
            <Badge>{dependencyLabels[dependency.dependency_type]}</Badge>
            <ItemContent className="min-w-0 items-start">
              <Badge tone={outcome.tone}>{outcome.label}</Badge>
              <DependencyReference dependency={dependency}/>
              {reason && <ItemDescription className="line-clamp-none">{dependencyReasons[reason]}</ItemDescription>}
              {matchCount > 0 && <ItemDescription>共 {matchCount} 条相关记录{matchCount > matches.length && `（显示前 ${matches.length} 条）`}</ItemDescription>}
              {matches.map(match => <PathText key={match.installId}>
                记录 {match.installId} · 项目 {match.projectId} · 版本 {match.versionId} · {match.status === 'enabled' ? '事务已部署' : match.status === 'partially_restored' ? '部分恢复' : match.status}
              </PathText>)}
            </ItemContent>
          </li>
        </Item>;
      })}
    </ul>}
  </section>;
}
export default function DiscoverPage() {
  const sources = useBackendQuery<Source[]>(['sources'], 'list_mod_sources');
  const artifacts = useBackendQuery<ArtifactRecord[]>(['downloads'], 'list_downloads');
  const games = useBackendQuery<Game[]>(['games'], 'list_games');
  const bootstrap = useBackendQuery<{ recoveryError: string | null }>(['bootstrap'], 'get_bootstrap_state');
  const writable = backendAvailable && !!bootstrap.data && !bootstrap.data.recoveryError;
  const reviewPlan = useUI(s => s.reviewPlan);
  const notify = useUI(s => s.notify);
  const [query, setQuery] = useState('');
  const [source, setSource] = useState('');
  const [providerGame, setProviderGame] = useState('');
  const [gameVersion, setGameVersion] = useState('');
  const [loader, setLoader] = useState('');
  const [results, setResults] = useState<{ mods: Mod[]; sources: Source[] } | null>(null);
  const [offset, setOffset] = useState(0);
  const [selection, setSelection] = useState<Mod | null>(null);
  const [resolutionState, setResolutionState] = useState<ResolutionState | null>(null);
  const [versionPage, setVersionPage] = useState<VersionPage | null>(null);
  const versionPanelId = useId();
  const [operationBusy, setOperationBusy] = useState(false);
  const busy = operationBusy || (resolutionState?.state === 'loading' && selection?.source_id !== 'modrinth');
  const [error, setError] = useState<unknown>(null);
  const [listenerReady, setListenerReady] = useState(false);
  const [task, setTask] = useState<{ id: string; name: string } | null>(null);
  const [progress, setProgress] = useState<Record<string, Progress>>({});
  const [install, setInstall] = useState<ArtifactRecord | null>(null);
  const [gameId, setGameId] = useState('');
  const [modType, setModType] = useState('');
  const [replaceInstallId, setReplaceInstallId] = useState('');
  const [fomodPreview, setFomodPreview] = useState<FomodPreview | null>(null);
  const [fomodSelections, setFomodSelections] = useState<FomodSelections>({});
  const fomodMappings = selectedFomodMappings(fomodPreview, fomodSelections);
  const adapter = useBackendQuery<Adapter>(['game-adapter', gameId], 'get_game_adapter', {gameId}, !!install && !!gameId);
  const installs = useQuery({
    queryKey: ['installs', gameId], queryFn: () => api<InstalledVersion[]>('list_installs', { gameId }),
    enabled: backendAvailable && !!install && !!gameId, staleTime: 0, retry: false, networkMode: 'always',
  });
  const options = installOptions(installs.data ?? [], install, gameId);
  const selectedReplacement = options.candidate?.id === replaceInstallId ? options.candidate : null;
  const originalTypeName = adapter.data?.modTypes.find(type => type.id === options.originalType)?.name ?? options.originalType;
  const installsReady = installs.isSuccess && !installs.isFetching;
  const showInstallOptions = installs.isSuccess && (busy || !installs.isFetching);
  const canPreviewInstall = writable && !busy && !!install && !!gameId && adapter.isSuccess && !adapter.isFetching &&
    adapter.data.modTypes.some(type => type.id === modType) && installsReady && options.canPlan(replaceInstallId, modType) && (!fomodPreview || (fomodMappings.length > 0 && validFomodSelection(fomodPreview, fomodSelections)));
  const planningInstall = useRef(false);
  const fomodPreviewing = useRef(false);
  const mounted = useRef(true);
  const resolveSequence = useRef(0);
  const versionSequence = useRef(0);
  useEffect(() => {
    mounted.current = true;
    if (!backendAvailable) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<{ taskId: string; progress: Progress }>('download://progress', event => {
      if (!disposed) setProgress(current => ({ ...current, [event.payload.taskId]: event.payload.progress }));
    }).then(stop => { if (disposed) stop(); else { unlisten = stop; setListenerReady(true); } }).catch(e => { if (!disposed) setError(e); });
    return () => { disposed = true; mounted.current = false; unlisten?.(); };
  }, []);
  function closeVersions() {
    ++versionSequence.current;
    setVersionPage(null);
  }
  function invalidateInspector() {
    // Retire both requests immediately, including results already displayed for the old filters.
    ++resolveSequence.current;
    setResolutionState(null);
    closeVersions();
  }
  function updateFilter(value: string, previous: string, setValue: (value: string) => void) {
    setValue(value);
    if (value.trim() !== previous.trim()) invalidateInspector();
  }
  function projectRequest(mod: Mod): ProjectRequest {
    return {
      source_id: mod.source_id, project_id: mod.project_id, game_id: mod.game_id,
      game_version: gameVersion.trim() || null, loader: loader.trim() || null,
    };
  }
  async function search(nextOffset = 0) {
    invalidateInspector();
    setOperationBusy(true); setError(null);
    try {
      const result = await api<{ mods: Mod[]; sources: Source[] }>('search_mods', { query: { query: query.trim(), game_ids: source && providerGame.trim() ? { [source]: providerGame.trim() } : {}, game_version: gameVersion.trim() || null, loader: loader.trim() || null, offset: nextOffset, limit: 20, source_ids: source ? [source] : [] } });
      setResults(result); setOffset(nextOffset); setSelection(null);
    } catch (e) { setError(e); } finally { setOperationBusy(false); }
  }
  function selectProject(mod: Mod) {
    closeVersions();
    setSelection(mod);
    void resolve(projectRequest(mod));
  }
  async function listVersions(request: ProjectRequest, offset = 0) {
    const sequence = ++versionSequence.current;
    const page = { request, offset };
    setVersionPage({ ...page, state: 'loading' });
    try {
      const result = await api<VersionList>('list_mod_versions', { request: { ...request, limit: versionPageSize, offset } });
      if (!mounted.current || sequence !== versionSequence.current) return;
      if (result.status.source_id !== request.source_id) throw new Error('返回的版本列表与所选来源不一致。');
      if (result.status.state !== 'available' && result.status.state !== 'limited') {
        throw new Error(result.status.message || '无法读取来源版本。');
      }
      setVersionPage({ ...page, state: 'ready', result });
    } catch (error) {
      if (mounted.current && sequence === versionSequence.current) setVersionPage({ ...page, state: 'error', error });
    }
  }
  async function resolve(request: ProjectRequest, versionId: string | null = null) {
    const sequence = ++resolveSequence.current;
    const choice = { request, versionId };
    setError(null);
    setResolutionState({ ...choice, state: 'loading' });
    try {
      const result = await api<Resolution>('resolve_mod', { request: { ...request, version_id: versionId } });
      if (!mounted.current || sequence !== resolveSequence.current) return;
      if (versionId !== null) {
        const resolved = result.download;
        if (result.status.source_id !== request.source_id || (resolved && (
          resolved.source_id !== request.source_id || resolved.project_id !== request.project_id || resolved.version_id !== versionId
        ))) throw new Error('来源返回的下载与所选版本不一致。');
        if (!resolved) throw new Error(result.status.message || '无法解析所选版本。');
      }
      setResolutionState({ ...choice, state: 'ready', result, sequence });
    } catch (error) {
      if (mounted.current && sequence === resolveSequence.current) setResolutionState({ ...choice, state: 'error', error });
    }
  }
  async function download() {
    if (!writable || !listenerReady || task || busy || resolutionState?.state !== 'ready' ||
      resolutionState.sequence !== resolveSequence.current || !resolutionState.result.download) return;
    const resolved = resolutionState.result.download;
    const id = crypto.randomUUID();
    // Retire the one-use ID synchronously, before another click can use the same state.
    ++resolveSequence.current;
    setResolutionState({ ...resolutionState, state: 'consumed' });
    setTask({ id, name: resolved.name }); setError(null);
    try {
      await api<Artifact>('download_mod', { taskId: id, resolutionId: resolved.resolution_id });
      await artifacts.refetch();
      notify('下载已完成，尚未安装');
    } catch (e) { if (mounted.current) setError(e); } finally { if (mounted.current) setTask(null); }
  }
  async function openHomepage(value: string | null) {
    const url = https(value); if (!url) return;
    try { if (backendAvailable) await openUrl(url); else window.open(url, '_blank', 'noopener,noreferrer'); } catch (e) { setError(e); }
  }
  async function previewFomod() {
    if (!writable || busy || !install || fomodPreviewing.current) return;
    fomodPreviewing.current = true;
    setOperationBusy(true); setError(null); setFomodPreview(null); setFomodSelections({});
    try {
      const preview = await api<FomodPreview>('preview_fomod', { artifactPath: install.artifact.path });
      if (!mounted.current) return;
      if (preview.artifactPath !== install.artifact.path || preview.artifactSha256 !== install.artifact.sha256) {
        throw new Error('FOMOD 预览归档与当前下载记录不一致，请重新下载后再试。');
      }
      setFomodPreview(preview);
      setFomodSelections(initialFomodSelections(preview));
    } catch (e) { if (mounted.current) setError(e); }
    finally { fomodPreviewing.current = false; if (mounted.current) setOperationBusy(false); }
  }
  async function previewInstall() {
    if (!install || !canPreviewInstall || planningInstall.current) return;
    planningInstall.current = true;
    setOperationBusy(true); setError(null);
    try {
      const { data } = await installs.refetch({ throwOnError: true });
      if (!mounted.current) return;
      if (!data || !installOptions(data, install, gameId).canPlan(replaceInstallId, modType)) {
        throw new Error('安装记录已变化，请重新核对安装版本。');
      }
      const result = await api<{ plan: FilePlan; warnings: string[] }>('plan_install', { request: {
        gameId, artifactPath: install.artifact.path, filename: install.artifact.filename, name: install.name,
        modType, version: install.version, expectedSha256: install.artifact.sha256, folderName: null,
        mappings: fomodMappings, source: install.sourceId, sourceModId: install.projectId,
        ...(replaceInstallId ? { replaceInstallId } : {}),
      } });
      if (!mounted.current) return;
      setInstall(null); if (result.warnings.length) notify(result.warnings.join('；'));
      reviewPlan(result.plan);
    } catch (e) { if (mounted.current) setError(e); }
    finally { planningInstall.current = false; if (mounted.current) setOperationBusy(false); }
  }
  const statuses = results?.sources ?? sources.data ?? [];
  const currentProgress = task ? progress[task.id] : undefined;
  const resolution = resolutionState?.state === 'ready' || resolutionState?.state === 'consumed' ? resolutionState.result : null;
  const selectedVersionId = resolutionState?.versionId;
  const versionId = selectedVersionId ?? (selection?.source_id === 'modrinth' ? resolution?.download?.version_id : null);
  const hasMoreVersions = versionPage?.state === 'ready' && versionPage.result.has_more;
  const fieldId = `${versionPanelId}-field`;
  const modTypeNotes = adapter.data?.modTypes.find(type => type.id === modType)?.notes;
  return <Page data-testid="discover-page">
    <PageHeader title="发现 Mod" actions={
      <IconButton title="刷新来源状态" disabled={!backendAvailable} onClick={() => { void sources.refetch(); }}>
        <RefreshCwIcon data-icon="inline-start" aria-hidden="true"/>
      </IconButton>
    }/>
    <Card className="min-w-0">
      <CardContent className="flex min-w-0 flex-col gap-4">
        <form className="flex min-w-0 flex-col gap-3" onSubmit={e => { e.preventDefault(); if (backendAvailable && !busy) void search(); }}>
          <FieldGroup className="grid items-end gap-3 sm:grid-cols-[minmax(0,1fr)_auto_auto]">
            <Field>
              <FieldLabel className="sr-only" htmlFor={`${fieldId}-search`}>搜索 Mod</FieldLabel>
              <InputGroup>
                <InputGroupAddon><SearchIcon aria-hidden="true"/></InputGroupAddon>
                <InputGroupInput id={`${fieldId}-search`} aria-label="搜索 Mod" placeholder="搜索 Mod…" value={query}
                  onChange={e => setQuery(e.target.value)} maxLength={512}/>
              </InputGroup>
            </Field>
            <Field className="[&>[data-slot=native-select-wrapper]]:w-full">
              <FieldLabel className="sr-only" htmlFor={`${fieldId}-source`}>Mod 来源</FieldLabel>
              <NativeSelect id={`${fieldId}-source`} aria-label="Mod 来源" value={source}
                onChange={e => { setSource(e.target.value); setProviderGame(''); }}>
                <NativeSelectOption value="">所有来源</NativeSelectOption>
                {(sources.data ?? []).map(item => <NativeSelectOption value={item.source_id} key={item.source_id}>
                  {sourceNames[item.source_id] ?? item.source_id}
                </NativeSelectOption>)}
              </NativeSelect>
            </Field>
            <Button type="submit" className="justify-self-start" disabled={!backendAvailable || busy}>
              {busy ? <Spinner data-icon="inline-start"/> : <SearchIcon data-icon="inline-start" aria-hidden="true"/>}
              {busy ? '请求中…' : '搜索'}
            </Button>
          </FieldGroup>
          <FieldGroup className="grid gap-3 sm:grid-cols-3">
            <Field className="gap-1.5">
              <FieldLabel htmlFor={`${fieldId}-provider-game`}>来源游戏 ID</FieldLabel>
              <InputGroup>
                <InputGroupInput id={`${fieldId}-provider-game`} aria-label="来源游戏 ID" placeholder="来源游戏 ID" disabled={!source}
                  value={providerGame} onChange={e => setProviderGame(e.target.value)}/>
              </InputGroup>
            </Field>
            <Field className="gap-1.5">
              <FieldLabel htmlFor={`${fieldId}-game-version`}>游戏版本</FieldLabel>
              <InputGroup>
                <InputGroupInput id={`${fieldId}-game-version`} aria-label="游戏版本" placeholder="游戏版本" value={gameVersion}
                  onChange={e => updateFilter(e.target.value, gameVersion, setGameVersion)}/>
              </InputGroup>
            </Field>
            <Field className="gap-1.5">
              <FieldLabel htmlFor={`${fieldId}-loader`}>Mod 加载器</FieldLabel>
              <InputGroup>
                <InputGroupInput id={`${fieldId}-loader`} aria-label="Mod 加载器" placeholder="加载器" value={loader}
                  onChange={e => updateFilter(e.target.value, loader, setLoader)}/>
              </InputGroup>
            </Field>
          </FieldGroup>
        </form>
        {statuses.length > 0 && <div className="flex flex-wrap gap-2">
          {statuses.map(item => <span title={item.message} key={item.source_id}>
            <Badge tone={item.state === 'available' ? 'success' : 'warning'}>{sourceNames[item.source_id] ?? item.source_id} · {sourceStates[item.state] ?? item.state}</Badge>
          </span>)}
        </div>}
        <ErrorState error={sources.error}/>
        <ErrorState error={error}/>
      </CardContent>
    </Card>
    <div className="grid min-w-0 items-start gap-6 lg:grid-cols-[minmax(0,1fr)_minmax(0,20rem)]">
      <section className="flex min-w-0 flex-col gap-4">
        {results?.mods.length ? <div className="grid min-w-0 gap-3 xl:grid-cols-2">
          {results.mods.map(mod => {
            const iconUrl = https(mod.icon_url);
            return <Item asChild variant={selection === mod ? 'muted' : 'outline'} key={`${mod.source_id}:${mod.project_id}`}
              className="min-w-0 flex-nowrap items-start bg-card text-left hover:border-ring data-[variant=muted]:bg-accent disabled:opacity-50">
              <button type="button" onClick={() => selectProject(mod)} disabled={busy}>
                <ItemMedia variant={iconUrl ? 'image' : 'icon'}>
                  {iconUrl ? <img alt="" src={iconUrl} loading="lazy" referrerPolicy="no-referrer"/> : <PackageIcon aria-hidden="true"/>}
                </ItemMedia>
                <ItemContent className="min-w-0 [overflow-wrap:anywhere]">
                  <ItemTitle className="max-w-full"><h2>{mod.name}</h2></ItemTitle>
                  <ItemDescription className="line-clamp-3">{mod.summary}</ItemDescription>
                  <small className="text-muted-foreground">{mod.author ?? '—'} · {sourceNames[mod.source_id] ?? mod.source_id}{mod.downloads !== null ? ` · ${mod.downloads.toLocaleString('zh-CN')} 次下载` : ''}</small>
                </ItemContent>
              </button>
            </Item>;
          })}
        </div> : <EmptyState title={!backendAvailable ? '桌面能力未连接' : results ? '没有匹配的 Mod' : '尚未搜索'}/>}
        {results && <div className="flex flex-wrap items-center gap-2">
          <IconButton title="上一页" disabled={busy || offset === 0} onClick={() => { void search(Math.max(0, offset - 20)); }}>
            <ArrowLeftIcon data-icon="inline-start" aria-hidden="true"/>
          </IconButton>
          <span className="text-sm text-muted-foreground">第 {offset / 20 + 1} 页</span>
          <IconButton title="下一页" disabled={busy || !results.mods.length || offset >= 10000} onClick={() => { void search(offset + 20); }}>
            <ArrowRightIcon data-icon="inline-start" aria-hidden="true"/>
          </IconButton>
        </div>}
      </section>
      <aside className="min-w-0">
        <Card className="min-w-0">
          <CardHeader className="min-w-0 [overflow-wrap:anywhere]">
            <CardTitle>{selection?.name ?? 'Mod 详情'}</CardTitle>
            {selection && <CardDescription>{selection.summary}</CardDescription>}
          </CardHeader>
          <CardContent className="flex min-w-0 flex-col gap-4 text-sm [overflow-wrap:anywhere]">
            {selection ? <>
              <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-2 [&>dt]:text-muted-foreground">
                <dt>来源</dt><dd>{sourceNames[selection.source_id] ?? selection.source_id}</dd>
                <dt>项目</dt><dd>{selection.project_id}</dd>
                <dt>作者</dt><dd>{selection.author ?? '—'}</dd>
                {versionId && <><dt>{selectedVersionId ? '已选版本 ID' : '版本 ID'}</dt><dd>{versionId}</dd></>}
                {resolution?.download && <>
                  <dt>版本</dt><dd>{resolution.download.version}</dd>
                  <dt>文件</dt><dd>{resolution.download.filename}</dd>
                  <dt>大小</dt><dd>{resolution.download.size === null ? '未知' : bytes(resolution.download.size)}</dd>
                </>}
              </dl>
              {resolution?.download && <DependencyDetails dependencies={resolution.dependencies}/>}
              {resolution && <Alert variant="warning" role="note">
                <TriangleAlertIcon aria-hidden="true"/>
                <AlertDescription className="min-w-0">{resolution.status.message}</AlertDescription>
              </Alert>}
              {resolution?.download?.warnings.map((warning, i) => <p className="text-warning" key={i}>{warning}</p>)}
              {resolutionState?.state === 'loading' && <p className="flex items-center gap-2" role="status">
                <Spinner/>{selectedVersionId ? '正在解析所选版本…' : '正在解析默认版本…'}
              </p>}
              <ErrorState error={resolutionState?.state === 'error' ? resolutionState.error : null}/>
              <div className="flex flex-wrap gap-2">
                <Button variant="outline" disabled={!https(resolution?.homepage_url ?? selection.homepage_url)}
                  onClick={() => { void openHomepage(resolution?.homepage_url ?? selection.homepage_url); }}>
                  <ExternalLinkIcon data-icon="inline-start" aria-hidden="true"/>原始页面
                </Button>
                {resolution?.download && <Button disabled={resolutionState?.state !== 'ready' || busy || !writable || !listenerReady || !!task}
                  onClick={() => { void download(); }}><DownloadIcon data-icon="inline-start" aria-hidden="true"/>下载归档</Button>}
                {(['modrinth', 'thunderstore'] as const).includes(selection.source_id as 'modrinth' | 'thunderstore') && <Button variant="outline"
                  disabled={busy} aria-expanded={!!versionPage} aria-controls={versionPage ? versionPanelId : undefined}
                  onClick={() => { if (versionPage) closeVersions(); else void listVersions(projectRequest(selection)); }}>
                  <ListChecksIcon data-icon="inline-start" aria-hidden="true"/>{versionPage ? '收起来源版本' : '查看来源版本'}
                </Button>}
                {!resolutionState && <Button variant="outline" disabled={busy} onClick={() => { void resolve(projectRequest(selection)); }}>
                  <RefreshCwIcon data-icon="inline-start" aria-hidden="true"/>解析默认版本
                </Button>}
                {resolutionState?.state === 'error' && <Button variant="outline" disabled={busy}
                  onClick={() => { void resolve(resolutionState.request, resolutionState.versionId); }}>
                  <RefreshCwIcon data-icon="inline-start" aria-hidden="true"/>{selectedVersionId ? '重试解析所选版本' : '重试解析默认版本'}
                </Button>}
              </div>
              {versionPage && <section className="source-dependencies flex min-w-0 flex-col gap-3 border-t pt-3" id={versionPanelId} aria-label="来源版本">
                {versionPage.state === 'loading' && <>
                  <p role="status">正在读取来源版本…</p>
                  <Skeleton className="h-16 w-full" aria-hidden="true"/>
                </>}
                {versionPage.state === 'error' && <>
                  <ErrorState error={versionPage.error}/>
                  <Button variant="outline" className="self-start" disabled={busy}
                    onClick={() => { void listVersions(versionPage.request, versionPage.offset); }}>
                    <RefreshCwIcon data-icon="inline-start" aria-hidden="true"/>重试读取来源版本
                  </Button>
                </>}
                {versionPage.state === 'ready' && versionPage.result.status.state === 'limited' && <Alert variant="warning" role="status">
                  <TriangleAlertIcon aria-hidden="true"/>
                  <AlertDescription className="min-w-0">{versionPage.result.status.message}</AlertDescription>
                </Alert>}
                {versionPage.state === 'ready' && (versionPage.result.versions.length ? <ToggleGroup asChild type="single"
                  value={selectedVersionId ?? ''} variant="outline" size="sm" spacing={1} orientation="vertical"
                  className="w-full min-w-0 list-none flex-col items-stretch gap-2">
                  <ul role="list" aria-label={`${versionPage.request.source_id} 版本`}>
                    {versionPage.result.versions.map(version => {
                      const selected = selectedVersionId === version.version_id;
                      return <Item asChild variant="outline" size="sm" className="flex-col items-stretch" key={version.version_id}>
                        <li>
                          <ItemContent className="min-w-0 items-start">
                            <ItemTitle className="max-w-full"><strong>{version.name}</strong></ItemTitle>
                            <ItemDescription className="line-clamp-none">版本：{version.version}</ItemDescription>
                            <PathText>版本 ID：{version.version_id}</PathText>
                            <Badge tone={version.release_channel === 'release' ? 'success' : 'warning'}>{channelLabels[version.release_channel]}</Badge>
                            <ItemDescription className="line-clamp-none">发布日期：<time dateTime={version.published_at}>{date(version.published_at)}</time></ItemDescription>
                            {!versionPage.request.loader && <ItemDescription className="line-clamp-none">来源声明加载器：{sourceListSummary(version.loaders)}</ItemDescription>}
                            {!versionPage.request.game_version && <ItemDescription className="line-clamp-none">来源声明游戏版本：{sourceListSummary(version.game_versions)}</ItemDescription>}
                          </ItemContent>
                          <ItemActions>
                            <ToggleGroupItem value={version.version_id} role="button" aria-checked={undefined} aria-pressed={selected}
                              aria-label={`选择版本 ${version.version}（${version.version_id}）`}
                              disabled={busy || (selected && resolutionState?.state === 'loading')}
                              onClick={() => { void resolve(versionPage.request, version.version_id); }}>选择版本</ToggleGroupItem>
                          </ItemActions>
                        </li>
                      </Item>;
                    })}
                  </ul>
                </ToggleGroup> : <p role="status">没有符合当前筛选条件的来源版本。</p>)}
                <div className="flex flex-wrap items-center gap-2">
                  <IconButton title="上一页来源版本" disabled={busy || versionPage.state === 'loading' || versionPage.offset === 0}
                    onClick={() => { void listVersions(versionPage.request, versionPage.offset - versionPageSize); }}>
                    <ArrowLeftIcon data-icon="inline-start" aria-hidden="true"/>
                  </IconButton>
                  <span className="text-muted-foreground" aria-live="polite">第 {versionPage.offset / versionPageSize + 1} 页</span>
                  <IconButton title="下一页来源版本" disabled={busy || !hasMoreVersions}
                    onClick={() => { void listVersions(versionPage.request, versionPage.offset + versionPageSize); }}>
                    <ArrowRightIcon data-icon="inline-start" aria-hidden="true"/>
                  </IconButton>
                </div>
              </section>}
            </> : <EmptyState title="未选择 Mod"/>}
          </CardContent>
        </Card>
      </aside>
    </div>
    <Card className="min-w-0">
      <CardHeader className="flex flex-wrap items-center justify-between gap-2">
        <CardTitle>下载记录</CardTitle>
        <IconButton title="刷新下载记录" disabled={!backendAvailable} onClick={() => { void artifacts.refetch(); }}>
          <RefreshCwIcon data-icon="inline-start" aria-hidden="true"/>
        </IconButton>
      </CardHeader>
      <CardContent className="flex min-w-0 flex-col gap-3 [overflow-wrap:anywhere]">
        <ErrorState error={artifacts.error}/>
        {task && <Item variant="outline" className="download-row min-w-0">
          <ItemMedia variant="icon"><DownloadIcon aria-hidden="true"/></ItemMedia>
          <ItemContent className="min-w-0">
            <ItemTitle className="max-w-full">{task.name}</ItemTitle>
            <ItemDescription className="line-clamp-none">
              {phases[currentProgress?.phase ?? 'connecting']} · {bytes(currentProgress?.downloaded ?? 0)}{currentProgress?.total != null ? ` / ${bytes(currentProgress.total)}` : ''}
            </ItemDescription>
            {currentProgress?.total != null && <ProgressBar aria-label={task.name}
              value={currentProgress.total > 0 ? Math.min(100, Math.max(0, currentProgress.downloaded / currentProgress.total * 100)) : 0}/>}
          </ItemContent>
          <ItemActions>
            <IconButton title="取消下载" onClick={() => { void api('cancel_download', { taskId: task.id }).catch(setError); }}>
              <XIcon data-icon="inline-start" aria-hidden="true"/>
            </IconButton>
          </ItemActions>
        </Item>}
        {(artifacts.data ?? []).map(record => <Item variant="outline" className="download-row grid min-w-0 grid-cols-[auto_minmax(0,1fr)] items-start sm:grid-cols-[auto_minmax(0,1fr)_auto]" key={record.artifact.id}>
          <ItemMedia variant="icon"><PackageIcon aria-hidden="true"/></ItemMedia>
          <ItemContent className="min-w-0">
            <ItemTitle className="block max-w-full">{record.name} <small>{record.version}</small></ItemTitle>
            <ItemDescription className="line-clamp-none">{record.artifact.filename} · {bytes(record.artifact.size)} · {date(record.createdAt)}</ItemDescription>
            {record.artifact.warnings.map((warning, i) => <small className="text-warning" key={i}>{warning}</small>)}
          </ItemContent>
          <ItemActions className="col-span-2 flex-wrap sm:col-span-1">
            <Badge tone={record.artifact.integrity_verified ? 'success' : 'warning'}>{record.artifact.integrity_verified ? '完整性已校验' : '需人工审核'}</Badge>
            <Button variant="outline" disabled={!writable || busy} onClick={() => {
              setInstall(record); setGameId(''); setModType(''); setReplaceInstallId(''); setFomodPreview(null); setFomodSelections({}); setError(null);
            }}><PackagePlusIcon data-icon="inline-start" aria-hidden="true"/>安装预览</Button>
          </ItemActions>
        </Item>)}
        {!artifacts.data?.length && !task && <EmptyState title="暂无下载记录"/>}
      </CardContent>
    </Card>
    {install && <Modal title="下载归档安装预览" onClose={() => { if (!busy) { setInstall(null); setFomodPreview(null); setFomodSelections({}); } }} actions={<>
      <Button variant="outline" disabled={busy} onClick={() => { setInstall(null); setFomodPreview(null); setFomodSelections({}); }}>取消</Button>
      <Button aria-busy={busy} disabled={!canPreviewInstall} onClick={() => { void previewInstall(); }}>
        {busy && <Spinner data-icon="inline-start"/>}生成变更预览
      </Button>
    </>}>
      <ErrorState error={error}/>
      <ErrorState error={adapter.error}/>
      <div className="flex min-w-0 flex-col gap-1 [overflow-wrap:anywhere]">
        <p>{install.name} · {install.version}</p>
        <PathText>{install.artifact.filename}</PathText>
      </div>
      <FieldGroup className="grid min-w-0 gap-3 sm:grid-cols-2">
        <Field className="min-w-0 gap-1.5 [&>[data-slot=native-select-wrapper]]:w-full">
          <FieldLabel htmlFor={`${fieldId}-game`}>目标游戏</FieldLabel>
          <NativeSelect id={`${fieldId}-game`} aria-label="目标游戏" value={gameId} disabled={busy}
            onChange={e => { setGameId(e.target.value); setModType(''); setReplaceInstallId(''); setFomodPreview(null); setFomodSelections({}); setError(null); }}>
            <NativeSelectOption value="">选择已登记游戏</NativeSelectOption>
            {games.data?.map(game => <NativeSelectOption key={game.id} value={game.id}>{game.name}</NativeSelectOption>)}
          </NativeSelect>
        </Field>
        <Field className="min-w-0 gap-1.5 [&>[data-slot=native-select-wrapper]]:w-full">
          <FieldLabel htmlFor={`${fieldId}-mod-type`}>适配包 Mod 类型</FieldLabel>
          <NativeSelect id={`${fieldId}-mod-type`} aria-label="适配包 Mod 类型" value={modType} disabled={busy || !gameId || !adapter.data}
            onChange={e => { setModType(e.target.value); setFomodPreview(null); setFomodSelections({}); setError(null); }}>
            <NativeSelectOption value="">选择 Mod 类型</NativeSelectOption>
            {adapter.data?.modTypes.map(type => <NativeSelectOption key={type.id} value={type.id}>{type.name}</NativeSelectOption>)}
          </NativeSelect>
        </Field>
      </FieldGroup>
      {modTypeNotes && <p className="text-sm text-warning [overflow-wrap:anywhere]">{modTypeNotes}</p>}
      <Button variant="outline" className="self-start" disabled={!writable || busy} aria-busy={fomodPreviewing.current} onClick={() => { void previewFomod(); }}>
        {fomodPreviewing.current ? <Spinner data-icon="inline-start"/> : <PackagePlusIcon data-icon="inline-start" aria-hidden="true"/>}
        扫描 FOMOD 配置
      </Button>
      {fomodPreview && <FomodOptions preview={fomodPreview} selections={fomodSelections} disabled={busy} onChange={setFomodSelections}/>}
      {gameId && <>
        {installs.isFetching && !busy && <p className="flex items-center gap-2 text-sm text-muted-foreground" role="status"><Spinner/>正在读取已装版本…</p>}
        <ErrorState error={installs.error}/>
        {installs.isError && <Button variant="outline" className="self-start" disabled={busy || installs.isFetching}
          onClick={() => { setError(null); void installs.refetch(); }}><RefreshCwIcon data-icon="inline-start" aria-hidden="true"/>重试读取已装版本</Button>}
        {showInstallOptions && options.ambiguousNexus && <p className="text-sm text-warning" role="status">此 Nexus 项目与旧记录的数字 ID 相同，但旧 ID 不含游戏域名，无法确认是否为同一项目。请先检查旧安装。</p>}
        {showInstallOptions && !options.ambiguousNexus && options.duplicateProject && <p className="text-sm text-warning" role="status">该项目有多个有效安装记录，请先处理重复版本，才能替换。</p>}
        {showInstallOptions && !options.ambiguousNexus && !options.duplicateProject && options.sameVersion && <p className="text-sm text-warning" role="status">该项目的此版本已安装，不能重复安装。</p>}
        {showInstallOptions && options.candidate && <>
          <p className="text-sm text-warning" role="status">该项目已有其他版本。请明确选择要替换的版本，不会同时安装副本。</p>
          <Field className="min-w-0 gap-1.5 [&>[data-slot=native-select-wrapper]]:w-full">
            <FieldLabel htmlFor={`${fieldId}-replacement`}>要替换的已装版本</FieldLabel>
            <NativeSelect id={`${fieldId}-replacement`} aria-label="要替换的已装版本" value={selectedReplacement?.id ?? ''} disabled={busy}
              onChange={e => { setReplaceInstallId(e.target.value); setError(null); }}>
              <NativeSelectOption value="">选择要替换的版本</NativeSelectOption>
              <NativeSelectOption value={options.candidate.id}>{options.candidate.name} · {options.candidate.version ?? '未标注版本'} · 版本 ID：{options.candidate.downloadOrigin?.versionId} · {date(options.candidate.installedAt)}</NativeSelectOption>
            </NativeSelect>
          </Field>
          {selectedReplacement?.downloadOrigin && <PathText>已选版本 ID：{selectedReplacement.downloadOrigin.versionId}</PathText>}
          {selectedReplacement && options.originalType !== modType && <p className="text-sm text-warning" role="status">
            {options.originalType ? `原版本使用「${originalTypeName}」类型，请选择相同的 Mod 类型。` : '原版本未记录 Mod 类型，不能替换。'}
          </p>}
        </>}
        {showInstallOptions && !options.ambiguousNexus && !options.duplicateProject && !options.sameVersion && !options.candidate && (options.ordinary
          ? !replaceInstallId && <p className="text-sm text-muted-foreground">普通安装</p>
          : <p className="text-sm text-warning" role="status">该项目已有安装记录，但当前状态不允许替换。请先检查已装 Mod。</p>)}
        {installsReady && options.ordinary && replaceInstallId && <Button variant="outline" className="self-start" disabled={busy}
          onClick={() => { setReplaceInstallId(''); setError(null); }}>清除已失效的替换选择</Button>}
      </>}
      {install.sourceId === 'modrinth' && games.data?.some(game => game.id === gameId) && <DependencyReview
        key={JSON.stringify([install.artifact.id, install.sourceId, install.projectId, install.versionId, gameId, selectedReplacement?.id])}
        install={install} gameId={gameId} replaceInstallId={selectedReplacement?.id ?? null} disabled={busy}
      />}
      {install.sourceId === 'modrinth' && games.data?.some(game => game.id === gameId && game.adapterSlug === 'minecraft-java') && <ModrinthGraphReview
        key={JSON.stringify([install.artifact.id, install.projectId, install.versionId, gameId, selectedReplacement?.id])}
        scope={{ artifactId: install.artifact.id, projectId: install.projectId, versionId: install.versionId, gameId, replaceInstallId: selectedReplacement?.id ?? null }} disabled={busy}
      />}
      {install.artifact.warnings.map((warning, i) => <p className="text-sm text-warning [overflow-wrap:anywhere]" key={i}>{warning}</p>)}
    </Modal>}
  </Page>;
}
