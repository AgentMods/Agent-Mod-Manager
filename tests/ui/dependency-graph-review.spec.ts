import { expect, test, type Locator, type Page } from '@playwright/test';
import { installMockTauri } from './mock-tauri';

const command = 'review_modrinth_dependency_graph';
const artifactId = 'artifact-fixture';
const firstGame = 'game-fixture';
const secondGame = 'second-java-game';
const unadaptedGame = 'unadapted-game';
const replacementId = 'old-install';
const root = { project_id: 'Fixture1', version_id: 'Release1', declarations_known: true };
const bridgeProject = 'Bridge01';
const candidateVersions = ['BridgeV1', 'BridgeV2'];
const chosenVersion = candidateVersions[1];
const bridge = { project_id: bridgeProject, version_id: chosenVersion, declarations_known: true };
const leaf = { project_id: 'LeafDep1', version_id: 'LeafVer1', declarations_known: true };
const bridgeFileEvidence = {
  status: 'mismatch', matched: 1, modified: 1, missing: 1, unverified: 1, limitReached: true,
  issues: [
    { path: 'mods/Changed.jar', state: 'modified' },
    { path: 'mods/Missing.jar', state: 'missing' },
    { path: 'mods/Unknown.jar', state: 'unverified' },
  ],
};

function required(project_id: string, version_id: string | null) {
  return { dependency_type: 'required', project_id, version_id, file_name: null };
}

const bridgeDeclaration = required(bridgeProject, null);
const leafDeclaration = required(leaf.project_id, leaf.version_id);
const bridgeMatch = { installId: 'bridge-install', projectId: bridgeProject, versionId: chosenVersion, status: 'partially_restored' };
const reports = {
  unresolved: {
    graph: {
      root_version_id: root.version_id, complete: false, truncated: false, nodes: [root], issues: [],
      edges: [{ from_version_id: root.version_id, declaration: bridgeDeclaration, state: 'choice_required', selected_version_id: null, candidates: candidateVersions }],
    },
    checks: [{ dependency: bridgeDeclaration, status: 'recorded_project', reason: null, matchCount: 1, matches: [bridgeMatch] }],
    fileEvidence: { [bridgeMatch.installId]: bridgeFileEvidence },
  },
  pinned: {
    graph: {
      root_version_id: root.version_id, complete: true, truncated: false, nodes: [root, bridge, leaf], issues: [],
      edges: [
        { from_version_id: root.version_id, declaration: bridgeDeclaration, state: 'selected', selected_version_id: chosenVersion, candidates: [] },
        { from_version_id: chosenVersion, declaration: leafDeclaration, state: 'selected', selected_version_id: leaf.version_id, candidates: [] },
      ],
    },
    checks: [
      { dependency: required(bridgeProject, chosenVersion), status: 'recorded_exact', reason: null, matchCount: 1, matches: [bridgeMatch] },
      { dependency: leafDeclaration, status: 'not_recorded', reason: null, matchCount: 0, matches: [] },
    ],
    fileEvidence: { [bridgeMatch.installId]: bridgeFileEvidence },
  },
};

type GraphRequest = {
  artifactId: string; gameId: string; replaceInstallId: string | null;
  gameVersion: string; loader: string; projectPins: Record<string, string>;
};

