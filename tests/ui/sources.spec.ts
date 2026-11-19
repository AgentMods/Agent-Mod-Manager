import { expect, test, type Page } from '@playwright/test';
import { installMockTauri } from './mock-tauri';

const homepage = 'https://mod.3dmgame.com/mod/123';
const manualMessage = '没有唯一可核验的直链资源，可能是网盘或多个可选文件；请在原始页面选择';
const reviewWarning = '3DM 未提供可核验的精确字节数及文件 hash；下载后必须人工审查，尚未安装';

async function openSources(page: Page, outcome: 'missing_key' | 'manual_download' | 'direct') {
  await installMockTauri(page);
  await page.goto('/');
  // Wrap the fixture before navigating to Discover, preserving its event transport.
  await page.evaluate(({outcome, homepage, manualMessage, reviewWarning}) => {
    const target = window as any;
    const invoke = target.__TAURI_INTERNALS__.invoke;
    const status = {source_id: '3dm', state: outcome === 'missing_key' ? 'missing_key' : 'limited', message: outcome === 'missing_key' ? '需要配置 3DM API Key' : '3DM API v3 已启用；网盘、多文件及版本兼容性需人工核对'};
    const mod = {source_id: '3dm', project_id: '123', game_id: '42', name: '3DM Fixture Mod', summary: '3DM source fixture', author: 'Fixture Author', homepage_url: homepage, icon_url: null, downloads: 1024};
    const resolved = {
      source_id: '3dm', project_id: '123', version_id: '456', name: mod.name, version: '1.0.0', filename: 'Fixture.zip', size: null,
      hashes: {sha256: null, sha512: null, sha1: null, md5: null},
      homepage_url: homepage, warnings: [reviewWarning],
    };
    const artifact = {path: '/sandbox/data/downloads/3dm/Fixture.zip', filename: 'Fixture.zip', size: 1024, sha256: 'b'.repeat(64), integrity_verified: false, requires_review: true, warnings: [reviewWarning]};
    const downloads: (typeof artifact & {id: string})[] = [];
    let resolutionCount = 0;
    target.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, any> = {}) => {
      if (!['list_mod_sources', 'search_mods', 'resolve_mod', 'download_mod', 'list_downloads'].includes(command)) return invoke(command, args);
      target.__AMM_TEST__.calls.push({command, args});
      switch (command) {
        case 'list_mod_sources': return [status];
        case 'search_mods': return {mods: outcome === 'missing_key' ? [] : [mod], sources: [status]};
        case 'resolve_mod': return {
          status: {...status, state: outcome === 'manual_download' ? 'manual_download' : status.state, message: outcome === 'manual_download' ? manualMessage : status.message},
          download: outcome === 'direct' ? {...resolved, resolution_id: `3dm-resolution-${++resolutionCount}`} : null,
          homepage_url: homepage, dependencies: null,
        };
        case 'download_mod': {
          const record = {...artifact, id: `artifact-3dm-${downloads.length + 1}`};
          downloads.push(record);
          return record;
        }
        case 'list_downloads': return downloads.map(artifact => ({artifact, sourceId: '3dm', projectId: '123', versionId: '456', name: mod.name, version: resolved.version, createdAt: '2026-11-18T00:00:00Z'}));
      }
    };
  }, {outcome, homepage, manualMessage, reviewWarning});
  await page.getByRole('button', {name: 'Mod 发现', exact: true}).click();
  const discover = page.getByTestId('discover-page');
  await expect(discover.getByRole('option', {name: '3DM', exact: true})).toHaveAttribute('value', '3dm');
  await discover.getByLabel('Mod 来源', {exact: true}).selectOption({label: '3DM'});
  return discover;
}

async function search3dm(page: Page) {
  await page.getByLabel('搜索 Mod', {exact: true}).fill('  Fixture  ');
  await page.getByLabel('来源游戏 ID', {exact: true}).fill(' 42 ');
  await page.getByRole('button', {name: '搜索', exact: true}).click();
}

