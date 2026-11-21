import { expect, test, type Page } from '@playwright/test';
import { installMockTauri } from './mock-tauri';

async function openIdentity(page: Page) {
  await installMockTauri(page);
  await page.goto('/');
  await page.getByRole('button', {name: '游戏库', exact: true}).click();
  await page.getByLabel('游戏目录', {exact: true}).fill('/sandbox/unknown-game');
  await page.getByRole('button', {name: '检查单个游戏', exact: true}).click();
  await page.getByRole('button', {name: '联网识别', exact: true}).click();
  const modal = page.getByRole('dialog', {name: '联网识别游戏', exact: true});
  await expect(modal.getByLabel('公开搜索词', {exact: true})).toHaveValue('FixtureGame.exe game');
  return modal;
}

test('online game identity separates search consent from root registration', async ({page}, info) => {
  const modal = await openIdentity(page);
  await expect(modal.getByRole('button', {name: '搜索候选', exact: true})).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => ['search_game_identity', 'confirm_game_identity', 'register_game'].includes(call.command)))).toHaveLength(0);
  await modal.getByLabel('同意发送公开搜索词', {exact: true}).check();
  await modal.getByLabel('公开搜索词', {exact: true}).fill('Public Fixture game executable');
  await expect(modal.getByLabel('同意发送公开搜索词', {exact: true})).not.toBeChecked();
  await modal.getByLabel('同意发送公开搜索词', {exact: true}).check();
  await modal.getByRole('button', {name: '搜索候选', exact: true}).click();
  await expect(modal.getByLabel('同意发送公开搜索词', {exact: true})).not.toBeChecked();
  await expect(modal.getByRole('button', {name: '添加', exact: true})).toBeDisabled();
  await modal.getByRole('radio').first().check();
  await expect(modal.getByLabel('确认的游戏名称', {exact: true})).toHaveValue('Fixture Game');
  await modal.getByLabel('确认添加', {exact: true}).check();
  await modal.getByLabel('确认的游戏名称', {exact: true}).fill('Confirmed Fixture Game');
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
  await expect(page.getByText('Confirmed Fixture Game', {exact: true})).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((call: any) => call.command === 'register_game')).toHaveLength(0);
  expect(calls.find((call: any) => call.command === 'search_game_identity').args).toEqual({directory: '/sandbox/unknown-game', expectedFingerprint: 'e'.repeat(64), publicQuery: 'Public Fixture game executable', confirmation: 'SEARCH_GAME_IDENTITY'});
  expect(calls.find((call: any) => call.command === 'confirm_game_identity').args).toEqual({searchId: 'identity-search-fixture', candidateIndex: 0, name: 'Confirmed Fixture Game', confirmation: 'CONFIRM_GAME_IDENTITY'});
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
  await page.getByRole('button', {name: '联网识别', exact: true}).click();
  modal = page.getByRole('dialog', {name: '联网识别游戏', exact: true});
  await expect(modal.getByLabel('同意发送公开搜索词', {exact: true})).not.toBeChecked();
  await expect(modal.getByRole('radio')).toHaveCount(0);
  await expect(modal.getByRole('button', {name: '添加', exact: true})).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === 'confirm_game_identity'))).toHaveLength(0);
});

test('missing search credentials never produce fake game identity candidates', async ({page}) => {
  const modal = await openIdentity(page);
  await page.evaluate(() => (window as any).__AMM_TEST__.identityOutcome = 'missing-key');
  await modal.getByLabel('同意发送公开搜索词', {exact: true}).check();
  await modal.getByRole('button', {name: '搜索候选', exact: true}).click();
  await expect(modal.getByRole('status')).toContainText('Brave Search API Key');
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
