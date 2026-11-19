import { expect, test, type Locator, type Page } from '@playwright/test';
import { installMockTauri } from './mock-tauri';

const project = {
  source_id: 'modrinth', project_id: 'Project1', game_id: 'minecraft' as string | null,
  name: 'Modrinth Version Fixture', summary: 'Fixture description', author: null,
  homepage_url: 'https://modrinth.com/mod/Project1', icon_url: null, downloads: 42,
};
const secondProject = { ...project, project_id: 'Project2', name: 'Other Version Fixture', game_id: null };
const scope = {
  source_id: project.source_id, project_id: project.project_id, game_id: 'minecraft',
  game_version: '1.21.1', loader: 'fabric',
};
const release = {
  version_id: 'Release1', version: '2.0.0', name: 'Stable release', release_channel: 'release',
  published_at: '2026-11-10T12:00:00Z', game_versions: ['1.21.1'], loaders: ['fabric'],
};
const beta = {
  ...release, version_id: 'BetaVer1', name: 'Beta build', release_channel: 'beta',
  game_versions: ['1.21.1', '1.21.2', '1.21.3', '1.21.4', '1.21.5'], loaders: ['fabric', 'quilt', 'neoforge', 'forge'],
};
const alpha = { ...release, version_id: 'AlphaVer', version: '1.9.0-alpha', name: 'AlphaBuild'.repeat(20), release_channel: 'alpha' };

type Controls = {
  list?: 'rejected' | 'unavailable' | 'empty' | 'limited';
  exact?: 'rejected' | 'missing' | 'wrong-source' | 'wrong-project' | 'wrong-version';
  downloadError?: string;
  holdList?: boolean;
  holdExact?: boolean;
};

async function openDiscover(page: Page, filtered = true) {
  await installMockTauri(page);
  await page.goto('/');
  await page.evaluate(({ project, secondProject, release, beta, alpha }) => {
    const target = window as any;
    const invoke = target.__TAURI_INTERNALS__.invoke;
    const status = { source_id: 'modrinth', state: 'available', message: 'Modrinth 可用' };
    const control: Controls = {};
    const pending = new Map<string, () => void>();
    let resolutionCount = 0;
    target.__AMM_VERSION_TEST__ = { control, pending };
    const hold = (key: string) => new Promise<void>(resolve => pending.set(key, resolve));
    target.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, any> = {}) => {
      if (!['list_mod_sources', 'search_mods', 'list_mod_versions', 'resolve_mod', 'download_mod'].includes(command)) {
        return invoke(command, args);
      }
      target.__AMM_TEST__.calls.push({ command, args });
      const request = args.request;
      switch (command) {
        case 'list_mod_sources': return [status];
        case 'search_mods': return { mods: [project, secondProject], sources: [status] };
        case 'list_mod_versions': {
          const outcome = control.list;
          if (control.holdList) { control.holdList = false; await hold('list'); }
          if (outcome === 'rejected') throw new Error('版本列表请求失败');
          if (outcome === 'unavailable') return { status: { ...status, state: 'unavailable', message: '来源暂不可用' }, versions: [], has_more: false };
          if (outcome === 'empty') return { status, versions: [], has_more: false };
          if (outcome === 'limited') return { status: { ...status, state: 'limited', message: '达到版本浏览上限' }, versions: [alpha], has_more: false };
          return { status, versions: request.offset === 0 ? [release, beta] : [alpha], has_more: request.offset === 0 };
        }
        case 'resolve_mod': {
          const mod = request.project_id === secondProject.project_id ? secondProject : project;
          const resolutionId = `${request.version_id ?? 'default'}-resolution-${++resolutionCount}`;
          const outcome = request.version_id === null ? undefined : control.exact;
          if (request.version_id !== null && control.holdExact) { control.holdExact = false; await hold('exact'); }
          if (outcome === 'rejected') throw new Error('精确版本请求失败');
          if (outcome === 'missing') return { status: { ...status, state: 'unavailable', message: '所选版本没有可下载文件' }, download: null, homepage_url: mod.homepage_url };
          const version = [release, beta, alpha].find(entry => entry.version_id === request.version_id) ?? release;
          const filename = `${version.version_id}-${request.loader ?? 'any'}.jar`;
          return { status, homepage_url: mod.homepage_url, dependencies: [], download: {
            source_id: outcome === 'wrong-source' ? 'thunderstore' : 'modrinth',
            project_id: outcome === 'wrong-project' ? secondProject.project_id : mod.project_id,
            version_id: outcome === 'wrong-version' ? release.version_id : version.version_id,
            name: version.name, version: version.version, filename, size: 650,
            resolution_id: resolutionId,
            hashes: { sha512: 'a'.repeat(128) }, homepage_url: mod.homepage_url, warnings: [],
          } };
        }
        case 'download_mod':
          if (control.downloadError) throw new Error(control.downloadError);
          return { id: 'test-artifact' };
      }
    };
  }, { project, secondProject, release, beta, alpha });
  await page.getByRole('button', { name: 'Mod 发现', exact: true }).click();
  if (filtered) {
    await page.getByLabel('游戏版本', { exact: true }).fill(' 1.21.1 ');
    await page.getByLabel('Mod 加载器', { exact: true }).fill(' fabric ');
  }
  await page.getByRole('button', { name: '搜索', exact: true }).click();
  await selectProject(page, project.name);
  const inspector = page.getByTestId('discover-page').locator('aside');
  await expect(downloadButton(inspector)).toBeEnabled();
  return inspector;
}

