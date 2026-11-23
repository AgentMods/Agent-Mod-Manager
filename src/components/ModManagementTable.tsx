import { useEffect, useMemo, useRef, useState } from 'react';
import { DownloadIcon, MinusIcon, PackageMinusIcon, Trash2Icon, Undo2Icon } from 'lucide-react';
import { ModStatusSwitch } from '@/components/ModStatusSwitch';
import { ErrorState, IconButton, Modal, PathText, date } from '@/components/ui';
import { Button } from '@/components/ui/button';
import { Card } from '@/components/ui/card';
import { Checkbox } from '@/components/ui/checkbox';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { errorText } from '@/lib/api';
import { batchIntent, modStatusLabels, replacementTransaction, type BatchAction, type Installed, type ModAction, type ModRow, type Removal } from '@/lib/mod-management';
import { findPendingModSwitch, useModSwitch, type ModSwitchTarget } from '@/lib/mod-switch-store';
import { useUI } from '@/lib/store';
import { cn } from '@/lib/utils';

const labels = { install: '批量安装', uninstall: '批量卸载', remove: '批量删除' };
const icons = { install: DownloadIcon, uninstall: PackageMinusIcon, remove: Trash2Icon };
const actions = ['install', 'uninstall', 'remove'] as const;

type BatchRequest = { action: BatchAction; rows: ModRow[]; skipped: number };
type BatchOutcome = 'submitted' | 'removed' | 'skipped';

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

export function ModManagementTable({ rows, batchMode, disabled, fetching, onStart, onRemove, onRollback, onBatchItem, onBatchBusy, refreshRemovals }: {
  rows: ModRow[]; batchMode: boolean; disabled: boolean; fetching: boolean;
  onStart: (action: ModAction, target: ModSwitchTarget) => void;
  onRemove: (removal: Removal) => void; onRollback: (record: Installed) => void;
  onBatchItem: (action: BatchAction, row: ModRow) => Promise<BatchOutcome>;
  onBatchBusy: (busy: boolean) => void; refreshRemovals: () => Promise<void>;
}) {
  const pending = useModSwitch(state => state.pending);
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
    let submitted = 0, removed = 0;
    const errors: string[] = [];
    try {
      for (const [index, row] of targets.entries()) {
        setProgress(`${labels[action]}：正在提交 ${index + 1} / ${targets.length}`);
        try {
          const outcome = await onBatchItem(action, row);
          if (outcome === 'skipped') { skipped++; continue; }
          if (outcome === 'submitted') submitted++; else removed++;
          setSelected(prev => { const next = new Set(prev); next.delete(row.key); return next; });
        } catch (error) { errors.push(`${row.item.name}：${errorText(error)}`); }
      }
      if (removed) await refreshRemovals();
      const summary = [submitted && `已提交 ${submitted} 个 Agent 任务`, removed && `已删除 ${removed} 项`, skipped && `跳过 ${skipped} 项`, errors.length && `${errors.length} 项失败`].filter(Boolean);
      useUI.getState().notify(`${labels[action]}：${summary.join('，')}`);
    } catch (error) { errors.push(`刷新 Mod 列表失败：${errorText(error)}`); }
    finally {
      setFailures(errors); setProgress('');
      onBatchBusy(false); dispatching.current = false;
    }
  }
  const checked = selectedRows.length === 0 ? false : selectedRows.length === selectable.length ? true : 'indeterminate';
  const secondaryColumn = batchMode ? 'hidden sm:table-cell' : undefined;
  return <>
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
            <TableCell className="min-w-28 whitespace-normal"><strong className="wrap-anywhere">{item.name}</strong><small className="block wrap-anywhere text-muted-foreground">{record ? `${record.files.length} 个文件` : archive?.filename}</small></TableCell>
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
              {removal && <IconButton title={removal.kind === 'plan' ? '取消安装计划' : '从管理列表删除'} disabled={disabled || !!task} onClick={() => onRemove(removal)}><Trash2Icon data-icon="inline-start" /></IconButton>}
              {record && replacementTransaction(record) && <IconButton title="预览回滚替换" disabled={disabled || !!task || fetching} onClick={() => onRollback(record)}><Undo2Icon data-icon="inline-start" /></IconButton>}
            </div></TableCell>}
          </TableRow>;
        })}</TableBody>
      </Table>
    </Card>
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
