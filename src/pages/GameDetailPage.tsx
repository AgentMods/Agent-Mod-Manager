import { useState, type ReactNode } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { open } from '@tauri-apps/plugin-dialog';
import { ArrowLeftIcon, CameraIcon, FolderOpenIcon, Gamepad2Icon, InboxIcon, PackagePlusIcon, ShieldCheckIcon, Trash2Icon, TriangleAlertIcon, Undo2Icon } from 'lucide-react';
import { BethesdaPluginEvidence, type BethesdaEvidence } from '@/components/BethesdaPluginEvidence';
import { FabricDiskEvidence, type FabricDisk } from '@/components/FabricDiskEvidence';
import { FabricStartupEvidence, type FabricStartup } from '@/components/FabricStartupEvidence';
import { GameFileBrowser } from '@/components/GameFileBrowser';
import { FomodOptions } from '@/components/FomodOptions';
import { Badge, ErrorState, IconButton, Modal, Page, PageHeader, PathText, bytes, date } from '@/components/ui';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Empty, EmptyContent, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty';
import { Field, FieldGroup, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from '@/components/ui/item';
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select';
import { Spinner } from '@/components/ui/spinner';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { api, backendAvailable, useBackendQuery } from '@/lib/api';
import { initialFomodSelections, selectedFomodMappings, validFomodSelection, type FomodPreview, type FomodSelections } from '@/lib/fomod';
import { useUI } from '@/lib/store';
import type { FilePlan, Game } from '@/lib/types';

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
type Adapter = { modTypes: { id: string; name: string; root: string; notes: string }[]; installRoots: { id: string; kind: string; notes: string }[] };
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
  return <details className="min-w-0 rounded-md border p-3">
    <summary className="cursor-pointer rounded-sm text-sm font-medium focus-visible:outline-2 focus-visible:outline-ring">文件读取状态（{files.length}）</summary>
    <ItemGroup className="mt-3 gap-2">
      {files.map(({ file, kind, status, encoding, truncated, notes }, i) => <Item role="listitem" size="sm" variant="muted" key={i}>
        <ItemContent className="min-w-0 wrap-anywhere">
          <PathText>{clipSourceText(file.split(/[\\/]/).at(-1) || '未命名文件', 120)}</PathText>
          <p>{kind === 'log' ? '日志' : '加载顺序'} · {readStatusLabels[status] ?? '读取状态未知'}
            {encoding && ` · ${clipSourceText(encoding, 32)}`}{truncated && ' · 已截断'}</p>
          {notes.map((note, index) => <p key={index}>{clipSourceText(note, 256)}</p>)}
        </ItemContent>
      </Item>)}
    </ItemGroup>
  </details>;
}

function ReplacementEffects({ plan }: { plan: FilePlan }) {
  return <div className="plan-preview flex min-w-0 flex-col gap-3">
    <p className="wrap-anywhere">原替换事务：{plan.summary}</p>
    <Table aria-label="回滚文件影响" className="table-fixed">
      <colgroup><col className="w-[36%]" /><col /></colgroup>
      <TableHeader><TableRow><TableHead scope="col">撤销效果</TableHead><TableHead scope="col">文件路径</TableHead></TableRow></TableHeader>
      <TableBody>{[...plan.entries].reverse().map(({ operation, before }, index) => {
        const effect = operation.op === 'move' ? (before ? '移回原位置并还原覆盖' : '移回原位置')
          : operation.op === 'copy' ? (before ? '还原覆盖前文件' : '移出新增文件')
            : operation.op === 'mkdir' ? '移除本次创建的空目录' : '撤销原操作';
        return <TableRow key={index}>
          <TableCell className="align-top whitespace-normal">{effect}</TableCell>
          <TableCell className="whitespace-normal">
            <PathText className="block">{operation.dst}</PathText>
            {operation.op === 'move' && operation.src && <><small className="text-muted-foreground">原位置</small><PathText className="block">{operation.src}</PathText></>}
          </TableCell>
        </TableRow>;
      })}</TableBody>
    </Table>
    <PathText>事务 {plan.id}</PathText>
  </div>;
}