async function setup(page: Page, colorScheme: 'light' | 'dark' = 'light', includeReplacement = false) {
  await page.emulateMedia({ colorScheme });
  await installMockTauri(page);
  await page.addInitScript(({ command, firstGame, secondGame, unadaptedGame, replacementId, includeReplacement, root, bridgeProject, chosenVersion, reports }) => {
    const target = window as any;
    function configure(mock: any) {
      const transport = target.__TAURI_INTERNALS__;
      const invoke = transport.invoke;
      mock.seedDownload = true;
      const controls: { holdNext: boolean; release?: () => void; fileEvidence?: Record<string, unknown> } = mock.dependencyGraph = { holdNext: false };
      transport.invoke = async (name: string, args: { request?: GraphRequest; gameId?: string } = {}) => {
        if (name === command) {
          mock.calls.push({ command: name, args });
          const { artifactId, gameId, replaceInstallId, gameVersion, loader, projectPins } = args.request!;
          const report = projectPins[bridgeProject] === chosenVersion ? reports.pinned : reports.unresolved;
          if (controls.holdNext) {
            controls.holdNext = false;
            await new Promise<void>(resolve => { controls.release = resolve; });
          }
          return { artifactId, gameId, excludedInstallId: replaceInstallId, gameVersion, loader, ...report,
            fileEvidence: controls.fileEvidence ?? report.fileEvidence };
        }
        const result = await invoke(name, args);
        if (name === 'list_games') return [...result,
          { ...result[0], id: secondGame, name: 'Second Minecraft', rootPath: '/sandbox/second-java' },
          { ...result[0], id: unadaptedGame, name: 'Unadapted Minecraft', rootPath: '/sandbox/unadapted', adapterSlug: null },
        ];
        if (name === 'list_downloads') {
          const seeded = { ...result[0], projectId: root.project_id, versionId: root.version_id };
          return [seeded, { ...seeded, artifact: { ...seeded.artifact, id: 'non-modrinth-artifact' }, name: 'Other Provider', sourceId: 'thunderstore' }];
        }
        if (name === 'list_installs' && includeReplacement && args.gameId === firstGame) return [{
          ...result[0], id: replacementId, metadata: { modType: 'java-mod' },
          downloadOrigin: { sourceId: 'modrinth', projectId: root.project_id, versionId: 'OldVers1' },
        }];
        return result;
      };
    }

    // Playwright does not guarantee the order of the two init scripts.
    if (target.__AMM_TEST__) configure(target.__AMM_TEST__);
    else Object.defineProperty(target, '__AMM_TEST__', {
      configurable: true,
      set(mock) {
        Object.defineProperty(target, '__AMM_TEST__', { value: mock, configurable: true, writable: true });
        configure(mock);
      },
    });
  }, { command, firstGame, secondGame, unadaptedGame, replacementId, includeReplacement, root, bridgeProject, chosenVersion, reports });
  await page.goto('/');
  await page.getByRole('button', { name: 'Mod 发现', exact: true }).click();
}

async function openDialog(page: Page, name = 'Downloaded Fixture') {
  await page.locator('.download-row').filter({ hasText: name }).getByRole('button', { name: '安装预览', exact: true }).click();
  return page.getByRole('dialog', { name: '下载归档安装预览', exact: true });
}

const game = (dialog: Locator) => dialog.getByRole('combobox', { name: '目标游戏', exact: true });
const disclosure = (dialog: Locator) => dialog.getByRole('button', { name: '传递依赖核对', exact: true })
  .or(dialog.locator('summary').filter({ hasText: /^传递依赖核对$/ }));
const version = (dialog: Locator) => dialog.getByRole('textbox', { name: 'Minecraft 版本', exact: true });
const loader = (dialog: Locator) => dialog.getByRole('textbox', { name: '加载器', exact: true });
const review = (dialog: Locator) => dialog.getByRole('button', { name: '核对传递依赖', exact: true });
const candidate = (dialog: Locator) => dialog.getByRole('combobox', { name: `依赖版本 ${bridgeProject}`, exact: true });
const confirm = (dialog: Locator) => dialog.getByRole('button', { name: '确认版本', exact: true });
const rows = (dialog: Locator) => dialog.getByRole('list', { name: '传递依赖核对结果', exact: true }).getByRole('listitem');
const fileResults = (row: Locator) => row.locator('.graph-file-result');
const choice = (dialog: Locator) => dialog.getByText('需选择版本', { exact: true });
const recorded = (dialog: Locator) => dialog.getByText('已记录确切版本', { exact: true });
const missing = (dialog: Locator) => dialog.getByText('缺少必需依赖的安装记录', { exact: true });

async function enterEnvironment(dialog: Locator, gameVersion = '1.21.1') {
  if (!(await version(dialog).isVisible())) await disclosure(dialog).press('Enter');
  await version(dialog).fill(gameVersion);
  await loader(dialog).fill('fabric');
}

function graphCall(overrides: Partial<GraphRequest> = {}) {
  return { command, args: { request: {
    artifactId, gameId: firstGame, replaceInstallId: null, gameVersion: '1.21.1', loader: 'fabric', projectPins: {}, ...overrides,
  } } };
}

async function calls(page: Page, ...commands: string[]) {
  return page.evaluate(commands => (window as any).__AMM_TEST__?.calls.filter((call: { command: string }) => commands.includes(call.command)), commands);
}

async function expectHorizontalFit(dialog: Locator) {
  expect(await dialog.locator('.modal-body').evaluate(body => {
    const bounds = body.getBoundingClientRect();
    return bounds.left >= 0 && bounds.right <= innerWidth + 1 && body.scrollWidth <= body.clientWidth + 1;
  })).toBe(true);
}

