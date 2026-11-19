import { expect, test, type Locator, type Page } from '@playwright/test';
import { installMockTauri } from './mock-tauri';

const exactRequest = { source_id: 'modrinth', project_id: 'Fixture1', version_id: 'Release1' };
const replacementId = 'old-install';
const firstGame = 'game-fixture';
const otherGame = 'other-game';
const fileName = `${'longExternalDependency'.repeat(8)}.jar`;

function declaration(dependency_type: string, project_id: string | null, version_id: string | null = null, file_name: string | null = null) {
  return { dependency_type, project_id, version_id, file_name };
}

function match(installId: string, projectId: string, versionId: string, status = 'enabled') {
  return { installId, projectId, versionId, status };
}

function check(dependency: ReturnType<typeof declaration>, status: string, matches: ReturnType<typeof match>[] = [], reason: string | null = null, matchCount = matches.length) {
  return { dependency, status, reason, matches, matchCount };
}

const findings = [
  check(declaration('required', 'ExactDep', 'PinVers1'), 'recorded_exact', [match('deployed', 'ExactDep', 'PinVers1')]),
  check(declaration('optional', 'Optional'), 'recorded_project', [match('optional', 'Optional', 'AnyVers1')]),
  check(declaration('required', 'Missing1'), 'not_recorded'),
  check(declaration('required', 'Mismatch', 'Pinned01'), 'different_version', [match('older', 'Mismatch', 'Older001')]),
  check(declaration('incompatible', 'Conflict', 'ConfVer1'), 'recorded_exact', [match('conflict', 'Conflict', 'ConfVer1')]),
  check(declaration('incompatible', 'AmbigDep'), 'unconfirmed', Array.from({ length: 4 }, (_, i) => match(`ambiguous-${i}`, 'AmbigDep', `Version${i}`)), 'ambiguous_records', 7),
  check(declaration('incompatible', 'ExpectP1', 'SharedV1'), 'unconfirmed', [match('inconsistent', 'WrongPrj', 'SharedV1')], 'inconsistent_ids'),
  check(declaration('required', 'PartialP'), 'unconfirmed', [match('partial', 'PartialP', 'AnyVers2', 'partially_restored')], 'partial_install'),
  check(declaration('optional', null, null, fileName), 'unconfirmed', [], 'file_reference'),
  check(declaration('embedded', 'Bundled1'), 'embedded'),
];

type ReviewControl = {
  checks: typeof findings | null;
  downloadOverride: Partial<typeof exactRequest>;
  reportOverride: Partial<{ gameId: string; excludedInstallId: string | null }>;
  error: string | null;
  holdNext: 'resolve_mod' | 'check_modrinth_dependencies' | null;
};

