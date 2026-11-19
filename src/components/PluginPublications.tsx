import { useEffect, useLayoutEffect, useState } from 'react';
import { CircleAlert, CircleCheck, Eye, FileArchive, RefreshCw, Upload } from 'lucide-react';
import { api, backendAvailable, useBackendQuery } from '../lib/api';
import { Badge, Button, bytes, date, EmptyState, ErrorState, Modal } from './ui';

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
    {showHistory && <section aria-label="适配包发布记录">
      <div className="toolbar"><span className="muted">最近 100 条发布记录</span><div className="toolbar-spacer"/><Button disabled={busy || records.isFetching || !backendAvailable} onClick={() => { void records.refetch(); }}><RefreshCw size={15}/>刷新记录</Button></div>
      <ErrorState error={records.error}/>
      {records.data?.length ? <div className="table-wrap"><table><thead><tr><th>适配包</th><th>状态</th><th>远程标识</th><th>更新时间</th><th/></tr></thead><tbody>{records.data.map(record => <tr key={record.id}>
        <td><strong>{record.preview.package.title}</strong><small>{record.preview.package.slug} · {record.preview.package.version}</small></td>
        <td><Badge tone={tone(record)}>{status(record)}</Badge></td><td><small>文件 {record.fileId ?? '-'}</small><small>作品 {record.workId ?? '-'}</small></td><td>{date(record.updatedAt)}</td>
        <td><Button disabled={busy} aria-label={`查看发布 ${record.preview.package.title}`} onClick={() => review(record)}><Eye size={15}/>查看</Button></td>
      </tr>)}</tbody></table></div> : <EmptyState title={records.isLoading ? '正在读取发布记录…' : '暂无发布记录'}/>}
    </section>}
    {draft && <Modal title={`发布元数据：${draft.slug}`} onClose={() => { if (!busy) onDraftClose(); }} actions={<><Button disabled={busy} onClick={onDraftClose}>取消</Button><Button className="primary" disabled={busy || !writable || metadata.title.trim().length < 2} onClick={() => { void prepare(); }}><FileArchive size={15}/>生成上传预览</Button></>}>
      <ErrorState error={error}/>
      <label className="field">作品标题<input aria-label="发布标题" maxLength={160} value={metadata.title} onChange={e => setMetadata(current => ({ ...current, title: e.target.value }))}/></label>
      <label className="field">摘要<textarea aria-label="发布摘要" rows={2} maxLength={500} value={metadata.summary} onChange={e => setMetadata(current => ({ ...current, summary: e.target.value }))}/></label>
      <label className="field">作品描述<textarea aria-label="发布描述" rows={5} maxLength={50000} value={metadata.description} onChange={e => setMetadata(current => ({ ...current, description: e.target.value }))}/></label>
      <div className="form-grid"><label className="field">标签<input aria-label="发布标签" placeholder="用逗号分隔，最多 5 个" maxLength={104} value={tags} onChange={e => setTags(e.target.value)}/></label><label className="field">Mod 客栈游戏 ID<input aria-label="发布游戏 ID" placeholder="可留空，多个 ID 用逗号分隔" maxLength={200} value={gameIds} onChange={e => setGameIds(e.target.value)}/></label></div>
    </Modal>}
    {selected && pack && <Modal title="确认公开发布适配包" onClose={() => { if (!busy) setSelected(null); }} actions={<>
      <Button disabled={busy} onClick={() => setSelected(null)}>关闭</Button>
      {checkable && <Button disabled={busy || !writable} onClick={() => { void reconcile(); }}><RefreshCw size={15}/>{activity === 'reconcile' ? '正在核对…' : '核对远端状态'}</Button>}
      {publishable && <Button className="primary" disabled={busy || !writable || !acknowledged || confirmation !== 'PUBLISH_PLUGIN'} onClick={() => { void publish(); }}><Upload size={15}/>{activity === 'publish' ? '正在发布…' : selected.phase === 'prepared' ? '上传并公开发布' : '确认继续发布'}</Button>}
    </>}>
      <ErrorState error={error}/><Badge tone={tone(selected)}>{status(selected)}</Badge>
      {((selected.state === 'unknown' && !checkable) || selected.state === 'error') && <p className="warning-text publication-notice">{selected.state === 'unknown' ? '请在 Mod 客栈核对文件和作品，当前记录不会自动重发。' : '当前发布记录已终止，未自动重发。'}</p>}
      <dl className="metadata-list publication-metadata"><dt>目的地</dt><dd className="path-text">{pack.destination}</dd><dt>公开状态</dt><dd>{pack.workStatus}</dd><dt>标题</dt><dd>{pack.title}</dd><dt>摘要</dt><dd className="publication-text">{pack.summary || '-'}</dd><dt>描述</dt><dd className="publication-text">{pack.description || '-'}</dd><dt>标签</dt><dd>{pack.tags.join('、') || '-'}</dd><dt>游戏 ID</dt><dd>{pack.gameIds.join(', ') || '-'}</dd><dt>版本</dt><dd>{pack.version}</dd><dt>ZIP 文件</dt><dd>{pack.archiveName} ({bytes(pack.archiveSize)})</dd><dt>ZIP SHA-256</dt><dd className="path-text">{pack.archiveSha256}</dd><dt>预览摘要</dt><dd className="path-text">{selected.preview.digest}</dd><dt>发布记录</dt><dd className="path-text">{selected.id}</dd><dt>远程标识</dt><dd>文件 {selected.fileId ?? '-'} / 作品 {selected.workId ?? '-'}</dd></dl>
      {reconciliation && <p role="status" className={`publication-notice publication-reconciliation ${reconciliation}`}>
        {reconciliation === 'verified' ? <CircleCheck size={15}/> : <CircleAlert size={15}/>}
        {reconciliation === 'verified' ? '已核实远端状态' : '尚未核实，发布状态保持不变'}
      </p>}
      <section className="publication-files" aria-label="将上传的文件内容">{pack.files.map(file => <details key={file.name}><summary>{file.name} <span className="muted">{bytes(file.size)}</span></summary><p className="path-text">SHA-256 {file.sha256}</p><pre className="json-preview">{file.text}</pre></details>)}</section>
      {publishable && <><label className="checkbox-row"><input type="checkbox" aria-label="已核对公开发布内容" checked={acknowledged} disabled={busy} onChange={e => setAcknowledged(e.target.checked)}/><span>我已核对两份文件原文与全部元数据，确认没有密钥或私人数据，并同意公开发布。</span></label><label className="field">发布确认<input aria-label="公开发布确认文本" autoComplete="off" spellCheck={false} placeholder="PUBLISH_PLUGIN" value={confirmation} disabled={busy} onChange={e => setConfirmation(e.target.value)}/></label></>}
    </Modal>}
  </>;
}
