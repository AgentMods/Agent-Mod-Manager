import { useRef, useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { EllipsisIcon, ExternalLinkIcon, FolderOpenIcon, InfoIcon, PencilIcon, RefreshCwIcon } from 'lucide-react';
import { EditModDetails, type ModTag } from '@/components/ModTags';
import { ModUpdateDialog } from '@/components/ModUpdateDialog';
import { Modal } from '@/components/ui';
import { Button } from '@/components/ui/button';
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu';
import { api, errorText } from '@/lib/api';
import { tr } from '@/lib/i18n';
import { modMetadataTarget, modProjectPage, modSource, modVersion, type ModRow } from '@/lib/mod-management';
import { useUI } from '@/lib/store';

export function ModMoreMenu({ gameId, row, tags, disabled, canEdit }: {
  gameId: string; row: ModRow; tags: ModTag[]; disabled: boolean; canEdit: boolean;
}) {
  const [dialog, setDialog] = useState<'info' | 'edit' | 'update' | null>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const page = modProjectPage(row);
  const missing = tr('未提供');
  function close() {
    setDialog(null);
    trigger.current?.focus();
  }
  function report(error: unknown) { useUI.getState().notify(errorText(error)); }
  return <>
    <DropdownMenu>
      <DropdownMenuTrigger asChild><Button ref={trigger} size="icon-sm" variant="ghost" aria-label={tr('更多')} title={tr('更多')}><EllipsisIcon /></Button></DropdownMenuTrigger>
      <DropdownMenuContent align="end" onCloseAutoFocus={event => { if (dialog) event.preventDefault(); }}>
        <DropdownMenuGroup>
          <DropdownMenuItem onSelect={() => { void api('open_mod_storage_directory', { gameId, target: modMetadataTarget(row) }).catch(report); }}><FolderOpenIcon />{tr('目录')}</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => setDialog('info')}><InfoIcon />{tr('信息')}</DropdownMenuItem>
          <DropdownMenuItem disabled={!page} onSelect={() => { if (page) void openUrl(page).catch(report); }}><ExternalLinkIcon />{tr('网站')}</DropdownMenuItem>
          <DropdownMenuItem disabled={disabled || !canEdit} onSelect={() => setDialog('edit')}><PencilIcon />{tr('编辑')}</DropdownMenuItem>
          <DropdownMenuItem disabled={disabled} onSelect={() => setDialog('update')}><RefreshCwIcon />{tr('更新')}</DropdownMenuItem>
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
    {dialog === 'info' && <Modal title={tr('Mod 信息')} className="sm:max-w-md" onClose={close}>
      <dl className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] gap-x-5 gap-y-3 text-sm">
        <dt className="text-muted-foreground">{tr('名称')}</dt><dd className="wrap-anywhere">{row.item.name}</dd>
        <dt className="text-muted-foreground">{tr('版本')}</dt><dd className="wrap-anywhere">{modVersion(row) || tr('未知版本')}</dd>
        <dt className="text-muted-foreground">{tr('作者')}</dt><dd className="wrap-anywhere">{row.item.authorHint || row.archive?.authorHint || missing}</dd>
        <dt className="text-muted-foreground">{tr('来源')}</dt><dd className="wrap-anywhere">{modSource(row) || missing}</dd>
        <dt className="text-muted-foreground">{tr('网址')}</dt><dd className="wrap-anywhere">{page ? <a className="text-primary underline underline-offset-4" href={page} onClick={event => { event.preventDefault(); void openUrl(page).catch(report); }}>{page}</a> : missing}</dd>
      </dl>
    </Modal>}
    {dialog === 'edit' && <EditModDetails gameId={gameId} row={row} tags={tags} disabled={disabled || !canEdit} onClose={close} />}
    {dialog === 'update' && <ModUpdateDialog gameId={gameId} row={row} disabled={disabled} onClose={close} />}
  </>;
}