async function setup(page: Page, colorScheme: 'light' | 'dark' = 'light') {
  await page.emulateMedia({ colorScheme });
  await installMockTauri(page);
  await page.addInitScript(({ exactRequest, replacementId, firstGame, otherGame, findings }) => {
    const target = window as any;
    function configure(mock: any) {
      const transport = target.__TAURI_INTERNALS__;
      const invoke = transport.invoke;
      mock.seedDownload = true;
      const controls: ReviewControl & { release?: () => void } = mock.dependencyReview = {
        checks: findings, downloadOverride: {}, reportOverride: {}, error: null, holdNext: null,
      };
      transport.invoke = async (command: string, args: Record<string, any> = {}) => {
        if (command === 'resolve_mod' || command === 'check_modrinth_dependencies') {
          mock.calls.push({ command, args });
          const isResolve = command === 'resolve_mod';
          const checks = controls.checks;
          const error = isResolve ? null : controls.error;
          const response = isResolve ? {
            status: { source_id: 'modrinth', state: 'available', message: '' }, homepage_url: null,
            download: {
              ...args.request, ...controls.downloadOverride, resolution_id: 'dependency-review-resolution',
              name: 'Downloaded Fixture', version: '1.0.0', filename: 'fixture.jar', size: 650,
              hashes: { sha512: 'a'.repeat(128) }, homepage_url: null, warnings: [],
            },
            dependencies: checks?.map(entry => entry.dependency) ?? null,
          } : {
            gameId: args.request.gameId, excludedInstallId: checks?.length ? args.request.replaceInstallId : null,
            checks, ...controls.reportOverride,
          };
          if (controls.holdNext === command) {
            controls.holdNext = null;
            await new Promise<void>(resolve => { controls.release = resolve; });
          }
          if (error) throw new Error(error);
          return response;
        }
        const result = await invoke(command, args);
        if (command === 'list_games') return [...result, { ...result[0], id: otherGame, name: '其他游戏', rootPath: '/sandbox/other' }];
        if (command === 'list_downloads') {
          const [record] = result;
          const download = { ...record, projectId: exactRequest.project_id, versionId: exactRequest.version_id };
          return [download,
            { ...download, artifact: { ...download.artifact, id: 'second-artifact' }, name: 'Second Download', projectId: 'Fixture2', versionId: 'Release2' },
            { ...download, artifact: { ...download.artifact, id: 'other-provider-artifact' }, name: 'Other Provider', sourceId: 'thunderstore' },
          ];
        }
        if (command === 'list_installs') return args.gameId !== firstGame ? [] : [{
          ...result[0], id: replacementId, name: 'Old Fixture', metadata: { modType: 'java-mod' },
          downloadOrigin: { sourceId: 'modrinth', projectId: exactRequest.project_id, versionId: 'OldVers1' },
        }];
        return result;
      };
    }

    // Playwright can run the two init scripts in either order. Configure before the app caches list_games.
    if (target.__AMM_TEST__) configure(target.__AMM_TEST__);
    else Object.defineProperty(target, '__AMM_TEST__', {
      configurable: true,
      set(mock) {
        Object.defineProperty(target, '__AMM_TEST__', { value: mock, configurable: true, writable: true });
        configure(mock);
      },
    });
  }, { exactRequest, replacementId, firstGame, otherGame, findings });
  await page.goto('/');
  await page.getByRole('button', { name: 'Mod 发现', exact: true }).click();
}

async function openDialog(page: Page, name = 'Downloaded Fixture') {
  await page.locator('.download-row').filter({ hasText: name }).getByRole('button', { name: '安装预览', exact: true }).click();
  return page.getByRole('dialog', { name: '下载归档安装预览', exact: true });
}

const gameChoice = (dialog: Locator) => dialog.getByLabel('目标游戏', { exact: true });
const replacementChoice = (dialog: Locator) => dialog.getByLabel('要替换的已装版本', { exact: true });
const checkButton = (dialog: Locator) => dialog.getByRole('button', { name: '核对已装依赖', exact: true });
const retryButton = (dialog: Locator) => dialog.getByRole('button', { name: '重试核对已装依赖', exact: true });
const preview = (dialog: Locator) => dialog.getByRole('button', { name: '生成变更预览', exact: true });
const review = (dialog: Locator) => dialog.getByRole('region', { name: '已装依赖记录核对' });
const results = (dialog: Locator) => review(dialog).getByRole('list', { name: '直接依赖核对结果' });

async function calls(page: Page, ...commands: string[]) {
  return page.evaluate(commands => (window as any).__AMM_TEST__?.calls.filter((call: { command: string }) => commands.includes(call.command)), commands);
}

async function updateMock(page: Page, options: Partial<ReviewControl>) {
  await page.evaluate(options => { Object.assign((window as any).__AMM_TEST__.dependencyReview, options); }, options);
}

async function waitForHeldResponse(page: Page) {
  await expect.poll(() => page.evaluate(() => typeof (window as any).__AMM_TEST__.dependencyReview.release)).toBe('function');
}

async function releaseHeldResponse(page: Page) {
  await page.evaluate(async () => {
    const controls = (window as any).__AMM_TEST__.dependencyReview;
    controls.release();
    delete controls.release;
    // Let IPC continuations and the next React paint run before asserting no update.
    await new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
  });
}

test.afterEach(async ({ page }) => {
  expect(await calls(page, 'download_mod', 'plan_install', 'apply_plan')).toEqual([]);
});

