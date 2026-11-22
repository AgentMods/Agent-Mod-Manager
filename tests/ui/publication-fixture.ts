import type { PublicationRecord } from '../../src/components/PluginPublications';
import type { TrashItem } from '../../src/lib/types';

type LegacyFixtureState = 'prepared' | 'unknown' | 'awaitingScan' | 'uploadReady' | 'completed';
type FixtureControls = {
  outcome: 'completed' | 'hidden' | 'unknown';
  reconciliation: 'unconfirmed' | 'verified';
  listDenied: boolean;
  exposeCatalog: boolean;
  matched: boolean;
  ambiguous: boolean;
  invalidCover: boolean;
  agentDraftId: string | null;
  readonly recycledDrafts: TrashItem[];
  holdNext(command: 'prepare_auto_manager_publication' | 'publish_manager_publication'): void;
  resume(): void;
  seedManagerV1(count: number, state: 'intended' | 'unknown'): Promise<void>;
  legacyState: LegacyFixtureState | null;
  handles(command: string): boolean;
  invoke(command: string, args: Record<string, any>): Promise<unknown>;
};

declare global {
  interface Window { __publicationFixture: FixtureControls }
}

// Installed via page.addInitScript. Keep the browser callback self-contained.
export function installPublicationHandlers() {
  if (window.__publicationFixture) return;
  const now = '2026-11-21T00:00:00Z';
  const adapter = '{\r\n  "slug": "fixture-game", "name": "测试游戏", "version": "1.1.0"\r\n}\r\n';
  const skill = '---\nname: fixture-game\ndescription: 公开测试\n---\nReview <em>before</em> installing.\n';
  async function digestBytes(text: string) {
    const hash = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text));
    return Array.from(new Uint8Array(hash), byte => byte.toString(16).padStart(2, '0')).join('');
  }
  async function makeFile(name: string, text: string) {
    return {name, text, size: new TextEncoder().encode(text).length, sha256: await digestBytes(text)};
  }
  const draftFiles = Promise.all([makeFile('adapter.json', adapter), makeFile('SKILL.md', skill)]);
  const installedSlug = 'minecraft-java';
  const installedName = 'Minecraft Java Edition';
  const installedAdapter = JSON.stringify({slug: installedSlug, name: installedName, version: '1.0.0'}, null, 2) + '\n';
  const installedFiles = Promise.all([makeFile('adapter.json', installedAdapter), makeFile('SKILL.md', '---\nname: minecraft-java\ndescription: Minecraft 模组安装指引\n---\n已审核的安装指引。\n')]);
  const destination = 'https://www.glossmod.com/api/v1/mcp';
  const records: PublicationRecord[] = [];
  const ownPlugin = {id: 42, releaseId: 70, name: '测试游戏', slug: 'fixture-game', version: '1.0.0', tags: ['旧标签'], hidden: true, reason: '测试下架原因'};
  function complete(record: PublicationRecord, hidden: boolean) {
    record.state = 'completed';
    record.remote = {...ownPlugin, id: record.preview.target.kind === 'release' ? record.preview.target.pluginId : 41,
      slug: record.preview.slug, name: record.preview.slug === installedSlug ? installedName : ownPlugin.name,
      releaseId: 71, version: record.preview.version, tags: record.preview.tags, hidden, reason: hidden ? '测试下架原因' : ''};
    recycleDraft(record.preview.source);
  }
  const legacySource = {kind: 'draft', draftId: 'draft-fixture', writeTransactionId: 'txn-draft-fixture'} as const;
  function recycleDraft(source: PublicationRecord['preview']['source']) {
    if (source.kind !== 'draft' || source.draftId !== fixture.agentDraftId) return;
    const {pluginDrafts} = (window as any).__AMM_TEST__ as {pluginDrafts: {id: string; slug: string; path: string}[]};
    const index = pluginDrafts.findIndex(draft => draft.id === source.draftId);
    if (index < 0) return;
    const [draft] = pluginDrafts.splice(index, 1);
    fixture.recycledDrafts.push({id: `trash-${draft.id}`, original_path: draft.path,
      trash_path: `/sandbox/data/trash/${draft.id}/payload`, transaction_id: `recycle-${draft.id}`,
      size: 1024, is_dir: true, game_id: null, label: draft.slug, reason: `自动清理已用完的 Agent 插件草稿 ${draft.id}`,
      created_at: now, restored_at: null, exists: true, restore_requires_rollback: false});
  }
  const waits = new Map<string, Promise<void>>();
  let release = () => {};
  // Wait for an explicit test signal, not a timer; a later opening need not wait for the first.
  async function waitIfHeld(command: string) {
    const wait = waits.get(command);
    waits.delete(command);
    if (wait) await wait;
  }
  const canvas = document.createElement('canvas');
  canvas.width = 320; canvas.height = 180;
  const ctx = canvas.getContext('2d')!;
  const gradient = ctx.createLinearGradient(0, 0, 320, 180);
  gradient.addColorStop(0, '#244953'); gradient.addColorStop(0.55, '#122e38'); gradient.addColorStop(1, '#8a915c');
  ctx.fillStyle = gradient; ctx.fillRect(0, 0, 320, 180);
  ctx.translate(160, 90); ctx.rotate(Math.PI / 4); ctx.strokeStyle = '#e6c26e'; ctx.lineWidth = 4;
  ctx.strokeRect(-12, -12, 24, 24); ctx.fillStyle = '#dce8e9'; ctx.fillRect(-6, -6, 12, 12);
  const png = canvas.toDataURL('image/png').split(',')[1];
  const commands = new Set(['list_manager_publications', 'get_manager_publication', 'prepare_auto_manager_publication', 'publish_manager_publication', 'reconcile_manager_publication', 'list_my_manager_plugins']);
  const fixture: FixtureControls = {
    outcome: 'completed', reconciliation: 'verified', listDenied: false, exposeCatalog: false, legacyState: null,
    matched: false, ambiguous: false, invalidCover: false, agentDraftId: null, recycledDrafts: [],
    holdNext(command) { waits.set(command, new Promise<void>(resolve => { release = resolve; })); },
    resume() { release(); },
    async seedManagerV1(count, state) {
      const files = await draftFiles;
      for (let i = 0; i < count; i++) {
        const slug = 'v1-' + i;
        const digest = await digestBytes(slug);
        records.push({id: slug, state, createdAt: now, updatedAt: now, remote: null, approvedDigest: state === 'intended' ? null : digest,
          preview: {source: {kind: 'draft', draftId: 'v1-draft', writeTransactionId: 'v1-txn'}, target: {kind: 'first'},
            slug, tags: ['旧标签'], version: '0.9.0', digest, files, destination}});
      }
    },
    handles(command) {
      return commands.has(command) || (fixture.exposeCatalog && command === 'list_manager_plugin_games') || (fixture.legacyState !== null && ['list_plugin_publications', 'get_plugin_publication', 'reconcile_plugin_publication'].includes(command));
    },
    async invoke(command, args) {
      switch (command) {
        case 'list_manager_plugin_games': {
          const plugins = records.flatMap(({remote}) => remote && !remote.hidden ? [{...remote, id: String(remote.id), releaseId: String(remote.releaseId), authorName: '测试作者', platforms: ['windows'], steamAppId: '', description: ''}] : []);
          const games = plugins.length ? [{id: '1', name: installedName, slug: installedSlug, aliases: [], steamAppId: '', plugins}] : [];
          return {games, totalGames: games.length, skippedEntries: 0};
        }
        case 'list_my_manager_plugins':
          if (fixture.listDenied) throw new Error('读取我的插件需要 manager_plugins.read 权限');
          return structuredClone([ownPlugin]);
        case 'get_manager_publication': return structuredClone(records.find(record => record.id === args.id));
        case 'list_manager_publications': {
          const start = args.before ? records.findIndex(record => record.id === args.before) + 1 : 0;
          return structuredClone(records.slice(start, start + 50));
        }
        case 'prepare_auto_manager_publication': {
          await waitIfHeld(command);
          if (fixture.listDenied) throw new Error('读取我的插件需要 manager_plugins.read 权限');
          if (fixture.ambiguous) throw new Error('匹配到多个同标识的插件，无法确定发布目标');
          const installed = args.draftId === undefined && args.slug === installedSlug;
          const draft = args.draftId === 'draft-fixture' && args.slug === undefined;
          if (!installed && !draft) throw new Error('请选择一个已审核的草稿或已安装适配包');
          const source: PublicationRecord['preview']['source'] = installed
            ? {kind: 'installed', slug: installedSlug, source: 'external', deploymentTransactionId: 'txn-installed-fixture'}
            : {kind: 'draft', draftId: args.draftId, writeTransactionId: 'txn-draft-fixture'};
          const target: PublicationRecord['preview']['target'] = fixture.matched ? {kind: 'release', pluginId: 42} : {kind: 'first'};
          const listing = {title: installed ? 'Minecraft Java · 游戏适配包' : '测试游戏 · 游戏适配包', summary: '按已验证的规则识别并安装模组。',
            description: '自动定位模组目录。\n安装前请核对 <em>文件变更</em>。', tags: ['游戏适配', '中文'], coverPng: fixture.invalidCover ? 'https://covers.invalid/fixture.png' : png};
          const preview = {source, target, listing, remoteVersion: fixture.matched ? '1.3.0' : null, originalVersion: installed ? '1.0.0' : '1.1.0', slug: installed ? installedSlug : 'fixture-game',
            version: fixture.matched ? '1.3.1' : installed ? '1.0.0' : '1.1.0', tags: listing.tags, files: await (installed ? installedFiles : draftFiles), destination};
          const digest = await digestBytes(JSON.stringify(preview));
          const previous = records.find(item => item.preview.digest === digest);
          if (previous) return structuredClone(previous);
          const record: PublicationRecord = {id: `manager_${records.length + 1}`, state: 'intended', preview: {...preview, digest}, remote: null, approvedDigest: null, createdAt: now, updatedAt: now};
          records.unshift(record);
          return structuredClone(record);
        }
        case 'publish_manager_publication': {
          await waitIfHeld(command);
          const record = records.find(item => item.id === args.id);
          if (!record || record.state !== 'intended' || record.preview.digest !== args.digest || args.confirmation !== 'PUBLISH_PLUGIN') throw new Error('审批不匹配');
          record.approvedDigest = args.digest;
          if (fixture.outcome === 'unknown') { record.state = 'unknown'; throw new Error('发布请求中断，结果待核对'); }
          complete(record, fixture.outcome === 'hidden');
          return structuredClone(record);
        }
        case 'reconcile_manager_publication': {
          const record = records.find(item => item.id === args.id);
          if (!record || record.state !== 'unknown') throw new Error('无法核对这条记录');
          if (fixture.listDenied) throw new Error('核对远端状态需要 manager_plugins.read 权限');
          if (fixture.reconciliation === 'verified') {
            complete(record, false);
          }
          return fixture.reconciliation;
        }
        case 'get_plugin_publication':
        case 'list_plugin_publications': {
          const state = fixture.legacyState;
          const pack = {title: '旧版测试作品', slug: 'fixture-game', version: '1.0.0', summary: '旧版摘要', description: '旧版描述', tags: ['旧标签'], gameIds: [3], files: await draftFiles, destination, workStatus: 'published', archiveName: 'legacy.zip', archiveSize: 240, archiveSha256: 'c'.repeat(64)};
          const record = {id: 'legacy_fixture', state: state === 'completed' || state === 'unknown' ? state : 'intended', phase: state === 'unknown' ? 'publishWorkPending' : state, fileId: state === 'prepared' ? null : 12, workId: state === 'completed' ? 99 : null, preview: {digest: 'f'.repeat(64), package: pack, source: legacySource}, createdAt: now, updatedAt: now};
          return command === 'get_plugin_publication' ? record : [record];
        }
        case 'reconcile_plugin_publication':
          if (args.id !== 'legacy_fixture' || !['unknown', 'awaitingScan'].includes(fixture.legacyState ?? '')) throw new Error('无法核对旧版作品');
          if (fixture.reconciliation === 'verified') {
            fixture.legacyState = fixture.legacyState === 'unknown' ? 'completed' : 'uploadReady';
            if (fixture.legacyState === 'completed') recycleDraft(legacySource);
          }
          return fixture.reconciliation;
        default: throw new Error(`未知的发布测试命令 ${command}`);
      }
    },
  };
  window.__publicationFixture = fixture;
}

// Direct fixture for publication-only specs. The shared mock can also install the
// same handler before navigation for tests that cross feature boundaries.
export function attachPublicationHandlers() {
  const host = window as unknown as {
    __TAURI_INTERNALS__: {invoke(command: string, args?: Record<string, unknown>): Promise<unknown>};
    __AMM_TEST__: {calls: {command: string; args: Record<string, unknown>}[]};
  };
  const original = host.__TAURI_INTERNALS__.invoke;
  host.__TAURI_INTERNALS__.invoke = async (command, args = {}) => {
    const fixture = window.__publicationFixture;
    if (command === 'list_trash' && fixture.recycledDrafts.length) {
      const items = await original(command, args) as TrashItem[];
      return [...items, ...fixture.recycledDrafts];
    }
    if (!fixture.handles(command)) return original(command, args);
    host.__AMM_TEST__.calls.push({command, args});
    return fixture.invoke(command, args);
  };
}
