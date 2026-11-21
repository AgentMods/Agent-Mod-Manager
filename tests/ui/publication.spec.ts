import { createHash } from 'node:crypto';
import { expect, test, type Locator, type Page } from '@playwright/test';
import { installMockTauri } from './mock-tauri';
import { attachPublicationHandlers, installPublicationHandlers } from './publication-fixture';

const button = (scope: Page | Locator, name: string) => scope.getByRole('button', {name, exact: true});
const dialog = (page: Page) => page.getByRole('dialog', {name: '发布管理器插件', exact: true});
const approval = (review: Locator) => review.getByLabel('输入 PUBLISH_PLUGIN 确认发布');
const acknowledge = (review: Locator) => review.getByRole('checkbox', {name: '已检查公开发布的内容'});
const calls = (page: Page, command: string) => page.evaluate(name => (window as any).__AMM_TEST__.calls
  .filter((call: {command: string}) => call.command === name).map((call: {args: unknown}) => call.args), command);

async function openForm(page: Page) {
  await page.getByRole('tab', {name: /^草稿/}).click();
  await button(page, '准备发布 fixture-game').click();
  return page.getByRole('dialog', {name: '准备发布管理器插件', exact: true});
}
async function preview(page: Page, form: Locator) {
  await button(form, '生成发布预览').click();
  const review = dialog(page);
  await expect(review).toBeVisible();
  return review;
}
async function approve(review: Locator) {
  await acknowledge(review).check();
  await approval(review).fill('PUBLISH_PLUGIN');
  await button(review, '确认发布').click();
}
async function openHistory(page: Page, name: string) {
  await page.getByRole('tab', {name: '发布记录', exact: true}).click();
  await button(page, `查看发布 ${name}`).click();
}

test.beforeEach(async ({page}) => {
  await installMockTauri(page, {withExternalAdapter: true});
  await page.addInitScript(installPublicationHandlers);
  await page.goto('/');
  await page.evaluate(attachPublicationHandlers);
  await button(page, '游戏库').click();
  await button(page, '插件中心').click();
});