async function openNexus(page: Page) {
  await installMockTauri(page);
  await page.goto('/');
  await page.evaluate(() => {
    const target = window as any;
    const invoke = target.__TAURI_INTERNALS__.invoke;
    const status = { source_id: 'nexus', state: 'limited', message: '支持按游戏搜索 Mod 名称或数字 ID；直接下载受账户权限限制' };
    const mod = { source_id: 'nexus', project_id: 'skyrimspecialedition/12604', game_id: 'skyrimspecialedition', name: 'SkyUI', summary: 'Fixture Nexus search result', author: 'SkyUI Team', homepage_url: 'https://www.nexusmods.com/skyrimspecialedition/mods/12604', icon_url: null, downloads: 12345 };
    target.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, any> = {}) => {
      if (!['list_mod_sources', 'search_mods'].includes(command)) return invoke(command, args);
      target.__AMM_TEST__.calls.push({ command, args });
      if (command === 'list_mod_sources') return [status];
      const query = args.query;
      if (query.game_version || query.loader) {
        return { mods: [], sources: [{ source_id: 'nexus', state: 'unsupported', message: 'Nexus 搜索不支持游戏版本或加载器筛选；请清除筛选后按名称搜索' }] };
      }
      return { mods: [mod], sources: [{ ...status, message: '已按游戏搜索 Mod 名称；还有更多结果。直接下载受账户权限限制' }] };
    };
  });
  await page.getByRole('button', { name: 'Mod 发现', exact: true }).click();
  const discover = page.getByTestId('discover-page');
  await expect(discover.getByRole('option', { name: 'Nexus Mods', exact: true })).toHaveAttribute('value', 'nexus');
  await discover.getByLabel('Mod 来源', { exact: true }).selectOption({ label: 'Nexus Mods' });
  return discover;
}

test('Nexus text search keeps scoped project identity and rejects unsupported filters in the UI', async ({ page }) => {
  const discover = await openNexus(page);
  await discover.getByLabel('搜索 Mod', { exact: true }).fill('SkyUI');
  await discover.getByLabel('来源游戏 ID', { exact: true }).fill('skyrimspecialedition');
  await discover.getByRole('button', { name: '搜索', exact: true }).click();
  const result = discover.getByRole('button').filter({ has: page.getByRole('heading', { name: 'SkyUI', exact: true }) });
  await expect(result).toContainText('SkyUI Team · Nexus Mods');
  await expect(discover.getByText('Nexus Mods · 受限', { exact: true })).toBeVisible();
  await expect(discover.getByTitle('已按游戏搜索 Mod 名称；还有更多结果。直接下载受账户权限限制', { exact: true })).toBeVisible();
  await discover.getByLabel('游戏版本', { exact: true }).fill('1.6.1170');
  await discover.getByRole('button', { name: '搜索', exact: true }).click();
  await expect(discover.getByText('没有匹配的 Mod', { exact: true })).toBeVisible();
  await expect(discover.getByText('Nexus Mods · 不支持', { exact: true })).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((call: any) => call.command === 'search_mods').map((call: any) => call.args)).toEqual([
    { query: { query: 'SkyUI', game_ids: { nexus: 'skyrimspecialedition' }, game_version: null, loader: null, offset: 0, limit: 20, source_ids: ['nexus'] } },
    { query: { query: 'SkyUI', game_ids: { nexus: 'skyrimspecialedition' }, game_version: '1.6.1170', loader: null, offset: 0, limit: 20, source_ids: ['nexus'] } },
  ]);
});

