import { tr } from "@/lib/i18n";
import { publicHttpsUrl } from "./public-url";
import type { ModSwitchTarget } from './mod-switch-store';

type DownloadOrigin = { artifactId: string; sourceId: string; projectId: string; versionId: string };
type ModDisplay = {
    name: string; version: string | null; tagIds?: string[]; authorHint?: string | null;
    website?: string | null; projectPage?: string | null;
};
export type Installed = ModDisplay & {
    id: string; status: string; source: string; downloadOrigin?: DownloadOrigin | null;
    installRoot: string; files: { path: string; size: number }[]; installedAt: string;
    metadata?: { libraryEntryId?: string; transactionId?: string | null; replacesInstallId?: string | null } | null;
};
export type LibraryMod = ModDisplay & {
    artifactId: string; gameId: string | null; gameName: string; filename: string; sha256: string;
    sourceId: string; projectId?: string | null; versionId?: string | null;
};
export type Removal = { kind: 'archive' | 'plan' | 'record' | 'installed'; id: string; name: string };
export type ModAction = Pick<LibraryMod, 'artifactId' | 'gameId' | 'name'> | { installId: string; name: string; remove?: boolean };
export type ModRow = {
    key: string; item: Installed | LibraryMod; target: ModSwitchTarget; installed: boolean;
    archive?: LibraryMod; install?: LibraryMod; removal?: Removal;
};
export const modStatusLabels: Record<string, string> = {
    get installed() { return tr("已安装"); },
    get enabled() { return tr("已安装"); },
    get superseded() { return tr("已被替换"); },
    get uninstalled() { return tr("已卸载"); },
    get rolled_back() { return tr("已卸载（已回滚）"); },
    get partially_restored() { return tr("部分文件已恢复"); },
    get pending() { return tr("待执行"); },
    get planned() { return tr("待审批"); },
    get purged() { return tr("已永久删除"); },
};

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
        const archive = artifactId ? byId.get(artifactId) : undefined;
        const installed = ['enabled', 'installed'].includes(item.status);
        const kind = installed ? 'installed' : item.status === 'planned' ? 'plan'
            : ['uninstalled', 'rolled_back', 'purged'].includes(item.status) ? 'record' : undefined;
        return {
            key: `install:${item.id}`, item, archive, target: { gameId, installId: item.id, artifactId }, installed,
            install: artifactId && !owned.has(artifactId) && ['uninstalled', 'rolled_back', 'superseded', 'purged'].includes(item.status) ? byId.get(artifactId) : undefined,
            removal: kind && { kind, id: item.id, name: item.name }
        };
    });
    for (const item of library) {
        if (!represented.has(item.artifactId)) rows.push({
            key: `archive:${item.artifactId}`, item, archive: item, target: { gameId, artifactId: item.artifactId },
            installed: false, install: item, removal: { kind: 'archive', id: item.artifactId, name: item.name }
        });
    }
    return rows;
}

export function modVersion({ item, archive }: ModRow) {
    return item.version?.trim() || archive?.version?.trim() || null;
}

export function modSource({ item, archive }: ModRow) {
    const source = 'id' in item ? item.downloadOrigin?.sourceId || archive?.sourceId || item.source : item.sourceId;
    return source === 'local' ? tr("本地归档") : source;
}

/** Prefer the native validated field (even when null); older list results only have projectPage. */
export function modProjectPage({ item, archive }: ModRow) {
    for (const entry of [item, archive]) {
        if (!entry) continue;
        const page = publicHttpsUrl(entry.website === undefined ? entry.projectPage : entry.website);
        if (page && !/[?#]/.test(page)) return page;
    }
    return null;
}

export function modMetadataTarget({ item }: ModRow) {
    return 'id' in item ? { kind: 'install', id: item.id } : { kind: 'archive', id: item.artifactId };
}

export type BatchAction = 'install' | 'uninstall' | 'remove';
// A stable intent identity: never upgrade a reviewed record removal into an Agent uninstall.
export function batchIntent(action: BatchAction, row: ModRow) {
    if (action === 'install') return row.install && `install:${row.install.artifactId}`;
    if (action === 'uninstall') return row.installed ? `uninstall:${row.target.installId}` : undefined;
    return row.removal && `${row.removal.kind}:${row.removal.id}`;
}
