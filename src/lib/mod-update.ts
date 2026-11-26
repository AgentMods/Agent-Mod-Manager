import { api } from './api';
import { tr } from './i18n';
import { modProjectPage as updateProjectPage, modVersion, type Installed, type LibraryMod, type ModRow } from './mod-management';
export { updateProjectPage };

type Project = { sourceId: string; projectId: string; versionId: string };
type CatalogProject = Project & { providerGameId?: string };
type SourceStatus = { source_id: string; state: string; message: string };
type VersionSummary = {
  version_id: string; version: string; release_channel: 'release' | 'beta' | 'alpha';
  published_at: string; game_versions: string[]; loaders: string[];
};
type VersionPage = { status: SourceStatus; versions: VersionSummary[]; has_more: boolean };
type Resolution = {
  status: SourceStatus;
  download: { resolution_id: string; source_id: string; project_id: string; version_id: string; version: string; filename: string } | null;
};
type DownloadReceipt = { artifactId: string; gameId: string | null; filename: string };
export type UpdateCheck =
  | { kind: 'available'; version: VersionSummary; current: VersionSummary }
  | { kind: 'current'; current: VersionSummary }
  | { kind: 'unverified'; current?: VersionSummary };

function archiveProject(archive?: LibraryMod): Project | null {
  const { sourceId, projectId, versionId } = archive ?? {};
  return sourceId && !['local', 'web'].includes(sourceId) && projectId && versionId ? { sourceId, projectId, versionId } : null;
}

function exactProject(left: Project, right: Project) {
  return left.sourceId === right.sourceId && left.projectId === right.projectId && left.versionId === right.versionId;
}

export function updateProject(row: ModRow): Project | null {
  if (!('id' in row.item)) return archiveProject(row.item);
  const origin = row.item.downloadOrigin;
  const linkedId = row.item.metadata?.libraryEntryId ?? origin?.artifactId;
  if (origin && linkedId !== origin.artifactId) return null;
  const archive = linkedId && linkedId === row.archive?.artifactId ? archiveProject(row.archive) : null;
  if (origin && archive && !exactProject(origin, archive)) return null;
  return origin ? (origin.sourceId && !['local', 'web'].includes(origin.sourceId) && origin.projectId && origin.versionId ? origin : null) : archive;
}

// A manual source check can start from a recorded project page even for an
// imported archive. The page is never promoted to downloadable provenance.
export function canAgentCheckUpdate(project: Project | null, page: string | null) {
  if (project && project.sourceId !== 'local' && project.sourceId !== 'web') return true;
  if (!page) return false;
  const { hostname, pathname } = new URL(page);
  const host = hostname.replace(/^www\./, '');
  if (pathname.includes('%')) return false;
  const paths: Record<string, RegExp> = {
    'nexusmods.com': /^\/[\w-]+\/mods\/\d+\/?$/,
    'glossmod.com': /^\/mods\/\d+\/?$/,
    'mod.3dmgame.com': /^\/mod\/\d+\/?$/,
    'gamebanana.com': /^\/mods\/\d+\/?$/,
    'modrinth.com': /^\/(?:mod|modpack|resourcepack|shader|plugin|datapack)\/[\w-]+\/?$/,
    'thunderstore.io': /^\/c\/[\w-]+\/p\/[\w-]+\/[\w-]+\/?$/,
    'curseforge.com': /^\/[\w-]+\/[\w-]+\/[\w-]+\/?$/,
    'mod.io': /^\/g\/[\w-]+\/m\/[\w-]+\/?$/,
    'github.com': /^\/[\w.-]+\/[\w.-]+(?:\/releases)?\/?$/,
  };
  return Object.hasOwn(paths, host) && paths[host].test(pathname);
}

export function updateCatalog(project: Project | null, page: string | null): CatalogProject | null {
  if (project?.sourceId === 'modrinth') return project;
  if (project?.sourceId !== 'thunderstore' || !page) return null;
  const url = new URL(page);
  const match = url.pathname.match(/^\/c\/([\w-]+)\/p\/([\w-]+)\/([\w-]+)\/?$/);
  return ['thunderstore.io', 'www.thunderstore.io'].includes(url.hostname) && match && `${match[2]}/${match[3]}` === project.projectId
    ? { ...project, providerGameId: match[1] } : null;
}