test('checks the exact Modrinth version only on request and reports record evidence without blocking preview', async ({ page }, info) => {
  await setup(page, info.project.name === 'desktop-1280' ? 'light' : 'dark');
  const dialog = await openDialog(page);
  await expect(checkButton(dialog)).toHaveCount(0);
  await gameChoice(dialog).selectOption(otherGame);
  await dialog.getByLabel('适配包 Mod 类型', { exact: true }).selectOption('java-mod');
  await expect(preview(dialog)).toBeEnabled();
  expect(await calls(page, 'resolve_mod', 'check_modrinth_dependencies')).toEqual([]);
  await expect(checkButton(dialog)).toHaveAccessibleDescription(/Modrinth 安装记录.*其他来源或本地安装.*文件是否存在、校验和或加载器兼容性.*不阻止安装/);
  await checkButton(dialog).focus();
  await page.keyboard.press('Enter');

  await expect(review(dialog).getByRole('status')).toHaveText(`已核对 ${findings.length} 条直接依赖声明。`);
  expect(await calls(page, 'resolve_mod', 'check_modrinth_dependencies')).toEqual([
    { command: 'resolve_mod', args: { request: exactRequest } },
    { command: 'check_modrinth_dependencies', args: { request: { gameId: otherGame, dependencies: findings.map(entry => entry.dependency), replaceInstallId: null } } },
  ]);
  for (const label of ['已记录确切版本', '已记录项目', '来源声明已内嵌', '部分恢复', '仅有文件名，无法比对安装记录', fileName]) {
    await expect(results(dialog)).toContainText(label);
  }
  for (const warning of ['缺少必需依赖的安装记录', '必需依赖的记录版本不符', '已记录不兼容版本']) {
    await expect(results(dialog).getByText(warning, { exact: true })).toHaveClass(/\bwarning\b/);
  }
  for (const reason of ['有多条相关安装记录', '相关记录的项目 ID 与版本 ID 不一致']) {
    const row = results(dialog).getByRole('listitem').filter({ hasText: reason });
    await expect(row.getByText('无法确认是否冲突', { exact: true })).toHaveClass(/\bwarning\b/);
  }
  await expect(results(dialog)).toContainText('共 7 条相关记录（显示前 4 条）');
  await expect(results(dialog)).toContainText('记录 deployed · 项目 ExactDep · 版本 PinVers1 · 事务已部署');
  await expect(preview(dialog)).toBeEnabled();
  expect(await dialog.locator('[data-slot="modal-body"]').evaluate(body => body.scrollWidth <= body.clientWidth + 1)).toBe(true);
});

test('distinguishes unknown and empty declarations without claiming to exclude a replacement', async ({ page }) => {
  await setup(page);
  const dialog = await openDialog(page);
  await gameChoice(dialog).selectOption(firstGame);
  await replacementChoice(dialog).selectOption(replacementId);
  for (const [checks, message] of [[null, '该版本的直接依赖声明未知。'], [[], '该版本未声明直接依赖。']] as const) {
    await updateMock(page, { checks: checks === null ? null : [] });
    await checkButton(dialog).click();
    await expect(review(dialog).getByRole('status')).toHaveText(message);
    await expect(results(dialog)).toHaveCount(0);
    await expect(review(dialog)).not.toContainText('本次核对已排除');
  }
  expect(await calls(page, 'check_modrinth_dependencies')).toEqual([null, []].map(dependencies => ({
    command: 'check_modrinth_dependencies', args: { request: { gameId: firstGame, dependencies, replaceInstallId: replacementId } },
  })));
});

test('does not offer the record review for downloads from another provider', async ({ page }) => {
  await setup(page);
  const dialog = await openDialog(page, 'Other Provider');
  await gameChoice(dialog).selectOption(otherGame);
  await dialog.getByLabel('适配包 Mod 类型', { exact: true }).selectOption('java-mod');
  await expect(preview(dialog)).toBeEnabled();
  await expect(review(dialog)).toHaveCount(0);
  expect(await calls(page, 'resolve_mod', 'check_modrinth_dependencies')).toEqual([]);
});

test('rejects every mismatched provider ID and allows retry after a local error or mismatched report', async ({ page }) => {
  await setup(page);
  const dialog = await openDialog(page);
  await gameChoice(dialog).selectOption(otherGame);
  await dialog.getByLabel('适配包 Mod 类型', { exact: true }).selectOption('java-mod');
  for (const [index, downloadOverride] of [{ source_id: 'thunderstore' }, { project_id: 'WrongPrj' }, { version_id: 'WrongVer' }].entries()) {
    await updateMock(page, { downloadOverride });
    await (index === 0 ? checkButton(dialog) : retryButton(dialog)).click();
    await expect(review(dialog).getByRole('alert')).toHaveText('来源返回的下载与所选归档不一致。');
    await expect(results(dialog)).toHaveCount(0);
  }
  expect(await calls(page, 'check_modrinth_dependencies')).toEqual([]);

  await updateMock(page, { downloadOverride: {}, error: '无法读取已装依赖记录' });
  await retryButton(dialog).click();
  await expect(review(dialog).getByRole('alert')).toHaveText('无法读取已装依赖记录');
  await expect(preview(dialog)).toBeEnabled();
  for (const reportOverride of [{ gameId: firstGame }, { excludedInstallId: replacementId }]) {
    await updateMock(page, { error: null, reportOverride });
    await retryButton(dialog).click();
    await expect(review(dialog).getByRole('alert')).toHaveText('核对结果与所选游戏或待替换记录不一致。');
    await expect(results(dialog)).toHaveCount(0);
  }
  await updateMock(page, { reportOverride: {} });
  await retryButton(dialog).click();
  await expect(results(dialog)).toBeVisible();
  await expect(review(dialog).getByRole('alert')).toHaveCount(0);
});

