import { expect, test, type Page } from '@playwright/test';
import { installMockTauri } from './mock-tauri';

async function prepare(page: Page) {
  await page.getByRole('button', {name: '插件中心', exact: true}).click();
  await page.getByRole('tab', {name: /^草稿/}).click();
  await page.getByRole('button', {name: '准备发布 fixture-game', exact: true}).click();
  const metadata = page.getByRole('dialog', {name: '发布元数据：fixture-game', exact: true});
  await metadata.getByLabel('发布标题', {exact: true}).fill('Fixture 游戏适配');
  await metadata.getByLabel('发布摘要', {exact: true}).fill('公开适配包摘要');
  await metadata.getByLabel('发布描述', {exact: true}).fill('公开描述，不含本地日志。');
  await metadata.getByLabel('发布标签', {exact: true}).fill('adapter, fixture');
  await metadata.getByLabel('发布游戏 ID', {exact: true}).fill('1, 2');
  await metadata.getByRole('button', {name: '生成上传预览', exact: true}).click();
  const preview = page.getByRole('dialog', {name: '确认公开发布适配包', exact: true});
  await expect(preview).toBeVisible();
  return preview;
}

test('publication exposes exact contents and requires fresh explicit consent', async ({page}, info) => {
  await installMockTauri(page);
  await page.goto('/');
  const preview = await prepare(page);
  await expect(preview.getByRole('button', {name: '核对远端状态', exact: true})).toHaveCount(0);
  await expect(preview).toContainText('https://www.glossmod.com/api/v1/mcp');
  await expect(preview).toContainText('c'.repeat(64));
  await expect(preview).toContainText('公开描述，不含本地日志。');
  await preview.locator('summary').filter({hasText: 'adapter.json'}).click();
  await expect(preview.locator('pre').first()).toContainText('"slug":"fixture-game"');
  await preview.locator('summary').filter({hasText: 'SKILL.md'}).click();
  await expect(preview.locator('pre').last()).toContainText('Review before installing.');
  for (const theme of ['light', 'dark']) {
    await page.locator('html').evaluate((element, value) => element.setAttribute('data-theme', value), theme);
    await page.screenshot({path: `output/playwright/screenshots/${info.project.name}-publication-${theme}.png`, fullPage: true, animations: 'disabled'});
    expect(await preview.evaluate(element => element.scrollWidth > element.clientWidth + 1)).toBe(false);
    const bounds = await preview.boundingBox();
    expect(Math.abs(bounds!.x - (page.viewportSize()!.width - bounds!.width) / 2)).toBeLessThan(2);
    expect(bounds!.y).toBeGreaterThanOrEqual(15);
  }
  await preview.getByLabel('已核对公开发布内容', {exact: true}).check();
  await preview.getByLabel('公开发布确认文本', {exact: true}).fill('PUBLISH_PLUGIN');
  await preview.getByRole('button', {name: '关闭', exact: true}).last().click();
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'publish_plugin'))).toHaveLength(0);
  await page.getByRole('tab', {name: '发布记录', exact: true}).click();
  await page.getByRole('button', {name: '查看发布 Fixture 游戏适配', exact: true}).click();
  const publish = preview.getByRole('button', {name: '上传并公开发布', exact: true});
  await expect(preview.getByLabel('已核对公开发布内容', {exact: true})).not.toBeChecked();
  await expect(preview.getByLabel('公开发布确认文本', {exact: true})).toHaveValue('');
  await preview.getByLabel('公开发布确认文本', {exact: true}).fill('PUBLISH_PLUGIN');
  await expect(publish).toBeDisabled();
  await preview.getByLabel('已核对公开发布内容', {exact: true}).check();
  await publish.click();
  await expect(preview).toContainText('已发布');
  await expect(preview).toContainText('作品 99');
  await expect(preview.getByRole('button', {name: '上传并公开发布', exact: true})).toHaveCount(0);
  await expect(preview.getByRole('button', {name: '核对远端状态', exact: true})).toHaveCount(0);
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((c: any) => c.command === 'publish_plugin').map((c: any) => c.args)).toEqual([{id: 'publish_fixture', digest: 'd'.repeat(64), confirmation: 'PUBLISH_PLUGIN'}]);
  expect(calls.find((c: any) => c.command === 'prepare_plugin_publication').args.metadata).toMatchObject({title: 'Fixture 游戏适配', tags: ['adapter', 'fixture'], gameIds: [1, 2]});
});