function selectProject(page: Page, name: string) {
  return page.getByRole('button').filter({ has: page.getByRole('heading', { name, exact: true }) }).click();
}

const browseButton = (inspector: Locator) => inspector.getByRole('button', { name: '查看来源版本', exact: true });
const versionPanel = (inspector: Locator) => inspector.getByRole('region', { name: '来源版本', exact: true });
const chooseButton = (inspector: Locator, version: typeof release) => versionPanel(inspector).getByRole('button', { name: `选择版本 ${version.version}（${version.version_id}）`, exact: true });
const downloadButton = (inspector: Locator) => inspector.getByRole('button', { name: '下载归档', exact: true });

async function calls(page: Page, ...commands: string[]) {
  return page.evaluate(commands => (window as any).__AMM_TEST__.calls.filter((call: { command: string }) => commands.includes(call.command)), commands);
}

async function controls(page: Page, updates: Controls) {
  await page.evaluate(updates => { Object.assign((window as any).__AMM_VERSION_TEST__.control, updates); }, updates);
}

async function releaseResponse(page: Page, key: 'list' | 'exact') {
  await page.evaluate(async key => {
    const pending = (window as any).__AMM_VERSION_TEST__.pending;
    if (!pending.has(key)) throw new Error(`No pending ${key} request`);
    pending.get(key)();
    pending.delete(key);
    // Let the released IPC promise reach React and commit before negative assertions.
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  }, key);
}

async function expectNoOverflow(page: Page, panel: Locator) {
  for (const theme of ['亮色', '暗色']) {
    const toggle = page.getByRole('button', { name: `切换${theme}主题`, exact: true });
    if (await toggle.isVisible()) await toggle.click();
    expect(await panel.evaluate(element => {
      const main = element.closest('main')!;
      return element.scrollWidth <= element.clientWidth + 1 && element.getBoundingClientRect().right <= innerWidth + 1 &&
        main.scrollWidth <= main.clientWidth + 1;
    }), `version chooser fits the viewport in ${theme} mode`).toBe(true);
  }
}