function catalogRequest(project: CatalogProject) {
  return {
    source_id: project.sourceId, project_id: project.projectId,
    ...(project.providerGameId && { game_id: project.providerGameId }),
  };
}

// Compare numeric releases only. Build metadata does not increase precedence.
function versionParts(version: string) {
  const core = version.trim().match(/^v?(\d{1,9}(?:\.\d{1,9}){0,3})(?:\+[\da-z.-]+)?$/i)?.[1];
  return core?.split('.').map(Number);
}

function compareVersions(left: number[], right: number[]) {
  for (let index = 0; index < Math.max(left.length, right.length); index++) {
    const order = (left[index] ?? 0) - (right[index] ?? 0);
    if (order) return order;
  }
  return 0;
}

function publicationTime(value: string) {
  // Catalog dates are RFC 3339. Date.parse alone also accepts partial dates
  // and normalizes some impossible calendar days into the following month.
  const fields = /^(\d{4})-(\d{2})-(\d{2})T(?:[01]\d|2[0-3]):[0-5]\d:[0-5]\d(?:\.\d+)?(?:Z|[+-](?:[01]\d|2[0-3]):[0-5]\d)$/.exec(value);
  if (!fields) return NaN;
  const [, y, m, d] = fields;
  const [year, month, day] = [Number(y), Number(m), Number(d)];
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const monthDays = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  if (month < 1 || month > 12 || day < 1 || day > monthDays[month - 1]) return NaN;
  return Date.parse(value);
}

function compatibility(current: VersionSummary, next: VersionSummary) {
  // A mixed branch can overlap the current profile without supporting this
  // installation (e.g. a Fabric + NeoForge release followed by Fabric only).
  const dimensions = [[current.game_versions, next.game_versions], [current.loaders, next.loaders]];
  if (dimensions.some(([old, newer]) => old.length && newer.length && !old.some(value => newer.includes(value)))) return 'different';
  return dimensions.every(([old, newer]) => (!old.length === !newer.length) && old.every(value => newer.includes(value)))
    ? 'matching' : 'ambiguous';
}

function requireVersionSource(status: SourceStatus, project: CatalogProject) {
  if (status.source_id !== project.sourceId) throw new Error(tr('来源返回的项目或版本与所选目标不符。'));
  if (status.state !== 'available') throw new Error(status.message || tr('此来源暂时无法检查或下载版本。'));
}

export async function checkModUpdate(project: CatalogProject, displayedVersion: string | null): Promise<UpdateCheck> {
  const versionPageSize = 50;
  const versions = new Map<string, VersionSummary>();
  let hasMore = false;
  for (let page = 0; page < 4; page++) {
    const result = await api<VersionPage>('list_mod_versions', {
      request: { ...catalogRequest(project), offset: page * versionPageSize, limit: versionPageSize },
    });
    requireVersionSource(result.status, project);
    for (const version of result.versions) {
      if (versions.has(version.version_id)) return { kind: 'unverified' };
      versions.set(version.version_id, version);
    }
    hasMore = result.has_more;
    if (!hasMore) break;
  }

  const current = versions.get(project.versionId);
  if (!current || hasMore) return { kind: 'unverified', current };
  const currentParts = versionParts(current.version);
  const displayParts = displayedVersion && versionParts(displayedVersion);
  const currentDate = publicationTime(current.published_at);
  if (!currentParts || !Number.isFinite(currentDate) ||
    (displayedVersion && (!displayParts || compareVersions(displayParts, currentParts) !== 0))) return { kind: 'unverified', current };

  const newer: { version: VersionSummary; parts: number[] }[] = [];
  for (const version of versions.values()) {
    if (version.version_id === current.version_id || version.release_channel !== 'release') continue;
    const branch = compatibility(current, version);
    if (branch === 'different') continue;
    const published = publicationTime(version.published_at);
    if (!Number.isFinite(published)) return { kind: 'unverified', current };
    if (published < currentDate) continue;
    if (branch === 'ambiguous') return { kind: 'unverified', current };
    const parts = versionParts(version.version);
    if (!parts) return { kind: 'unverified', current };
    if (compareVersions(parts, currentParts) <= 0) continue;
    if (published === currentDate) return { kind: 'unverified', current };
    newer.push({ version, parts });
  }
  newer.sort((left, right) => compareVersions(right.parts, left.parts));
  const [latest, next] = newer;
  if (latest && next && compareVersions(latest.parts, next.parts) === 0) return { kind: 'unverified', current };
  return latest ? { kind: 'available', version: latest.version, current } : { kind: 'current', current };
}

