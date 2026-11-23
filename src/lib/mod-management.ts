import type { ModSwitchTarget } from './mod-switch-store';

type DownloadOrigin = { artifactId: string; sourceId: string; projectId: string; versionId: string };
export type Installed = {
  id: string; name: string; version: string | null; status: string; source: string; downloadOrigin?: DownloadOrigin | null;
  installRoot: string; files: { path: string; size: number }[]; installedAt: string; tagIds?: string[];
  metadata?: { libraryEntryId?: string; transactionId?: string | null; replacesInstallId?: string | null } | null;
};
export type LibraryMod = {
  artifactId: string; gameId: string | null; gameName: string; name: string; filename: string; version: string | null;
  sourceId: string; sha256: string; projectPage?: string | null;
};
export type Removal = { kind: 'archive' | 'plan' | 'record' | 'installed'; id: string; name: string };
export type ModAction = Pick<LibraryMod, 'artifactId' | 'gameId' | 'name'> | { installId: string; name: string; remove?: boolean };
export type ModRow = {
  key: string; item: Installed | LibraryMod; target: ModSwitchTarget; installed: boolean;
  install?: LibraryMod; removal?: Removal;
};
export const modStatusLabels: Record<string, string> = { installed: '已安装', enabled: '已安装', superseded: '已被替换', uninstalled: '已卸载', rolled_back: '已卸载（已回滚）', partially_restored: '部分文件已恢复', pending: '待执行', planned: '待审批', purged: '已永久删除' };

export function replacementTransaction(mod?: Installed) {
  const { replacesInstallId, transactionId } = mod?.metadata ?? {};
  return mod?.status === 'enabled' && replacesInstallId && transactionId ? transactionId : null;
}

// One capability model for the single-row controls, selection and bulk dispatch.
export function buildModRows(gameId: string, installs: Installed[] | undefined, library: LibraryMod[] = []): ModRow[] {
  if (!installs) return []; // Archive-only rows are unsafe until we know what is already installed.
  const archiveFor = (mod: Installed) => mod.metadata?.libraryEntryId ?? mod.downloadOrigin?.artifactId;
  const byId = new Map(library.map(mod => [mod.artifactId, mod]));
  const owned = new Set(installs.filter(mod => ['enabled', 'installed', 'partially_restored'].includes(mod.status)).map(archiveFor));
  const represented = new Set(installs.map(archiveFor));
  const rows: ModRow[] = installs.map(item => {
    const artifactId = archiveFor(item);
    const installed = ['enabled', 'installed'].includes(item.status);
    const kind = installed ? 'installed' : item.status === 'planned' ? 'plan'
      : ['uninstalled', 'rolled_back', 'purged'].includes(item.status) ? 'record' : undefined;
    return { key: `install:${item.id}`, item, target: { gameId, installId: item.id, artifactId }, installed,
      install: artifactId && !owned.has(artifactId) && ['uninstalled', 'rolled_back', 'superseded', 'purged'].includes(item.status) ? byId.get(artifactId) : undefined,
      removal: kind && { kind, id: item.id, name: item.name } };
  });
  for (const item of library) {
    if (!represented.has(item.artifactId)) rows.push({ key: `archive:${item.artifactId}`, item, target: { gameId, artifactId: item.artifactId },
      installed: false, install: item, removal: { kind: 'archive', id: item.artifactId, name: item.name } });
  }
  return rows;
}

export type BatchAction = 'install' | 'uninstall' | 'remove';
// A stable intent identity: never upgrade a reviewed record removal into an Agent uninstall.
export function batchIntent(action: BatchAction, row: ModRow) {
  if (action === 'install') return row.install && `install:${row.install.artifactId}`;
  if (action === 'uninstall') return row.installed ? `uninstall:${row.target.installId}` : undefined;
  return row.removal && `${row.removal.kind}:${row.removal.id}`;
}