test('Modrinth browses pages without downloading and downloads only the latest exact choice', async ({ page }) => {
  const inspector = await openDiscover(page);
  expect(await calls(page, 'resolve_mod', 'list_mod_versions')).toEqual([
    { command: 'resolve_mod', args: { request: { ...scope, version_id: null } } },
  ]);
  await browseButton(inspector).click();
  const panel = versionPanel(inspector);
  const nextPage = panel.getByRole('button', { name: '下一页来源版本', exact: true });
  const prevPage = panel.getByRole('button', { name: '上一页来源版本', exact: true });
  await expect(panel.getByText('正式版', { exact: true })).toBeVisible();
  await expect(panel.getByText('Beta 测试版', { exact: true })).toBeVisible();
  await expect(panel.getByText(/^来源声明/)).toHaveCount(0);
  await expect(prevPage).toBeDisabled();

  await controls(page, { holdExact: true });
  await chooseButton(inspector, beta).click();
  await expect(inspector.getByRole('status').filter({ hasText: '正在解析所选版本' })).toBeVisible();
  await expect(downloadButton(inspector)).toHaveCount(0);
  await nextPage.click();
  await expect(panel.getByText('Alpha 预览版', { exact: true })).toBeVisible();
  await expect(panel.getByText(`版本 ID：${alpha.version_id}`, { exact: true })).toBeVisible();
  const published = panel.getByText(/^发布日期：/).locator('time');
  await expect(published).toHaveAttribute('datetime', alpha.published_at);
  await expect(published).not.toBeEmpty();
  await expect(nextPage).toBeDisabled();
  await expect(prevPage).toBeEnabled();

  await chooseButton(inspector, alpha).click();
  await expect(chooseButton(inspector, alpha)).toHaveAttribute('aria-pressed', 'true');
  await expect(downloadButton(inspector)).toBeEnabled();
  await releaseResponse(page, 'exact');
  await expect(inspector.getByText('AlphaVer-fabric.jar', { exact: true })).toBeVisible();
  await expectNoOverflow(page, panel);
  expect(await calls(page, 'list_mod_versions', 'resolve_mod')).toEqual([
    { command: 'resolve_mod', args: { request: { ...scope, version_id: null } } },
    { command: 'list_mod_versions', args: { request: { ...scope, limit: 20, offset: 0 } } },
    { command: 'resolve_mod', args: { request: { ...scope, version_id: beta.version_id } } },
    { command: 'list_mod_versions', args: { request: { ...scope, limit: 20, offset: 20 } } },
    { command: 'resolve_mod', args: { request: { ...scope, version_id: alpha.version_id } } },
  ]);
  expect(await calls(page, 'download_mod', 'plan_install', 'apply_plan')).toEqual([]);
  await downloadButton(inspector).click();
  expect(await calls(page, 'download_mod', 'plan_install', 'apply_plan')).toStrictEqual([
    { command: 'download_mod', args: { taskId: expect.any(String), resolutionId: 'AlphaVer-resolution-3' } },
  ]);
});

test('Modrinth requires a fresh exact version resolution after a failed download', async ({ page }) => {
  const inspector = await openDiscover(page);
  await browseButton(inspector).click();
  const choose = chooseButton(inspector, beta);
  await choose.click();
  await expect(inspector.getByText('BetaVer1-fabric.jar', { exact: true })).toBeVisible();
  await controls(page, { downloadError: '下载失败' });
  const download = downloadButton(inspector);
  await download.click();
  await expect(page.getByRole('alert')).toHaveText('下载失败');
  await expect(download).toBeDisabled();
  await expect(choose).toHaveAttribute('aria-pressed', 'true');

  await controls(page, { downloadError: undefined });
  await choose.click();
  await expect(download).toBeEnabled();
  await download.click();
  await expect(page.getByRole('status').filter({ hasText: '下载已完成，尚未安装' })).toBeVisible();
  const request = { ...scope, version_id: beta.version_id };
  expect(await calls(page, 'resolve_mod', 'download_mod', 'plan_install', 'apply_plan')).toStrictEqual([
    { command: 'resolve_mod', args: { request: { ...scope, version_id: null } } },
    { command: 'resolve_mod', args: { request } },
    { command: 'download_mod', args: { taskId: expect.any(String), resolutionId: 'BetaVer1-resolution-2' } },
    { command: 'resolve_mod', args: { request } },
    { command: 'download_mod', args: { taskId: expect.any(String), resolutionId: 'BetaVer1-resolution-3' } },
  ]);
});