test('a delayed resolution cannot start a local check after the game changes, even when switched back', async ({ page }) => {
  await setup(page);
  const dialog = await openDialog(page);
  await gameChoice(dialog).selectOption(otherGame);
  await dialog.getByLabel('适配包 Mod 类型', { exact: true }).selectOption('java-mod');
  await updateMock(page, { holdNext: 'resolve_mod' });
  await checkButton(dialog).click();
  await waitForHeldResponse(page);
  await expect(review(dialog).getByRole('status')).toHaveText('正在核对已装依赖记录…');
  await expect(checkButton(dialog)).toBeDisabled();
  await expect(preview(dialog)).toBeEnabled();
  await gameChoice(dialog).selectOption(firstGame);
  await gameChoice(dialog).selectOption(otherGame);
  await releaseHeldResponse(page);
  await expect(results(dialog)).toHaveCount(0);
  expect(await calls(page, 'check_modrinth_dependencies')).toEqual([]);
  await checkButton(dialog).click();
  await expect(results(dialog)).toBeVisible();
});

test('replacement selection discards an in-flight report and reruns with only the selected exclusion', async ({ page }) => {
  await setup(page);
  const dialog = await openDialog(page);
  await gameChoice(dialog).selectOption(firstGame);
  await updateMock(page, { holdNext: 'check_modrinth_dependencies', checks: [] });
  await checkButton(dialog).click();
  await waitForHeldResponse(page);

  await replacementChoice(dialog).selectOption(replacementId);
  await updateMock(page, { checks: findings });
  await checkButton(dialog).click();
  await expect(review(dialog).getByRole('status')).toContainText('本次核对已排除所选待替换记录。');
  await releaseHeldResponse(page);
  await expect(results(dialog)).toBeVisible();
  expect(await calls(page, 'check_modrinth_dependencies')).toEqual([
    { command: 'check_modrinth_dependencies', args: { request: { gameId: firstGame, dependencies: [], replaceInstallId: null } } },
    { command: 'check_modrinth_dependencies', args: { request: { gameId: firstGame, dependencies: findings.map(entry => entry.dependency), replaceInstallId: replacementId } } },
  ]);
  await replacementChoice(dialog).selectOption('');
  await expect(results(dialog)).toHaveCount(0);
  await expect(review(dialog).getByRole('status')).toBeEmpty();
  expect(await calls(page, 'check_modrinth_dependencies')).toHaveLength(2);
});

test('closing and opening a different download discards the earlier report', async ({ page }) => {
  await setup(page);
  const first = await openDialog(page);
  await gameChoice(first).selectOption(otherGame);
  await updateMock(page, { holdNext: 'check_modrinth_dependencies', checks: null });
  await checkButton(first).click();
  await waitForHeldResponse(page);
  await page.keyboard.press('Escape');
  await expect(first).toHaveCount(0);

  const second = await openDialog(page, 'Second Download');
  await gameChoice(second).selectOption(otherGame);
  await releaseHeldResponse(page);
  await expect(review(second).getByRole('status')).toBeEmpty();
  await updateMock(page, { checks: [] });
  await checkButton(second).click();
  await expect(review(second).getByRole('status')).toHaveText('该版本未声明直接依赖。');
  expect(await calls(page, 'resolve_mod')).toEqual([
    { command: 'resolve_mod', args: { request: exactRequest } },
    { command: 'resolve_mod', args: { request: { source_id: 'modrinth', project_id: 'Fixture2', version_id: 'Release2' } } },
  ]);
});
