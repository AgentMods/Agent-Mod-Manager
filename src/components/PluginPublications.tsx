import { useEffect, useEffectEvent, useRef, useState } from 'react';
import { useInfiniteQuery, useQueryClient } from '@tanstack/react-query';
import { CheckCircle2Icon, CircleAlertIcon, EyeIcon, RefreshCwIcon } from 'lucide-react';
import { api, backendAvailable } from '@/lib/api';
import { Badge, bytes, date, EmptyState, ErrorState, Modal, PathText } from './ui';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { PublicationListingPreview, type PublicationListing } from './PublicationListing';
import { Separator } from '@/components/ui/separator';
import { Spinner } from '@/components/ui/spinner';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';

type PublicationInput = { id?: string; slug: string };
type Target = { kind: 'first' } | { kind: 'release'; pluginId: number };
type PublicationFile = { name: string; size: number; sha256: string; text: string };
type RecordState = 'intended' | 'pending' | 'unknown' | 'completed' | 'error';
type PublicationSource = { kind: 'draft'; draftId: string; writeTransactionId: string } | { kind: 'installed'; slug: string; source: string; deploymentTransactionId: string | null };
type PublishedPlugin = { id: number; releaseId: number; slug: string; name: string; version: string; tags: string[]; hidden: boolean; reason: string };
type RecordBase = { id: string; state: RecordState; createdAt: string; updatedAt: string };
export type PublicationRecord = RecordBase & {
  preview: { digest: string; listing?: PublicationListing | null; remoteVersion?: string | null; originalVersion?: string | null; source: PublicationSource; target: Target; slug: string; version: string; tags: string[]; destination: string; files: PublicationFile[] };
  approvedDigest: string | null; remote: PublishedPlugin | null;
};
type LegacyRecord = RecordBase & {
  phase: string; fileId: number | null; workId: number | null;
  preview: { digest: string; source?: PublicationSource; package: {
    title: string; slug: string; version: string; summary: string; description: string; tags: string[]; gameIds: number[];
    archiveName: string; archiveSize: number; archiveSha256: string; workStatus: string; destination: string; files: PublicationFile[];
  } };
};
type HistoryRecord = PublicationRecord | LegacyRecord;
type Reconciliation = 'verified' | 'unconfirmed';
const isLegacy = (record: HistoryRecord): record is LegacyRecord => 'phase' in record && 'package' in record.preview;
const targetName = (target: Target) => target.kind === 'first' ? '首次发布' : '发布新版本';
const legacyPhases: Record<string, string> = {
  prepared: '待确认', createUploadPending: '正在创建上传', uploadCreated: '上传已创建', startUploadPending: '正在启动上传',
  uploadStarted: '等待传输', partPutPending: '正在传输', partUploaded: '文件已传输', completeUploadPending: '正在完成上传',
  awaitingScan: '等待安全扫描', uploadReady: '文件已就绪', publishWorkPending: '正在发布作品', completed: '已发布',
};
function status(record: HistoryRecord) {
  if (record.state === 'unknown') return '结果待核对';
  if (record.state === 'error') return '发布失败';
  if (isLegacy(record)) return legacyPhases[record.phase] ?? record.phase;
  if (record.state === 'completed') {
    if (record.remote?.hidden) return record.preview.target.kind === 'release' ? '新版已提交，插件仍已下架' : '已提交，插件已下架';
    return '已发布';
  }
  return record.state === 'pending' ? '正在发布' : record.preview.listing ? '待确认' : '历史预览';
}
function StatusBadge({record}: {record: HistoryRecord}) {
  const tone = record.state === 'error' ? 'error' : record.state === 'unknown' || record.state === 'pending' || (!isLegacy(record) && record.remote?.hidden) ? 'warning' : record.state === 'completed' ? 'success' : 'neutral';
  return <Badge tone={tone}>{status(record)}</Badge>;
}
function canReconcile(record: HistoryRecord) {
  if (!isLegacy(record)) return record.state === 'unknown';
  return record.fileId !== null && ((record.state === 'intended' && record.phase === 'awaitingScan') ||
    (record.state === 'unknown' && ['completeUploadPending', 'publishWorkPending'].includes(record.phase)));
}
function Files({files}: {files: PublicationFile[]}) {
  return <section aria-label="文件原文" className="flex min-w-0 flex-col gap-2">
    {files.map(file => <details key={file.name} className="min-w-0 rounded-md border px-3 py-2 text-sm">
      <summary className="cursor-pointer font-medium"><span className="mx-1">{file.name}</span> <span className="text-xs font-normal text-muted-foreground">{bytes(file.size)}</span></summary>
      <PathText className="mt-2 block">SHA-256 {file.sha256}</PathText>
      <pre className="mt-2 max-h-72 overflow-auto rounded-md bg-muted p-3 font-mono text-xs break-all whitespace-pre-wrap">{file.text}</pre>
    </details>)}
  </section>;
}

