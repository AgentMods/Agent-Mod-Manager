import { useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { CheckCircle2Icon, CircleAlertIcon, EyeIcon, FileTextIcon, RefreshCwIcon, UploadIcon } from 'lucide-react';
import { api, backendAvailable, useBackendQuery } from '@/lib/api';
import { Badge, bytes, date, EmptyState, ErrorState, Modal, PathText } from './ui';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Field, FieldDescription, FieldGroup, FieldLabel, FieldLegend, FieldSet } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select';
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group';
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
  preview: { digest: string; source: PublicationSource; target: Target; slug: string; version: string; tags: string[]; destination: string; files: PublicationFile[] };
  approvedDigest: string | null; remote: PublishedPlugin | null;
};
type LegacyRecord = RecordBase & {
  phase: string; fileId: number | null; workId: number | null;
  preview: { digest: string; package: {
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
  return record.state === 'pending' ? '正在发布' : '待确认';
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
  const [mode, setMode] = useState<Target['kind']>('first');
  const [pluginId, setPluginId] = useState('');
  const [tags, setTags] = useState('');
  const [own, setOwn] = useState<PublishedPlugin[] | null>(null);
  const [more, setMore] = useState(false);
  const [reading, setReading] = useState(false);
  const [readError, setReadError] = useState<unknown>(null);
  const [preparing, setPreparing] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const id = Number(pluginId);
  const validId = /^\d+$/.test(pluginId.trim()) && Number.isSafeInteger(id) && id > 0;
  const valid = writable && !preparing && (mode === 'first' || validId);

  async function loadOwn() {
    if (reading) return;
    setReadError(null); setReading(true);
    try {
      const limit = 50;
      const items = await api<PublishedPlugin[]>('list_my_manager_plugins', {limit, offset: own?.length ?? 0});
      setOwn(current => [...current ?? [], ...items]); setMore(items.length === limit);
    } catch (err) { setReadError(err); } finally { setReading(false); }
  }
  async function prepare() {
    if (!valid) return;
    setPreparing(true); setError(null);
    try {
      const target: Target = mode === 'first' ? {kind: 'first'} : {kind: 'release', pluginId: id};
      const record = await api<PublicationRecord>('prepare_manager_publication', {
        ...(draft.id === undefined ? {slug: draft.slug} : {draftId: draft.id}), target, tags: tags.split(/[,，]/).map(tag => tag.trim()).filter(Boolean),
      });
      onPrepared(record);
    } catch (err) { setError(err); } finally { setPreparing(false); }
  }
  return <Modal className="sm:max-w-xl" title="准备发布管理器插件" description={`Mod客栈 / ${draft.slug}`} onClose={() => { if (!preparing) onClose(); }} actions={<>
    <Button variant="outline" disabled={preparing} onClick={onClose}>取消</Button>
    <Button disabled={!valid} onClick={() => void prepare()}>{preparing ? <Spinner/> : <FileTextIcon/>}生成发布预览</Button>
  </>}>
    <ErrorState error={error}/>
    <FieldGroup>
      <FieldSet><FieldLegend variant="label">发布方式</FieldLegend>
        <RadioGroup aria-label="发布方式" value={mode} disabled={preparing} onValueChange={value => { setMode(value as Target['kind']); setError(null); }} className="flex flex-wrap gap-x-6">
          <Label><RadioGroupItem value="first"/>首次发布</Label><Label><RadioGroupItem value="release"/>发布新版本</Label>
        </RadioGroup>
      </FieldSet>
      {mode === 'release' && <Field>
        <FieldLabel htmlFor="pub-target">目标插件 ID</FieldLabel>
        <div className="flex flex-wrap items-center gap-2"><Input id="pub-target" className="min-w-36 flex-1" inputMode="numeric" autoComplete="off" placeholder="输入本人插件的数字 ID" value={pluginId} disabled={preparing} aria-invalid={pluginId !== '' && !validId} onChange={event => setPluginId(event.target.value)}/>
          {(own === null || more) && <Button variant="outline" disabled={reading || preparing} onClick={() => void loadOwn()}>{reading && <Spinner/>}{own === null ? '加载我的插件' : '加载更多插件'}</Button>}
        </div>
        {own !== null && (own.length ? <NativeSelect aria-label="选择我的插件" disabled={preparing} value={own.some(item => item.id === id) ? String(id) : ''} onChange={event => setPluginId(event.target.value)}>
          <NativeSelectOption value="">选择我的插件</NativeSelectOption>{own.map(item => <NativeSelectOption key={item.id} value={item.id}>{item.name || item.slug} · {item.version} · #{item.id}{item.hidden ? ' · 已下架' : ''}</NativeSelectOption>)}
        </NativeSelect> : <FieldDescription>未找到本人插件。</FieldDescription>)}
        {pluginId && !validId && <FieldDescription>请输入大于零的整数 ID。</FieldDescription>}
        <ErrorState error={readError}/>
      </Field>}
      <Field><FieldLabel htmlFor="pub-tags">版本标签</FieldLabel><Input id="pub-tags" placeholder="例如：游戏适配，中文" value={tags} disabled={preparing} onChange={event => setTags(event.target.value)}/>
        <FieldDescription>用逗号分隔，可留空。{mode === 'release' && '请填写新版本的完整标签。'}</FieldDescription>
      </Field>
    </FieldGroup>
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
  const [checked, setChecked] = useState(false);
  const [confirmation, setConfirmation] = useState('');
  const [activity, setActivity] = useState<'publish' | 'reconcile' | 'refresh' | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [needsRefresh, setNeedsRefresh] = useState(false);
  const [reconciliation, setReconciliation] = useState<Reconciliation | null>(null);
  const busy = activity !== null;
  const legacy = isLegacy(record);
  const pack = legacy ? record.preview.package : record.preview;
  const target = legacy ? null : record.preview.target;
  const publishable = !legacy && record.state === 'intended' && !needsRefresh;

  async function refresh() {
    try { onChange(await reload(record)); setNeedsRefresh(false); }
    catch (err) { setNeedsRefresh(true); throw err; }
  }
  async function act(action: 'publish' | 'reconcile' | 'refresh') {
    if (busy || (action !== 'refresh' && !writable)) return;
    if (action === 'publish' && (!publishable || !checked || confirmation !== 'PUBLISH_PLUGIN')) return;
    if (action === 'reconcile' && !canReconcile(record)) return;
    setActivity(action); setError(null); setReconciliation(null); setChecked(false); setConfirmation('');
    try {
      if (action === 'publish') {
        let result: PublicationRecord;
        try {
          result = await api<PublicationRecord>('publish_manager_publication', {id: record.id, digest: record.preview.digest, confirmation});
        } catch (err) {
          // The request could have reached the server. Read the durable state before enabling another approval.
          await refresh(); throw err;
        }
        onChange(result);
      } else if (action === 'reconcile') {
        const outcome = await api<Reconciliation>(legacy ? 'reconcile_plugin_publication' : 'reconcile_manager_publication', {id: record.id});
        await refresh(); setReconciliation(outcome);
      } else { await refresh(); }
    } catch (err) { setError(err); } finally { setActivity(null); }
  }
  return <Modal className={legacy ? undefined : 'sm:max-w-2xl'} title={legacy ? '旧版作品发布记录' : '发布管理器插件'} description={`Mod客栈 / ${target ? targetName(target) : '旧版普通作品'}`} onClose={() => { if (!busy) onClose(); }} actions={<>
    <Button variant="outline" disabled={busy} onClick={onClose}>关闭</Button>
    {needsRefresh && <Button variant="outline" disabled={busy} onClick={() => void act('refresh')}>{activity === 'refresh' ? <Spinner/> : <RefreshCwIcon/>}刷新发布状态</Button>}
    {canReconcile(record) && !needsRefresh && <Button variant="outline" disabled={busy || !writable} onClick={() => void act('reconcile')}>{activity === 'reconcile' ? <Spinner/> : <RefreshCwIcon/>}核对远端状态</Button>}
    {publishable && <Button disabled={busy || !writable || !checked || confirmation !== 'PUBLISH_PLUGIN'} onClick={() => void act('publish')}>{activity === 'publish' ? <Spinner/> : <UploadIcon/>}确认发布</Button>}
  </>}>
    <div className="flex flex-wrap items-center gap-x-3 gap-y-2"><strong className="break-all">{pack.slug}</strong><span className="text-sm">{pack.version}</span><StatusBadge record={record}/></div>
    <dl className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 text-sm">
      {target && <><dt className="text-muted-foreground">目标插件</dt><dd>{target.kind === 'release' ? `#${target.pluginId}` : '新建管理器插件'}</dd></>}
      <dt className="text-muted-foreground">版本标签</dt><dd className="break-words">{pack.tags.join('、') || '无标签'}</dd>
      {!legacy && record.remote != null && <><dt className="text-muted-foreground">远端插件 ID</dt><dd>{record.remote.id}</dd></>}
      {!legacy && record.remote != null && <><dt className="text-muted-foreground">远端版本 ID</dt><dd>{record.remote.releaseId}</dd></>}
      {!legacy && record.remote?.hidden && record.remote.reason && <><dt className="text-muted-foreground">下架原因</dt><dd className="break-words">{record.remote.reason}</dd></>}
      <dt className="text-muted-foreground">发布地址</dt><dd><PathText>{pack.destination}</PathText></dd>
      <dt className="text-muted-foreground">审批摘要</dt><dd><PathText>{record.preview.digest}</PathText></dd>
    </dl>
    {legacy && <><Separator/><LegacyMetadata record={record}/></>}
    <ErrorState error={error}/>
    {reconciliation && <Alert role="status" variant={reconciliation === 'verified' ? 'success' : 'warning'}>{reconciliation === 'verified' ? <CheckCircle2Icon/> : <CircleAlertIcon/>}<AlertDescription>{reconciliation === 'verified' ? '已核实远端状态' : '尚未核实，发布状态保持不变'}</AlertDescription></Alert>}
    {record.state === 'unknown' && !reconciliation && <Alert variant="warning"><CircleAlertIcon/><AlertDescription>{canReconcile(record) ? '远端可能已经收到本次发布，请先核对结果。' : '请在 Mod客栈核对该作品的发布结果。'}</AlertDescription></Alert>}
    <Files files={pack.files}/>
    {publishable && <>
      <Label className="items-start font-normal"><Checkbox aria-label="已检查公开发布的内容" checked={checked} disabled={busy} onCheckedChange={value => setChecked(value === true)}/><span>已检查两份文件原文、目标插件和标签，同意公开发布。</span></Label>
      <Field><FieldLabel htmlFor="pub-confirm">输入 PUBLISH_PLUGIN 确认发布</FieldLabel><Input id="pub-confirm" placeholder="PUBLISH_PLUGIN" autoComplete="off" spellCheck={false} disabled={busy} value={confirmation} onChange={event => setConfirmation(event.target.value)}/></Field>
    </>}
  </Modal>;
}

export function PluginPublications({draft, showHistory, writable, onDraftClose}: {
  draft: PublicationInput | null; showHistory: boolean; writable: boolean; onDraftClose: () => void;
}) {
  const client = useQueryClient();
  const manager = useBackendQuery<PublicationRecord[]>(['manager-publications'], 'list_manager_publications', undefined, showHistory);
  const legacy = useBackendQuery<LegacyRecord[]>(['plugin-publications'], 'list_plugin_publications', undefined, showHistory);
  const [selected, setSelected] = useState<HistoryRecord | null>(null);
  const history: HistoryRecord[] = [...manager.data ?? [], ...legacy.data ?? []].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  const loading = manager.isLoading || legacy.isLoading;
  async function reload(record: HistoryRecord) {
    const query = isLegacy(record) ? legacy : manager;
    const result = await query.refetch({throwOnError: true});
    const updated = result.data?.find(item => item.id === record.id);
    if (!updated) throw new Error('未找到这条发布记录，请刷新发布历史。');
    return updated;
  }
  function remember(record: HistoryRecord) {
    const key = isLegacy(record) ? ['plugin-publications'] : ['manager-publications'];
    client.setQueryData<HistoryRecord[]>(key, current => current && [record, ...current.filter(item => item.id !== record.id)]);
    setSelected(record);
    if (!isLegacy(record) && record.state === 'completed') {
      void client.invalidateQueries({queryKey: ['manager-plugin-games']});
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
    </section>}
    {draft && <PrepareDialog key={draft.id ?? `installed:${draft.slug}`} draft={draft} writable={writable} onClose={onDraftClose} onPrepared={record => { onDraftClose(); remember(record); }}/>}
    {selected && <ReviewDialog key={`${selected.id}-${selected.preview.digest}`} record={selected} writable={writable} onClose={closeReview} onChange={remember} reload={reload}/>}
  </>;
}
