import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { after, test } from 'node:test';
import { createServer } from 'vite';
import { conversationCountSources, pluralTemplate, trashPurgeSources } from '../src/locales/plurals.ts';

// Use Vite to load the production tr(), including its generated JSON catalog imports.
const root = fileURLToPath(new URL('../', import.meta.url));
const vite = await createServer({
  root,
  configFile: false,
  appType: 'custom',
  optimizeDeps: { noDiscovery: true },
  resolve: { alias: { '@': fileURLToPath(new URL('../src', import.meta.url)) } },
  server: { middlewareMode: true, watch: null, ws: false },
});
after(() => vite.close());
globalThis.document = { documentElement: { lang: '' } };
const { applyLanguageSettings, languages, tr } = await vite.ssrLoadModule('/src/lib/i18n.ts');
const { workspaceLabel } = await vite.ssrLoadModule('/src/lib/game-workspace.ts');
const useLanguage = locale => applyLanguageSettings({ locale, effective_locale: locale });
const tokens = text => [...text.matchAll(/\{(\w+)\}/g)].map(match => match[1]).sort();
const managedGames = '{value1} 个管理中的游戏';
const installedPlugins = '{value1} 个已安装插件';
const cardMods = '{platform} · {installedModCount} 个 Mod';
const drafts = '草稿 {value1}';
const messages = '{message_count} 条消息';
const tools = '· {length} 个工具';
const selected = '已选 {length} 项';
const scan = '已检查 {directoriesChecked} 个目录、{entriesChecked} 个条目，本批 {length} 个候选目录{value4}';

function strings(locale, source, field, counts, values = {}) {
  useLanguage(locale);
  return counts.map(count => tr(source, { ...values, [field]: count }));
}

test('visible English summary labels distinguish zero, one, and two', () => {
  const counts = [0, 1, 2];
  for (const [source, field, expected, values] of [
    [managedGames, 'value1', ['0 managed games', '1 managed game', '2 managed games']],
    [installedPlugins, 'value1', ['0 installed plugins', '1 installed plugin', '2 installed plugins']],
    [cardMods, 'installedModCount', ['Steam · 0 mods', 'Steam · 1 mod', 'Steam · 2 mods'], { platform: 'Steam' }],
    [drafts, 'value1', ['0 drafts', '1 draft', '2 drafts']],
    [messages, 'message_count', ['0 messages', '1 message', '2 messages']],
    [tools, 'length', ['· 0 tools', '· 1 tool', '· 2 tools']],
    [selected, 'length', ['0 items selected', '1 item selected', '2 items selected']],
    [conversationCountSources.group, 'length', ['0 conversations', '1 conversation', '2 conversations']],
    [conversationCountSources.active, 'length', ['0 active conversations', '1 active conversation', '2 active conversations']],
    [conversationCountSources.archived, 'length', ['0 archived conversations', '1 archived conversation', '2 archived conversations']],
  ]) {
    assert.deepEqual(strings('en', source, field, counts, values), expected, source);
  }
});

test('Russian handles teens, compound counts, and fractional other agreement', () => {
  assert.deepEqual(strings('ru', messages, 'message_count', [0, 1, 2, 5, 11, 12, 21, 22, 25, 1.5], { length: 9 }), [
    '0 сообщений', '1 сообщение', '2 сообщения', '5 сообщений', '11 сообщений', '12 сообщений', '21 сообщение', '22 сообщения', '25 сообщений', '1,5 сообщения',
  ]);
  assert.deepEqual(strings('ru', conversationCountSources.archived, 'length', [0, 1, 2, 21, 1.5]), [
    '0 архивных чатов', '1 архивный чат', '2 архивных чата', '21 архивный чат', '1,5 архивного чата',
  ]);
  assert.deepEqual(strings('ru', installedPlugins, 'value1', [0, 1, 2]), [
    '0 установленных плагинов', '1 установленный плагин', '2 установленных плагина',
  ]);
});

test('French and Brazilian Portuguese follow Intl zero rules, not English rules', () => {
  assert.deepEqual(strings('fr', managedGames, 'value1', [0, 1, 2]), ['0 jeu géré', '1 jeu géré', '2 jeux gérés']);
  assert.deepEqual(strings('pt-BR', managedGames, 'value1', [0, 1, 2]), ['0 jogo gerenciado', '1 jogo gerenciado', '2 jogos gerenciados']);
});

test('scan counts inflect independently and other placeholders survive intact', () => {
  useLanguage('en');
  assert.equal(tr(scan, { directoriesChecked: 1, entriesChecked: 0, length: 2, value4: ' · Scan limit reached' }),
    'Checked: 1 directory, 0 entries · This batch: 2 candidate directories · Scan limit reached');
  assert.equal(tr(cardMods, { platform: 'Steam {profile}', installedModCount: 1001 }), 'Steam {profile} · 1,001 mods');
  assert.equal(tr('{value1}：{length} 项可执行{value3}', { value1: 'Delete', length: 1,
    value3: tr('，{skipped} 项不适用', { skipped: 2 }) }), 'Delete: 1 eligible item, 2 not applicable');
  useLanguage('ru');
  assert.equal(tr(scan, { directoriesChecked: 21, entriesChecked: 2, length: 11, value4: '' }),
    'Проверено: 21 каталог, 2 элемента · В этой партии: 11 подходящих каталогов');
});