const updatesInProgress = new Set<string>();
export function reserveCatalogUpdate(gameId: string, project: Project | null, page?: string | null) {
  const key = JSON.stringify([gameId, project?.sourceId, project?.projectId ?? page]);
  if (updatesInProgress.has(key)) throw new Error(tr('此项目的更新任务正在处理。'));
  updatesInProgress.add(key);
  return () => { updatesInProgress.delete(key); };
}

export async function verifyUpdateTarget(gameId: string, row: ModRow, project: CatalogProject, library: LibraryMod[]) {
  const { item } = row;
  if (row.target.gameId !== gameId) throw new Error(tr('更新目标已变化，请关闭后重新检查。'));
  let current: ModRow | undefined;
  if ('id' in item) {
    const records = await api<Installed[]>('list_installs', { gameId });
    const install = records.find(entry => entry.id === item.id && 'gameId' in entry && entry.gameId === gameId);
    const active = install?.status === 'enabled' || install?.status === 'installed';
    if (install?.status === item.status && active === row.installed &&
      (!row.installed || (install.status === 'enabled' && install.downloadOrigin))) {
      const archiveId = install.metadata?.libraryEntryId ?? install.downloadOrigin?.artifactId;
      current = { ...row, item: install, archive: library.find(entry => entry.artifactId === archiveId) };
    }
  } else {
    const archive = library.find(entry => entry.artifactId === item.artifactId && (entry.gameId === gameId || entry.gameId === null));
    if (archive && archive.sha256 === item.sha256) current = { ...row, item: archive, archive };
  }
  const identity = current && updateProject(current);
  if (!current || !identity || !exactProject(identity, project) || modVersion(current) !== modVersion(row) ||
    updateCatalog(identity, updateProjectPage(current))?.providerGameId !== project.providerGameId) {
    throw new Error(tr('更新目标已变化，请关闭后重新检查。'));
  }
}

export async function downloadModUpdate(gameId: string, project: CatalogProject, version: VersionSummary, library: LibraryMod[]) {
  function candidates(rows: LibraryMod[]) {
    const exact = rows.filter(entry => entry.artifactId && entry.gameId === gameId && entry.sourceId === project.sourceId &&
      entry.projectId === project.projectId && entry.versionId === version.version_id);
    if (exact.length > 1) throw new Error(tr('库中有多个相同版本的归档，请先在管理列表中核对。'));
    return exact[0];
  }
  const existing = candidates(library);
  if (existing && existing.version !== version.version) throw new Error(tr('来源返回的项目或版本与所选目标不符。'));
  if (existing) return { archive: existing, library };

  const { status, download } = await api<Resolution>('resolve_mod', {
    request: { ...catalogRequest(project), version_id: version.version_id },
  });
  requireVersionSource(status, project);
  if (!download) throw new Error(tr('此版本需要从原发布页手动下载。'));
  if (!download.resolution_id || download.source_id !== project.sourceId || download.project_id !== project.projectId ||
    download.version_id !== version.version_id || download.version !== version.version || !download.filename) {
    throw new Error(tr('来源返回的项目或版本与所选目标不符。'));
  }
  const receipt = await api<DownloadReceipt>('download_mod', {
    taskId: crypto.randomUUID(), resolutionId: download.resolution_id, classification: { gameId },
  });
  const rows = await api<LibraryMod[]>('list_mod_library', { gameId });
  const archive = candidates(rows);
  if (!archive || !receipt.artifactId || archive.artifactId !== receipt.artifactId || archive.version !== version.version ||
    receipt.gameId !== gameId || receipt.filename !== download.filename || archive.filename !== receipt.filename) {
    throw new Error(tr('无法确认新版归档已保存到此游戏的库。'));
  }
  return { archive, library: rows };
}