function PrepareDialog({draft, writable, onClose, onPrepared}: {
  draft: PublicationInput; writable: boolean; onClose: () => void; onPrepared: (record: PublicationRecord) => void;
}) {
  const [attempt, setAttempt] = useState(0);
  const [error, setError] = useState<unknown>(null);
  const prepared = useEffectEvent(onPrepared);
  const {id, slug} = draft;

  useEffect(() => {
    if (!writable) return;
    let active = true;
    // StrictMode replays mount effects synchronously. Only the live opening starts a request.
    queueMicrotask(async () => {
      if (!active) return;
      try {
        const record = await api<PublicationRecord>('prepare_auto_manager_publication', id === undefined ? {slug} : {draftId: id});
        if (!record.preview.listing) throw new Error('未收到完整的插件发布预览，请重试。');
        if (active) prepared(record);
      } catch (err) {
        if (active) setError(err);
      }
    });
    return () => { active = false; };
  }, [id, slug, writable, attempt]);

  return <Modal className="sm:max-w-[600px]" title="发布插件" description={`Mod客栈 · ${slug}`} onClose={onClose} actions={<>
    <Button variant="outline" onClick={onClose}>取消</Button>
    {!!error && <Button disabled={!writable} onClick={() => { setError(null); setAttempt(attempt + 1); }}>重试</Button>}
  </>}>
    {error ? <ErrorState error={error}/> : <div role="status" className="flex items-center gap-3 py-4 text-sm text-muted-foreground">
      <Spinner/>正在核对当前账号的插件，并生成发布内容…
    </div>}
  </Modal>;
}

function LegacyMetadata({record}: {record: LegacyRecord}) {
  const pack = record.preview.package;
  return <dl className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 text-sm">
    <dt className="text-muted-foreground">标题</dt><dd>{pack.title}</dd>
    <dt className="text-muted-foreground">摘要</dt><dd className="break-words whitespace-pre-wrap">{pack.summary || '—'}</dd>
    <dt className="text-muted-foreground">描述</dt><dd className="break-words whitespace-pre-wrap">{pack.description || '—'}</dd>
    <dt className="text-muted-foreground">公开状态</dt><dd>{pack.workStatus}</dd>
    <dt className="text-muted-foreground">游戏 ID</dt><dd>{pack.gameIds.join('、') || '—'}</dd>
    <dt className="text-muted-foreground">ZIP 文件</dt><dd className="break-all">{pack.archiveName} ({bytes(pack.archiveSize)})</dd>
    <dt className="text-muted-foreground">ZIP SHA-256</dt><dd><PathText>{pack.archiveSha256}</PathText></dd>
    <dt className="text-muted-foreground">发布记录 ID</dt><dd><PathText>{record.id}</PathText></dd>
    <dt className="text-muted-foreground">远端标识</dt><dd>文件 {record.fileId ?? '—'} / 作品 {record.workId ?? '—'}</dd>
  </dl>;
}