test('首次发布展示原文和摘要，再次打开审批需要重新确认', async ({page}, info) => {
  await page.getByRole('tab', {name: '发布记录', exact: true}).click();
  await expect(page.getByText('暂无发布记录')).toBeVisible();
  const form = await openForm(page);
  await expect(form.getByRole('radio', {name: '首次发布'})).toBeChecked();
  await form.getByLabel('版本标签').fill('游戏适配，中文');
  const review = await preview(page, form);
  await expect(review).toContainText('新建管理器插件');
  await expect(review).toContainText('fixture-game');
  await expect(review).toContainText('1.1.0');
  await expect(review).toContainText('游戏适配、中文');
  await expect(review).toContainText('https://www.glossmod.com/api/v1/mcp');
  await expect(review).not.toContainText(/ZIP|作品标题|游戏 ID/);
  const digest = await review.getByText(/^[a-f0-9]{64}$/).first().textContent();
  for (const theme of ['light', 'dark']) {
    await page.locator('html').evaluate((el, value) => el.setAttribute('data-theme', value), theme);
    await page.screenshot({path: `output/playwright/screenshots/${info.project.name}-manager-publication-${theme}.png`, animations: 'disabled'});
    expect(await review.evaluate(el => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);
    const bounds = (await review.boundingBox())!;
    expect(Math.abs(bounds.x - (page.viewportSize()!.width - bounds.width) / 2)).toBeLessThan(2);
  }
  await review.locator('summary').filter({hasText: 'adapter.json'}).click();
  await review.locator('summary').filter({hasText: 'SKILL.md'}).click();
  const contents = await review.locator('pre').allTextContents();
  expect(contents).toEqual([
    '{\r\n  "slug": "fixture-game", "name": "测试游戏", "version": "1.1.0"\r\n}\r\n',
    '---\nname: fixture-game\ndescription: 公开测试\n---\nReview <em>before</em> installing.\n',
  ]);
  for (const raw of contents) await expect(review.getByText(`SHA-256 ${createHash('sha256').update(raw).digest('hex')}`, {exact: true})).toBeVisible();
  await expect(review.locator('pre em')).toHaveCount(0);
  await acknowledge(review).check();
  await approval(review).fill('PUBLISH_PLUGIN');
  await button(review, '关闭').last().click();
  expect(await calls(page, 'publish_manager_publication')).toEqual([]);
  await openHistory(page, 'fixture-game');
  await expect(acknowledge(review)).not.toBeChecked();
  await expect(approval(review)).toHaveValue('');
  await approval(review).fill('PUBLISH_PLUGIN');
  await expect(button(review, '确认发布')).toBeDisabled();
  await acknowledge(review).check();
  await approval(review).fill('publish_plugin');
  await expect(button(review, '确认发布')).toBeDisabled();
  await approval(review).fill('PUBLISH_PLUGIN');
  await button(review, '确认发布').click();
  await expect(review.getByText('已发布', {exact: true})).toBeVisible();
  await expect(button(review, '确认发布')).toHaveCount(0);
  await expect(button(review, '核对远端状态')).toHaveCount(0);
  expect(await calls(page, 'prepare_manager_publication')).toEqual([{draftId: 'draft-fixture', target: {kind: 'first'}, tags: ['游戏适配', '中文']}]);
  expect(await calls(page, 'publish_manager_publication')).toEqual([{id: 'manager_1', digest, confirmation: 'PUBLISH_PLUGIN'}]);
  expect(await calls(page, 'list_my_manager_plugins')).toEqual([]);
  expect(await calls(page, 'publish_plugin')).toEqual([]);
});

test('从已安装外部适配包按 slug 发布，返回云端目录即可看到新版', async ({page}) => {
  // Keep the catalog cache fresh for this visit so only publication invalidation can trigger a second read.
  await page.clock.setFixedTime(new Date());
  await page.evaluate(() => { window.__publicationFixture.exposeCatalog = true; });
  const catalog = page.getByRole('tab', {name: '云端插件', exact: true});
  await catalog.click();
  await expect(page.getByText('暂无已发布插件', {exact: true})).toBeVisible();
  expect(await calls(page, 'list_manager_plugin_games')).toHaveLength(1);
  await page.getByRole('tab', {name: '已安装', exact: true}).click();
  await page.getByRole('row').filter({hasText: 'Minecraft Java Edition'}).getByRole('button').click();
  await page.getByRole('button', {name: /发布/}).click();
  const form = page.getByRole('dialog', {name: '准备发布管理器插件', exact: true});
  const review = await preview(page, form);
  await expect(review).toContainText('minecraft-java');
  await expect(review).toContainText('1.0.0');
  await review.locator('summary').filter({hasText: 'SKILL.md'}).click();
  await expect(review.locator('pre').last()).toContainText('已审核的安装指引。');
  expect(await calls(page, 'prepare_manager_publication')).toEqual([{slug: 'minecraft-java', target: {kind: 'first'}, tags: []}]);
  expect(await calls(page, 'publish_manager_publication')).toHaveLength(0);
  await approve(review);
  await expect(review.getByText('已发布', {exact: true})).toBeVisible();
  await button(review, '关闭').last().click();
  await catalog.click();
  await expect(page.getByRole('button', {name: /查看并导入 Minecraft Java Edition/})).toBeVisible();
  expect(await calls(page, 'list_manager_plugin_games')).toHaveLength(2);
  expect(await calls(page, 'publish_manager_publication')).toHaveLength(1);
});

test('新版可手填目标 ID，发布已下架插件后显示实际可见状态', async ({page}) => {
  await page.evaluate(() => { window.__publicationFixture.outcome = 'hidden'; window.__publicationFixture.listDenied = true; });
  const form = await openForm(page);
  await form.getByRole('radio', {name: '发布新版本'}).check();
  const target = form.getByLabel('目标插件 ID');
  const prepare = button(form, '生成发布预览');
  await expect(prepare).toBeDisabled();
  for (const value of ['-1', '1e3', '1.5', '9007199254740992']) {
    await target.fill(value);
    await expect(prepare).toBeDisabled();
  }
  expect(await calls(page, 'list_my_manager_plugins')).toEqual([]);
  await target.fill('42');
  await form.getByLabel('版本标签').fill('新标签');
  const review = await preview(page, form);
  await expect(review.getByText('#42', {exact: true})).toBeVisible();
  await approve(review);
  await expect(review).toContainText('新版已提交，插件仍已下架');
  await expect(review.getByText('71', {exact: true})).toBeVisible();
  expect(await calls(page, 'list_my_manager_plugins')).toEqual([]);
  expect(await calls(page, 'prepare_manager_publication')).toEqual([{draftId: 'draft-fixture', target: {kind: 'release', pluginId: 42}, tags: ['新标签']}]);
});

test('可从本人插件选择目标，缺少读取权限时仍能手动输入', async ({page}) => {
  const form = await openForm(page);
  await form.getByRole('radio', {name: '发布新版本'}).check();
  await page.evaluate(() => { window.__publicationFixture.listDenied = true; });
  await button(form, '加载我的插件').click();
  await expect(form).toContainText('manager_plugins.read');
  await form.getByLabel('目标插件 ID').fill('13');
  await expect(button(form, '生成发布预览')).toBeEnabled();
  await page.evaluate(() => { window.__publicationFixture.listDenied = false; });
  await button(form, '加载我的插件').click();
  await form.getByRole('combobox', {name: '选择我的插件'}).selectOption('42');
  await expect(form.getByLabel('目标插件 ID')).toHaveValue('42');
  const review = await preview(page, form);
  await expect(review.getByText('#42', {exact: true})).toBeVisible();
  const firstDigest = await review.getByText(/^[a-f0-9]{64}$/).first().textContent();
  await acknowledge(review).check();
  await approval(review).fill('PUBLISH_PLUGIN');
  await button(review, '关闭').last().click();
  const other = await openForm(page);
  await other.getByRole('radio', {name: '发布新版本'}).check();
  await other.getByLabel('目标插件 ID').fill('13');
  await preview(page, other);
  await expect(review.getByText('#13', {exact: true})).toBeVisible();
  expect(await review.getByText(/^[a-f0-9]{64}$/).first().textContent()).not.toEqual(firstDigest);
  await expect(acknowledge(review)).not.toBeChecked();
  await expect(button(review, '确认发布')).toBeDisabled();
  expect(await calls(page, 'publish_manager_publication')).toEqual([]);
});

test('未知发布结果只能只读核对，未核实或权限不足时保持未知', async ({page}) => {
  await page.evaluate(() => { window.__publicationFixture.outcome = 'unknown'; window.__publicationFixture.reconciliation = 'unconfirmed'; });
  const review = await preview(page, await openForm(page));
  await approve(review);
  await expect(review.getByText('结果待核对', {exact: true})).toBeVisible();
  await expect(button(review, '确认发布')).toHaveCount(0);
  await button(review, '关闭').last().click();
  await openHistory(page, 'fixture-game');
  const reconcile = button(review, '核对远端状态');
  await expect(approval(review)).toHaveCount(0);
  await page.evaluate(() => { window.__publicationFixture.listDenied = true; });
  await reconcile.click();
  await expect(review).toContainText('manager_plugins.read');
  await page.evaluate(() => { window.__publicationFixture.listDenied = false; });
  await reconcile.click();
  await expect(review.getByRole('status')).toContainText('尚未核实');
  await expect(button(review, '确认发布')).toHaveCount(0);
  await page.evaluate(() => { window.__publicationFixture.reconciliation = 'verified'; });
  await reconcile.click();
  await expect(review.getByRole('status')).toContainText('已核实');
  await expect(review.getByText('已发布', {exact: true})).toBeVisible();
  await expect(reconcile).toHaveCount(0);
  expect(await calls(page, 'reconcile_manager_publication')).toEqual(Array(3).fill({id: 'manager_1'}));
  expect(await calls(page, 'publish_manager_publication')).toHaveLength(1);
  expect(await calls(page, 'publish_plugin')).toEqual([]);
});

for (const state of ['prepared', 'unknown', 'awaitingScan'] as const) {
  test(`旧版作品记录 ${state} 保留原始信息，仅允许适用的只读核对`, async ({page}) => {
    await page.evaluate(value => { window.__publicationFixture.legacyState = value; }, state);
    await openHistory(page, '旧版测试作品');
    const review = page.getByRole('dialog', {name: '旧版作品发布记录'});
    await expect(review).toContainText('legacy.zip');
    await expect(review).toContainText('旧版摘要');
    await expect(review).toContainText('旧版描述');
    await expect(button(review, '确认发布')).toHaveCount(0);
    await expect(acknowledge(review)).toHaveCount(0);
    if (state !== 'prepared') {
      await button(review, '核对远端状态').click();
      await expect(review.getByRole('status')).toContainText('已核实');
      await expect(review).toContainText(state === 'unknown' ? '作品 99' : '文件已就绪');
      await expect(button(review, '确认发布')).toHaveCount(0);
      await expect(acknowledge(review)).toHaveCount(0);
      expect(await calls(page, 'reconcile_plugin_publication')).toEqual([{id: 'legacy_fixture'}]);
    } else { await expect(button(review, '核对远端状态')).toHaveCount(0); }
    expect(await calls(page, 'publish_plugin')).toEqual([]);
    expect(await calls(page, 'publish_manager_publication')).toEqual([]);
    expect(await calls(page, 'reconcile_manager_publication')).toEqual([]);
  });
}