test('invariant locales and workspace states preserve Chinese copy', () => {
  assert.deepEqual(strings('zh-CN', conversationCountSources.archived, 'length', [0, 1, 2]), ['0 个已归档对话', '1 个已归档对话', '2 个已归档对话']);
  assert.deepEqual(strings('zh-CN', conversationCountSources.group, 'length', [1, 2]), ['1个对话', '2个对话']);
  assert.deepEqual(strings('zh-CN', drafts, 'value1', [0, 1, 2]), ['草稿 0', '草稿 1', '草稿 2']);
  assert.deepEqual(strings('ja', conversationCountSources.active, 'length', [0, 1, 2]), ['進行中の会話 0 件', '進行中の会話 1 件', '進行中の会話 2 件']);
  assert.deepEqual(strings('ko', drafts, 'value1', [0, 1, 2]), ['초안 0개', '초안 1개', '초안 2개']);
  useLanguage('zh-CN');
  assert.equal(workspaceLabel(null, []), '全部游戏');
  assert.equal(workspaceLabel([], []), '未选择游戏');
  assert.equal(workspaceLabel(['id'], []), '已选 1 款游戏');
  assert.equal(workspaceLabel(['a', 'b'], []), '已选 2 款游戏');
  useLanguage('ru');
  assert.equal(workspaceLabel(['id'], []), 'Выбрана 1 игра');
  assert.equal(workspaceLabel(['id'], [{ id: 'id', name: 'Game {literal}' }]), 'Game {literal}');
});

test('trash purge names the scope, shows count and size, and warns that deletion is irreversible', () => {
  const size = { size: '4 KB' };
  const counts = [0, 1, 2];
  assert.deepEqual(strings('en', trashPurgeSources.all, 'length', counts, size), [
    'This will permanently delete everything in the Trash (0 items, 4 KB). This action cannot be undone.',
    'This will permanently delete everything in the Trash (1 item, 4 KB). This action cannot be undone.',
    'This will permanently delete everything in the Trash (2 items, 4 KB). This action cannot be undone.',
  ]);
  assert.deepEqual(strings('en', trashPurgeSources.selected, 'length', counts, size), [
    'This will permanently delete 0 selected items (4 KB). This action cannot be undone.',
    'This will permanently delete 1 selected item (4 KB). This action cannot be undone.',
    'This will permanently delete 2 selected items (4 KB). This action cannot be undone.',
  ]);
  assert.deepEqual(strings('ru', trashPurgeSources.selected, 'length', [1, 2, 5, 21], size), [
    'Из корзины будет безвозвратно удалён 1 выбранный элемент (4 KB). Это действие нельзя отменить.',
    'Из корзины будут безвозвратно удалены 2 выбранных элемента (4 KB). Это действие нельзя отменить.',
    'Из корзины будут безвозвратно удалены 5 выбранных элементов (4 KB). Это действие нельзя отменить.',
    'Из корзины будет безвозвратно удалён 21 выбранный элемент (4 KB). Это действие нельзя отменить.',
  ]);
  assert.deepEqual(strings('zh-CN', trashPurgeSources.all, 'length', [1], size), ['将永久删除回收站内全部 1 项（4 KB）。此操作不可撤销。']);
  assert.deepEqual(strings('zh-CN', trashPurgeSources.selected, 'length', [1], size), ['将永久删除所选的 1 项（4 KB）。此操作不可撤销。']);
});

test('localized count templates retain all source placeholders in each exercised category', () => {
  const extraSources = [...Object.values(conversationCountSources), ...Object.values(trashPurgeSources)];
  for (const locale of Object.keys(languages).filter(locale => locale !== 'zh-CN')) {
    const catalog = JSON.parse(readFileSync(new URL(`../src/locales/translations/${locale}.json`, import.meta.url), 'utf8'));
    for (const source of [...Object.keys(catalog), ...extraSources]) {
      const names = tokens(source);
      for (const n of [0, 1, 2, 5, 11, 21, 22, 1.5, 1_000_000]) {
        const template = pluralTemplate(locale, source, Object.fromEntries(names.map(name => [name, n])));
        const context = `${locale} @ ${n}: ${source}`;
        if (extraSources.includes(source)) assert.ok(template, `Missing complete count template: ${context}`);
        if (template !== undefined) {
          assert.deepEqual(tokens(template), names, `Placeholder mismatch: ${context}`);
          assert.doesNotMatch(template, /\[\w+\]/, `Unresolved plural fragment: ${context}`);
        }
      }
    }
  }
});

test('pluralization falls back on unrelated keys and missing or invalid counts', () => {
  assert.equal(pluralTemplate('en', '__proto__', { length: 1 }), undefined);
  assert.equal(pluralTemplate('en', '', { length: 1 }), undefined);
  assert.equal(pluralTemplate('en', messages, { length: 1 }), undefined);
  for (const value1 of ['1', null, undefined, NaN, Infinity]) {
    assert.equal(pluralTemplate('en', managedGames, { value1 }), undefined);
  }
});