test('3DM is selectable by name and missing credentials stay explicit', async ({page}) => {
  const discover = await openSources(page, 'missing_key');
  await expect(discover.getByLabel('Mod 来源', {exact: true})).toHaveValue('3dm');
  await expect(discover.getByText('3DM · 缺少密钥', {exact: true})).toBeVisible();
  await expect(discover.getByTitle('需要配置 3DM API Key', {exact: true})).toBeVisible();
  await search3dm(page);
  await expect(discover.getByText('没有匹配的 Mod', {exact: true})).toBeVisible();
  await expect(discover.getByText('3DM · 缺少密钥', {exact: true})).toBeVisible();
  await expect(discover.getByRole('button', {name: '下载归档', exact: true})).toHaveCount(0);
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((call: any) => call.command === 'list_mod_sources')).toHaveLength(1);
  expect(calls.filter((call: any) => call.command === 'search_mods').map((call: any) => call.args)).toEqual([
    {query: {query: 'Fixture', game_ids: {'3dm': '42'}, game_version: null, loader: null, offset: 0, limit: 20, source_ids: ['3dm']}},
  ]);
  expect(calls.filter((call: any) => ['resolve_mod', 'download_mod', 'plan_install', 'apply_plan'].includes(call.command))).toHaveLength(0);
});

test('3DM search results resolve to ManualDownload with only the original page', async ({page}) => {
  const discover = await openSources(page, 'manual_download');
  await expect(discover.getByText('3DM · 受限', {exact: true})).toBeVisible();
  await search3dm(page);
  const result = discover.getByRole('button').filter({has: page.getByRole('heading', {name: '3DM Fixture Mod', exact: true})});
  await expect(result).toContainText('Fixture Author · 3DM');
  await result.click();
  const details = discover.locator('aside');
  await expect(details.getByText('3DM', {exact: true})).toBeVisible();
  await expect(details.getByText(manualMessage, {exact: true})).toBeVisible();
  await expect(details.getByRole('button', {name: '下载归档', exact: true})).toHaveCount(0);
  await expect(discover.getByRole('button', {name: '安装预览', exact: true})).toHaveCount(0);
  await details.getByRole('button', {name: '原始页面', exact: true}).click();
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((call: any) => call.command === 'resolve_mod').map((call: any) => call.args)).toEqual([
    {request: {source_id: '3dm', project_id: '123', version_id: null, game_id: '42', game_version: null, loader: null}},
  ]);
  expect(calls.filter((call: any) => call.command === 'plugin:opener|open_url').map((call: any) => call.args.url)).toEqual([homepage]);
  expect(calls.filter((call: any) => ['download_mod', 'plan_install', 'apply_plan'].includes(call.command))).toHaveLength(0);
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('3DM requires a fresh resolution for each explicit download and never installs implicitly', async ({page}) => {
  const discover = await openSources(page, 'direct');
  await search3dm(page);
  const project = discover.getByRole('button').filter({has: page.getByRole('heading', {name: '3DM Fixture Mod', exact: true})});
  await project.click();
  const details = discover.locator('aside');
  await expect(details).toContainText('Fixture.zip');
  await expect(details.getByText(reviewWarning, {exact: true})).toBeVisible();
  await expect(details.getByRole('button', {name: '原始页面', exact: true})).toBeEnabled();
  const download = details.getByRole('button', {name: '下载归档', exact: true});
  await expect(download).toBeEnabled();
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => ['download_mod', 'plan_install', 'apply_plan'].includes(call.command)))).toHaveLength(0);
  await download.click();
  await expect(page.getByRole('status')).toHaveText('下载已完成，尚未安装');
  await expect(discover.getByText('需人工审核', {exact: true})).toBeVisible();
  await expect(discover.getByRole('button', {name: '安装预览', exact: true})).toBeVisible();
  await expect(download).toBeDisabled();

  await project.click();
  await expect(download).toBeEnabled();
  await download.click();
  await expect(discover.getByRole('button', {name: '安装预览', exact: true})).toHaveCount(2);
  await expect(page.getByRole('dialog')).toHaveCount(0);
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  const request = {source_id: '3dm', project_id: '123', version_id: null, game_id: '42', game_version: null, loader: null};
  expect(calls.filter((call: any) => ['resolve_mod', 'download_mod', 'plan_install', 'apply_plan'].includes(call.command))).toStrictEqual([
    {command: 'resolve_mod', args: {request}},
    {command: 'download_mod', args: {taskId: expect.any(String), resolutionId: '3dm-resolution-1'}},
    {command: 'resolve_mod', args: {request}},
    {command: 'download_mod', args: {taskId: expect.any(String), resolutionId: '3dm-resolution-2'}},
  ]);
});
