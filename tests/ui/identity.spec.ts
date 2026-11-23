import { expect, test, type Page } from '@playwright/test';
import { installMockTauri } from './mock-tauri';

async function openIdentity(page: Page) {
  await installMockTauri(page);
  await page.goto('/');
  await page.getByRole('button', {name: '游戏库', exact: true}).click();
  await page.getByRole('button', {name: '扫描游戏', exact: true}).click();
  const discovery = page.getByRole('dialog', {name: '扫描游戏', exact: true});
  await expect(discovery).toBeVisible();
  await expect(discovery.getByRole('status').filter({hasText: '已检查'})).toBeVisible();
  await discovery.getByLabel('游戏目录', {exact: true}).fill('/sandbox/unknown-game');
  await discovery.getByRole('button', {name: '检查单个游戏', exact: true}).click();
  await discovery.getByRole('button', {name: '联网识别', exact: true}).click();
  const modal = page.getByRole('dialog', {name: '联网识别游戏', exact: true});
  await expect(modal.getByLabel('公开搜索词', {exact: true})).toHaveValue('FixtureGame.exe game');
  return modal;
}

test('built-in Bing identity search needs no API key or configurable Playwright MCP connection', async ({page}, info) => {
  const modal = await openIdentity(page);
  const publicQuery = 'Public game edition '.repeat(100) + 'official site';
  const confirmedName = '已确认游戏名称 '.repeat(90) + '完整版';
  await expect(modal.getByText('Bing（应用内置搜索）', {exact: true})).toBeVisible();
  await expect(modal.getByRole('button', {name: '搜索候选', exact: true})).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => ['search_game_identity', 'confirm_game_identity', 'register_game'].includes(call.command)))).toHaveLength(0);
  await modal.getByLabel('同意发送公开搜索词', {exact: true}).check();
  await modal.getByLabel('公开搜索词', {exact: true}).fill(publicQuery);
  await expect(modal.getByLabel('同意发送公开搜索词', {exact: true})).not.toBeChecked();
  await modal.getByLabel('同意发送公开搜索词', {exact: true}).check();
  await modal.getByRole('button', {name: '搜索候选', exact: true}).click();
  await expect(modal.getByLabel('同意发送公开搜索词', {exact: true})).not.toBeChecked();
  await expect(modal.getByRole('button', {name: '添加', exact: true})).toBeDisabled();
  await modal.getByRole('radio').first().check();
  await expect(modal.getByLabel('确认的游戏名称', {exact: true})).toHaveValue('Fixture Game');
  await modal.getByLabel('确认添加', {exact: true}).check();
  await modal.getByLabel('确认的游戏名称', {exact: true}).fill(confirmedName);
  await expect(modal.getByLabel('确认添加', {exact: true})).not.toBeChecked();
  await modal.getByLabel('确认添加', {exact: true}).check();
  for (const theme of ['light', 'dark']) {
    await page.evaluate(value => document.documentElement.dataset.theme = value, theme);
    await expect(modal).toBeVisible();
    const bounds = await modal.boundingBox();
    expect(bounds).not.toBeNull();
    expect(bounds!.x).toBeGreaterThanOrEqual(0);
    expect(bounds!.width).toBeLessThanOrEqual(page.viewportSize()!.width);
    expect(await modal.evaluate(element => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    await page.screenshot({path: `output/playwright/screenshots/${info.project.name}-identity-${theme}.png`, animations: 'disabled'});
  }
  await modal.getByRole('button', {name: '添加', exact: true}).click();
  await expect(modal).not.toBeVisible();
  const discovery = page.getByRole('dialog', {name: '扫描游戏', exact: true});
  await expect(discovery).toBeVisible();
  await expect(page.getByText(`已将「${confirmedName}」添加到游戏库`, {exact: true})).toBeVisible();
  const candidate = discovery.locator('[data-slot="item"]').filter({has: page.getByText('/sandbox/unknown-game', {exact: true})});
  await expect(candidate.getByRole('button', {name: '管理中', exact: true})).toBeDisabled();
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((call: any) => call.command === 'register_game')).toHaveLength(0);
  expect(calls.filter((call: any) => call.command === 'install_manager_plugin')).toHaveLength(0);
  expect(calls.find((call: any) => call.command === 'search_game_identity').args).toEqual({directory: '/sandbox/unknown-game', expectedFingerprint: 'e'.repeat(64), publicQuery, confirmation: 'SEARCH_GAME_IDENTITY'});
  expect(calls.filter((call: any) => call.command === 'set_secret' && call.args.key === 'sources.brave')).toHaveLength(0);
  expect(calls.filter((call: any) => call.command === 'mcp_connect')).toHaveLength(0);
  expect(calls.find((call: any) => call.command === 'confirm_game_identity').args).toEqual({searchId: 'identity-search-fixture', candidateIndex: 0, name: confirmedName, confirmation: 'CONFIRM_GAME_IDENTITY'});
  await page.keyboard.press('Escape');
  await expect(discovery).toBeHidden();
  await expect(page.getByTestId('library-page').getByRole('heading', {name: confirmedName, exact: true})).toBeVisible();
});