test('unfiltered Modrinth candidates show bounded source-declared loaders and game versions', async ({ page }) => {
  const inspector = await openDiscover(page, false);
  await browseButton(inspector).click();
  const panel = versionPanel(inspector);
  const betaRow = panel.getByRole('listitem').filter({ hasText: beta.name });
  await expect(betaRow.getByText('来源声明加载器：fabric、quilt、neoforge +1', { exact: true })).toBeVisible();
  await expect(betaRow.getByText('来源声明游戏版本：1.21.1、1.21.2、1.21.3 +2', { exact: true })).toBeVisible();
  await expectNoOverflow(page, panel);
  expect(await calls(page, 'list_mod_versions')).toEqual([
    { command: 'list_mod_versions', args: { request: { ...scope, game_version: null, loader: null, limit: 20, offset: 0 } } },
  ]);
});

test('Modrinth version browsing retries the failed page and distinguishes empty and limited results', async ({ page }) => {
  const inspector = await openDiscover(page);
  const panel = versionPanel(inspector);
  await browseButton(inspector).click();
  await controls(page, { list: 'rejected' });
  await panel.getByRole('button', { name: '下一页来源版本', exact: true }).click();
  await expect(panel.getByRole('alert')).toHaveText('版本列表请求失败');
  const retry = panel.getByRole('button', { name: '重试读取来源版本', exact: true });
  await controls(page, { list: 'unavailable' });
  await retry.click();
  await expect(panel.getByRole('alert')).toHaveText('来源暂不可用');
  await controls(page, { list: 'limited' });
  await retry.click();
  await expect(panel.getByRole('status')).toHaveText('达到版本浏览上限');
  await expect(chooseButton(inspector, alpha)).toBeEnabled();
  await controls(page, { list: 'empty' });
  await panel.getByRole('button', { name: '上一页来源版本', exact: true }).click();
  await expect(panel.getByRole('status')).toHaveText('没有符合当前筛选条件的来源版本。');
  await expect(panel.getByRole('alert')).toHaveCount(0);
  expect(await calls(page, 'list_mod_versions')).toEqual([0, 20, 20, 20, 0].map(offset => ({
    command: 'list_mod_versions', args: { request: { ...scope, limit: 20, offset } },
  })));
  expect(await calls(page, 'resolve_mod', 'download_mod')).toEqual([
    { command: 'resolve_mod', args: { request: { ...scope, version_id: null } } },
  ]);
});

test('Modrinth exact errors and mismatched identities never fall back to the default archive', async ({ page }) => {
  const inspector = await openDiscover(page);
  await browseButton(inspector).click();
  const retry = inspector.getByRole('button', { name: '重试解析所选版本', exact: true });
  const failures = [
    ['wrong-source', '来源返回的下载与所选版本不一致。'],
    ['wrong-project', '来源返回的下载与所选版本不一致。'],
    ['wrong-version', '来源返回的下载与所选版本不一致。'],
    ['missing', '所选版本没有可下载文件'],
    ['rejected', '精确版本请求失败'],
  ] as const;
  for (const [index, [exact, message]] of failures.entries()) {
    await controls(page, { exact });
    await (index === 0 ? chooseButton(inspector, beta) : retry).click();
    await expect(inspector.getByRole('alert')).toHaveText(message);
    await expect(downloadButton(inspector)).toHaveCount(0);
  }
  await controls(page, { exact: undefined });
  await retry.click();
  await expect(downloadButton(inspector)).toBeEnabled();
  await expect(inspector.getByText('BetaVer1-fabric.jar', { exact: true })).toBeVisible();
  expect(await calls(page, 'resolve_mod', 'download_mod')).toEqual([
    { command: 'resolve_mod', args: { request: { ...scope, version_id: null } } },
    ...Array.from({ length: failures.length + 1 }, () => ({ command: 'resolve_mod', args: { request: { ...scope, version_id: beta.version_id } } })),
  ]);
});

