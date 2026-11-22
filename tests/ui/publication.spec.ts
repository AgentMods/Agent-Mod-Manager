import { createHash } from 'node:crypto';
import { expect, test, type Locator, type Page } from '@playwright/test';
import { installMockTauri } from './mock-tauri';
import { attachPublicationHandlers, installPublicationHandlers } from './publication-fixture';

const prepare = 'prepare_auto_manager_publication';
const publish = 'publish_manager_publication';
const button = (scope: Page | Locator, name: string) => scope.getByRole('button', {name, exact: true});
const dialog = (page: Page) => page.getByRole('dialog', {name: '发布插件', exact: true});
const calls = (page: Page, command: string) => page.evaluate(name => (window as any).__AMM_TEST__.calls
  .filter((call: {command: string}) => call.command === name).map((call: {args: unknown}) => call.args), command);

async function openDraft(page: Page) {
  await page.getByRole('tab', {name: /^草稿/}).click();
  await button(page, '发布插件 fixture-game').click();
  return dialog(page);
}
async function openInstalled(page: Page) {
  await page.getByRole('tab', {name: '已安装', exact: true}).click();
  await page.getByRole('row').filter({hasText: 'Minecraft Java Edition'}).getByRole('button').click();
  await button(page, '发布插件').click();
  return dialog(page);
}
async function openHistory(page: Page, name: string) {
  await page.getByRole('tab', {name: '发布记录', exact: true}).click();
  await button(page, `查看发布 ${name}`).click();
  return dialog(page);
}
async function showFiles(review: Locator) {
  await review.locator('summary').filter({hasText: '查看发布文件'}).click();
}
async function expectAutomaticReview(review: Locator) {
  for (const role of ['textbox', 'combobox', 'checkbox', 'radio'] as const) await expect(review.getByRole(role)).toHaveCount(0);
  await expect(button(review, '生成发布预览')).toHaveCount(0);
}

test.beforeEach(async ({page}) => {
  await installMockTauri(page, {withExternalAdapter: true});
  await page.addInitScript(installPublicationHandlers);
  await page.goto('/');
  await page.evaluate(attachPublicationHandlers);
  await button(page, '游戏库').click();
  await button(page, '插件中心').click();
});

