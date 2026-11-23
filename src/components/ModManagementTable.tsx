import { useEffect, useMemo, useRef, useState } from 'react';
import { DownloadIcon, MinusIcon, PackageMinusIcon, TagIcon, Trash2Icon, Undo2Icon } from 'lucide-react';
import { EditModTags, ManageModTags, ModTagBadge, ModTagFilters, type ModTag } from '@/components/ModTags';
import { ModStatusSwitch } from '@/components/ModStatusSwitch';
import { ErrorState, IconButton, Modal, PathText, date } from '@/components/ui';
import { Button } from '@/components/ui/button';
import { Card } from '@/components/ui/card';
import { Checkbox } from '@/components/ui/checkbox';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { errorText, useBackendQuery } from '@/lib/api';
import { batchIntent, modStatusLabels, replacementTransaction, type BatchAction, type Installed, type ModAction, type ModRow, type Removal } from '@/lib/mod-management';
import { findPendingModSwitch, useModSwitch, type ModSwitchTarget } from '@/lib/mod-switch-store';
import { batchLabels as labels, type BatchResult } from '@/lib/mod-agent-actions';
import { useUI } from '@/lib/store';
import { cn } from '@/lib/utils';


const icons = { install: DownloadIcon, uninstall: PackageMinusIcon, remove: Trash2Icon };
const actions = ['install', 'uninstall', 'remove'] as const;

type BatchRequest = { action: BatchAction; rows: ModRow[]; skipped: number };

function requestFor(action: BatchAction, selected: ModRow[]): BatchRequest {
  const seen = new Set<string>();
  const rows = selected.filter(row => {
    const intent = batchIntent(action, row);
    if (!intent || seen.has(intent)) return false;
    seen.add(intent);
    return true;
  });
  return { action, rows, skipped: selected.length - rows.length };
}

