import { tr } from "@/lib/i18n";
import { displayPath, pathBasename } from "@/lib/paths";
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { open } from '@tauri-apps/plugin-dialog';
import { ArrowLeftIcon, CameraIcon, FolderOpenIcon, Gamepad2Icon, InboxIcon, ShieldCheckIcon, TriangleAlertIcon, Undo2Icon } from 'lucide-react';
import { BethesdaPluginEvidence, type BethesdaEvidence } from '@/components/BethesdaPluginEvidence';
import { FabricDiskEvidence, type FabricDisk } from '@/components/FabricDiskEvidence';
import { FabricStartupEvidence, type FabricStartup } from '@/components/FabricStartupEvidence';
import { ModActionReview } from '@/components/ModActionReview';
import { ModManagementTable } from '@/components/ModManagementTable';
import { Switch } from '@/components/ui/switch';
import { Label } from '@/components/ui/label';
import { batchIntent, buildModRows, replacementTransaction, type BatchAction, type Installed, type LibraryMod, type ModAction, type ModRow, type Removal } from '@/lib/mod-management';
import { GameFileBrowser } from '@/components/GameFileBrowser';
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
import { findPendingModSwitch, retryModSwitchReadback, useModSwitch, type ModSwitchTarget } from '@/lib/mod-switch-store';
import { batchLabels, startModTurn, type AgentModItem, type BatchResult } from '@/lib/mod-agent-actions';
import { api, backendAvailable, errorText, useBackendQuery } from '@/lib/api';
import { useParams } from "react-router";
import { currentRoute } from "@/router";
import { useUI } from '@/lib/store';
import type { FilePlan, Game } from '@/lib/types';

type ReplacementRollback = { installId: string; name: string; version: string | null; plan: FilePlan };
type Snapshot = { id: string; label: string; file_count: number; total_size: number; created_at: string };
const legacyLogEncodings = ['windows-1252', 'windows-1251', 'gbk', 'big5', 'shift_jis'] as const;
const readStatusLabels = {
  get read() { return tr("已读取"); }, get missing() { return tr("文件缺失"); }, get denied() { return tr("拒绝读取"); }, get binary() { return tr("二进制内容"); },
  get unknown_encoding() { return tr("编码未知"); }, get io_error() { return tr("读取失败"); }, get limit_reached() { return tr("达到读取上限"); },
};
type DiagnosticFile = {
  file: string; kind: 'log' | 'load_order'; status: keyof typeof readStatusLabels;
  encoding: string | null; truncated: boolean; notes: string[]; continued?: boolean; continues?: boolean;
};
type Diagnosis = {
  status: string; causes: { summary: string; basis: string }[]; unknowns: string[];
  evidence: { file: string; line: number | null; text: string }[];
  repairSuggestions: { description: string; fileOpsInput: { ops: unknown[]; summary: string } | null }[];
  logs?: { files: DiagnosticFile[]; nextCursor?: string | null };
  fabricStartup?: FabricStartup;
  fabricDisk?: FabricDisk;
  bethesdaPlugins?: BethesdaEvidence;
};
function clipSourceText(text: string, limit: number) {
  return text.length <= limit ? text : `${text.slice(0, limit - 1)}…`;
}

function DiagnosticSources({ files }: { files: DiagnosticFile[] }) {
  if (!files.length) return null;
  return <details className="min-w-0 rounded-md border p-3">
    <summary className="cursor-pointer rounded-sm text-sm font-medium focus-visible:outline-2 focus-visible:outline-ring">{tr("文件读取状态（{length}）", { length: files.length })}</summary>
    <ItemGroup className="mt-3 gap-2">
      {files.map(({ file, kind, status, encoding, truncated, notes, continued, continues }, i) => <Item role="listitem" size="sm" variant="muted" key={i}>
        <ItemContent className="min-w-0 wrap-anywhere">
          <PathText>{clipSourceText(pathBasename(file) || tr("未命名文件"), 120)}</PathText>
          <p>{kind === 'log' ? tr("日志") : tr("加载顺序")} · {readStatusLabels[status] ?? tr("读取状态未知")}
            {encoding && ` · ${clipSourceText(encoding, 32)}`}{truncated && tr(" · 已截断")}{continued && tr(" · 续读")}{continues && tr(" · 未读完")}</p>
          {notes.map((note, index) => <p key={index}>{clipSourceText(tr(note), 256)}</p>)}
        </ItemContent>
      </Item>)}
    </ItemGroup>
  </details>;
}

