import type { PublicationRecord } from '../../src/components/PluginPublications';

type LegacyFixtureState = 'prepared' | 'unknown' | 'awaitingScan' | 'uploadReady' | 'completed';
type FixtureControls = {
  outcome: 'completed' | 'hidden' | 'unknown';
  reconciliation: 'unconfirmed' | 'verified';
  listDenied: boolean;
  exposeCatalog: boolean;
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
  }
  const commands = new Set(['list_manager_publications', 'prepare_manager_publication', 'publish_manager_publication', 'reconcile_manager_publication', 'list_my_manager_plugins']);
  const fixture: FixtureControls = {
    outcome: 'completed', reconciliation: 'verified', listDenied: false, exposeCatalog: false, legacyState: null,
    handles(command) {
      return commands.has(command) || (fixture.exposeCatalog && command === 'list_manager_plugin_games') || (fixture.legacyState !== null && ['list_plugin_publications', 'reconcile_plugin_publication'].includes(command));
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
        case 'list_manager_publications': return structuredClone(records);
        case 'prepare_manager_publication': {
          const installed = args.draftId === undefined && args.slug === installedSlug;
          const draft = args.draftId === 'draft-fixture' && args.slug === undefined;
          if (!installed && !draft) throw new Error('请选择一个已审核的草稿或已安装适配包');
          const source: PublicationRecord['preview']['source'] = installed
            ? {kind: 'installed', slug: installedSlug, source: 'external', deploymentTransactionId: 'txn-installed-fixture'}
            : {kind: 'draft', draftId: args.draftId, writeTransactionId: 'txn-draft-fixture'};
          const preview = {source, target: args.target, slug: installed ? installedSlug : 'fixture-game', version: installed ? '1.0.0' : '1.1.0',
            tags: args.tags, files: await (installed ? installedFiles : draftFiles), destination};
          const digest = await digestBytes(JSON.stringify(preview));
          const previous = records.find(item => item.preview.digest === digest);
          if (previous) return structuredClone(previous);
          const record: PublicationRecord = {id: `manager_${records.length + 1}`, state: 'intended', preview: {...preview, digest}, remote: null, approvedDigest: null, createdAt: now, updatedAt: now};
          records.unshift(record);
          return structuredClone(record);
        }
        case 'publish_manager_publication': {
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
        case 'list_plugin_publications': {
          const state = fixture.legacyState;
          const pack = {title: '旧版测试作品', slug: 'fixture-game', version: '1.0.0', summary: '旧版摘要', description: '旧版描述', tags: ['旧标签'], gameIds: [3], files: await draftFiles, destination, workStatus: 'published', archiveName: 'legacy.zip', archiveSize: 240, archiveSha256: 'c'.repeat(64)};
          return [{id: 'legacy_fixture', state: state === 'completed' || state === 'unknown' ? state : 'intended', phase: state === 'unknown' ? 'publishWorkPending' : state, fileId: state === 'prepared' ? null : 12, workId: state === 'completed' ? 99 : null, preview: {digest: 'f'.repeat(64), package: pack}, createdAt: now, updatedAt: now}];
        }
        case 'reconcile_plugin_publication':
          if (args.id !== 'legacy_fixture' || !['unknown', 'awaitingScan'].includes(fixture.legacyState ?? '')) throw new Error('无法核对旧版作品');
          if (fixture.reconciliation === 'verified') fixture.legacyState = fixture.legacyState === 'unknown' ? 'completed' : 'uploadReady';
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
  host.__TAURI_INTERNALS__.invoke = (command, args = {}) => {
    if (!window.__publicationFixture.handles(command)) return original(command, args);
    host.__AMM_TEST__.calls.push({command, args});
    return window.__publicationFixture.invoke(command, args);
  };
}