export function ModManagementTable({ gameId, rows: allRows, batchMode, disabled, fetching, onStart, onRemove, onRollback, onBatch, onBatchBusy, refreshRemovals }: {
  gameId: string; rows: ModRow[]; batchMode: boolean; disabled: boolean; fetching: boolean;
  onStart: (action: ModAction, target: ModSwitchTarget) => void;
  onRemove: (removal: Removal) => void; onRollback: (record: Installed) => void;
  onBatch: (action: BatchAction, rows: ModRow[]) => Promise<BatchResult>;
  onBatchBusy: (busy: boolean) => void; refreshRemovals: () => Promise<void>;
}) {
  const pending = useModSwitch(state => state.pending);
  const tags = useBackendQuery<ModTag[]>(['mod-tags', gameId], 'list_mod_tags', { gameId }, !!gameId);
  const tagsById = useMemo(() => new Map(tags.data?.map(tag => [tag.id, tag])), [tags.data]);
  const [filter, setFilter] = useState('');
  const [manageTags, setManageTags] = useState(false);
  const [editId, setEditId] = useState<string | null>(null);
  const editing = allRows.find(row => row.target.installId === editId)?.item;
  const activeFilter = tagsById.has(filter) ? filter : '';
  const rows = useMemo(() => activeFilter ? allRows.filter(({ item }) => 'id' in item && item.tagIds?.includes(activeFilter)) : allRows, [allRows, activeFilter]);
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [review, setReview] = useState<BatchRequest | null>(null);
  const [progress, setProgress] = useState('');
  const [failures, setFailures] = useState<string[]>([]);
  const dispatching = useRef(false);
  const selectable = useMemo(() => rows.filter(row => (row.install || row.installed || row.removal) && !findPendingModSwitch(pending, row.target)), [rows, pending]);
  const selectedRows = selectable.filter(row => selected.has(row.key));
  useEffect(() => {
    // A failed launch briefly reserves a pending switch. Keep its selection for retry.
    const keys = new Set(batchMode ? rows.map(row => row.key) : []);
    setSelected(prev => [...prev].every(key => keys.has(key)) ? prev : new Set([...prev].filter(key => keys.has(key))));
    if (!batchMode) { setReview(null); setFailures([]); }
  }, [batchMode, rows]);

  async function dispatch({ action, rows: targets, skipped }: BatchRequest) {
    if (disabled || dispatching.current || !targets.length) return;
    dispatching.current = true;
    onBatchBusy(true);
    setReview(null); setFailures([]);
    setProgress(`${labels[action]}：正在提交 ${targets.length} 项`);
    const errors: string[] = [];
    try {
      const result = await onBatch(action, targets);
      errors.push(...result.failures);
      skipped += result.skipped;
      setSelected(prev => {
        const next = new Set(prev);
        for (const key of [...result.submitted, ...result.removed]) next.delete(key);
        return next;
      });
      const summary = [result.submitted.length && `已提交 1 个 Agent 任务（${result.submitted.length} 项）`,
        result.removed.length && `已删除 ${result.removed.length} 项`, skipped && `跳过 ${skipped} 项`, errors.length && `${errors.length} 项失败`].filter(Boolean);
      useUI.getState().notify(`${labels[action]}：${summary.join('，')}`);
      if (result.removed.length) {
        try { await refreshRemovals(); } catch (error) { errors.push(`刷新 Mod 列表失败：${errorText(error)}`); }
      }
    } catch (error) { errors.push(errorText(error)); }
    finally {
      setFailures(errors); setProgress('');
      onBatchBusy(false); dispatching.current = false;
    }
  }
  const checked = selectedRows.length === 0 ? false : selectedRows.length === selectable.length ? true : 'indeterminate';
  const secondaryColumn = batchMode ? 'hidden sm:table-cell' : undefined;
  return <>
    <ModTagFilters tags={tags.data ?? []} value={activeFilter} busy={disabled} disabled={disabled || !tags.isSuccess}
      onChange={value => { setFilter(value); setSelected(new Set()); }} onManage={() => setManageTags(true)} />
    {tags.isError && <><ErrorState error={tags.error} /><Button size="sm" variant="outline" className="self-start" onClick={() => void tags.refetch()}>重新读取标签</Button></>}
    {batchMode && <div className="flex flex-wrap items-center justify-between gap-2 rounded-lg border bg-card px-3 py-2" role="group" aria-label="批量操作工具栏">
      <span className="text-sm tabular-nums" role="status">{progress || `已选 ${selectedRows.length} 项`}</span>
      <div className="flex flex-wrap gap-2">{actions.map(action => {
        const request = requestFor(action, selectedRows);
        const Icon = icons[action];
        return <Button key={action} size="sm" variant={action === 'remove' ? 'destructive' : 'outline'} disabled={disabled || !request.rows.length}
          title={`${labels[action]}：${request.rows.length} 项可执行${request.skipped ? `，${request.skipped} 项不适用` : ''}`}
          onClick={() => action === 'remove' ? setReview(request) : void dispatch(request)}>
          <Icon data-icon="inline-start" className="hidden sm:block" />{labels[action]}</Button>;
      })}</div>
    </div>}
    {!!failures.length && <ErrorState error={failures.join('；')} />}
    <Card className="min-w-0 py-0">
      <Table aria-label="Mod 管理" aria-busy={fetching}>
        <TableHeader><TableRow>
          {batchMode && <TableHead className="w-9 pl-3"><span className="relative inline-flex items-center justify-center align-middle"><Checkbox aria-label="全选 Mod" className={checked === 'indeterminate' ? '[&_svg]:invisible' : undefined} checked={checked} disabled={disabled || !selectable.length}
            onCheckedChange={value => setSelected(new Set(value === true ? selectable.map(row => row.key) : []))} />
            {checked === 'indeterminate' && <MinusIcon aria-hidden="true" className="pointer-events-none absolute size-3.5" />}</span></TableHead>}
          <TableHead>名称</TableHead><TableHead className={secondaryColumn}>版本</TableHead><TableHead className={secondaryColumn}>来源</TableHead>
          <TableHead>状态</TableHead><TableHead className={secondaryColumn}>安装时间</TableHead>{!batchMode && <TableHead />}
        </TableRow></TableHeader>
        <TableBody>{rows.map(row => {
          const { item, target, installed, install, removal } = row;
          const record = 'id' in item ? item : undefined;
          const archive = 'filename' in item ? item : undefined;
          const origin = record?.downloadOrigin;
          const source = origin?.sourceId ?? record?.source ?? archive?.sourceId;
          const task = findPendingModSwitch(pending, target);
          const canSelect = !task && !!(installed || install || removal);
          const isSelected = batchMode && canSelect && selected.has(row.key);
          return <TableRow key={row.key} data-state={isSelected ? 'selected' : undefined}>
            {batchMode && <TableCell className="pl-3"><Checkbox aria-label={`选择 ${item.name}`} checked={isSelected} disabled={disabled || !canSelect}
              onCheckedChange={value => setSelected(prev => { const next = new Set(prev); if (value === true) next.add(row.key); else next.delete(row.key); return next; })} /></TableCell>}
            <TableCell className="min-w-28 whitespace-normal"><div className="flex flex-wrap items-center gap-x-2 gap-y-1">
              {record?.tagIds?.map(id => { const tag = tagsById.get(id); return tag && <ModTagBadge key={id} tag={tag} />; })}
              <strong className="wrap-anywhere">{item.name}</strong>
            </div><small className="block wrap-anywhere text-muted-foreground">{record ? `${record.files.length} 个文件` : archive?.filename}</small></TableCell>
            <TableCell className={cn('min-w-24 wrap-anywhere whitespace-normal', secondaryColumn)}>{item.version || '—'}{origin && <PathText className="block">版本 ID：{origin.versionId}</PathText>}</TableCell>
            <TableCell className={cn('min-w-28 wrap-anywhere whitespace-normal', secondaryColumn)}>{source === 'local' ? '本地归档' : source || '—'}
              {origin ? <PathText className="block">项目 ID：{origin.projectId}</PathText> : record && source !== 'local' && <small className="block text-muted-foreground">未绑定下载记录</small>}
            </TableCell>
            <TableCell><ModStatusSwitch checked={installed} action={installed ? '使用 Agent 卸载' : record && install ? '使用 Agent 重新安装' : '使用 Agent 安装'}
              status={record ? modStatusLabels[record.status] ?? record.status : '待安装'} pending={task} disabled={disabled || batchMode || (!installed && !install)}
              hint={record?.status === 'partially_restored' ? '部分恢复：需先处理剩余回收站文件' : record?.status === 'planned' ? '此安装计划尚未执行。如需重新安装，请先处理或取消旧计划。' : undefined}
              onToggle={() => { if (installed && record) onStart({ installId: record.id, name: item.name }, target); else if (install) onStart(install, target); }} /></TableCell>
            <TableCell className={secondaryColumn}>{record ? date(record.installedAt) : '—'}</TableCell>
            {!batchMode && <TableCell><div className="flex justify-end gap-2">
              {record && !['planned', 'rolled_back'].includes(record.status) && <IconButton title={`编辑 ${item.name} 的标签`} disabled={disabled || !!task || !tags.isSuccess} onClick={() => setEditId(record.id)}><TagIcon /></IconButton>}
              {removal && <IconButton title={removal.kind === 'plan' ? '取消安装计划' : '从管理列表删除'} disabled={disabled || !!task} onClick={() => onRemove(removal)}><Trash2Icon data-icon="inline-start" /></IconButton>}
              {record && replacementTransaction(record) && <IconButton title="预览回滚替换" disabled={disabled || !!task || fetching} onClick={() => onRollback(record)}><Undo2Icon data-icon="inline-start" /></IconButton>}
            </div></TableCell>}
          </TableRow>;
        })}</TableBody>
      </Table>
    </Card>
    {activeFilter && !rows.length && <p className="py-4 text-center text-sm text-muted-foreground">没有使用此标签的 Mod</p>}
    {manageTags && <ManageModTags gameId={gameId} tags={tags.data ?? []} onClose={() => setManageTags(false)} />}
    {editing && 'id' in editing && <EditModTags key={editing.id} gameId={gameId} record={editing} tags={tags.data ?? []} onClose={() => setEditId(null)} />}
    {review && <Modal title="批量删除 Mod" description={`将从此游戏的管理列表删除 ${review.rows.length} 项。`} onClose={() => setReview(null)} actions={<>
      <Button variant="outline" onClick={() => setReview(null)}>取消</Button>
      <Button variant="destructive" disabled={disabled} onClick={() => void dispatch(review)}>确认删除</Button>
    </>}>
      <p className="text-sm">已安装的 Mod 会先由 Agent 核对，安全卸载或回滚后才移除记录；文件已全部缺失时按核对结果处理。未执行的安装计划将取消，原始归档和事务历史会保留。</p>
      <ul className="max-h-48 list-disc overflow-auto pl-5 text-sm">{review.rows.map(row => <li className="wrap-anywhere" key={row.key}>{row.item.name}{row.item.version && ` · ${row.item.version}`}</li>)}</ul>
      {!!review.skipped && <p className="text-sm text-muted-foreground">另有 {review.skipped} 项不适用，将跳过。</p>}
    </Modal>}
  </>;
}