function DetailEmptyState({ title, action, loading = false }: { title: string; action?: ReactNode; loading?: boolean }) {
  return <Empty className="border">
    <EmptyHeader role={loading ? 'status' : undefined}>
      <EmptyMedia variant="icon">{loading ? <Spinner /> : <InboxIcon aria-hidden="true" />}</EmptyMedia>
      <EmptyTitle role="heading" aria-level={2}>{title}</EmptyTitle>
    </EmptyHeader>
    {action && <EmptyContent>{action}</EmptyContent>}
  </Empty>;
}

export default function GameDetailPage() {
  const gameId = useUI(s => s.gameId);
  const navigate = useUI(s => s.navigate);
  if (!gameId) return <Page><PageHeader title="游戏详情" /><DetailEmptyState title="尚未选择游戏" action={<Button onClick={() => navigate('library')}><ArrowLeftIcon data-icon="inline-start" />返回游戏库</Button>} /></Page>;
  return <GameDetails key={gameId} gameId={gameId} />;
}

function GameDetails({ gameId }: { gameId: string }) {
  const navigate = useUI(s => s.navigate);
  const client = useQueryClient();
  const reviewPlan = useUI(s => s.reviewPlan);
  const notify = useUI(s => s.notify);
  const [tab, setTab] = useState('mods');
  const [fileRootPath, setFileRootPath] = useState<string | null>(null);
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
  const adapter = useBackendQuery<Adapter>(['game-adapter', gameId], 'get_game_adapter', { gameId }, !!game.data?.adapterSlug);
  const bootstrap = useBackendQuery<{ recoveryError: string | null }>(['bootstrap'], 'get_bootstrap_state');
  const writable = backendAvailable && !!bootstrap.data && !bootstrap.data.recoveryError;
  const installs = useBackendQuery<Installed[]>(['installs', gameId], 'list_installs', { gameId }, !!gameId);
  const rollbackCurrent = !!rollback && replacementTransaction(installs.data?.find(mod => mod.id === rollback.installId)) === rollback.plan.id;
  const snapshots = useBackendQuery<Snapshot[]>(['snapshots', gameId], 'list_snapshots', { gameId }, !!gameId);
  const roots = useBackendQuery<Record<string, string>>(['user-roots', gameId], 'list_user_roots', { gameId }, !!gameId);
  function refresh() {
    void installs.refetch(); void snapshots.refetch(); void game.refetch();
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
        await client.invalidateQueries();
      }
    });
  }
  async function importArchive() {
    if (!writable || busy) return;
    await run(async () => {
      const selected = await open({ multiple: false, directory: false, title: '选择本地 Mod 归档', filters: [{ name: 'Mod 归档', extensions: ['zip', 'jar', 'mrpack', '7z', 'gz', 'tgz'] }] });
      if (typeof selected !== 'string') return;
      const result = await api<{ filename: string; artifactPath: string; sha256: string; plan: FilePlan }>('plan_import_archive', { path: selected });
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
  const fileRoots = [{ name: '游戏目录', path: game.data?.rootPath ?? '' },
    ...Object.entries(roots.data ?? {}).map(([name, path]) => ({ name, path }))];
  const fileRoot = fileRoots.find(root => root.path === fileRootPath) ?? fileRoots[0];
  const installTypeNotes = adapter.data?.modTypes.find(type => type.id === modType)?.notes;

  return <Page className="min-w-0" data-testid="game-detail-page">
    <div className="flex items-center gap-2 text-sm text-muted-foreground">
      <IconButton title="返回游戏库" onClick={() => navigate('library')}><ArrowLeftIcon data-icon="inline-start" /></IconButton>
      <span>游戏库 / 游戏详情</span>
    </div>
    <ErrorState error={game.error} />
    <ErrorState error={error} />
    <Card>
      <CardHeader className="flex flex-wrap items-center justify-between gap-4">
        <div className="flex min-w-0 items-center gap-3">
          <div className="flex size-12 shrink-0 items-center justify-center rounded-lg border bg-muted">
            <Gamepad2Icon className="size-6" aria-hidden="true" />
          </div>
          <div className="flex min-w-0 flex-col gap-1 wrap-anywhere">
            <PageHeader title={game.data?.name ?? '正在读取游戏…'} />
            <CardDescription>
              <p>{game.data?.platform ?? '—'} · <span className="whitespace-nowrap">已装 Mod {game.data?.installedModCount ?? 0}</span> · <span className="whitespace-nowrap">已启用 {game.data?.enabledModCount ?? 0}</span></p>
            </CardDescription>
          </div>
        </div>
        <Button disabled={!writable || busy} onClick={() => setInstallOpen(true)}><PackagePlusIcon data-icon="inline-start" />安装归档</Button>
      </CardHeader>
    </Card>

    <Tabs className="min-w-0 gap-4" value={tab} onValueChange={setTab}>
      <TabsList className="w-full sm:w-fit">
        <TabsTrigger value="mods">已装 Mod</TabsTrigger>
        <TabsTrigger value="files">文件</TabsTrigger>
        <TabsTrigger value="snapshots">快照</TabsTrigger>
        <TabsTrigger value="diagnostics">诊断</TabsTrigger>
      </TabsList>

      <TabsContent value="files" forceMount className={tab === 'files' ? 'min-w-0' : 'hidden'}>
        <ErrorState error={roots.error} />
        {fileRoot.path ? <GameFileBrowser key={fileRoot.path} root={fileRoot} roots={fileRoots} onRootChange={setFileRootPath}
          active={tab === 'files'} busy={busy} writable={writable}
          onTrash={path => preview('trash_paths', { request: { paths: [path], game_id: gameId, reason: '用户从文件浏览器移除' } })} />
          : game.isLoading && <DetailEmptyState loading title="正在读取游戏目录…" />}
      </TabsContent>

      <TabsContent value="mods" className="flex min-w-0 flex-col gap-3">
        <ErrorState error={installs.error} />
        <Card className="min-w-0 py-0">
          <Table aria-busy={installs.isFetching}>
            <TableHeader><TableRow><TableHead>名称</TableHead><TableHead>版本</TableHead><TableHead>来源</TableHead><TableHead>状态</TableHead><TableHead>安装时间</TableHead><TableHead /></TableRow></TableHeader>
            <TableBody>{(installs.data ?? []).map(mod => <TableRow key={mod.id}>
              <TableCell className="min-w-28 whitespace-normal"><strong className="wrap-anywhere">{mod.name}</strong><small className="block text-muted-foreground">{mod.files.length} 个文件</small></TableCell>
              <TableCell className="min-w-24 wrap-anywhere whitespace-normal">{mod.version ?? '—'}{mod.downloadOrigin && <PathText className="block">版本 ID：{mod.downloadOrigin.versionId}</PathText>}</TableCell>
              <TableCell className="min-w-28 wrap-anywhere whitespace-normal">{mod.downloadOrigin?.sourceId ?? (mod.source === 'local' ? '本地归档' : mod.source)}
                {mod.downloadOrigin ? <PathText className="block">项目 ID：{mod.downloadOrigin.projectId}</PathText> : mod.source !== 'local' && <small className="block text-muted-foreground">未绑定下载记录</small>}
              </TableCell>
              <TableCell><Badge tone={mod.status === 'enabled' ? 'success' : 'warning'}>{statuses[mod.status] ?? mod.status}</Badge></TableCell>
              <TableCell>{date(mod.installedAt)}</TableCell>
              <TableCell><div className="flex justify-end gap-2">
                <IconButton title={mod.status === 'partially_restored' ? '部分恢复：需先处理剩余回收站文件' : '预览卸载'} disabled={!writable || busy || mod.status !== 'enabled'} onClick={() => preview('plan_uninstall', { installId: mod.id })}><Trash2Icon data-icon="inline-start" /></IconButton>
                {replacementTransaction(mod) && <IconButton title="预览回滚替换" disabled={!writable || busy || installs.isFetching} onClick={() => { void previewReplacementRollback(mod); }}><Undo2Icon data-icon="inline-start" /></IconButton>}
              </div></TableCell>
            </TableRow>)}</TableBody>
          </Table>
        </Card>
        {!installs.data?.length && !installs.error && <DetailEmptyState loading={installs.isLoading} title={installs.isLoading ? '正在读取 Mod…' : '尚未安装 Mod'} action={<Button disabled={!writable} onClick={() => setInstallOpen(true)}><PackagePlusIcon data-icon="inline-start" />选择归档</Button>} />}
      </TabsContent>

      <TabsContent value="snapshots" className="flex min-w-0 flex-col gap-3">
        <Card className="min-w-0">
          <CardHeader>
            <div className="flex flex-wrap items-center gap-2">
              <Field className="min-w-0 flex-1 basis-40">
                <FieldLabel htmlFor="snapshot-label" className="sr-only">快照名称</FieldLabel>
                <Input id="snapshot-label" placeholder="快照名称" value={snapshotLabel} onChange={e => setSnapshotLabel(e.target.value)} />
              </Field>
              <Button variant="outline" disabled={!writable || busy || !snapshotLabel.trim()} onClick={() => run(async () => {
                const [, plan] = await api<[Snapshot, FilePlan]>('snapshot_create', { gameId, label: snapshotLabel });
                reviewPlan(plan, () => { void snapshots.refetch(); });
              })}><CameraIcon data-icon="inline-start" />预览创建快照</Button>
            </div>
          </CardHeader>
          <CardContent className="flex min-w-0 flex-col gap-3">
            <ErrorState error={snapshots.error} />
            <Table aria-busy={snapshots.isFetching}>
              <TableHeader><TableRow><TableHead>名称</TableHead><TableHead>文件</TableHead><TableHead>大小</TableHead><TableHead>创建时间</TableHead><TableHead /></TableRow></TableHeader>
              <TableBody>{(snapshots.data ?? []).map(item => <TableRow key={item.id}>
                <TableCell className="min-w-24 wrap-anywhere whitespace-normal">{item.label}</TableCell><TableCell>{item.file_count}</TableCell>
                <TableCell>{bytes(item.total_size)}</TableCell><TableCell>{date(item.created_at)}</TableCell>
                <TableCell><Button variant="outline" disabled={!writable || busy} onClick={() => preview('restore_snapshot', { gameId, snapshotId: item.id })}><Undo2Icon data-icon="inline-start" />恢复预览</Button></TableCell>
              </TableRow>)}</TableBody>
            </Table>
            {!snapshots.data?.length && <DetailEmptyState title="尚无快照" />}
          </CardContent>
        </Card>
      </TabsContent>

      <TabsContent value="diagnostics" className="flex min-w-0 flex-col gap-4">
        <Card className="min-w-0">
          <CardHeader className="flex flex-wrap items-end justify-between gap-4">
            <div className="flex min-w-0 flex-col gap-1">
              <CardTitle>游戏诊断</CardTitle>
              <CardDescription>仅分析管理中的目录与日志</CardDescription>
            </div>
            <div className="flex min-w-0 flex-wrap items-end gap-2">
              <Field className="w-auto gap-1">
                <FieldLabel htmlFor="diagnostic-log-encoding">日志编码</FieldLabel>
                <NativeSelect id="diagnostic-log-encoding" value={legacyLogEncoding ?? ''} disabled={!backendAvailable || busy} onChange={event => {
                  const value = event.currentTarget.value;
                  setLegacyLogEncoding(legacyLogEncodings.find(encoding => encoding === value) ?? null);
                  setDiagnosis(null);
                }}>
                  <NativeSelectOption value="">默认（不回退）</NativeSelectOption>
                  {legacyLogEncodings.map(encoding => <NativeSelectOption value={encoding} key={encoding}>回退：{encoding}</NativeSelectOption>)}
                </NativeSelect>
              </Field>
              <Button variant="outline" disabled={!backendAvailable || busy} onClick={runDiagnosis}>
                {busy ? <Spinner data-icon="inline-start" /> : <ShieldCheckIcon data-icon="inline-start" />}
                {busy ? '正在诊断…' : '开始诊断'}
              </Button>
            </div>
          </CardHeader>
        </Card>
        {diagnosis ? <div className="grid min-w-0 items-start gap-4 xl:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
          <section className="flex min-w-0 flex-col gap-4">
            {diagnosis.fabricStartup && <FabricStartupEvidence report={diagnosis.fabricStartup} />}
            {diagnosis.fabricDisk && <FabricDiskEvidence report={diagnosis.fabricDisk} />}
            {diagnosis.bethesdaPlugins && <BethesdaPluginEvidence report={diagnosis.bethesdaPlugins} />}
            <Card className="min-w-0">
              <CardContent className="flex min-w-0 flex-col items-start gap-3">
                <Badge>{diagnosis.status === 'findings' ? '已发现诊断线索' : '未确认故障原因'}</Badge>
                {diagnosis.causes.length > 0 && <ItemGroup className="w-full min-w-0 gap-2">
                  {diagnosis.causes.map((cause, i) => <Item role="listitem" variant="outline" size="sm" key={i}>
                    <ItemContent className="min-w-0 wrap-anywhere">
                      <h3 className="font-medium">{cause.summary}</h3>
                      <ItemDescription className="line-clamp-none">{cause.basis}</ItemDescription>
                    </ItemContent>
                  </Item>)}
                </ItemGroup>}
                {diagnosis.unknowns.map((text, i) => <Alert role="note" variant="warning" key={i}>
                  <TriangleAlertIcon /><AlertDescription className="wrap-anywhere">{text}</AlertDescription>
                </Alert>)}
                {diagnosis.repairSuggestions.length > 0 && <ItemGroup className="w-full min-w-0 gap-2">
                  {diagnosis.repairSuggestions.map((suggestion, i) => <Item role="listitem" variant="outline" size="sm" key={i}>
                    <ItemContent className="min-w-0 basis-48 wrap-anywhere"><p>{suggestion.description}</p></ItemContent>
                    {suggestion.fileOpsInput && <ItemActions>
                      <Button variant="outline" disabled={!writable || busy} onClick={() => preview('plan_file_ops', { operations: suggestion.fileOpsInput?.ops, summary: suggestion.fileOpsInput?.summary })}>预览修复</Button>
                    </ItemActions>}
                  </Item>)}
                </ItemGroup>}
              </CardContent>
            </Card>
          </section>
          <aside className="min-w-0">
            <Card>
              <CardHeader><CardTitle>诊断证据</CardTitle></CardHeader>
              <CardContent className="flex min-w-0 flex-col gap-3">
                <DiagnosticSources files={diagnosis.logs?.files ?? []} />
                {diagnosis.evidence.length > 0 && <ItemGroup className="gap-2">
                  {diagnosis.evidence.map((e, i) => <Item role="listitem" variant="muted" size="sm" key={i}>
                    <ItemContent className="min-w-0 wrap-anywhere"><PathText>{e.file}{e.line ? `:${e.line}` : ''}</PathText><p>{e.text}</p></ItemContent>
                  </Item>)}
                </ItemGroup>}
              </CardContent>
            </Card>
          </aside>
        </div> : <DetailEmptyState title="尚未运行诊断" />}
        <Card className="min-w-0">
          <CardHeader><CardTitle>用户管理目录</CardTitle></CardHeader>
          <CardContent className="flex min-w-0 flex-col gap-3">
            <ErrorState error={roots.error} />
            {Object.keys(roots.data ?? {}).length > 0 && <ItemGroup className="gap-2">
              {Object.entries(roots.data ?? {}).map(([id, value]) => <Item role="listitem" size="sm" variant="outline" key={id}>
                <ItemContent className="min-w-0"><ItemTitle className="max-w-full wrap-anywhere">{id}</ItemTitle><PathText>{value}</PathText></ItemContent>
              </Item>)}
            </ItemGroup>}
            <FieldGroup className="flex-row flex-wrap items-end gap-2">
              <Field className="min-w-0 flex-1 basis-36">
                <FieldLabel htmlFor="managed-root-id" className="sr-only">适配包目录 ID</FieldLabel>
                <Input id="managed-root-id" placeholder="适配包目录 ID" value={rootId} onChange={e => setRootId(e.target.value)} />
              </Field>
              <Field className="min-w-0 flex-1 basis-52">
                <FieldLabel htmlFor="managed-root-path" className="sr-only">本机目录完整路径</FieldLabel>
                <Input id="managed-root-path" placeholder="本机目录完整路径" value={rootPath} onChange={e => setRootPath(e.target.value)} />
              </Field>
              <Button variant="outline" disabled={!writable || busy || !rootId || !rootPath} onClick={() => run(async () => {
                await api('register_user_root', { gameId, rootId, directory: rootPath });
                await roots.refetch();
                notify('用户目录已添加');
              })}>添加目录</Button>
            </FieldGroup>
          </CardContent>
        </Card>
      </TabsContent>
    </Tabs>

    {rollback && <Modal title="回滚版本替换" onClose={() => { if (!busy) setRollback(null); }} actions={<>
      <Button variant="outline" disabled={busy} onClick={() => setRollback(null)}>取消</Button>
      <Button disabled={!writable || busy || installs.isFetching || !rollbackCurrent} onClick={() => { void confirmReplacementRollback(); }}>
        {busy ? <Spinner data-icon="inline-start" /> : <Undo2Icon data-icon="inline-start" />}
        {busy ? '正在回滚…' : '确认回滚'}
      </Button>
    </>}>
      <Alert role="note" variant="warning">
        <TriangleAlertIcon />
        <AlertDescription className="wrap-anywhere">将撤销 {rollback.name}{rollback.version ? ` · ${rollback.version}` : ''} 的版本替换，以下为要撤销的文件影响。</AlertDescription>
      </Alert>
      <ReplacementEffects plan={rollback.plan} />
      <ErrorState error={rollbackError || (!rollbackCurrent && '安装记录已变化，不能继续回滚此替换。')} />
    </Modal>}
    {installOpen && <Modal title="归档安装预览" onClose={() => { if (!busy) setInstallOpen(false); }} actions={<>
      <Button variant="outline" disabled={busy} onClick={() => setInstallOpen(false)}>取消</Button>
      <Button disabled={!writable || busy || !artifactPath.trim() || !filename.trim() || !modName.trim() || !modType.trim() || !adapter.data || (fomodPreview !== null && (fomodMappings.length === 0 || !validFomodSelection(fomodPreview, fomodSelections)))} onClick={() => run(async () => {
        const result = await api<{ plan: FilePlan; warnings: string[] }>('plan_install', { request: { gameId, artifactPath: artifactPath.trim(), filename: filename.trim(), name: modName.trim(), modType: modType.trim(), version: null, expectedSha256: artifactHash, folderName: null, mappings: fomodMappings, source: 'local', sourceModId: null } });
        setInstallOpen(false);
        if (result.warnings.length) notify(result.warnings.join('；'));
        reviewPlan(result.plan, refresh);
      })}>生成变更预览</Button>
    </>}>
      <ErrorState error={error || adapter.error} />
      <div className="flex flex-wrap gap-2">
        <Button variant="outline" disabled={!writable || busy} onClick={() => { void importArchive(); }}><FolderOpenIcon data-icon="inline-start" />导入本地归档</Button>
        <Button variant="outline" disabled={!writable || busy || !artifactPath.trim()} onClick={() => { void previewFomod(); }}>扫描 FOMOD 配置</Button>
      </div>
      <FieldGroup className="gap-4">
        <Field>
          <FieldLabel htmlFor="install-artifact-path">管理器内归档路径</FieldLabel>
          <Input id="install-artifact-path" aria-label="管理器内归档路径" disabled={busy} value={artifactPath} placeholder="downloads 或 staging 内已完成的归档" onChange={e => { setArtifactPath(e.target.value); setArtifactHash(null); setFomodPreview(null); setFomodSelections({}); }} />
        </Field>
        <Field>
          <FieldLabel htmlFor="install-filename">原始文件名</FieldLabel>
          <Input id="install-filename" aria-label="原始文件名" disabled={busy} value={filename} placeholder="例如 MyMod.zip" onChange={e => { setFilename(e.target.value); setArtifactHash(null); setFomodPreview(null); setFomodSelections({}); }} />
        </Field>
        <div className="grid min-w-0 gap-4 sm:grid-cols-2">
          <Field>
            <FieldLabel htmlFor="install-mod-name">Mod 名称</FieldLabel>
            <Input id="install-mod-name" aria-label="Mod 名称" value={modName} onChange={e => setModName(e.target.value)} />
          </Field>
          <Field className="min-w-0 [&>[data-slot=native-select-wrapper]]:w-full">
            <FieldLabel htmlFor="install-mod-type">适配包 Mod 类型</FieldLabel>
            <NativeSelect id="install-mod-type" aria-label="适配包 Mod 类型" value={modType} disabled={busy || !adapter.data} onChange={e => { setModType(e.target.value); setFomodPreview(null); setFomodSelections({}); }}>
              <NativeSelectOption value="">选择 Mod 类型</NativeSelectOption>
              {adapter.data?.modTypes.map(type => <NativeSelectOption key={type.id} value={type.id}>{type.name}</NativeSelectOption>)}
            </NativeSelect>
          </Field>
        </div>
      </FieldGroup>
      {installTypeNotes && <Alert role="note" variant="warning"><TriangleAlertIcon /><AlertDescription className="wrap-anywhere">{installTypeNotes}</AlertDescription></Alert>}
      {fomodPreview && <FomodOptions preview={fomodPreview} selections={fomodSelections} disabled={busy} onChange={setFomodSelections} />}
    </Modal>}
  </Page>;
}
