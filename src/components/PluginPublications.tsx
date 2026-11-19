import { useEffect, useLayoutEffect, useState } from 'react';
import { CircleAlertIcon, CircleCheckIcon, EyeIcon, FileArchiveIcon, RefreshCwIcon, UploadIcon } from 'lucide-react';
import { api, backendAvailable, useBackendQuery } from '@/lib/api';
import { Badge, bytes, date, EmptyState, ErrorState, Modal, PathText } from './ui';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Field, FieldGroup, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Spinner } from '@/components/ui/spinner';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { Textarea } from '@/components/ui/textarea';

type Draft = { id: string; slug: string };
type PublicationMetadata = { title: string; summary: string; description: string; tags: string[]; gameIds: number[] };
type PublicationFile = { name: string; size: number; sha256: string; text: string };
type ReconciliationOutcome = 'verified' | 'unconfirmed';
export type PublicationRecord = {
  id: string; state: 'intended' | 'pending' | 'unknown' | 'completed' | 'error';
  phase: string; fileId: number | null; workId: number | null; createdAt: string; updatedAt: string;
  preview: { digest: string; package: PublicationMetadata & {
    slug: string; version: string; files: PublicationFile[]; archiveName: string;
    archiveSha256: string; archiveSize: number; destination: string; workStatus: string;
  } };
};

const phaseNames: Record<string, string> = {
  prepared: '待确认', createUploadPending: '正在创建上传', uploadCreated: '上传已创建',
  startUploadPending: '正在启动上传', uploadStarted: '等待传输', partPutPending: '正在传输',
  partUploaded: '文件已传输', completeUploadPending: '正在完成上传', awaitingScan: '等待安全扫描',
  uploadReady: '可发布', publishWorkPending: '正在发布', completed: '已发布',
};
function status(record: PublicationRecord) {
  if (record.state === 'unknown') return '结果不确定';
  if (record.state === 'error') return '发布失败';
  return phaseNames[record.phase] ?? record.phase;
}
function tone(record: PublicationRecord) {
  if (record.state === 'completed') return 'success';
  if (record.state === 'error') return 'error';
  if (record.state === 'unknown' || record.state === 'pending' || record.phase === 'awaitingScan') return 'warning';
  return 'neutral';
}