test('unknown publication outcomes are reloaded and cannot be resent', async ({page}) => {
  await installMockTauri(page);
  await page.goto('/');
  await page.evaluate(() => { (window as any).__AMM_TEST__.publicationOutcome = 'unknown-create'; });
  const preview = await prepare(page);
  await preview.getByLabel('已核对公开发布内容', {exact: true}).check();
  await preview.getByLabel('公开发布确认文本', {exact: true}).fill('PUBLISH_PLUGIN');
  await preview.getByRole('button', {name: '上传并公开发布', exact: true}).click();
  await expect(preview).toContainText('结果不确定');
  await expect(preview).toContainText('文件 - / 作品 -');
  await expect(preview.getByRole('button', {name: '上传并公开发布', exact: true})).toHaveCount(0);
  await expect(preview.getByRole('button', {name: '确认继续发布', exact: true})).toHaveCount(0);
  await expect(preview.getByRole('button', {name: '核对远端状态', exact: true})).toHaveCount(0);
  await preview.getByRole('button', {name: '关闭', exact: true}).last().click();
  await page.getByRole('tab', {name: '发布记录', exact: true}).click();
  await expect(page.getByRole('cell', {name: '结果不确定', exact: true})).toBeVisible();
  await page.getByRole('button', {name: '刷新记录', exact: true}).click();
  await page.getByRole('button', {name: '查看发布 Fixture 游戏适配', exact: true}).click();
  await expect(preview.getByRole('button', {name: '确认继续发布', exact: true})).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'publish_plugin'))).toHaveLength(1);
});

test('unconfirmed reconciliation preserves unknown completion without resending', async ({page}, info) => {
  await installMockTauri(page);
  await page.goto('/');
  await page.evaluate(() => { (window as any).__AMM_TEST__.publicationOutcome = 'unknown-complete'; });
  const preview = await prepare(page);
  await preview.getByLabel('已核对公开发布内容', {exact: true}).check();
  await preview.getByLabel('公开发布确认文本', {exact: true}).fill('PUBLISH_PLUGIN');
  await preview.getByRole('button', {name: '上传并公开发布', exact: true}).click();
  await expect(preview).toContainText('结果不确定');
  await expect(preview).toContainText('文件 42 / 作品 -');
  await expect(preview.getByRole('button', {name: '核对远端状态', exact: true})).toBeVisible();
  await expect(preview.getByRole('button', {name: '确认继续发布', exact: true})).toHaveCount(0);

  await page.evaluate(() => { (window as any).__AMM_TEST__.publicationReconcileOutcome = 'unconfirmed'; });
  await preview.getByRole('button', {name: '核对远端状态', exact: true}).click();
  await expect(preview.getByText('尚未核实，发布状态保持不变', {exact: true})).toBeVisible();
  await expect(preview).toContainText('结果不确定');
  await expect(preview).toContainText('文件 42 / 作品 -');
  await expect(preview.getByRole('button', {name: '确认继续发布', exact: true})).toHaveCount(0);
  for (const theme of ['light', 'dark']) {
    await page.locator('html').evaluate((element, value) => element.setAttribute('data-theme', value), theme);
    await page.screenshot({path: `output/playwright/screenshots/${info.project.name}-publication-status-${theme}.png`, animations: 'disabled'});
    expect(await preview.evaluate(element => element.scrollWidth > element.clientWidth + 1)).toBe(false);
    await expect(preview.getByRole('button', {name: '核对远端状态', exact: true})).toBeInViewport();
  }
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'reconcile_plugin_publication').map((c: any) => c.args))).toEqual([{id: 'publish_fixture'}]);
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'publish_plugin'))).toHaveLength(1);
});

test('verified upload completion is refetched and requires fresh consent to publish', async ({page}) => {
  await installMockTauri(page);
  await page.goto('/');
  await page.evaluate(() => { (window as any).__AMM_TEST__.publicationOutcome = 'unknown-complete'; });
  const preview = await prepare(page);
  await preview.getByLabel('已核对公开发布内容', {exact: true}).check();
  await preview.getByLabel('公开发布确认文本', {exact: true}).fill('PUBLISH_PLUGIN');
  await preview.getByRole('button', {name: '上传并公开发布', exact: true}).click();
  await expect(preview).toContainText('结果不确定');

  const listReadsBefore = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'list_plugin_publications').length);
  await preview.getByRole('button', {name: '核对远端状态', exact: true}).click();
  await expect(preview).toContainText('可发布');
  await expect.poll(() => page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'list_plugin_publications').length)).toBeGreaterThan(listReadsBefore);
  const resume = preview.getByRole('button', {name: '确认继续发布', exact: true});
  await expect(resume).toBeVisible();
  await expect(preview.getByLabel('已核对公开发布内容', {exact: true})).not.toBeChecked();
  await expect(preview.getByLabel('公开发布确认文本', {exact: true})).toHaveValue('');
  await expect(resume).toBeDisabled();
  await preview.getByLabel('公开发布确认文本', {exact: true}).fill('PUBLISH_PLUGIN');
  await expect(resume).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'publish_plugin'))).toHaveLength(1);

  await preview.getByLabel('已核对公开发布内容', {exact: true}).check();
  await page.evaluate(() => { (window as any).__AMM_TEST__.publicationOutcome = 'completed'; });
  await resume.click();
  await expect(preview).toContainText('已发布');
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'reconcile_plugin_publication').map((c: any) => c.args))).toEqual([{id: 'publish_fixture'}]);
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'publish_plugin'))).toHaveLength(2);
});