test.afterEach(async ({ page }) => {
  expect(await calls(page, 'download_mod', 'plan_install', 'apply_plan')).toEqual([]);
});

for (const theme of ['light', 'dark'] as const) {
  test(`reviews exact transitive dependencies only on request in ${theme} theme`, async ({ page }) => {
    await setup(page, theme, true);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const dialog = await openDialog(page);
    await expect(disclosure(dialog)).toBeHidden();
    await game(dialog).selectOption(firstGame);
    await dialog.getByLabel('适配包 Mod 类型', { exact: true }).selectOption('java-mod');
    await dialog.getByLabel('要替换的已装版本', { exact: true }).selectOption(replacementId);
    const preview = dialog.getByRole('button', { name: '生成变更预览', exact: true });
    await expect(preview).toBeEnabled();
    await expect(version(dialog)).toBeHidden();
    await disclosure(dialog).press('Enter');
    await expect(review(dialog)).toBeDisabled();
    await version(dialog).fill('1.21.1');
    await expect(review(dialog)).toBeDisabled();
    await loader(dialog).fill('fabric');
    expect(await calls(page, command)).toEqual([]);

    await review(dialog).press('Enter');
    await expect(choice(dialog)).toBeVisible();
    const unresolvedBridgeRow = rows(dialog).filter({ hasText: `项目 ${bridgeProject}` });
    await expect(fileResults(unresolvedBridgeRow)).toHaveCount(0);
    await expectHorizontalFit(dialog);
    await expect(recorded(dialog)).toBeHidden();
    await expect(confirm(dialog)).toBeDisabled();
    for (const id of candidateVersions) await expect(candidate(dialog).getByRole('option', { name: id, exact: true })).toHaveCount(1);
    await candidate(dialog).selectOption(chosenVersion);
    const initial = graphCall({ replaceInstallId: replacementId });
    expect(await calls(page, command)).toEqual([initial]);
    await expect(choice(dialog)).toBeVisible();
    await confirm(dialog).press('Enter');

    await expect(rows(dialog)).toHaveCount(2);
    const bridgeRow = rows(dialog).filter({ hasText: `项目 ${bridgeProject}` });
    await expect(bridgeRow.getByText(root.version_id, { exact: true })).toBeVisible();
    await expect(bridgeRow.getByText(chosenVersion, { exact: true })).toBeVisible();
    await expect(recorded(bridgeRow)).toBeVisible();
    await expect(bridgeRow.getByText('文件与清单不符', { exact: true })).toBeVisible();
    await expect(bridgeRow.getByText(/已修改 1/)).toBeVisible();
    await expect(bridgeRow.getByText(/缺失 1/)).toBeVisible();
    await expect(bridgeRow.getByText(/未核验 1/)).toBeVisible();
    await expect(bridgeRow.getByText('达到文件核验上限', { exact: true })).toBeVisible();
    await expect(bridgeRow.getByText('已修改：mods/Changed.jar', { exact: true })).toBeVisible();
    await expect(bridgeRow.getByText('缺失：mods/Missing.jar', { exact: true })).toBeVisible();
    await expect(bridgeRow.getByText('未核验：mods/Unknown.jar', { exact: true })).toBeVisible();
    await expect(dialog.getByText(/文件核验不代表加载器已启用。/)).toBeVisible();
    const leafRow = rows(dialog).filter({ hasText: `项目 ${leaf.project_id}` });
    await expect(leafRow.getByText(chosenVersion, { exact: true })).toBeVisible();
    await expect(leafRow.getByText(leaf.version_id, { exact: true })).toBeVisible();
    await expect(missing(leafRow)).toBeVisible();
    await expect(fileResults(leafRow)).toHaveCount(0);
    await expect(choice(dialog)).toBeHidden();
    await expect(preview).toBeEnabled();
    await expectHorizontalFit(dialog);
    await loader(dialog).fill('quilt');
    await expect(recorded(dialog)).toBeHidden();
    await expect(missing(dialog)).toBeHidden();
    expect(await calls(page, command)).toEqual([initial, graphCall({
      replaceInstallId: replacementId, projectPins: { [bridgeProject]: chosenVersion },
    })]);
  });
}

test('only offers transitive review for Modrinth archives and confirmed Minecraft Java adapters', async ({ page }) => {
  await setup(page);
  const dialog = await openDialog(page);
  await game(dialog).selectOption(unadaptedGame);
  await expect(disclosure(dialog)).toBeHidden();
  await game(dialog).selectOption(firstGame);
  await expect(disclosure(dialog)).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();

  const other = await openDialog(page, 'Other Provider');
  await game(other).selectOption(firstGame);
  await expect(disclosure(other)).toBeHidden();
  expect(await calls(page, command)).toEqual([]);
});