export function PluginPublications({ draft, showHistory, writable, onDraftClose }: {
  draft: Draft | null; showHistory: boolean; writable: boolean; onDraftClose: () => void;
}) {
  const records = useBackendQuery<PublicationRecord[]>(['plugin-publications'], 'list_plugin_publications', undefined, showHistory || !!draft);
  const [metadata, setMetadata] = useState<PublicationMetadata>({ title: '', summary: '', description: '', tags: [], gameIds: [] });
  const [tags, setTags] = useState('');
  const [gameIds, setGameIds] = useState('');
  const [selected, setSelected] = useState<PublicationRecord | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [confirmation, setConfirmation] = useState('');
  const [activity, setActivity] = useState<'prepare' | 'publish' | 'reconcile' | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [reconciliation, setReconciliation] = useState<ReconciliationOutcome | null>(null);
  const busy = activity !== null;
  const publishable = selected?.state === 'intended' && selected.phase !== 'awaitingScan';
  const checkable = selected?.fileId != null && (
    (selected.state === 'intended' && selected.phase === 'awaitingScan') ||
    (selected.state === 'unknown' && ['completeUploadPending', 'publishWorkPending'].includes(selected.phase))
  );

  // Initialize before the dialog can accept input, not in a later passive effect.
  useLayoutEffect(() => {
    if (draft) {
      setMetadata({ title: `${draft.slug} 适配包`, summary: '', description: '', tags: [], gameIds: [] });
      setTags(''); setGameIds(''); setError(null);
    }
  }, [draft?.id]);
  useEffect(() => {
    if (selected) {
      const current = records.data?.find(record => record.id === selected.id);
      if (current) setSelected(current);
    }
  }, [records.data, selected?.id]);

  function review(record: PublicationRecord) {
    setSelected(record); setAcknowledged(false); setConfirmation(''); setError(null); setReconciliation(null);
  }
  async function prepare() {
    if (!draft || !writable || busy) return;
    setActivity('prepare'); setError(null);
    try {
      const ids = gameIds.trim() ? gameIds.split(',').map(value => Number(value.trim())) : [];
      if (ids.some(id => !Number.isSafeInteger(id) || id <= 0)) throw new Error('游戏 ID 必须是逗号分隔的正整数');
      const result = await api<PublicationRecord>('prepare_plugin_publication', {
        draftId: draft.id,
        metadata: { ...metadata, title: metadata.title.trim(), tags: tags.trim() ? tags.split(',').map(tag => tag.trim()) : [], gameIds: ids },
      });
      onDraftClose(); review(result); await records.refetch();
    } catch (e) { setError(e); } finally { setActivity(null); }
  }
  async function publish() {
    if (!selected || !publishable || !writable || busy || !acknowledged || confirmation !== 'PUBLISH_PLUGIN') return;
    setActivity('publish'); setError(null); setReconciliation(null);
    try {
      const result = await api<PublicationRecord>('publish_plugin', { id: selected.id, digest: selected.preview.digest, confirmation });
      setSelected(result);
    } catch (e) { setError(e); }
    finally {
      setAcknowledged(false); setConfirmation('');
      const refreshed = await records.refetch();
      const current = refreshed.data?.find(record => record.id === selected.id);
      if (current) setSelected(current);
      setActivity(null);
    }
  }
  async function reconcile() {
    if (!selected || !checkable || !writable || busy) return;
    setActivity('reconcile'); setError(null); setReconciliation(null);
    setAcknowledged(false); setConfirmation('');
    let outcome: ReconciliationOutcome | null = null;
    try {
      outcome = await api<ReconciliationOutcome>('reconcile_plugin_publication', { id: selected.id });
    } catch (e) { setError(e); }
    finally {
      const refreshed = await records.refetch();
      if (outcome !== null) {
        if (refreshed.isError) setError(refreshed.error);
        else setReconciliation(outcome);
      }
      setActivity(null);
    }
  }


  const pack = selected?.preview.package;
  return <>
    {showHistory && <section aria-label="适配包发布记录" className="flex min-w-0 flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="text-sm text-muted-foreground">最近 100 条发布记录</span>
        <Button variant="outline" disabled={busy || records.isFetching || !backendAvailable} onClick={() => { void records.refetch(); }}><RefreshCwIcon data-icon="inline-start" className={records.isFetching ? 'animate-spin' : undefined}/>刷新记录</Button>
      </div>
      <ErrorState error={records.error}/>
      {records.data?.length ? <div className="rounded-lg border">
        <Table>
          <TableHeader><TableRow><TableHead>适配包</TableHead><TableHead>状态</TableHead><TableHead>远程标识</TableHead><TableHead>更新时间</TableHead><TableHead/></TableRow></TableHeader>
          <TableBody>{records.data.map(record => <TableRow key={record.id}>
            <TableCell className="whitespace-normal"><strong className="font-medium">{record.preview.package.title}</strong><span className="block text-xs text-muted-foreground">{record.preview.package.slug} · {record.preview.package.version}</span></TableCell>
            <TableCell><Badge tone={tone(record)}>{status(record)}</Badge></TableCell>
            <TableCell className="text-xs text-muted-foreground"><span className="block">文件 {record.fileId ?? '-'}</span><span className="block">作品 {record.workId ?? '-'}</span></TableCell>
            <TableCell>{date(record.updatedAt)}</TableCell>
            <TableCell><Button variant="outline" size="sm" disabled={busy} aria-label={`查看发布 ${record.preview.package.title}`} onClick={() => review(record)}><EyeIcon data-icon="inline-start"/>查看</Button></TableCell>
          </TableRow>)}</TableBody>
        </Table>
      </div> : <EmptyState title={records.isLoading ? '正在读取发布记录…' : '暂无发布记录'}/>}
    </section>}
    {draft && <Modal title={`发布元数据：${draft.slug}`} onClose={() => { if (!busy) onDraftClose(); }} actions={<>
      <Button variant="outline" disabled={busy} onClick={onDraftClose}>取消</Button>
      <Button disabled={busy || !writable || metadata.title.trim().length < 2} onClick={() => { void prepare(); }}>{activity === 'prepare' ? <Spinner data-icon="inline-start"/> : <FileArchiveIcon data-icon="inline-start"/>}生成上传预览</Button>
    </>}>
      <ErrorState error={error}/>
      <FieldGroup>
        <Field><FieldLabel htmlFor="pub-title">作品标题</FieldLabel><Input id="pub-title" aria-label="发布标题" maxLength={160} value={metadata.title} onChange={e => setMetadata(current => ({ ...current, title: e.target.value }))}/></Field>
        <Field><FieldLabel htmlFor="pub-summary">摘要</FieldLabel><Textarea id="pub-summary" aria-label="发布摘要" rows={2} maxLength={500} value={metadata.summary} onChange={e => setMetadata(current => ({ ...current, summary: e.target.value }))}/></Field>
        <Field><FieldLabel htmlFor="pub-description">作品描述</FieldLabel><Textarea id="pub-description" aria-label="发布描述" rows={5} maxLength={50000} value={metadata.description} onChange={e => setMetadata(current => ({ ...current, description: e.target.value }))}/></Field>
        <div className="grid gap-4 sm:grid-cols-2">
          <Field><FieldLabel htmlFor="pub-tags">标签</FieldLabel><Input id="pub-tags" aria-label="发布标签" placeholder="用逗号分隔，最多 5 个" maxLength={104} value={tags} onChange={e => setTags(e.target.value)}/></Field>
          <Field><FieldLabel htmlFor="pub-games">Mod 客栈游戏 ID</FieldLabel><Input id="pub-games" aria-label="发布游戏 ID" placeholder="可留空，多个 ID 用逗号分隔" maxLength={200} value={gameIds} onChange={e => setGameIds(e.target.value)}/></Field>
        </div>
      </FieldGroup>
    </Modal>}
    {selected && pack && <Modal title="确认公开发布适配包" onClose={() => { if (!busy) setSelected(null); }} actions={<>
      <Button variant="outline" disabled={busy} onClick={() => setSelected(null)}>关闭</Button>
      {checkable && <Button variant="outline" disabled={busy || !writable} onClick={() => { void reconcile(); }}>{activity === 'reconcile' ? <Spinner data-icon="inline-start"/> : <RefreshCwIcon data-icon="inline-start"/>}{activity === 'reconcile' ? '正在核对…' : '核对远端状态'}</Button>}
      {publishable && <Button disabled={busy || !writable || !acknowledged || confirmation !== 'PUBLISH_PLUGIN'} onClick={() => { void publish(); }}>{activity === 'publish' ? <Spinner data-icon="inline-start"/> : <UploadIcon data-icon="inline-start"/>}{activity === 'publish' ? '正在发布…' : selected.phase === 'prepared' ? '上传并公开发布' : '确认继续发布'}</Button>}
    </>}>
      <ErrorState error={error}/>
      <div><Badge tone={tone(selected)}>{status(selected)}</Badge></div>
      {((selected.state === 'unknown' && !checkable) || selected.state === 'error') && <Alert variant="warning" className=""><AlertDescription>{selected.state === 'unknown' ? '请在 Mod 客栈核对文件和作品，当前记录不会自动重发。' : '当前发布记录已终止，未自动重发。'}</AlertDescription></Alert>}
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
        <dt className="text-muted-foreground">目的地</dt><dd><PathText>{pack.destination}</PathText></dd>
        <dt className="text-muted-foreground">公开状态</dt><dd>{pack.workStatus}</dd>
        <dt className="text-muted-foreground">标题</dt><dd>{pack.title}</dd>
        <dt className="text-muted-foreground">摘要</dt><dd className="break-words whitespace-pre-wrap">{pack.summary || '-'}</dd>
        <dt className="text-muted-foreground">描述</dt><dd className="break-words whitespace-pre-wrap">{pack.description || '-'}</dd>
        <dt className="text-muted-foreground">标签</dt><dd>{pack.tags.join('、') || '-'}</dd>
        <dt className="text-muted-foreground">游戏 ID</dt><dd>{pack.gameIds.join(', ') || '-'}</dd>
        <dt className="text-muted-foreground">版本</dt><dd>{pack.version}</dd>
        <dt className="text-muted-foreground">ZIP 文件</dt><dd>{pack.archiveName} ({bytes(pack.archiveSize)})</dd>
        <dt className="text-muted-foreground">ZIP SHA-256</dt><dd><PathText>{pack.archiveSha256}</PathText></dd>
        <dt className="text-muted-foreground">预览摘要</dt><dd><PathText>{selected.preview.digest}</PathText></dd>
        <dt className="text-muted-foreground">发布记录</dt><dd><PathText>{selected.id}</PathText></dd>
        <dt className="text-muted-foreground">远程标识</dt><dd>文件 {selected.fileId ?? '-'} / 作品 {selected.workId ?? '-'}</dd>
      </dl>
      {reconciliation && <Alert role="status" variant={reconciliation === 'verified' ? 'success' : 'warning'}>
        {reconciliation === 'verified' ? <CircleCheckIcon/> : <CircleAlertIcon/>}
        <AlertDescription>{reconciliation === 'verified' ? '已核实远端状态' : '尚未核实，发布状态保持不变'}</AlertDescription>
      </Alert>}
      <section aria-label="将上传的文件内容" className="flex flex-col gap-2">{pack.files.map(file => <details key={file.name} className="rounded-md border px-3 py-2 text-sm">
        <summary className="cursor-pointer font-medium">{file.name} <span className="font-normal text-muted-foreground">{bytes(file.size)}</span></summary>
        <PathText className="mt-2 block">SHA-256 {file.sha256}</PathText>
        <pre className="mt-2 max-h-72 overflow-auto rounded-md border bg-muted p-3 font-mono text-xs break-all whitespace-pre-wrap">{file.text}</pre>
      </details>)}</section>
      {publishable && <>
        <Label className="items-start font-normal"><Checkbox aria-label="已核对公开发布内容" checked={acknowledged} disabled={busy} onCheckedChange={checked => setAcknowledged(checked === true)}/><span>我已核对两份文件原文与全部元数据，确认没有密钥或私人数据，并同意公开发布。</span></Label>
        <Field><FieldLabel htmlFor="pub-confirm">发布确认</FieldLabel><Input id="pub-confirm" aria-label="公开发布确认文本" autoComplete="off" spellCheck={false} placeholder="PUBLISH_PLUGIN" value={confirmation} disabled={busy} onChange={e => setConfirmation(e.target.value)}/></Field>
      </>}
    </Modal>}
  </>;
}