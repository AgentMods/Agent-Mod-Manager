import { useCallback, useEffect, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { open } from '@tauri-apps/plugin-dialog';
import { ArrowLeft, ArrowUp, Camera, File, Folder, FolderOpen, Gamepad2, PackagePlus, RefreshCw, Search, ShieldCheck, Trash2, Undo2 } from 'lucide-react';
import { api, backendAvailable, useBackendQuery } from '../lib/api';
import { useUI } from '../lib/store';
import { initialFomodSelections, selectedFomodMappings, validFomodSelection, type FomodPreview, type FomodSelections } from '../lib/fomod';
import { FomodOptions } from '../components/FomodOptions';
import type { FilePlan, Game } from '../lib/types';
import { Badge, Button, bytes, date, EmptyState, ErrorState, IconButton, Modal, PageHeader } from '../components/ui';
import { FabricStartupEvidence, type FabricStartup } from '../components/FabricStartupEvidence';
import { FabricDiskEvidence, type FabricDisk } from '../components/FabricDiskEvidence';
import { BethesdaPluginEvidence, type BethesdaEvidence } from '../components/BethesdaPluginEvidence';

type DirEntry = { name: string; path: string; is_dir: boolean; size: number; modified: string };
type DownloadOrigin = { artifactId: string; sourceId: string; projectId: string; versionId: string };
type Installed = {
  id: string; name: string; version: string | null; status: string; source: string; downloadOrigin?: DownloadOrigin | null;
  installRoot: string; files: { path: string; size: number }[]; installedAt: string;
  metadata?: { transactionId?: string | null; replacesInstallId?: string | null } | null;
};
type ReplacementRollback = { installId: string; name: string; version: string | null; plan: FilePlan };
type Snapshot = { id: string; label: string; file_count: number; total_size: number; created_at: string };
const legacyLogEncodings = ['windows-1252', 'windows-1251', 'gbk', 'big5', 'shift_jis'] as const;
const readStatusLabels = {
  read: '已读取', missing: '文件缺失', denied: '拒绝读取', binary: '二进制内容',
  unknown_encoding: '编码未知', io_error: '读取失败', limit_reached: '达到读取上限',
};
type DiagnosticFile = {
  file: string; kind: 'log' | 'load_order'; status: keyof typeof readStatusLabels;
  encoding: string | null; truncated: boolean; notes: string[];
};
type Diagnosis = {
  status: string; causes: { summary: string; basis: string }[]; unknowns: string[];
  evidence: { file: string; line: number | null; text: string }[];
  repairSuggestions: { description: string; fileOpsInput: { ops: unknown[]; summary: string } | null }[];
  logs?: { files: DiagnosticFile[] };
  fabricStartup?: FabricStartup;
  fabricDisk?: FabricDisk;
  bethesdaPlugins?: BethesdaEvidence;
};
type Adapter = { modTypes: {id: string; name: string; root: string; notes: string}[]; installRoots: {id: string; kind: string; notes: string}[] };
const statuses: Record<string, string> = { installed: '已安装', enabled: '已启用', superseded: '已被替换', uninstalled: '已卸载', rolled_back: '已回滚', partially_restored: '部分文件已恢复', pending: '待执行', planned: '待审批', purged: '已永久删除' };

function replacementTransaction(mod?: Installed) {
  const { replacesInstallId, transactionId } = mod?.metadata ?? {};
  return mod?.status === 'enabled' && replacesInstallId && transactionId ? transactionId : null;
}

function clipSourceText(text: string, limit: number) {
  return text.length <= limit ? text : `${text.slice(0, limit - 1)}…`;
}

function DiagnosticSources({ files }: { files: DiagnosticFile[] }) {
  if (!files.length) return null;
  return <details>
    <summary style={{ cursor: 'pointer', fontSize: 12 }}>文件读取状态（{files.length}）</summary>
    {files.map(({ file, kind, status, encoding, truncated, notes }, i) => <div className="diagnosis-cause" style={{ padding: '8px 0' }} key={i}>
      <small className="path-text">{clipSourceText(file.split(/[\\/]/).at(-1) || '未命名文件', 120)}</small>
      <p style={{ marginTop: 0 }}>{kind === 'log' ? '日志' : '加载顺序'} · {readStatusLabels[status] ?? '读取状态未知'}
        {encoding && ` · ${clipSourceText(encoding, 32)}`}{truncated && ' · 已截断'}</p>
      {notes.map((note, index) => <p style={{ marginTop: 2 }} key={index}>{clipSourceText(note, 256)}</p>)}
    </div>)}
  </details>;
}

function ReplacementEffects({ plan }: { plan: FilePlan }) {
  return <div className="plan-preview">
    <p>原替换事务：{plan.summary}</p>
    <div className="table-wrap"><table aria-label="回滚文件影响" style={{ tableLayout: 'fixed' }}>
      <colgroup><col style={{ width: '36%' }}/><col/></colgroup>
      <thead><tr><th scope="col">撤销效果</th><th scope="col">文件路径</th></tr></thead>
      <tbody>{[...plan.entries].reverse().map(({ operation, before }, index) => {
        const effect = operation.op === 'move' ? (before ? '移回原位置并还原覆盖' : '移回原位置')
          : operation.op === 'copy' ? (before ? '还原覆盖前文件' : '移出新增文件')
          : operation.op === 'mkdir' ? '移除本次创建的空目录' : '撤销原操作';
        return <tr key={index}><td>{effect}</td><td className="file-path">
          <div>{operation.dst}</div>{operation.op === 'move' && operation.src && <><small className="muted">原位置</small><div>{operation.src}</div></>}
        </td></tr>;
      })}</tbody>
    </table></div>
    <small className="muted path-text">事务 {plan.id}</small>
  </div>;
}

export default function GameDetailPage() {
  const gameId = useUI(s => s.gameId);
  const navigate = useUI(s => s.navigate);
  if (!gameId) return <div className="page"><PageHeader title="游戏详情"/><EmptyState title="尚未选择游戏" action={<Button onClick={() => navigate('library')}><ArrowLeft size={16}/>返回游戏库</Button>}/></div>;
  return <GameDetails key={gameId} gameId={gameId}/>;
}

function GameDetails({ gameId }: { gameId: string }) {
  const navigate = useUI(s => s.navigate);
  const client = useQueryClient();
  const reviewPlan = useUI(s => s.reviewPlan);
  const notify = useUI(s => s.notify);
  const [tab, setTab] = useState('files');
  const [location, setLocation] = useState<string | null>(null);
  const [search, setSearch] = useState('');
  const [file, setFile] = useState<DirEntry | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [rollback, setRollback] = useState<ReplacementRollback | null>(null);
  const [rollbackError, setRollbackError] = useState<unknown>(null);
  const [installOpen, setInstallOpen] = useState(false);
  const [artifactPath, setArtifactPath] = useState('');
  const [artifactHash, setArtifactHash] = useState<string | null>(null);
  const [fomodPreview, setFomodPreview] = useState<FomodPreview | null>(null);
  const [fomodSelections, setFomodSelections] = useState<FomodSelections>({});
  const fomodMappings = selectedFomodMappings(fomodPreview, fomodSelections);
  const [filename, setFilename] = useState('');
  const [modName, setModName] = useState('');
  const [modType, setModType] = useState('');
  const [snapshotLabel, setSnapshotLabel] = useState('');
  const [rootId, setRootId] = useState('');
  const [rootPath, setRootPath] = useState('');
  const [legacyLogEncoding, setLegacyLogEncoding] = useState<(typeof legacyLogEncodings)[number] | null>(null);
  const [diagnosis, setDiagnosis] = useState<Diagnosis | null>(null);
  const game = useBackendQuery<Game>(['game', gameId], 'get_game', { id: gameId }, !!gameId);
  const adapter = useBackendQuery<Adapter>(['game-adapter', gameId], 'get_game_adapter', {gameId}, !!game.data?.adapterSlug);
  const bootstrap = useBackendQuery<{ recoveryError: string | null }>(['bootstrap'], 'get_bootstrap_state');
  const writable = backendAvailable && !!bootstrap.data && !bootstrap.data.recoveryError;
  const path = location ?? game.data?.rootPath ?? '';
  const directory = useBackendQuery<DirEntry[]>(['directory', path], 'read_dir', { path }, !!path && tab === 'files');
  const content = useBackendQuery<{ lines: { line: number; text: string }[]; truncated: boolean }>(['file', file?.path], 'read_file', { path: file?.path }, !!file && !file.is_dir);
  const installs = useBackendQuery<Installed[]>(['installs', gameId], 'list_installs', { gameId }, !!gameId);
  const rollbackCurrent = !!rollback && replacementTransaction(installs.data?.find(mod => mod.id === rollback.installId)) === rollback.plan.id;
  const snapshots = useBackendQuery<Snapshot[]>(['snapshots', gameId], 'list_snapshots', { gameId }, !!gameId);
  const roots = useBackendQuery<Record<string, string>>(['user-roots', gameId], 'list_user_roots', { gameId }, !!gameId);
  const clearFilePreview = useCallback(() => {
    setFile(null);
    if (!file) return;
    const filters = { queryKey: ['file', file.path], exact: true };
    void client.cancelQueries(filters);
    client.removeQueries(filters);
  }, [client, file]);
  useEffect(() => {
    if (file && directory.isSuccess && !directory.isFetching &&
      !directory.data.some(entry => entry.path === file.path && !entry.is_dir)) {
      clearFilePreview();
    }
  }, [clearFilePreview, directory.data, directory.isFetching, directory.isSuccess, file]);
  function refresh() {
    clearFilePreview();
    void installs.refetch(); void snapshots.refetch(); void directory.refetch(); void game.refetch();
    void client.invalidateQueries({ queryKey: ['games'] });
  }
  async function run(work: () => Promise<void>) {
    setBusy(true); setError(null);
    try { await work(); } catch (e) { setError(e); } finally { setBusy(false); }
  }
  async function runDiagnosis() {
    await run(async () => {
      setDiagnosis(null);
      const options = legacyLogEncoding ? { legacyLogEncoding } : null;
      setDiagnosis(await api<Diagnosis>('diagnose_game', { gameId, options }));
    });
  }
  async function preview(command: string, args: Record<string, unknown>) {
    await run(async () => {
      const plan = await api<FilePlan>(command, args);
      reviewPlan(plan, refresh);
    });
  }
  async function previewReplacementRollback(mod: Installed) {
    const transactionId = replacementTransaction(mod);
    if (!writable || busy || installs.isFetching || !transactionId) return;
    await run(async () => {
      const plan = await api<FilePlan>('get_file_plan', { planId: transactionId });
      if (plan.id !== transactionId) throw new Error('无法读取对应的替换事务。');
      setRollbackError(null);
      setRollback({ installId: mod.id, name: mod.name, version: mod.version, plan });
    });
  }
  async function confirmReplacementRollback() {
    if (!rollback || !rollbackCurrent || !writable || busy || installs.isFetching) return;
    await run(async () => {
      setRollbackError(null);
      try {
        const { data } = await installs.refetch({ throwOnError: true });
        if (replacementTransaction(data?.find(mod => mod.id === rollback.installId)) !== rollback.plan.id) {
          throw new Error('安装记录已变化，不能继续回滚此替换。');
        }
      } catch (e) { setRollbackError(e); return; }
      try {
        await api('rollback_transaction', { transactionId: rollback.plan.id });
        setRollback(null);
        notify('版本替换已回滚');
      } catch (e) { setRollbackError(e); }
      finally {
        clearFilePreview();
        await client.invalidateQueries();
      }
    });
  }
  async function importArchive() {
    if (!writable || busy) return;
    await run(async () => {
      const selected = await open({multiple: false, directory: false, title: '选择本地 Mod 归档', filters: [{name: 'Mod 归档', extensions: ['zip', 'jar', 'mrpack', '7z', 'gz', 'tgz']}]});
      if (typeof selected !== 'string') return;
      const result = await api<{filename: string; artifactPath: string; sha256: string; plan: FilePlan}>('plan_import_archive', {path: selected});
      setInstallOpen(false);
      reviewPlan(result.plan, () => {
        setArtifactPath(result.artifactPath); setArtifactHash(result.sha256); setFilename(result.filename); setFomodPreview(null); setFomodSelections({});
        setInstallOpen(true);
      });
    });
  }
  async function previewFomod() {
    if (!writable || busy || !artifactPath.trim()) return;
    await run(async () => {
      const preview = await api<FomodPreview>('preview_fomod', { artifactPath: artifactPath.trim() });
      if (artifactHash && preview.artifactSha256 !== artifactHash) {
        throw new Error('归档内容自导入或上次扫描后已变化，请重新导入或验证归档路径。');
      }
      setFomodPreview(preview);
      setArtifactHash(preview.artifactSha256);
      setFomodSelections(initialFomodSelections(preview));
    });
  }
  function parent() {
    const normalized = path.replace(/\\/g, '/').replace(/\/$/, '');
    const next = normalized.slice(0, normalized.lastIndexOf('/'));
    const anchors = [game.data?.rootPath, ...Object.values(roots.data ?? {})].filter((value): value is string => !!value).map(value => value.replace(/\\/g, '/').replace(/\/$/, ''));
    const anchor = anchors.filter(value => normalized === value || normalized.startsWith(`${value}/`)).sort((a, b) => b.length - a.length)[0];
    if (anchor && next && next.length >= anchor.length && normalized !== anchor) { setLocation(next); setFile(null); }
  }
  return <div className="page game-detail-page" data-testid="game-detail-page">
    <div className="toolbar"><IconButton title="返回游戏库" onClick={() => navigate('library')}><ArrowLeft size={17}/></IconButton><span className="muted">游戏库 / 游戏详情</span></div>
    <ErrorState error={game.error}/><ErrorState error={error}/>
    <header className="game-heading"><div className="game-cover"><Gamepad2 size={38}/></div><div className="game-heading-info"><h1>{game.data?.name ?? '正在读取游戏…'}</h1><p className="path-text">{game.data?.rootPath ?? '—'}</p><p>{game.data?.platform ?? '—'} · 已装 Mod {game.data?.installedModCount ?? 0} · 已启用 {game.data?.enabledModCount ?? 0}</p></div><div className="page-actions"><Button className="primary" disabled={!writable || busy} onClick={() => setInstallOpen(true)}><PackagePlus size={16}/>安装归档</Button></div></header>
    <div className="tabs" role="tablist">{[['files', '文件'], ['mods', '已装 Mod'], ['snapshots', '快照'], ['diagnostics', '诊断']].map(([id, name]) => <button role="tab" aria-selected={tab === id} className={tab === id ? 'active' : ''} onClick={() => setTab(id)} key={id}>{name}</button>)}</div>
    {tab === 'files' && <div className="split-layout"><section><div className="toolbar"><IconButton title="上一级目录" onClick={parent} disabled={!path || path === game.data?.rootPath}><ArrowUp size={16}/></IconButton><span className="path-text grow">{path}</span><IconButton title="刷新目录" disabled={!backendAvailable} onClick={() => { void directory.refetch(); }}><RefreshCw size={16}/></IconButton></div><label className="search-field"><Search size={16}/><input placeholder="搜索当前目录…" value={search} onChange={e => setSearch(e.target.value)}/></label><ErrorState error={directory.error}/>
      <div className="file-browser" style={{ marginTop: 16 }}><aside className="directory-tree"><button className="tree-item active" onClick={() => { setLocation(null); setFile(null); }}><FolderOpen size={16}/><span>游戏目录</span></button>{Object.entries(roots.data ?? {}).map(([id, value]) => <button className="tree-item" key={id} onClick={() => { setLocation(value); setFile(null); }}><Folder size={16}/><span>{id}</span></button>)}</aside><div className="table-wrap"><table><thead><tr><th>名称</th><th>大小</th><th>修改时间</th></tr></thead><tbody>{(directory.data ?? []).filter(item => item.name.toLowerCase().includes(search.toLowerCase())).map(item => <tr key={item.path} className={`row-clickable ${file?.path === item.path ? 'selected' : ''}`} onClick={() => item.is_dir ? (setLocation(item.path), setFile(null)) : setFile(item)}><td><span className="table-name">{item.is_dir ? <Folder size={16}/> : <File size={16}/>} {item.name}</span></td><td>{item.is_dir ? '—' : bytes(item.size)}</td><td>{date(item.modified)}</td></tr>)}</tbody></table>{!directory.data?.length && <EmptyState title={directory.isLoading ? '正在读取目录…' : '目录中没有可显示的文件'}/>}</div></div></section>
      <aside className="inspector"><h2>文件详情</h2>{file ? <><h3>{file.name}</h3><p className="path-text">{file.path}</p><p>{bytes(file.size)} · {date(file.modified)}</p><ErrorState error={content.error}/>{content.data && !content.isError && <><h3>文件预览</h3><div className="file-viewer">{content.data.lines.map(line => <div className="file-line" key={line.line}><span>{line.line}</span><span>{line.text}</span></div>)}</div>{content.data.truncated && <p className="warning-text">内容已截断</p>}</>}<Button className="danger" style={{ marginTop: 16 }} disabled={!writable || busy} onClick={() => preview('trash_paths', { request: { paths: [file.path], game_id: gameId, reason: '用户从文件浏览器移除' } })}><Trash2 size={15}/>移入回收站</Button></> : <p>未选择文件</p>}</aside></div>}
    {tab === 'mods' && <>
      <ErrorState error={installs.error}/>
      <div className="table-wrap"><table>
        <thead><tr><th>名称</th><th>版本</th><th>来源</th><th>状态</th><th>安装时间</th><th/></tr></thead>
        <tbody>{(installs.data ?? []).map(mod => <tr key={mod.id}>
          <td><strong>{mod.name}</strong><small>{mod.files.length} 个文件</small></td>
          <td>{mod.version ?? '—'}{mod.downloadOrigin && <small className="path-text">版本 ID：{mod.downloadOrigin.versionId}</small>}</td>
          <td>{mod.downloadOrigin?.sourceId ?? (mod.source === 'local' ? '本地归档' : mod.source)}
            {mod.downloadOrigin ? <small className="path-text">项目 ID：{mod.downloadOrigin.projectId}</small> : mod.source !== 'local' && <small>未绑定下载记录</small>}
          </td>
          <td><Badge tone={mod.status === 'enabled' ? 'success' : 'warning'}>{statuses[mod.status] ?? mod.status}</Badge></td>
          <td>{date(mod.installedAt)}</td>
          <td><div style={{ display: 'flex', gap: 8 }}>
            <IconButton title={mod.status === 'partially_restored' ? '部分恢复：需先处理剩余回收站文件' : '预览卸载'} disabled={!writable || busy || mod.status !== 'enabled'} onClick={() => preview('plan_uninstall', { installId: mod.id })}><Trash2 size={16}/></IconButton>
            {replacementTransaction(mod) && <IconButton title="预览回滚替换" disabled={!writable || busy || installs.isFetching} onClick={() => { void previewReplacementRollback(mod); }}><Undo2 size={16}/></IconButton>}
          </div></td>
        </tr>)}</tbody>
      </table></div>
      {!installs.data?.length && !installs.error && <EmptyState title={installs.isLoading ? '正在读取 Mod…' : '尚未安装 Mod'} action={<Button disabled={!writable} onClick={() => setInstallOpen(true)}><PackagePlus size={16}/>选择归档</Button>}/>}
    </>}
    {tab === 'snapshots' && <><div className="toolbar"><input className="grow" placeholder="快照名称" value={snapshotLabel} onChange={e => setSnapshotLabel(e.target.value)}/><Button disabled={!writable || busy || !snapshotLabel.trim()} onClick={() => run(async () => { const [, plan] = await api<[Snapshot, FilePlan]>('snapshot_create', { gameId, label: snapshotLabel }); reviewPlan(plan, () => { void snapshots.refetch(); }); })}><Camera size={16}/>预览创建快照</Button></div><ErrorState error={snapshots.error}/><div className="table-wrap"><table><thead><tr><th>名称</th><th>文件</th><th>大小</th><th>创建时间</th><th/></tr></thead><tbody>{(snapshots.data ?? []).map(item => <tr key={item.id}><td>{item.label}</td><td>{item.file_count}</td><td>{bytes(item.total_size)}</td><td>{date(item.created_at)}</td><td><Button disabled={!writable || busy} onClick={() => preview('restore_snapshot', { gameId, snapshotId: item.id })}><Undo2 size={15}/>恢复预览</Button></td></tr>)}</tbody></table></div>{!snapshots.data?.length && <EmptyState title="尚无快照"/>}</>}
    {tab === 'diagnostics' && <>
      <PageHeader title="游戏诊断" subtitle="仅分析已登记目录与日志" actions={<>
        <label className="field" style={{ flexDirection: 'row', alignItems: 'center', flexWrap: 'wrap', marginBottom: 0 }}>
          <span>日志编码</span>
          <select value={legacyLogEncoding ?? ''} disabled={!backendAvailable || busy} onChange={event => {
            const value = event.currentTarget.value;
            setLegacyLogEncoding(legacyLogEncodings.find(encoding => encoding === value) ?? null);
            setDiagnosis(null);
          }}>
            <option value="">默认（不回退）</option>
            {legacyLogEncodings.map(encoding => <option value={encoding} key={encoding}>回退：{encoding}</option>)}
          </select>
        </label>
        <Button disabled={!backendAvailable || busy} onClick={runDiagnosis}><ShieldCheck size={16}/>{busy ? '正在诊断…' : '开始诊断'}</Button>
      </>}/>
      {diagnosis ? <div className="split-layout">
        <section style={{ overflowWrap: 'anywhere' }}>
          {diagnosis.fabricStartup && <FabricStartupEvidence report={diagnosis.fabricStartup}/>}
          {diagnosis.fabricDisk && <FabricDiskEvidence report={diagnosis.fabricDisk}/>}
          {diagnosis.bethesdaPlugins && <BethesdaPluginEvidence report={diagnosis.bethesdaPlugins}/>}
          <Badge>{diagnosis.status === 'findings' ? '已发现诊断线索' : '未确认故障原因'}</Badge>
          {diagnosis.causes.map((cause, i) => <div className="diagnosis-cause" key={i}><h3>{cause.summary}</h3><p className="muted">{cause.basis}</p></div>)}
          {diagnosis.unknowns.map((text, i) => <p className="warning-text" key={i}>{text}</p>)}
          {diagnosis.repairSuggestions.map((suggestion, i) => <div className="list-row" key={i}><p className="grow">{suggestion.description}</p>{suggestion.fileOpsInput && <Button disabled={!writable || busy} onClick={() => preview('plan_file_ops', { operations: suggestion.fileOpsInput?.ops, summary: suggestion.fileOpsInput?.summary })}>预览修复</Button>}</div>)}
        </section>
        <aside className="inspector"><h2>诊断证据</h2>
          <DiagnosticSources files={diagnosis.logs?.files ?? []}/>
          {diagnosis.evidence.map((e, i) => <div className="diagnosis-cause" key={i}><small className="path-text">{e.file}{e.line ? `:${e.line}` : ''}</small><p>{e.text}</p></div>)}
        </aside>
      </div> : <EmptyState title="尚未运行诊断"/>}
      <section className="page-section"><h2>用户管理目录</h2><ErrorState error={roots.error}/>{Object.entries(roots.data ?? {}).map(([id, value]) => <div className="list-row" key={id}><strong>{id}</strong><span className="path-text">{value}</span></div>)}<div className="toolbar"><input placeholder="适配包目录 ID" value={rootId} onChange={e => setRootId(e.target.value)}/><input className="grow" placeholder="本机目录完整路径" value={rootPath} onChange={e => setRootPath(e.target.value)}/><Button disabled={!writable || busy || !rootId || !rootPath} onClick={() => run(async () => { await api('register_user_root', { gameId, rootId, directory: rootPath }); await roots.refetch(); notify('用户目录已登记'); })}>确认登记目录</Button></div></section></>}
    {rollback && <Modal title="回滚版本替换" onClose={() => { if (!busy) setRollback(null); }} actions={<>
      <Button disabled={busy} onClick={() => setRollback(null)}>取消</Button>
      <Button className="primary" disabled={!writable || busy || installs.isFetching || !rollbackCurrent} onClick={() => { void confirmReplacementRollback(); }}><Undo2 size={16}/>{busy ? '正在回滚…' : '确认回滚'}</Button>
    </>}>
      <p className="warning-text" style={{ overflowWrap: 'anywhere' }}>将撤销 {rollback.name}{rollback.version ? ` · ${rollback.version}` : ''} 的版本替换，以下为要撤销的文件影响。</p>
      <ReplacementEffects plan={rollback.plan}/>
      <ErrorState error={rollbackError || (!rollbackCurrent && '安装记录已变化，不能继续回滚此替换。')}/>
    </Modal>}
    {installOpen && <Modal title="归档安装预览" onClose={() => { if (!busy) setInstallOpen(false); }} actions={<><Button disabled={busy} onClick={() => setInstallOpen(false)}>取消</Button><Button className="primary" disabled={!writable || busy || !artifactPath.trim() || !filename.trim() || !modName.trim() || !modType.trim() || !adapter.data || !(!fomodPreview || (fomodMappings.length > 0 && validFomodSelection(fomodPreview, fomodSelections)))} onClick={() => run(async () => {
      const result = await api<{ plan: FilePlan; warnings: string[] }>('plan_install', { request: { gameId, artifactPath: artifactPath.trim(), filename: filename.trim(), name: modName.trim(), modType: modType.trim(), version: null, expectedSha256: artifactHash, folderName: null, mappings: fomodMappings, source: 'local', sourceModId: null } });
      setInstallOpen(false); if (result.warnings.length) notify(result.warnings.join('；')); reviewPlan(result.plan, refresh);
    })}>生成变更预览</Button></>}>
      <ErrorState error={error || adapter.error}/>
      <div className="toolbar"><Button disabled={!writable || busy} onClick={() => {void importArchive();}}><FolderOpen size={16}/>导入本地归档</Button><Button disabled={!writable || busy || !artifactPath.trim()} onClick={() => {void previewFomod();}}>扫描 FOMOD 配置</Button></div>
      <label className="field">管理器内归档路径<input aria-label="管理器内归档路径" disabled={busy} value={artifactPath} onChange={e => {setArtifactPath(e.target.value); setArtifactHash(null); setFomodPreview(null); setFomodSelections({});}} placeholder="downloads 或 staging 内已完成的归档"/></label>
      <label className="field">原始文件名<input aria-label="原始文件名" disabled={busy} value={filename} onChange={e => {setFilename(e.target.value); setArtifactHash(null); setFomodPreview(null); setFomodSelections({});}} placeholder="例如 MyMod.zip"/></label>
      <div className="form-grid"><label className="field">Mod 名称<input aria-label="Mod 名称" value={modName} onChange={e => setModName(e.target.value)}/></label><label className="field">适配包 Mod 类型<select aria-label="适配包 Mod 类型" value={modType} disabled={busy || !adapter.data} onChange={e => {setModType(e.target.value); setFomodPreview(null); setFomodSelections({});}}><option value="">选择 Mod 类型</option>{adapter.data?.modTypes.map(type => <option key={type.id} value={type.id}>{type.name}</option>)}</select></label></div>
      {adapter.data?.modTypes.find(type => type.id === modType)?.notes && <p className="warning-text">{adapter.data.modTypes.find(type => type.id === modType)?.notes}</p>}
      {fomodPreview && <FomodOptions preview={fomodPreview} selections={fomodSelections} disabled={busy}
        onChange={setFomodSelections}/>}
    </Modal>}
  </div>;
}
