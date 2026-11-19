import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { listen } from '@tauri-apps/api/event';
import { openUrl } from '@tauri-apps/plugin-opener';
import { ArrowLeft, ArrowRight, Download, ExternalLink, ListChecks, LoaderCircle, Package, PackagePlus, RefreshCw, Search, X } from 'lucide-react';
import { api, backendAvailable, useBackendQuery } from '../lib/api';
import { dependencyLabels, dependencyOutcome, dependencyReasons, type Dependency, type DependencyCheck } from '../lib/mod-dependencies';
import { useUI } from '../lib/store';
import { initialFomodSelections, selectedFomodMappings, validFomodSelection, type FomodPreview, type FomodSelections } from '../lib/fomod';
import { FomodOptions } from '../components/FomodOptions';
import type { FilePlan, Game } from '../lib/types';
import { ModrinthGraphReview } from '../components/ModrinthGraphReview';
import { Badge, Button, bytes, date, EmptyState, ErrorState, IconButton, Modal, PageHeader } from '../components/ui';

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
  return <div className="path-text">
    {dependency.project_id && <div>项目 {dependency.project_id}</div>}
    {dependency.version_id && <div>版本 {dependency.version_id}</div>}
    {dependency.file_name && <div>{dependency.file_name}</div>}
  </div>;
}