function ReviewDialog({record, writable, onClose, onChange, reload}: {
  record: HistoryRecord; writable: boolean; onClose: () => void; onChange: (record: HistoryRecord) => void;
  reload: (record: HistoryRecord) => Promise<HistoryRecord>;
}) {
  const inFlight = useRef(false);
  const [activity, setActivity] = useState<'publish' | 'reconcile' | 'refresh' | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [needsRefresh, setNeedsRefresh] = useState(false);
  const [reconciliation, setReconciliation] = useState<Reconciliation | null>(null);
  const busy = activity !== null;
  const legacy = isLegacy(record);
  const pack = legacy ? record.preview.package : record.preview;
  const target = legacy ? null : record.preview.target;
  const listing = legacy ? null : record.preview.listing;
  const publishable = !!listing && record.state === 'intended' && !needsRefresh;
  const previousVersion = !legacy && target?.kind === 'release' ? record.preview.remoteVersion : null;
  const heading = legacy ? 'Mod客栈 · 旧版普通作品' : `Mod客栈 · ${targetName(record.preview.target)}　${previousVersion ? `${previousVersion} → ` : ''}${pack.version}`;

  async function refresh() {
    try { onChange(await reload(record)); setNeedsRefresh(false); }
    catch (err) { setNeedsRefresh(true); throw err; }
  }
  async function act(action: 'publish' | 'reconcile' | 'refresh') {
    if (inFlight.current || (action !== 'refresh' && !writable)) return;
    if (action === 'publish' && !publishable) return;
    if (action === 'reconcile' && !canReconcile(record)) return;
    inFlight.current = true;
    setActivity(action); setError(null); setReconciliation(null);
    try {
      if (action === 'publish') {
        let result: PublicationRecord;
        try {
          result = await api<PublicationRecord>('publish_manager_publication', {id: record.id, digest: record.preview.digest, confirmation: 'PUBLISH_PLUGIN'});
        } catch (err) {
          // The request could have reached the server. Read the durable state before enabling another approval.
          await refresh(); throw err;
        }
        onChange(result);
      } else if (action === 'reconcile') {
        const outcome = await api<Reconciliation>(legacy ? 'reconcile_plugin_publication' : 'reconcile_manager_publication', {id: record.id});
        await refresh(); setReconciliation(outcome);
      } else { await refresh(); }
    } catch (err) { setError(err); } finally { inFlight.current = false; setActivity(null); }
  }
  const metadata = <dl className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 text-sm">
    {!legacy && <>
      <dt className="text-muted-foreground">来源</dt><dd className="wrap-anywhere">{record.preview.source.kind === 'draft' ? `草稿 · ${record.preview.source.draftId}` : `已安装适配包 · ${record.preview.source.slug}`}</dd>
      <dt className="text-muted-foreground">适配包</dt><dd className="wrap-anywhere">{pack.slug}</dd>
      {record.preview.originalVersion && <><dt className="text-muted-foreground">本地版本</dt><dd>{record.preview.originalVersion}</dd></>}
      <dt className="text-muted-foreground">目标插件</dt><dd>{target?.kind === 'release' ? `#${target.pluginId}` : '新建管理器插件'}</dd>
    </>}
    {!listing && <><dt className="text-muted-foreground">版本标签</dt><dd className="wrap-anywhere">{pack.tags.join('、') || '无标签'}</dd></>}
    <dt className="text-muted-foreground">发布地址</dt><dd><PathText>{pack.destination}</PathText></dd>
    <dt className="text-muted-foreground">审批摘要</dt><dd><PathText>{record.preview.digest}</PathText></dd>
  </dl>;
  return <Modal className={legacy ? undefined : 'sm:max-w-[600px]'} title={legacy ? '旧版作品发布记录' : '发布插件'} description={heading} onClose={() => { if (!inFlight.current) onClose(); }} actions={<>
    <Button variant="outline" disabled={busy} onClick={onClose}>{publishable ? '取消' : '关闭'}</Button>
    {needsRefresh && <Button variant="outline" disabled={busy} onClick={() => void act('refresh')}>{activity === 'refresh' ? <Spinner/> : <RefreshCwIcon data-icon="inline-start"/>}刷新发布状态</Button>}
    {canReconcile(record) && !needsRefresh && <Button variant="outline" disabled={busy || !writable} onClick={() => void act('reconcile')}>{activity === 'reconcile' ? <Spinner/> : <RefreshCwIcon data-icon="inline-start"/>}核对远端状态</Button>}
    {publishable && <Button disabled={busy || !writable} onClick={() => void act('publish')}>{activity === 'publish' && <Spinner/>}发布</Button>}
  </>}>
    {listing && <PublicationListingPreview listing={listing}/>}
    {(!listing || record.state !== 'intended') && <div className="flex flex-wrap items-center gap-2"><strong className="wrap-anywhere">{pack.slug}</strong><span className="text-sm">{pack.version}</span><StatusBadge record={record}/></div>}
    {!legacy && record.remote && <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 text-sm">
      <dt className="text-muted-foreground">远端插件 ID</dt><dd>{record.remote.id}</dd>
      <dt className="text-muted-foreground">远端版本 ID</dt><dd>{record.remote.releaseId}</dd>
      {record.remote.hidden && record.remote.reason && <><dt className="text-muted-foreground">下架原因</dt><dd className="wrap-anywhere">{record.remote.reason}</dd></>}
    </dl>}
    {!listing && metadata}
    {legacy && <><Separator/><LegacyMetadata record={record}/></>}
    <ErrorState error={error}/>
    {reconciliation && <Alert role="status" variant={reconciliation === 'verified' ? 'success' : 'warning'}>{reconciliation === 'verified' ? <CheckCircle2Icon/> : <CircleAlertIcon/>}<AlertDescription>{reconciliation === 'verified' ? '已核实远端状态' : '尚未核实，发布状态保持不变'}</AlertDescription></Alert>}
    {record.state === 'unknown' && !reconciliation && <Alert variant="warning"><CircleAlertIcon/><AlertDescription>{canReconcile(record) ? '远端可能已经收到本次发布，请先核对结果。' : '请在 Mod客栈核对该作品的发布结果。'}</AlertDescription></Alert>}
    {listing ? <details className="min-w-0 text-sm">
      <summary className="cursor-pointer font-medium text-muted-foreground">查看发布文件</summary>
      <div className="mt-3 flex flex-col gap-3">{metadata}<Files files={pack.files}/></div>
    </details> : <Files files={pack.files}/>}
  </Modal>;
}