test('changing the loader or game version invalidates results and late resolutions', async ({ page }) => {
  const inspector = await openDiscover(page);
  await browseButton(inspector).click();
  await chooseButton(inspector, beta).click();
  await expect(downloadButton(inspector)).toBeEnabled();

  await page.getByLabel('Mod 加载器', { exact: true }).fill('quilt');
  await expect(downloadButton(inspector)).toHaveCount(0);
  await expect(versionPanel(inspector)).toHaveCount(0);
  await inspector.getByRole('button', { name: '解析默认版本', exact: true }).click();
  await expect(inspector.getByText('Release1-quilt.jar', { exact: true })).toBeVisible();
  await browseButton(inspector).click();
  await controls(page, { holdExact: true });
  await chooseButton(inspector, beta).click();
  await expect(inspector.getByRole('status').filter({ hasText: '正在解析所选版本' })).toBeVisible();

  await page.getByLabel('游戏版本', { exact: true }).fill('1.21.4');
  await releaseResponse(page, 'exact');
  await expect(downloadButton(inspector)).toHaveCount(0);
  await expect(versionPanel(inspector)).toHaveCount(0);
  expect(await calls(page, 'resolve_mod')).toHaveLength(4);
  await browseButton(inspector).click();
  await chooseButton(inspector, beta).click();
  await expect(inspector.getByText('BetaVer1-quilt.jar', { exact: true })).toBeVisible();
  const newScope = { ...scope, game_version: '1.21.4', loader: 'quilt' };
  expect((await calls(page, 'list_mod_versions', 'resolve_mod')).slice(-2)).toEqual([
    { command: 'list_mod_versions', args: { request: { ...newScope, limit: 20, offset: 0 } } },
    { command: 'resolve_mod', args: { request: { ...newScope, version_id: beta.version_id } } },
  ]);
  expect(await calls(page, 'download_mod')).toEqual([]);
});

test('changing projects discards both pending version requests and uses the new provider game ID', async ({ page }) => {
  const inspector = await openDiscover(page);
  await browseButton(inspector).click();
  await controls(page, { holdExact: true, holdList: true });
  await chooseButton(inspector, beta).click();
  await versionPanel(inspector).getByRole('button', { name: '下一页来源版本', exact: true }).click();
  await expect(versionPanel(inspector).getByRole('status')).toHaveText('正在读取来源版本…');
  await selectProject(page, secondProject.name);
  await expect(inspector.getByRole('heading', { name: secondProject.name, exact: true })).toBeVisible();
  await expect(inspector.getByText('Release1-fabric.jar', { exact: true })).toBeVisible();
  await releaseResponse(page, 'list');
  await releaseResponse(page, 'exact');
  await expect(versionPanel(inspector)).toHaveCount(0);
  await expect(inspector.getByText('Release1-fabric.jar', { exact: true })).toBeVisible();
  await browseButton(inspector).click();
  await chooseButton(inspector, beta).click();
  await expect(inspector.getByText('BetaVer1-fabric.jar', { exact: true })).toBeVisible();
  const secondScope = { ...scope, project_id: secondProject.project_id, game_id: null };
  expect((await calls(page, 'list_mod_versions', 'resolve_mod')).slice(-3)).toEqual([
    { command: 'resolve_mod', args: { request: { ...secondScope, version_id: null } } },
    { command: 'list_mod_versions', args: { request: { ...secondScope, limit: 20, offset: 0 } } },
    { command: 'resolve_mod', args: { request: { ...secondScope, version_id: beta.version_id } } },
  ]);
  expect(await calls(page, 'download_mod')).toEqual([]);
});