test('verified publish-work reconciliation refetches the completed work without resending', async ({page}) => {
  await installMockTauri(page);
  await page.goto('/');
  await page.evaluate(() => { (window as any).__AMM_TEST__.publicationOutcome = 'unknown-publish'; });
  const preview = await prepare(page);
  await preview.getByLabel('已核对公开发布内容', {exact: true}).check();
  await preview.getByLabel('公开发布确认文本', {exact: true}).fill('PUBLISH_PLUGIN');
  await preview.getByRole('button', {name: '上传并公开发布', exact: true}).click();
  await expect(preview).toContainText('结果不确定');
  await expect(preview).toContainText('文件 42 / 作品 -');
  await expect(preview.getByRole('button', {name: '核对远端状态', exact: true})).toBeVisible();
  await expect(preview.getByRole('button', {name: '确认继续发布', exact: true})).toHaveCount(0);

  const listReadsBefore = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'list_plugin_publications').length);
  await preview.getByRole('button', {name: '核对远端状态', exact: true}).click();
  await expect(preview).toContainText('已发布');
  await expect(preview).toContainText('文件 42 / 作品 99');
  await expect.poll(() => page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'list_plugin_publications').length)).toBeGreaterThan(listReadsBefore);
  await expect(preview.getByRole('button', {name: '核对远端状态', exact: true})).toHaveCount(0);
  await expect(preview.getByRole('button', {name: '确认继续发布', exact: true})).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'reconcile_plugin_publication').map((c: any) => c.args))).toEqual([{id: 'publish_fixture'}]);
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'publish_plugin'))).toHaveLength(1);
});

test('awaiting-scan publication must be reconciled before renewed confirmation', async ({page}) => {
  await installMockTauri(page);
  await page.goto('/');
  await page.evaluate(() => { (window as any).__AMM_TEST__.publicationOutcome = 'scan'; });
  const preview = await prepare(page);
  await preview.getByLabel('已核对公开发布内容', {exact: true}).check();
  await preview.getByLabel('公开发布确认文本', {exact: true}).fill('PUBLISH_PLUGIN');
  await preview.getByRole('button', {name: '上传并公开发布', exact: true}).click();
  const resume = preview.getByRole('button', {name: '确认继续发布', exact: true});
  await expect(resume).toHaveCount(0);
  await expect(preview).toContainText('等待安全扫描');
  await expect(preview.getByRole('button', {name: '核对远端状态', exact: true})).toBeVisible();
  const listReadsBefore = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'list_plugin_publications').length);
  await preview.getByRole('button', {name: '核对远端状态', exact: true}).click();
  await expect(preview).toContainText('可发布');
  await expect.poll(() => page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'list_plugin_publications').length)).toBeGreaterThan(listReadsBefore);
  await expect(resume).toBeVisible();
  await expect(preview.getByLabel('已核对公开发布内容', {exact: true})).not.toBeChecked();
  await expect(preview.getByLabel('公开发布确认文本', {exact: true})).toHaveValue('');
  await expect(resume).toBeDisabled();
  await preview.getByLabel('公开发布确认文本', {exact: true}).fill('PUBLISH_PLUGIN');
  await expect(resume).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'publish_plugin'))).toHaveLength(1);
  await preview.getByLabel('已核对公开发布内容', {exact: true}).check();
  await page.evaluate(() => { (window as any).__AMM_TEST__.publicationOutcome = 'completed'; });
  await resume.click();
  await expect(preview).toContainText('已发布');
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'reconcile_plugin_publication').map((c: any) => c.args))).toEqual([{id: 'publish_fixture'}]);
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((c: any) => c.command === 'publish_plugin'))).toHaveLength(2);
});