function ReplacementEffects({ plan }: { plan: FilePlan }) {
  return <div className="plan-preview flex min-w-0 flex-col gap-3">
    <p className="wrap-anywhere">{tr("原替换事务：{summary}", { summary: plan.summary })}</p>
    <Table aria-label={tr("回滚文件影响")} className="table-fixed">
      <colgroup><col className="w-[36%]" /><col /></colgroup>
      <TableHeader><TableRow><TableHead scope="col">{tr("撤销效果")}</TableHead><TableHead scope="col">{tr("文件路径")}</TableHead></TableRow></TableHeader>
      <TableBody>{[...plan.entries].reverse().map(({ operation, before }, index) => {
        const effect = operation.op === 'move' ? (before ? tr("移回原位置并还原覆盖") : tr("移回原位置"))
          : operation.op === 'copy' ? (before ? tr("还原覆盖前文件") : tr("移出新增文件"))
            : operation.op === 'mkdir' ? tr("移除本次创建的空目录") : tr("撤销原操作");
        return <TableRow key={index}>
          <TableCell className="align-top whitespace-normal">{effect}</TableCell>
          <TableCell className="whitespace-normal">
            <PathText className="block">{displayPath(operation.dst)}</PathText>
            {operation.op === 'move' && operation.src && <><small className="text-muted-foreground">{tr("原位置")}</small><PathText className="block">{displayPath(operation.src)}</PathText></>}
          </TableCell>
        </TableRow>;
      })}</TableBody>
    </Table>
    <PathText>{tr("事务 {id}", { id: plan.id })}</PathText>
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
  const { gameId } = useParams<"gameId">();
  const navigate = useUI(s => s.navigate);
  if (!gameId) return <Page><PageHeader title={tr("游戏详情")} /><DetailEmptyState title={tr("尚未选择游戏")} action={<Button onClick={() => navigate('library')}><ArrowLeftIcon data-icon="inline-start" />{tr("返回游戏库")}</Button>} /></Page>;
  return <GameDetails key={gameId} gameId={gameId} />;
}