test('renders matched and unverified file evidence independently of install journal status', async ({ page }) => {
  await setup(page);
  const dialog = await openDialog(page);
  await game(dialog).selectOption(firstGame);
  await enterEnvironment(dialog);

  const evidenceFor = (status: 'matched' | 'unverified') => ({
    status, matched: status === 'matched' ? 2 : 0, modified: 0, missing: 0,
    unverified: status === 'unverified' ? 1 : 0, limitReached: false,
    issues: status === 'unverified' ? [{ path: 'mods/Uninspectable.jar', state: 'unverified' }] : [],
  });
  await page.evaluate(({ installId, evidence }) => {
    (window as any).__AMM_TEST__.dependencyGraph.fileEvidence = { [installId]: evidence };
  }, { installId: bridgeMatch.installId, evidence: evidenceFor('matched') });

  await review(dialog).click();
  await expect(choice(dialog)).toBeVisible();
  await expect(fileResults(rows(dialog).filter({ hasText: `项目 ${bridgeProject}` }))).toHaveCount(0);
  await candidate(dialog).selectOption(chosenVersion);
  await confirm(dialog).click();
  const bridgeRow = rows(dialog).filter({ hasText: `项目 ${bridgeProject}` });
  await expect(bridgeRow.getByText('文件与清单一致', { exact: true })).toBeVisible();
  await expect(bridgeRow.getByText('已记录确切版本', { exact: true })).toBeVisible();

  await loader(dialog).fill('quilt');
  await expect(rows(dialog)).toHaveCount(0);
  await page.evaluate(({ installId, evidence }) => {
    (window as any).__AMM_TEST__.dependencyGraph.fileEvidence = { [installId]: evidence };
  }, { installId: bridgeMatch.installId, evidence: evidenceFor('unverified') });
  await review(dialog).click();
  await expect(choice(dialog)).toBeVisible();
  await candidate(dialog).selectOption(chosenVersion);
  await confirm(dialog).click();
  const unverifiedRow = rows(dialog).filter({ hasText: `项目 ${bridgeProject}` });
  await expect(unverifiedRow.getByText('文件未能核验', { exact: true })).toBeVisible();
  await expect(unverifiedRow.getByText('未核验：mods/Uninspectable.jar', { exact: true })).toBeVisible();
});

for (const change of ['target game', 'Minecraft version'] as const) {
  test(`ignores a delayed pinned report after changing the ${change}`, async ({ page }) => {
    await setup(page);
    const dialog = await openDialog(page);
    await game(dialog).selectOption(firstGame);
    await enterEnvironment(dialog);
    await review(dialog).click();
    await expect(choice(dialog)).toBeVisible();
    await candidate(dialog).selectOption(chosenVersion);
    await page.evaluate(() => { (window as any).__AMM_TEST__.dependencyGraph.holdNext = true; });
    await confirm(dialog).click();
    await expect.poll(() => page.evaluate(() => typeof (window as any).__AMM_TEST__.dependencyGraph.release)).toBe('function');

    const nextGame = change === 'target game' ? secondGame : firstGame;
    const nextVersion = change === 'Minecraft version' ? '1.21.2' : '1.21.1';
    if (nextGame !== firstGame) await game(dialog).selectOption(nextGame);
    await enterEnvironment(dialog, nextVersion);
    await expect(choice(dialog)).toBeHidden();
    const priorCalls = [graphCall(), graphCall({ projectPins: { [bridgeProject]: chosenVersion } })];
    expect(await calls(page, command)).toEqual(priorCalls);

    await review(dialog).click();
    await expect(choice(dialog)).toBeVisible();
    await page.evaluate(async () => {
      (window as any).__AMM_TEST__.dependencyGraph.release();
      // Let the delayed IPC continuation and the next React paint run before asserting it was ignored.
      await new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
    });
    await expect(choice(dialog)).toBeVisible();
    await expect(recorded(dialog)).toBeHidden();
    await expect(missing(dialog)).toBeHidden();
    await expect(dialog.locator('.graph-file-result')).toHaveCount(0);
    expect(await calls(page, command)).toEqual([...priorCalls, graphCall({ gameId: nextGame, gameVersion: nextVersion })]);
  });
}