function usePublicationPages<T extends HistoryRecord>(key: string, command: string, pageSize: number, enabled: boolean) {
  return useInfiniteQuery({
    queryKey: [key],
    initialPageParam: null as string | null,
    queryFn: ({pageParam}) => api<T[]>(command, {before: pageParam}),
    getNextPageParam: (rows) => rows.length === pageSize ? rows.at(-1)?.id : undefined,
    enabled: enabled && backendAvailable,
    retry: false,
  });
}

export function PluginPublications({draft, showHistory, writable, onDraftClose}: {
  draft: PublicationInput | null; showHistory: boolean; writable: boolean; onDraftClose: () => void;
}) {
  const client = useQueryClient();
  const manager = usePublicationPages<PublicationRecord>('manager-publications', 'list_manager_publications', 50, showHistory);
  const legacy = usePublicationPages<LegacyRecord>('plugin-publications', 'list_plugin_publications', 100, showHistory);
  const [selected, setSelected] = useState<HistoryRecord | null>(null);
  const history: HistoryRecord[] = [...manager.data?.pages.flat() ?? [], ...legacy.data?.pages.flat() ?? []].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  const loading = manager.isLoading || legacy.isLoading;
  async function reload(record: HistoryRecord) {
    return api<HistoryRecord>(isLegacy(record) ? 'get_plugin_publication' : 'get_manager_publication', {id: record.id});
  }
  function remember(record: HistoryRecord) {
    const legacy = isLegacy(record);
    const key = legacy ? ['plugin-publications'] : ['manager-publications'];
    void client.invalidateQueries({queryKey: key});
    setSelected(record);
    if (record.state !== 'completed') return;
    if (!legacy) void client.invalidateQueries({queryKey: ['manager-plugin-games']});
    if (record.preview.source?.kind === 'draft') {
      // Either publication flow can move a consumed Agent draft to Trash.
      void client.invalidateQueries({queryKey: ['plugin-drafts']});
      void client.invalidateQueries({queryKey: ['trash']});
    }
  }
  function closeReview() {
    setSelected(null);
    if (showHistory) { void manager.refetch(); void legacy.refetch(); }
  }
  return <>
    {showHistory && <section aria-label="适配包发布记录" className="flex min-w-0 flex-col gap-4">
      <div className="flex justify-end"><Button variant="outline" disabled={manager.isFetching || legacy.isFetching || !backendAvailable} onClick={() => { void manager.refetch(); void legacy.refetch(); }}><RefreshCwIcon/>刷新记录</Button></div>
      <ErrorState error={manager.error}/><ErrorState error={legacy.error}/>
      {history.length ? <div className="min-w-0 rounded-lg border"><Table>
        <TableHeader><TableRow><TableHead>适配包</TableHead><TableHead>发布状态</TableHead><TableHead>远端标识</TableHead><TableHead>更新时间</TableHead><TableHead><span className="sr-only">操作</span></TableHead></TableRow></TableHeader>
        <TableBody>{history.map(record => {
          const old = isLegacy(record);
          const title = old ? record.preview.package.title : record.preview.slug;
          const version = old ? record.preview.package.version : record.preview.version;
          return <TableRow key={record.id}>
            <TableCell className="whitespace-normal"><strong className="break-all font-medium">{title}</strong><span className="block text-xs text-muted-foreground">{version} · {old ? '旧版作品' : targetName(record.preview.target)}</span></TableCell>
            <TableCell><StatusBadge record={record}/></TableCell>
            <TableCell className="text-xs text-muted-foreground">{old ? <>文件 {record.fileId ?? '—'} / 作品 {record.workId ?? '—'}</> : <>插件 {record.remote?.id ?? (record.preview.target.kind === 'release' ? record.preview.target.pluginId : '—')} / 版本 {record.remote?.releaseId ?? '—'}</>}</TableCell>
            <TableCell>{date(record.updatedAt)}</TableCell>
            <TableCell><Button variant="outline" size="sm" aria-label={`查看发布 ${title}`} onClick={() => setSelected(record)}><EyeIcon/>查看</Button></TableCell>
          </TableRow>;
        })}</TableBody>
      </Table></div> : !manager.isError && !legacy.isError && <EmptyState title={loading ? '正在读取发布记录…' : '暂无发布记录'}/>}
      {(manager.hasNextPage || legacy.hasNextPage) && <Button variant="outline" disabled={manager.isFetching || legacy.isFetching} onClick={() => {
        if (manager.hasNextPage) void manager.fetchNextPage();
        if (legacy.hasNextPage) void legacy.fetchNextPage();
      }}>加载更早的发布记录</Button>}
    </section>}
    {draft && <PrepareDialog key={draft.id ?? `installed:${draft.slug}`} draft={draft} writable={writable} onClose={onDraftClose} onPrepared={record => { onDraftClose(); remember(record); }}/>}
    {selected && <ReviewDialog key={`${selected.id}-${selected.preview.digest}`} record={selected} writable={writable} onClose={closeReview} onChange={remember} reload={reload}/>}
  </>;
}