function GameDetails({ gameId }: { gameId: string }) {
  const navigate = useUI(s => s.navigate);
  const client = useQueryClient();
  const reviewPlan = useUI(s => s.reviewPlan);
  const notify = useUI(s => s.notify);
  const [tab, setTab] = useState('mods');
  const [batchMode, setBatchMode] = useState(false);
  const [batchBusy, setBatchBusy] = useState(false);
  const [fileRootPath, setFileRootPath] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [rollback, setRollback] = useState<ReplacementRollback | null>(null);
  const [removal, setRemoval] = useState<Removal | null>(null);
  const [removalError, setRemovalError] = useState<unknown>(null);
  const [rollbackError, setRollbackError] = useState<unknown>(null);
  const [unindexedImport, setUnindexedImport] = useState<string | null>(null);
  const pendingSwitches = useModSwitch(s => s.pending);
  const startingRemoval = useRef(false);
  const [modActionError, setModActionError] = useState<string | null>(null);
  const modActionErrorRef = useRef<HTMLDivElement>(null);
  const unstartedConversations = useRef(new Map<string, string>());
  const [snapshotLabel, setSnapshotLabel] = useState('');
  const [rootId, setRootId] = useState('');
  const [rootPath, setRootPath] = useState('');
  const [legacyLogEncoding, setLegacyLogEncoding] = useState<(typeof legacyLogEncodings)[number] | null>(null);
  const diagnosisRequest = useRef<string | null>(null);
  const [diagnosing, setDiagnosing] = useState(false);
  const [diagnosis, setDiagnosis] = useState<Diagnosis | null>(null);
  const game = useBackendQuery<Game>(['game', gameId], 'get_game', { id: gameId }, !!gameId);
  const library = useBackendQuery<LibraryMod[]>(['mod-library', gameId], 'list_mod_library', { gameId });
  const bootstrap = useBackendQuery<{ recoveryError: string | null }>(['bootstrap'], 'get_bootstrap_state');
  const writable = backendAvailable && !!bootstrap.data && !bootstrap.data.recoveryError;
  const installs = useBackendQuery<Installed[]>(['installs', gameId], 'list_installs', { gameId }, !!gameId);
  const modRows = useMemo(() => buildModRows(gameId, installs.data, library.data), [gameId, installs.data, library.data]);
  const gameSwitches = Object.values(pendingSwitches).filter(entry => entry.gameId === gameId);
  const reviews = new Map<string, { id: string; conversationId: string; turnId: string; name: string; kind: BatchAction; count: number }>();
  for (const entry of gameSwitches) {
    if (entry.status !== 'running') continue;
    const key = JSON.stringify([entry.conversationId, entry.turnId]);
    const review = reviews.get(key);
    if (review) review.count++;
    else reviews.set(key, { ...entry, count: 1 });
  }
  const modWritesDisabled = !writable || busy || installs.isError || library.isError;
  const modsLoading = installs.isLoading || library.isLoading;
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
  function stopDiagnosis() {
    const requestId = diagnosisRequest.current;
    diagnosisRequest.current = null;
    if (requestId) void api('diagnose_game', { gameId, options: { page: { requestId, cancel: true } } }).catch(() => { });
    setDiagnosis((current) => current?.logs && { ...current, logs: { ...current.logs, nextCursor: null } } || current);
  }
  useEffect(() => () => {
    const requestId = diagnosisRequest.current;
    diagnosisRequest.current = null;
    if (requestId) void api('diagnose_game', { gameId, options: { page: { requestId, cancel: true } } }).catch(() => { });
  }, [gameId]);
  async function runDiagnosis(cursor?: string) {
    await run(async () => {
      if (!cursor) { stopDiagnosis(); setDiagnosis(null); }
      const requestId = cursor ?? crypto.randomUUID();
      diagnosisRequest.current = requestId;
      setDiagnosing(true);
      try {
        const options = { ...(legacyLogEncoding ? { legacyLogEncoding } : {}), page: cursor ? { cursor } : { requestId } };
        const page = await api<Diagnosis>('diagnose_game', { gameId, options });
        if (diagnosisRequest.current !== requestId) return;
        diagnosisRequest.current = page.logs?.nextCursor ?? null;
        setDiagnosis(page);
      } catch (error) {
        if (diagnosisRequest.current === requestId) { stopDiagnosis(); throw error; }
      } finally { setDiagnosing(false); }
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
      if (plan.id !== transactionId) throw new Error(tr("无法读取对应的替换事务。"));
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
          throw new Error(tr("安装记录已变化，不能继续回滚此替换。"));
        }
      } catch (e) { setRollbackError(e); return; }
      try {
        await api('rollback_transaction', { transactionId: rollback.plan.id });
        setRollback(null);
        notify(tr("版本替换已回滚"));
      } catch (e) { setRollbackError(e); }
      finally {
        await client.invalidateQueries();
      }
    });
  }
  useEffect(() => {
    if (modActionError) modActionErrorRef.current?.scrollIntoView({ block: 'nearest' });
  }, [modActionError]);
  async function startModAction(action: ModAction, target: ModSwitchTarget) {
    if (modWritesDisabled || startingRemoval.current) return;
    const kind = 'artifactId' in action ? 'install' : action.remove ? 'remove' : 'uninstall';
    if (kind === 'remove') { startingRemoval.current = true; setBusy(true); }
    setModActionError(null);
    try {
      const result = await startModTurn(client, unstartedConversations.current, gameId, kind,
        [{ key: 'artifactId' in action ? action.artifactId : action.installId, action, target }], false);
      if (result.failures.length) {
        const message = result.failures.join('；');
        const state = currentRoute();
        if (state.view === 'detail' && state.gameId === gameId) setModActionError(message);
        else notify(message);
      }
    } finally {
      if (kind === 'remove') { startingRemoval.current = false; setBusy(false); }
    }
  }
  async function runModBatch(kind: BatchAction, reviewed: ModRow[]): Promise<BatchResult> {
    // Direct removals may run after Agent setup or another write. Recheck the reviewed
    // action against current queries without rebuilding a large table for every row.
    let index: { installs: Installed[]; library: LibraryMod[]; rows: Map<string, ModRow> } | undefined;
    function eligible(previous: ModRow) {
      const installs = client.getQueryState<Installed[]>(['installs', gameId]);
      const library = client.getQueryState<LibraryMod[]>(['mod-library', gameId]);
      if (!backendAvailable || client.getQueryData<{ recoveryError: string | null }>(['bootstrap'])?.recoveryError !== null ||
          installs?.status !== 'success' || !installs.data || library?.status !== 'success' || !library.data) {
        throw new Error(tr("当前不能写入，请刷新 Mod 列表后重试。"));
      }
      if (!index || index.installs !== installs.data || index.library !== library.data) {
        const rows = buildModRows(gameId, installs.data, library.data);
        index = { installs: installs.data, library: library.data, rows: new Map(rows.map(row => [row.key, row])) };
      }
      const row = index.rows.get(previous.key);
      const intent = row && batchIntent(kind, row);
      return row && intent && intent === batchIntent(kind, previous) && !findPendingModSwitch(useModSwitch.getState().pending, row.target) ? row : undefined;
    }
    const agentItems: AgentModItem[] = [];
    const direct: ModRow[] = [];
    let skipped = 0;
    for (const previous of reviewed) {
      const row = eligible(previous);
      if (!row) {
        skipped++;
      } else if (kind === 'remove' && row.removal && row.removal.kind !== 'installed') {
        direct.push(row);
      } else {
        const action = kind === 'install' ? row.install! : { installId: row.target.installId!, name: row.item.name, remove: kind === 'remove' };
        agentItems.push({ key: row.key, target: row.target, action });
      }
    }
    const launch = await startModTurn(client, unstartedConversations.current, gameId, kind, agentItems, true);
    const removed: string[] = [];
    for (const row of direct) {
      try {
        const removal = eligible(row)?.removal;
        if (!removal) { skipped++; continue; }
        await api('remove_managed_mod', { gameId, target: { kind: removal.kind, id: removal.id } });
        removed.push(row.key);
      } catch (error) { launch.failures.push(`${row.item.name}：${errorText(error)}`); }
    }
    return { ...launch, removed, skipped: skipped + launch.skipped };
  }
  async function refreshBatchRemovals() {
    await Promise.all([['installs', gameId], ['mod-library', gameId], ['game', gameId], ['games']].map(queryKey =>
      client.invalidateQueries({ queryKey }, { throwOnError: true })));
  }
  function askRemoval(target: Removal) {
    setRemovalError(null);
    setRemoval(target);
  }
  async function confirmRemoval() {
    if (!removal || busy) return;
    const { kind, id, name } = removal;
    if (kind === 'installed') {
      setRemoval(null);
      await startModAction({ installId: id, name, remove: true }, { gameId, installId: id });
      return;
    }
    await run(async () => {
      try {
        await api('remove_managed_mod', { gameId, target: { kind, id } });
        setRemoval(null);
        await Promise.all([installs.refetch(), library.refetch(), game.refetch()]);
        notify(kind === 'plan' ? tr("安装计划已取消，记录已移除") : tr("已从管理列表删除"));
      } catch (error) { setRemovalError(error); }
    });
  }
  async function indexImport(transactionId: string) {
    await run(async () => {
      setUnindexedImport(transactionId);
      await api<LibraryMod>('register_imported_mod', { gameId, transactionId });
      setUnindexedImport(null);
      void client.invalidateQueries({ queryKey: ['mod-library', gameId] });
      notify(tr("归档已导入，可在 Mod 管理列表中安装"));
    });
  }
  async function importArchive() {
    if (!writable || busy || unindexedImport) return;
    await run(async () => {
      const selected = await open({ multiple: false, directory: false, title: tr("选择本地 Mod 归档"), filters: [{ name: tr("Mod 归档"), extensions: ['zip', 'jar', 'mrpack', '7z', 'rar', 'gz', 'tgz'] }] });
      if (typeof selected !== 'string') return;
      const { plan } = await api<{ plan: FilePlan }>('plan_import_archive', { path: selected });
      reviewPlan(plan, () => { void indexImport(plan.id); });
    });
  }
  const fileRoots = [{ name: tr("游戏目录"), path: game.data?.rootPath ?? '' },
  ...Object.entries(roots.data ?? {}).map(([name, path]) => ({ name, path }))];
  const fileRoot = fileRoots.find(root => root.path === fileRootPath) ?? fileRoots[0];

  return <Page className="min-w-0" data-testid="game-detail-page">
    <div className="flex items-center gap-2 text-sm text-muted-foreground">
      <IconButton title={tr("返回游戏库")} onClick={() => navigate('library')}><ArrowLeftIcon data-icon="inline-start" /></IconButton>
      <span>{tr("游戏库 / 游戏详情")}</span>
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
            <PageHeader title={game.data?.name ?? tr("正在读取游戏…")} />
            <CardDescription>
              <p>{game.data?.platform ?? '—'} · <span className="whitespace-nowrap">{tr("已安装 {value1} 个 Mod", { value1: game.data?.installedModCount ?? 0 })}</span></p>
            </CardDescription>
          </div>
        </div>
        <Button disabled={!writable || busy || batchBusy || !!unindexedImport} onClick={() => { void importArchive(); }}><FolderOpenIcon data-icon="inline-start" />{tr("导入本地归档")}</Button>
      </CardHeader>
    </Card>

    <Tabs className="min-w-0 gap-4" value={tab} onValueChange={value => { setTab(value); if (!batchBusy) setBatchMode(false); }}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <TabsList className="w-full sm:w-fit">
          <TabsTrigger value="mods">{tr("Mod 管理")}</TabsTrigger>
          <TabsTrigger value="files">{tr("文件")}</TabsTrigger>
          <TabsTrigger value="snapshots">{tr("快照")}</TabsTrigger>
          <TabsTrigger value="diagnostics">{tr("诊断")}</TabsTrigger>
        </TabsList>
        {tab === 'mods' && <div className="ml-auto flex items-center gap-2">
          <Switch id="mod-batch-mode" aria-label={tr("批量操作")} checked={batchMode} disabled={batchBusy} onCheckedChange={setBatchMode} />
          <Label htmlFor="mod-batch-mode">{tr("批量操作")}</Label>
        </div>}
      </div>

      <TabsContent value="files" forceMount className={tab === 'files' ? 'min-w-0' : 'hidden'}>
        <ErrorState error={roots.error} />
        {fileRoot.path ? <GameFileBrowser key={fileRoot.path} root={fileRoot} roots={fileRoots} onRootChange={setFileRootPath}
          active={tab === 'files'} busy={busy} writable={writable}
          onTrash={path => preview('trash_paths', { request: { paths: [path], game_id: gameId, reason: tr("用户从文件浏览器移除") } })} />
          : game.isLoading && <DetailEmptyState loading title={tr("正在读取游戏目录…")} />}
      </TabsContent>

      <TabsContent value="mods" className="flex min-w-0 flex-col gap-3">
        <ErrorState error={installs.error} />
        <ErrorState error={library.error} />
        {(installs.isError || library.isError) && !gameSwitches.some(entry => entry.status === 'readback_failed') &&
          <Button variant="outline" size="sm" className="self-start" onClick={() => { void installs.refetch(); void library.refetch(); }}>{tr("刷新 Mod 列表")}</Button>}
        {unindexedImport && <Button className="self-start" variant="outline" disabled={!writable || busy} onClick={() => { void indexImport(unindexedImport); }}>{tr("重试添加到 Mod 库")}</Button>}
        <ModManagementTable key={gameId} gameId={gameId} rows={modRows} batchMode={batchMode} disabled={modWritesDisabled || batchBusy} fetching={installs.isFetching || library.isFetching}
          onStart={startModAction} onRemove={askRemoval} onRollback={previewReplacementRollback} onBatch={runModBatch}
          onBatchBusy={setBatchBusy} refreshRemovals={refreshBatchRemovals} />
        {modActionError && <div ref={modActionErrorRef}><ErrorState error={modActionError} /></div>}
        {!modRows.length && !installs.error && !library.error && <DetailEmptyState loading={modsLoading} title={modsLoading ? tr("正在读取 Mod…") : tr("暂无 Mod")} />}
      </TabsContent>

      <TabsContent value="snapshots" className="flex min-w-0 flex-col gap-3">
        <Card className="min-w-0">
          <CardHeader>
            <div className="flex flex-wrap items-center gap-2">
              <Field className="min-w-0 flex-1 basis-40">
                <FieldLabel htmlFor="snapshot-label" className="sr-only">{tr("快照名称")}</FieldLabel>
                <Input id="snapshot-label" placeholder={tr("快照名称")} value={snapshotLabel} onChange={e => setSnapshotLabel(e.target.value)} />
              </Field>
              <Button variant="outline" disabled={!writable || busy || !snapshotLabel.trim()} onClick={() => run(async () => {
                const [, plan] = await api<[Snapshot, FilePlan]>('snapshot_create', { gameId, label: snapshotLabel });
                reviewPlan(plan, () => { void snapshots.refetch(); });
              })}><CameraIcon data-icon="inline-start" />{tr("预览创建快照")}</Button>
            </div>
          </CardHeader>
          <CardContent className="flex min-w-0 flex-col gap-3">
            <ErrorState error={snapshots.error} />
            <Table aria-busy={snapshots.isFetching}>
              <TableHeader><TableRow><TableHead>{tr("名称")}</TableHead><TableHead>{tr("文件")}</TableHead><TableHead>{tr("大小")}</TableHead><TableHead>{tr("创建时间")}</TableHead><TableHead /></TableRow></TableHeader>
              <TableBody>{(snapshots.data ?? []).map(item => <TableRow key={item.id}>
                <TableCell className="min-w-24 wrap-anywhere whitespace-normal">{item.label}</TableCell><TableCell>{item.file_count}</TableCell>
                <TableCell>{bytes(item.total_size)}</TableCell><TableCell>{date(item.created_at)}</TableCell>
                <TableCell><Button variant="outline" disabled={!writable || busy} onClick={() => preview('restore_snapshot', { gameId, snapshotId: item.id })}><Undo2Icon data-icon="inline-start" />{tr("恢复预览")}</Button></TableCell>
              </TableRow>)}</TableBody>
            </Table>
            {!snapshots.data?.length && <DetailEmptyState title={tr("尚无快照")} />}
          </CardContent>
        </Card>
      </TabsContent>

      <TabsContent value="diagnostics" className="flex min-w-0 flex-col gap-4">
        <Card className="min-w-0">
          <CardHeader className="flex flex-wrap items-end justify-between gap-4">
            <div className="flex min-w-0 flex-col gap-1">
              <CardTitle>{tr("游戏诊断")}</CardTitle>
              <CardDescription>{tr("仅分析管理中的目录与日志")}</CardDescription>
            </div>
            <div className="flex min-w-0 flex-wrap items-end gap-2">
              <Field className="w-auto gap-1">
                <FieldLabel htmlFor="diagnostic-log-encoding">{tr("日志编码")}</FieldLabel>
                <NativeSelect id="diagnostic-log-encoding" value={legacyLogEncoding ?? ''} disabled={!backendAvailable || busy} onChange={event => {
                  const value = event.currentTarget.value;
                  stopDiagnosis();
                  setLegacyLogEncoding(legacyLogEncodings.find(encoding => encoding === value) ?? null);
                  setDiagnosis(null);
                }}>
                  <NativeSelectOption value="">{tr("默认（不回退）")}</NativeSelectOption>
                  {legacyLogEncodings.map(encoding => <NativeSelectOption value={encoding} key={encoding}>{tr("回退：{encoding}", { encoding })}</NativeSelectOption>)}
                </NativeSelect>
              </Field>
              <Button variant="outline" disabled={!backendAvailable || busy} onClick={() => void runDiagnosis()}>
                {busy ? <Spinner data-icon="inline-start" /> : <ShieldCheckIcon data-icon="inline-start" />}
                {diagnosing ? tr("正在诊断…") : tr("开始诊断")}
              </Button>
              {diagnosis?.logs?.nextCursor && <Button variant="outline" disabled={busy} onClick={() => void runDiagnosis(diagnosis.logs!.nextCursor!)}>{tr("下一批诊断")}</Button>}
              {(diagnosing || diagnosis?.logs?.nextCursor) && <Button variant="ghost" onClick={stopDiagnosis}>{tr("停止诊断")}</Button>}
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
                <Badge>{diagnosis.status === 'findings' ? tr("已发现诊断线索") : tr("未确认故障原因")}</Badge>
                {diagnosis.causes.length > 0 && <ItemGroup className="w-full min-w-0 gap-2">
                  {diagnosis.causes.map((cause, i) => <Item role="listitem" variant="outline" size="sm" key={i}>
                    <ItemContent className="min-w-0 wrap-anywhere">
                      <h3 className="font-medium">{tr(cause.summary)}</h3>
                      <ItemDescription className="line-clamp-none">{cause.basis}</ItemDescription>
                    </ItemContent>
                  </Item>)}
                </ItemGroup>}
                {diagnosis.unknowns.map((text, i) => <Alert role="note" variant="warning" key={i}>
                  <TriangleAlertIcon /><AlertDescription className="wrap-anywhere">{tr(text)}</AlertDescription>
                </Alert>)}
                {diagnosis.repairSuggestions.length > 0 && <ItemGroup className="w-full min-w-0 gap-2">
                  {diagnosis.repairSuggestions.map((suggestion, i) => <Item role="listitem" variant="outline" size="sm" key={i}>
                    <ItemContent className="min-w-0 basis-48 wrap-anywhere"><p>{tr(suggestion.description)}</p></ItemContent>
                    {suggestion.fileOpsInput && <ItemActions>
                      <Button variant="outline" disabled={!writable || busy} onClick={() => preview('plan_file_ops', { operations: suggestion.fileOpsInput?.ops, summary: suggestion.fileOpsInput?.summary })}>{tr("预览修复")}</Button>
                    </ItemActions>}
                  </Item>)}
                </ItemGroup>}
              </CardContent>
            </Card>
          </section>
          <aside className="min-w-0">
            <Card>
              <CardHeader><CardTitle>{tr("诊断证据")}</CardTitle></CardHeader>
              <CardContent className="flex min-w-0 flex-col gap-3">
                <DiagnosticSources files={diagnosis.logs?.files ?? []} />
                {diagnosis.evidence.length > 0 && <ItemGroup className="gap-2">
                  {diagnosis.evidence.map((e, i) => <Item role="listitem" variant="muted" size="sm" key={i}>
                    <ItemContent className="min-w-0 wrap-anywhere"><PathText>{displayPath(e.file)}{e.line ? `:${e.line}` : ''}</PathText><p>{e.text}</p></ItemContent>
                  </Item>)}
                </ItemGroup>}
              </CardContent>
            </Card>
          </aside>
        </div> : <DetailEmptyState title={tr("尚未运行诊断")} />}
        <Card className="min-w-0">
          <CardHeader><CardTitle>{tr("用户管理目录")}</CardTitle></CardHeader>
          <CardContent className="flex min-w-0 flex-col gap-3">
            <ErrorState error={roots.error} />
            {Object.keys(roots.data ?? {}).length > 0 && <ItemGroup className="gap-2">
              {Object.entries(roots.data ?? {}).map(([id, value]) => <Item role="listitem" size="sm" variant="outline" key={id}>
                <ItemContent className="min-w-0"><ItemTitle className="max-w-full wrap-anywhere">{id}</ItemTitle><PathText>{displayPath(value)}</PathText></ItemContent>
              </Item>)}
            </ItemGroup>}
            <FieldGroup className="flex-row flex-wrap items-end gap-2">
              <Field className="min-w-0 flex-1 basis-36">
                <FieldLabel htmlFor="managed-root-id" className="sr-only">{tr("适配包目录 ID")}</FieldLabel>
                <Input id="managed-root-id" placeholder={tr("适配包目录 ID")} value={rootId} onChange={e => setRootId(e.target.value)} />
              </Field>
              <Field className="min-w-0 flex-1 basis-52">
                <FieldLabel htmlFor="managed-root-path" className="sr-only">{tr("本机目录完整路径")}</FieldLabel>
                <Input id="managed-root-path" placeholder={tr("本机目录完整路径")} value={rootPath} onChange={e => setRootPath(e.target.value)} />
              </Field>
              <Button variant="outline" disabled={!writable || busy || !rootId || !rootPath} onClick={() => run(async () => {
                await api('register_user_root', { gameId, rootId, directory: rootPath });
                await roots.refetch();
                notify(tr("用户目录已添加"));
              })}>{tr("添加目录")}</Button>
            </FieldGroup>
          </CardContent>
        </Card>
      </TabsContent>
    </Tabs>

    {Array.from(reviews, ([key, review]) => <ModActionReview key={key} operationId={review.id}
      name={review.count === 1 ? review.name : tr("{value1} {count} 个 Mod", { value1: batchLabels[review.kind], count: review.count })}
      conversationId={review.conversationId} turnId={review.turnId} />)}
    {gameSwitches.map(entry => entry.status === 'readback_failed' ? <div className="flex flex-col items-start gap-2" key={entry.id}>
          <ErrorState error={entry.error} /><Button size="sm" variant="outline" onClick={() => retryModSwitchReadback(entry.id)}>{tr("重新读取状态")}</Button>
        </div> : null)}

    {removal && <Modal title={removal.kind === 'plan' ? tr("取消安装计划") : tr("从管理列表删除")} onClose={() => { if (!busy) setRemoval(null); }} actions={<>
      <Button variant="outline" disabled={busy} onClick={() => setRemoval(null)}>{tr("返回")}</Button>
      <Button variant="destructive" disabled={!writable || busy} onClick={() => { void confirmRemoval(); }}>{removal.kind === 'installed' ? tr("使用 Agent 删除") : removal.kind === 'plan' ? tr("确认取消计划") : tr("确认删除")}</Button>
    </>}>
      <p className="wrap-anywhere">{removal.kind === 'plan' ? tr("取消「{name}」的这次安装计划？同名的其他安装记录会保留。", { name: removal.name }) : removal.kind === 'installed' ? tr("Agent 会先检查「{name}」。若清单文件全部缺失，且无覆盖、共享或替换关系，只移除管理记录；若可安全卸载或回滚，成功后才移除记录。其他异常需先审核。", { name: removal.name }) : tr("从此游戏的管理列表移除「{name}」？磁盘上的原包和事务历史会保留。", { name: removal.name })}</p>
      <ErrorState error={removalError} />
    </Modal>}
    {rollback && <Modal title={tr("回滚版本替换")} onClose={() => { if (!busy) setRollback(null); }} actions={<>
      <Button variant="outline" disabled={busy} onClick={() => setRollback(null)}>{tr("取消")}</Button>
      <Button disabled={!writable || busy || installs.isFetching || !rollbackCurrent} onClick={() => { void confirmReplacementRollback(); }}>
        {busy ? <Spinner data-icon="inline-start" /> : <Undo2Icon data-icon="inline-start" />}
        {busy ? tr("正在回滚…") : tr("确认回滚")}
      </Button>
    </>}>
      <Alert role="note" variant="warning">
        <TriangleAlertIcon />
        <AlertDescription className="wrap-anywhere">{tr("将撤销 {name}{value2} 的版本替换，以下为要撤销的文件影响。", { name: rollback.name, value2: rollback.version ? ` · ${rollback.version}` : '' })}</AlertDescription>
      </Alert>
      <ReplacementEffects plan={rollback.plan} />
      <ErrorState error={rollbackError || (!rollbackCurrent && tr("安装记录已变化，不能继续回滚此替换。"))} />
    </Modal>}
  </Page>;
}