test('草稿首次发布一次生成，展示完整的公开内容，第二次点击提交审批摘要', async ({page}, info) => {
  const review = await openDraft(page);
  await expect(review).toHaveAccessibleDescription(/首次发布\s+1\.1\.0/);
  await expect(review.getByRole('heading', {name: '测试游戏 · 游戏适配包'})).toBeVisible();
  await expect(review.getByText('按已验证的规则识别并安装模组。', {exact: true})).toBeVisible();
  await expect(review.getByText(/自动定位模组目录。/)).toHaveText('自动定位模组目录。\n安装前请核对 <em>文件变更</em>。');
  await expect(review.getByRole('list', {name: '标签'}).getByRole('listitem')).toHaveText(['游戏适配', '中文']);
  const cover = review.getByRole('img', {name: /封面/});
  await expect(cover).toHaveAttribute('src', /^data:image\/png;base64,iVBORw0KGgo/);
  await expect.poll(() => cover.evaluate(el => (el as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
  await expect(review.locator('em')).toHaveCount(0);
  await expectAutomaticReview(review);
  for (const theme of ['light', 'dark']) {
    await page.locator('html').evaluate((el, value) => el.setAttribute('data-theme', value), theme);
    await page.screenshot({path: info.outputPath(`review-${theme}.png`), animations: 'disabled'});
    expect(await review.evaluate(el => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);
    const bounds = (await review.boundingBox())!;
    expect(Math.abs(bounds.x - (page.viewportSize()!.width - bounds.width) / 2)).toBeLessThan(2);
  }
  await showFiles(review);
  await expect(review.getByText('草稿 · draft-fixture', {exact: true})).toBeVisible();
  await expect(review.getByText('新建管理器插件', {exact: true})).toBeVisible();
  await expect(review.getByText('https://www.glossmod.com/api/v1/mcp', {exact: true})).toBeVisible();
  const digest = await review.getByText(/^[a-f0-9]{64}$/).textContent();
  for (const name of ['adapter.json', 'SKILL.md']) await review.locator('summary').filter({hasText: name}).click();
  const contents = await review.locator('pre').allTextContents();
  expect(contents).toEqual([
    '{\r\n  "slug": "fixture-game", "name": "测试游戏", "version": "1.1.0"\r\n}\r\n',
    '---\nname: fixture-game\ndescription: 公开测试\n---\nReview <em>before</em> installing.\n',
  ]);
  for (const raw of contents) await expect(review.getByText(`SHA-256 ${createHash('sha256').update(raw).digest('hex')}`, {exact: true})).toBeVisible();
  await expect(review.locator('pre em')).toHaveCount(0);
  expect(await calls(page, prepare)).toEqual([{draftId: 'draft-fixture'}]);
  expect(await calls(page, publish)).toEqual([]);
  await button(review, '发布').click();
  await expect(review.getByText('已发布', {exact: true})).toBeVisible();
  await expect(button(review, '发布')).toHaveCount(0);
  expect(await calls(page, publish)).toEqual([{id: 'manager_1', digest, confirmation: 'PUBLISH_PLUGIN'}]);
});

for (const scenario of ['直接发布', '管理器发布核对', '旧版作品核对'] as const) {
  test(`Agent 草稿${scenario}完成后刷新草稿和回收站`, async ({page}) => {
    const legacy = scenario === '旧版作品核对';
    const reconcile = scenario !== '直接发布';
    // Prime a fresh Trash cache; revisiting without invalidation would reuse it.
    await page.clock.setFixedTime(new Date());
    await button(page, '回收站').click();
    await expect(page.getByText('Example.jar', {exact: true})).toBeVisible();
    await button(page, '插件中心').click();
    await expect(page.getByRole('tab', {name: '草稿 1', exact: true})).toBeVisible();
    await page.evaluate(({legacy, reconcile}) => {
      const fixture = window.__publicationFixture;
      fixture.agentDraftId = 'draft-fixture';
      if (reconcile) { fixture.outcome = 'unknown'; fixture.reconciliation = 'unconfirmed'; }
      if (legacy) fixture.legacyState = 'unknown';
    }, {legacy, reconcile});
    if (legacy) await openHistory(page, '旧版测试作品');
    const review = legacy ? page.getByRole('dialog', {name: '旧版作品发布记录'}) : await openDraft(page);
    const draftReads = (await calls(page, 'list_plugin_drafts')).length;
    if (!legacy) await button(review, '发布').click();
    if (reconcile) {
      await expect(review.getByText('结果待核对', {exact: true})).toBeVisible();
      await button(review, '核对远端状态').click();
      await expect(review.getByRole('status')).toContainText('尚未核实');
      expect(await calls(page, 'list_plugin_drafts')).toHaveLength(draftReads);
      await page.evaluate(() => { window.__publicationFixture.reconciliation = 'verified'; });
      await button(review, '核对远端状态').click();
    }
    await expect(review.getByText('已发布', {exact: true})).toBeVisible();
    await button(review, '关闭').last().click();
    const drafts = page.getByRole('tab', {name: '草稿 0', exact: true});
    await expect(drafts).toBeVisible();
    await drafts.click();
    await expect(page.getByRole('row').filter({hasText: 'txn-draft-fixture'})).toHaveCount(0);
    await button(page, '回收站').click();
    await expect(page.getByText('自动清理已用完的 Agent 插件草稿 draft-fixture', {exact: true})).toBeVisible();
  });
}

test('已安装适配包自动匹配本人插件，审核实际版本并更新云端列表', async ({page}) => {
  await page.clock.setFixedTime(new Date());
  await page.evaluate(() => { window.__publicationFixture.exposeCatalog = true; window.__publicationFixture.matched = true; });
  const catalog = page.getByRole('tab', {name: '云端插件', exact: true});
  await catalog.click();
  await expect(page.getByText('暂无已发布插件', {exact: true})).toBeVisible();
  expect(await calls(page, 'list_manager_plugin_games')).toHaveLength(1);
  const review = await openInstalled(page);
  await expect(review).toHaveAccessibleDescription(/发布新版本\s+1\.3\.0 → 1\.3\.1/);
  await expect(review.getByRole('heading', {name: 'Minecraft Java · 游戏适配包'})).toBeVisible();
  await showFiles(review);
  await expect(review.getByText('已安装适配包 · minecraft-java')).toBeVisible();
  await expect(review.getByText('#42', {exact: true})).toBeVisible();
  await expect(review.getByText('1.0.0', {exact: true})).toBeVisible();
  await review.locator('summary').filter({hasText: 'SKILL.md'}).click();
  await expect(review.locator('pre').last()).toContainText('已审核的安装指引。');
  expect(await calls(page, prepare)).toEqual([{slug: 'minecraft-java'}]);
  await button(review, '发布').click();
  await expect(review.getByText('已发布', {exact: true})).toBeVisible();
  await button(review, '关闭').last().click();
  await catalog.click();
  await expect(page.getByRole('button', {name: /查看并导入 Minecraft Java Edition/})).toBeVisible();
  await expect(page.getByText('1.3.1', {exact: false}).first()).toBeVisible();
  expect(await calls(page, 'list_manager_plugin_games')).toHaveLength(2);
  expect(await calls(page, publish)).toHaveLength(1);
});

test('生成中可退出，迟到的草稿不能覆盖后开的已安装适配包', async ({page}) => {
  await page.evaluate(() => window.__publicationFixture.holdNext('prepare_auto_manager_publication'));
  const review = await openDraft(page);
  await expect(review.getByRole('status')).toContainText(/账号.*生成/);
  await expect(button(review, '发布')).toHaveCount(0);
  await expect.poll(() => calls(page, prepare)).toEqual([{draftId: 'draft-fixture'}]);
  await page.keyboard.press('Escape');
  await expect(review).toHaveCount(0);
  await openInstalled(page);
  await expect(review.getByRole('heading', {name: 'Minecraft Java · 游戏适配包'})).toBeVisible();
  await page.evaluate(() => window.__publicationFixture.resume());
  await expect.poll(() => page.evaluate(async () => (await window.__publicationFixture.invoke('list_manager_publications', {}) as unknown[]).length)).toBe(2);
  await expect(review.getByRole('heading', {name: 'Minecraft Java · 游戏适配包'})).toBeVisible();
  await button(review, '取消').click();
  await openDraft(page);
  await expect(review.getByRole('heading', {name: '测试游戏 · 游戏适配包'})).toBeVisible();
  expect(await calls(page, prepare)).toEqual([{draftId: 'draft-fixture'}, {slug: 'minecraft-java'}, {draftId: 'draft-fixture'}]);
  expect(await calls(page, publish)).toEqual([]);
});

test('发布中阻止重复点击，同一份云端更新只提交一次', async ({page}) => {
  await page.evaluate(() => { window.__publicationFixture.holdNext('publish_manager_publication'); window.__publicationFixture.matched = true; window.__publicationFixture.outcome = 'hidden'; });
  const review = await openDraft(page);
  await button(review, '发布').evaluate(el => { (el as HTMLButtonElement).click(); (el as HTMLButtonElement).click(); });
  await expect(button(review, '发布')).toBeDisabled();
  expect(await calls(page, publish)).toHaveLength(1);
  await page.evaluate(() => window.__publicationFixture.resume());
  await expect(review.getByText('新版已提交，插件仍已下架')).toBeVisible();
  await expect(review.getByText('测试下架原因')).toBeVisible();
  await expect(review.getByText('71', {exact: true})).toBeVisible();
  expect(await calls(page, publish)).toHaveLength(1);
});

for (const failure of ['listDenied', 'ambiguous'] as const) {
  test(`匹配失败 ${failure} 时保留错误，仅显式重试后才审核`, async ({page}) => {
    await page.evaluate(key => { window.__publicationFixture[key] = true; }, failure);
    const review = await openDraft(page);
    await expect(review.getByRole('alert')).toContainText(failure === 'listDenied' ? 'manager_plugins.read' : '匹配到多个');
    await expect(button(review, '发布')).toHaveCount(0);
    await expectAutomaticReview(review);
    expect(await calls(page, prepare)).toEqual([{draftId: 'draft-fixture'}]);
    for (const command of [publish, 'prepare_manager_publication', 'prepare_plugin_publication', 'list_my_manager_plugins']) expect(await calls(page, command)).toEqual([]);
    await page.evaluate(key => { window.__publicationFixture[key] = false; window.__publicationFixture.matched = true; }, failure);
    await button(review, '重试').click();
    await expect(review).toHaveAccessibleDescription(/1\.3\.0 → 1\.3\.1/);
    await expect(button(review, '发布')).toBeEnabled();
    expect(await calls(page, prepare)).toHaveLength(2);
    expect(await calls(page, publish)).toEqual([]);
  });
}

test('不加载任意远程封面', async ({page}) => {
  let requests = 0;
  await page.route('https://covers.invalid/**', route => { requests++; return route.abort(); });
  await page.evaluate(() => { window.__publicationFixture.invalidCover = true; });
  const review = await openDraft(page);
  await expect(review.getByText('封面无法显示')).toBeVisible();
  await expect(review.locator('img')).toHaveCount(0);
  expect(requests).toBe(0);
});

test('历史中的自动生成预览可继续发布，未知结果只读核对', async ({page}) => {
  await page.evaluate(() => { window.__publicationFixture.outcome = 'unknown'; window.__publicationFixture.reconciliation = 'unconfirmed'; });
  const review = await openDraft(page);
  await expect(button(review, '发布')).toBeEnabled();
  await button(review, '取消').click();
  await openHistory(page, 'fixture-game');
  await button(review, '发布').click();
  await expect(review.getByText('结果待核对', {exact: true})).toBeVisible();
  await expect(button(review, '发布')).toHaveCount(0);
  const reconcile = button(review, '核对远端状态');
  await page.evaluate(() => { window.__publicationFixture.listDenied = true; });
  await reconcile.click();
  await expect(review).toContainText('manager_plugins.read');
  await page.evaluate(() => { window.__publicationFixture.listDenied = false; });
  await reconcile.click();
  await expect(review.getByRole('status')).toContainText('尚未核实');
  await page.evaluate(() => { window.__publicationFixture.reconciliation = 'verified'; });
  await reconcile.click();
  await expect(review.getByRole('status')).toContainText('已核实');
  await expect(review.getByText('已发布', {exact: true})).toBeVisible();
  await expect(reconcile).toHaveCount(0);
  expect(await calls(page, 'reconcile_manager_publication')).toEqual(Array(3).fill({id: 'manager_1'}));
  expect(await calls(page, prepare)).toHaveLength(1);
  expect(await calls(page, publish)).toHaveLength(1);
});

test('历史分页读取缺少 listing 的旧管理器记录，原待审核记录保持只读', async ({page}) => {
  await page.evaluate(() => window.__publicationFixture.seedManagerV1(51, 'intended'));
  await page.getByRole('tab', {name: '发布记录', exact: true}).click();
  await expect(button(page, '查看发布 v1-50')).toHaveCount(0);
  await button(page, '加载更早的发布记录').click();
  await button(page, '查看发布 v1-50').click();
  const review = dialog(page);
  await expect(review.getByText('旧标签', {exact: true})).toBeVisible();
  await expect(review.getByText('历史预览', {exact: true})).toBeVisible();
  await expect(button(review, '发布')).toHaveCount(0);
  await expect(button(review, '核对远端状态')).toHaveCount(0);
  expect(await calls(page, 'list_manager_publications')).toEqual([{before: null}, {before: 'v1-49'}]);
  expect(await calls(page, prepare)).toEqual([]);
  expect(await calls(page, publish)).toEqual([]);
});

test('旧管理器记录仍能核对未知远端状态', async ({page}) => {
  await page.evaluate(() => window.__publicationFixture.seedManagerV1(1, 'unknown'));
  const review = await openHistory(page, 'v1-0');
  await button(review, '核对远端状态').click();
  await expect(review.getByRole('status')).toContainText('已核实');
  await expect(button(review, '发布')).toHaveCount(0);
  expect(await calls(page, 'reconcile_manager_publication')).toEqual([{id: 'v1-0'}]);
  expect(await calls(page, publish)).toEqual([]);
});

for (const state of ['prepared', 'unknown', 'awaitingScan'] as const) {
  test(`旧版作品记录 ${state} 保留原始信息，仅允许适用的只读核对`, async ({page}) => {
    await page.evaluate(value => { window.__publicationFixture.legacyState = value; }, state);
    await openHistory(page, '旧版测试作品');
    const review = page.getByRole('dialog', {name: '旧版作品发布记录'});
    await expect(review).toContainText('legacy.zip');
    await expect(review).toContainText('旧版摘要');
    await expect(review).toContainText('旧版描述');
    await expect(button(review, '发布')).toHaveCount(0);
    await expect(review.getByRole('checkbox')).toHaveCount(0);
    if (state !== 'prepared') {
      await button(review, '核对远端状态').click();
      await expect(review.getByRole('status')).toContainText('已核实');
      await expect(review).toContainText(state === 'unknown' ? '作品 99' : '文件已就绪');
      await expect(button(review, '发布')).toHaveCount(0);
      await expect(review.getByRole('checkbox')).toHaveCount(0);
      expect(await calls(page, 'reconcile_plugin_publication')).toEqual([{id: 'legacy_fixture'}]);
    } else { await expect(button(review, '核对远端状态')).toHaveCount(0); }
    expect(await calls(page, 'publish_plugin')).toEqual([]);
    expect(await calls(page, 'publish_manager_publication')).toEqual([]);
    expect(await calls(page, 'reconcile_manager_publication')).toEqual([]);
  });
}