function DependencyDetails({ dependencies }: { dependencies: Dependency[] | null }) {
  if (!dependencies?.length) {
    return <p className="source-dependencies">{dependencies ? '来源未声明直接依赖' : '来源依赖未知'}</p>;
  }
  return <details className="source-dependencies">
    <summary>来源声明的直接依赖（{dependencies.length}）</summary>
    <ul>
      {dependencies.map((dependency, index) => (
        <li key={index}>
          <Badge tone={dependency.dependency_type === 'incompatible' ? 'error' : dependency.dependency_type === 'required' ? 'warning' : undefined}>{dependencyLabels[dependency.dependency_type]}</Badge>
          <div className="grow"><DependencyReference dependency={dependency}/></div>
        </li>
      ))}
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
  return <section className="source-dependencies" aria-label="已装依赖记录核对">
    <Button disabled={disabled || pending} aria-busy={pending} aria-describedby={scopeId} onClick={() => { void check(); }}>
      {pending ? <LoaderCircle size={15} className="spin" aria-hidden="true"/> : <ListChecks size={15} aria-hidden="true"/>}
      {state.status === 'error' ? '重试核对已装依赖' : '核对已装依赖'}
    </Button>
    <p className="muted" id={scopeId} style={{ marginTop: 8 }}>仅核对目标游戏的管理器 Modrinth 安装记录，不含其他来源或本地安装。不查文件是否存在、校验和或加载器兼容性；仅供参考，不阻止安装。</p>
    <p className="muted" role="status">
      {pending ? '正在核对已装依赖记录…' : checks === null ? '该版本的直接依赖声明未知。' : checks?.length === 0 ? '该版本未声明直接依赖。'
        : checks && `已核对 ${checks.length} 条直接依赖声明。${report?.excludedInstallId ? '本次核对已排除所选待替换记录。' : ''}`}
    </p>
    <ErrorState error={state.status === 'error' ? state.error : null}/>
    {!!checks?.length && <ul role="list" aria-label="直接依赖核对结果">{checks.map((entry, index) => {
      const { dependency, reason, matches, matchCount } = entry;
      const outcome = dependencyOutcome(entry);
      return <li key={index}>
        <Badge>{dependencyLabels[dependency.dependency_type]}</Badge>
        <div className="grow">
          <Badge tone={outcome.tone}>{outcome.label}</Badge>
          <DependencyReference dependency={dependency}/>
          {reason && <p className="muted">{dependencyReasons[reason]}</p>}
          {matchCount > 0 && <p className="muted">共 {matchCount} 条相关记录{matchCount > matches.length && `（显示前 ${matches.length} 条）`}</p>}
          {matches.map(match => <p className="path-text" key={match.installId}>
            记录 {match.installId} · 项目 {match.projectId} · 版本 {match.versionId} · {match.status === 'enabled' ? '事务已部署' : match.status === 'partially_restored' ? '部分恢复' : match.status}
          </p>)}
        </div>
      </li>;
    })}</ul>}
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
  return <div className="page discover-page" data-testid="discover-page">
    <PageHeader title="发现 Mod" actions={<IconButton title="刷新来源状态" disabled={!backendAvailable} onClick={() => { void sources.refetch(); }}><RefreshCw size={16}/></IconButton>}/>
    <form className="toolbar" onSubmit={e => { e.preventDefault(); if (backendAvailable && !busy) void search(); }}><label className="search-field grow"><Search size={17}/><input aria-label="搜索 Mod" placeholder="搜索 Mod…" value={query} onChange={e => setQuery(e.target.value)} maxLength={512}/></label><select aria-label="Mod 来源" value={source} onChange={e => { setSource(e.target.value); setProviderGame(''); }}><option value="">所有来源</option>{(sources.data ?? []).map(item => <option value={item.source_id} key={item.source_id}>{sourceNames[item.source_id] ?? item.source_id}</option>)}</select><Button type="submit" className="primary" disabled={!backendAvailable || busy}><Search size={16}/>{busy ? '请求中…' : '搜索'}</Button></form>
    <div className="toolbar"><input aria-label="来源游戏 ID" placeholder="来源游戏 ID" disabled={!source} value={providerGame} onChange={e => setProviderGame(e.target.value)}/><input aria-label="游戏版本" placeholder="游戏版本" value={gameVersion} onChange={e => updateFilter(e.target.value, gameVersion, setGameVersion)}/><input aria-label="Mod 加载器" placeholder="加载器" value={loader} onChange={e => updateFilter(e.target.value, loader, setLoader)}/></div>
    <div className="source-statuses">{statuses.map(item => <span title={item.message} key={item.source_id}><Badge tone={item.state === 'available' ? 'success' : 'warning'}>{sourceNames[item.source_id] ?? item.source_id} · {sourceStates[item.state] ?? item.state}</Badge></span>)}</div>
    <ErrorState error={sources.error}/><ErrorState error={error}/>
    <div className="split-layout" style={{ marginTop: 22 }}><section><div className="result-grid">{results?.mods.map(mod => <button className={`mod-result ${selection === mod ? 'selected' : ''}`} key={`${mod.source_id}:${mod.project_id}`} onClick={() => selectProject(mod)} disabled={busy}><div className="mod-thumbnail">{https(mod.icon_url) ? <img alt="" src={https(mod.icon_url)!} loading="lazy" referrerPolicy="no-referrer"/> : <Package size={27}/>}</div><div className="grow"><h2>{mod.name}</h2><p>{mod.summary}</p><small>{mod.author ?? '—'} · {sourceNames[mod.source_id] ?? mod.source_id}{mod.downloads !== null ? ` · ${mod.downloads.toLocaleString('zh-CN')} 次下载` : ''}</small></div></button>)}</div>{!results?.mods.length && <EmptyState title={!backendAvailable ? '桌面能力未连接' : results ? '没有匹配的 Mod' : '尚未搜索'}/>}{results && <div className="toolbar" style={{ marginTop: 18 }}><IconButton title="上一页" disabled={busy || offset === 0} onClick={() => { void search(Math.max(0, offset - 20)); }}><ArrowLeft size={16}/></IconButton><span className="muted">第 {offset / 20 + 1} 页</span><IconButton title="下一页" disabled={busy || !results.mods.length || offset >= 10000} onClick={() => { void search(offset + 20); }}><ArrowRight size={16}/></IconButton></div>}</section>
      <aside className="inspector">
        <h2>{selection?.name ?? 'Mod 详情'}</h2>
        {selection ? <>
          <p>{selection.summary}</p>
          <dl className="metadata-list" style={{ marginTop: 18 }}>
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
          {resolution && <p className="warning-text" style={{ marginTop: 16 }}>{resolution.status.message}</p>}
          {resolution?.download?.warnings.map((warning, i) => <p className="warning-text" key={i}>{warning}</p>)}
          {resolutionState?.state === 'loading' && <p role="status">{selectedVersionId ? '正在解析所选版本…' : '正在解析默认版本…'}</p>}
          <ErrorState error={resolutionState?.state === 'error' ? resolutionState.error : null}/>
          <div className="toolbar" style={{ marginTop: 20 }}>
            <Button disabled={!https(resolution?.homepage_url ?? selection.homepage_url)} onClick={() => { void openHomepage(resolution?.homepage_url ?? selection.homepage_url); }}><ExternalLink size={15}/>原始页面</Button>
            {resolution?.download && <Button className="primary" disabled={resolutionState?.state !== 'ready' || busy || !writable || !listenerReady || !!task} onClick={() => { void download(); }}><Download size={15}/>下载归档</Button>}
            {(['modrinth', 'thunderstore'] as const).includes(selection.source_id as 'modrinth' | 'thunderstore') && <Button disabled={busy} aria-expanded={!!versionPage} aria-controls={versionPage ? versionPanelId : undefined}
              onClick={() => { if (versionPage) closeVersions(); else void listVersions(projectRequest(selection)); }}>
              <ListChecks size={15}/>{versionPage ? '收起来源版本' : '查看来源版本'}
            </Button>}
            {!resolutionState && <Button disabled={busy} onClick={() => { void resolve(projectRequest(selection)); }}><RefreshCw size={15}/>解析默认版本</Button>}
            {resolutionState?.state === 'error' && <Button disabled={busy} onClick={() => { void resolve(resolutionState.request, resolutionState.versionId); }}>
              <RefreshCw size={15}/>{selectedVersionId ? '重试解析所选版本' : '重试解析默认版本'}
            </Button>}
          </div>
          {versionPage && <section className="source-dependencies" id={versionPanelId} aria-label="来源版本">
            {versionPage.state === 'loading' && <p role="status">正在读取来源版本…</p>}
            {versionPage.state === 'error' && <>
              <ErrorState error={versionPage.error}/>
              <Button disabled={busy} onClick={() => { void listVersions(versionPage.request, versionPage.offset); }}><RefreshCw size={15}/>重试读取来源版本</Button>
            </>}
            {versionPage.state === 'ready' && versionPage.result.status.state === 'limited' && <p className="warning-text" role="status">{versionPage.result.status.message}</p>}
            {versionPage.state === 'ready' && (versionPage.result.versions.length ? <ul role="list" aria-label={`${versionPage.request.source_id} 版本`}>
              {versionPage.result.versions.map(version => {
                const selected = selectedVersionId === version.version_id;
                return <li key={version.version_id}>
                  <div className="grow" style={{ overflowWrap: 'anywhere' }}>
                    <strong>{version.name}</strong>
                    <p>版本：{version.version}</p>
                    <p className="path-text">版本 ID：{version.version_id}</p>
                    <Badge tone={version.release_channel === 'release' ? 'success' : 'warning'}>{channelLabels[version.release_channel]}</Badge>
                    <p>发布日期：<time dateTime={version.published_at}>{date(version.published_at)}</time></p>
                    {!versionPage.request.loader && <p>来源声明加载器：{sourceListSummary(version.loaders)}</p>}
                    {!versionPage.request.game_version && <p>来源声明游戏版本：{sourceListSummary(version.game_versions)}</p>}
                  </div>
                  <Button aria-label={`选择版本 ${version.version}（${version.version_id}）`} aria-pressed={selected}
                    style={selected ? { background: 'var(--accent-soft)', color: 'var(--accent)', borderColor: 'var(--accent)' } : undefined}
                    disabled={busy || (selected && resolutionState?.state === 'loading')}
                    onClick={() => { void resolve(versionPage.request, version.version_id); }}>选择版本</Button>
                </li>;
              })}
            </ul> : <p role="status">没有符合当前筛选条件的来源版本。</p>)}
            <div className="toolbar" style={{ marginTop: 12 }}>
              <IconButton title="上一页来源版本" disabled={busy || versionPage.state === 'loading' || versionPage.offset === 0}
                onClick={() => { void listVersions(versionPage.request, versionPage.offset - versionPageSize); }}><ArrowLeft size={16}/></IconButton>
              <span className="muted" aria-live="polite">第 {versionPage.offset / versionPageSize + 1} 页</span>
              <IconButton title="下一页来源版本" disabled={busy || !hasMoreVersions}
                onClick={() => { void listVersions(versionPage.request, versionPage.offset + versionPageSize); }}><ArrowRight size={16}/></IconButton>
            </div>
          </section>}
        </> : <p>未选择 Mod</p>}
      </aside>
    </div>
    <section className="page-section"><div className="section-heading"><h2>下载记录</h2><IconButton title="刷新下载记录" disabled={!backendAvailable} onClick={() => { void artifacts.refetch(); }}><RefreshCw size={16}/></IconButton></div><ErrorState error={artifacts.error}/>{task && <div className="download-row"><Download size={18}/><div className="grow"><strong>{task.name}</strong><p>{phases[currentProgress?.phase ?? 'connecting']} · {bytes(currentProgress?.downloaded ?? 0)}{currentProgress?.total != null ? ` / ${bytes(currentProgress.total)}` : ''}</p>{currentProgress?.total != null && <progress max={currentProgress.total} value={currentProgress.downloaded}/>}</div><IconButton title="取消下载" onClick={() => { void api('cancel_download', { taskId: task.id }).catch(setError); }}><X size={16}/></IconButton></div>}{(artifacts.data ?? []).map(record => <div className="download-row" key={record.artifact.id}><Package size={21}/><div className="grow"><strong>{record.name} <small>{record.version}</small></strong><p>{record.artifact.filename} · {bytes(record.artifact.size)} · {date(record.createdAt)}</p>{record.artifact.warnings.map((warning, i) => <small className="warning-text" key={i}>{warning}</small>)}</div><Badge tone={record.artifact.integrity_verified ? 'success' : 'warning'}>{record.artifact.integrity_verified ? '完整性已校验' : '需人工审核'}</Badge><Button disabled={!writable || busy} onClick={() => { setInstall(record); setGameId(''); setModType(''); setReplaceInstallId(''); setFomodPreview(null); setFomodSelections({}); setError(null); }}><PackagePlus size={15}/>安装预览</Button></div>)}{!artifacts.data?.length && !task && <EmptyState title="暂无下载记录"/>}</section>
    {install && <Modal title="下载归档安装预览" onClose={() => { if (!busy) { setInstall(null); setFomodPreview(null); setFomodSelections({}); } }} actions={<><Button disabled={busy} onClick={() => { setInstall(null); setFomodPreview(null); setFomodSelections({}); }}>取消</Button><Button className="primary" aria-busy={busy} disabled={!canPreviewInstall} onClick={() => { void previewInstall(); }}>{busy && <LoaderCircle size={15} className="spin" aria-hidden="true"/>}生成变更预览</Button></>}>
      <ErrorState error={error}/><ErrorState error={adapter.error}/><p>{install.name} · {install.version}</p><p className="path-text">{install.artifact.filename}</p>
      <label className="field" style={{ marginTop: 20 }}>目标游戏<select aria-label="目标游戏" value={gameId} disabled={busy} onChange={e => { setGameId(e.target.value); setModType(''); setReplaceInstallId(''); setFomodPreview(null); setFomodSelections({}); setError(null); }}><option value="">选择已登记游戏</option>{games.data?.map(game => <option key={game.id} value={game.id}>{game.name}</option>)}</select></label>
      <label className="field">适配包 Mod 类型<select aria-label="适配包 Mod 类型" value={modType} disabled={busy || !gameId || !adapter.data} onChange={e => { setModType(e.target.value); setFomodPreview(null); setFomodSelections({}); setError(null); }}><option value="">选择 Mod 类型</option>{adapter.data?.modTypes.map(type => <option key={type.id} value={type.id}>{type.name}</option>)}</select></label>
      {adapter.data?.modTypes.find(type => type.id === modType)?.notes && <p className="warning-text">{adapter.data.modTypes.find(type => type.id === modType)?.notes}</p>}
      <div className="toolbar" style={{ marginTop: 18 }}><Button disabled={!writable || busy} aria-busy={fomodPreviewing.current} onClick={() => { void previewFomod(); }}><PackagePlus size={15}/>扫描 FOMOD 配置</Button></div>
      {fomodPreview && <FomodOptions preview={fomodPreview} selections={fomodSelections} disabled={busy}
        onChange={setFomodSelections}/>}
      {gameId && <>
        {installs.isFetching && !busy && <p className="muted" role="status">正在读取已装版本…</p>}
        <ErrorState error={installs.error}/>
        {installs.isError && <Button disabled={busy || installs.isFetching} onClick={() => { setError(null); void installs.refetch(); }}><RefreshCw size={15}/>重试读取已装版本</Button>}
        {showInstallOptions && options.ambiguousNexus && <p className="warning-text" role="status">此 Nexus 项目与旧记录的数字 ID 相同，但旧 ID 不含游戏域名，无法确认是否为同一项目。请先检查旧安装。</p>}
        {showInstallOptions && !options.ambiguousNexus && options.duplicateProject && <p className="warning-text" role="status">该项目有多个有效安装记录，请先处理重复版本，才能替换。</p>}
        {showInstallOptions && !options.ambiguousNexus && !options.duplicateProject && options.sameVersion && <p className="warning-text" role="status">该项目的此版本已安装，不能重复安装。</p>}
        {showInstallOptions && options.candidate && <>
          <p className="warning-text" role="status" style={{ marginBottom: 12 }}>该项目已有其他版本。请明确选择要替换的版本，不会同时安装副本。</p>
          <label className="field">要替换的已装版本<select aria-label="要替换的已装版本" style={{ width: '100%', minWidth: 0 }}
            value={selectedReplacement?.id ?? ''} disabled={busy}
            onChange={e => { setReplaceInstallId(e.target.value); setError(null); }}>
            <option value="">选择要替换的版本</option>
            <option value={options.candidate.id}>{options.candidate.name} · {options.candidate.version ?? '未标注版本'} · 版本 ID：{options.candidate.downloadOrigin?.versionId} · {date(options.candidate.installedAt)}</option>
          </select></label>
          {selectedReplacement?.downloadOrigin && <p className="path-text">已选版本 ID：{selectedReplacement.downloadOrigin.versionId}</p>}
          {selectedReplacement && options.originalType !== modType && <p className="warning-text" role="status">
            {options.originalType ? `原版本使用「${originalTypeName}」类型，请选择相同的 Mod 类型。` : '原版本未记录 Mod 类型，不能替换。'}
          </p>}
        </>}
        {showInstallOptions && !options.ambiguousNexus && !options.duplicateProject && !options.sameVersion && !options.candidate && (options.ordinary
          ? !replaceInstallId && <p className="muted">普通安装</p>
          : <p className="warning-text" role="status">该项目已有安装记录，但当前状态不允许替换。请先检查已装 Mod。</p>)}
        {installsReady && options.ordinary && replaceInstallId && <Button disabled={busy} onClick={() => { setReplaceInstallId(''); setError(null); }}>清除已失效的替换选择</Button>}
      </>}
      {install.sourceId === 'modrinth' && games.data?.some(game => game.id === gameId) && <DependencyReview
        key={JSON.stringify([install.artifact.id, install.sourceId, install.projectId, install.versionId, gameId, selectedReplacement?.id])}
        install={install} gameId={gameId} replaceInstallId={selectedReplacement?.id ?? null} disabled={busy}
      />}
      {install.sourceId === 'modrinth' && games.data?.some(game => game.id === gameId && game.adapterSlug === 'minecraft-java') && <ModrinthGraphReview
        key={JSON.stringify([install.artifact.id, install.projectId, install.versionId, gameId, selectedReplacement?.id])}
        scope={{ artifactId: install.artifact.id, projectId: install.projectId, versionId: install.versionId, gameId, replaceInstallId: selectedReplacement?.id ?? null }} disabled={busy}
      />}
      {install.artifact.warnings.map((warning, i) => <p className="warning-text" key={i}>{warning}</p>)}
    </Modal>}
  </div>;
}