test('changed queries and reopened identity dialogs discard earlier consent', async ({page}) => {
  let modal = await openIdentity(page);
  await modal.getByLabel('同意发送公开搜索词', {exact: true}).check();
  await modal.getByRole('button', {name: '搜索候选', exact: true}).click();
  await modal.getByRole('radio').first().check();
  await modal.getByLabel('确认添加', {exact: true}).check();
  await modal.getByLabel('公开搜索词', {exact: true}).fill('Different public query');
  await expect(modal.getByRole('radio')).toHaveCount(0);
  await expect(modal.getByRole('button', {name: '添加', exact: true})).toBeDisabled();
  await modal.getByLabel('同意发送公开搜索词', {exact: true}).check();
  await modal.getByRole('button', {name: '取消', exact: true}).click();
  const discovery = page.getByRole('dialog', {name: '扫描游戏', exact: true});
  await expect(discovery).toBeVisible();
  await expect(discovery.getByLabel('游戏目录', {exact: true})).toHaveValue('/sandbox/unknown-game');
  await discovery.getByRole('button', {name: '联网识别', exact: true}).click();
  modal = page.getByRole('dialog', {name: '联网识别游戏', exact: true});
  await expect(modal.getByLabel('同意发送公开搜索词', {exact: true})).not.toBeChecked();
  await expect(modal.getByRole('radio')).toHaveCount(0);
  await expect(modal.getByRole('button', {name: '添加', exact: true})).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === 'confirm_game_identity'))).toHaveLength(0);
});

test('built-in search cannot start without browser components and offers recovery steps', async ({page}) => {
  const modal = await openIdentity(page);
  await page.evaluate(() => (window as any).__AMM_TEST__.identityOutcome = 'needs-configuration');
  await modal.getByLabel('同意发送公开搜索词', {exact: true}).check();
  await modal.getByRole('button', {name: '搜索候选', exact: true}).click();
  const status = modal.getByRole('status');
  await expect(status).toContainText('Playwright 无法启动浏览器');
  await expect(status).toContainText('请安装 Microsoft Edge');
  await expect(status).not.toContainText('API key');
  await expect(modal.getByRole('radio')).toHaveCount(0);
  await expect(modal.getByRole('button', {name: '添加', exact: true})).toBeDisabled();
});

test('unavailable built-in search reports network and page failures without fake candidates', async ({page}) => {
  const modal = await openIdentity(page);
  await page.evaluate(() => (window as any).__AMM_TEST__.identityOutcome = 'unavailable');
  await modal.getByLabel('同意发送公开搜索词', {exact: true}).check();
  await modal.getByRole('button', {name: '搜索候选', exact: true}).click();
  await expect(modal.getByRole('status')).toContainText('未能读取 Bing 搜索结果');
  await expect(modal.getByRole('status')).toContainText('检查网络');
  await expect(modal.getByRole('status')).toContainText('验证码');
  await expect(modal.getByRole('radio')).toHaveCount(0);
  await expect(modal.getByRole('button', {name: '添加', exact: true})).toBeDisabled();
});

test('stale evidence remains unregistered and requires renewed consent', async ({page}) => {
  const modal = await openIdentity(page);
  await modal.getByLabel('同意发送公开搜索词', {exact: true}).check();
  await modal.getByRole('button', {name: '搜索候选', exact: true}).click();
  await modal.getByRole('radio').first().check();
  await modal.getByLabel('确认添加', {exact: true}).check();
  await page.evaluate(() => (window as any).__AMM_TEST__.identityOutcome = 'stale');
  await modal.getByRole('button', {name: '添加', exact: true}).click();
  await expect(modal.getByRole('alert')).toContainText('目录内容已变化');
  await expect(modal.getByRole('button', {name: '添加', exact: true})).toBeDisabled();
  await expect(page.getByText('Fixture Game', {exact: true})).not.toBeVisible();
});
