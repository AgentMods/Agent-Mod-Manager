import { expect, test, type Page } from '@playwright/test';
import { installMockTauri } from './mock-tauri';

type FabricStartup = {
  status: 'unavailable' | 'unrecognized' | 'startup_observed' | 'selection_reported' | 'partial_selection' | 'resolution_failed' | 'ambiguous';
  file: 'logs/latest.log';
  modifiedAt: string | null;
  gameVersion: string | null;
  loaderVersion: string | null;
  startupLine: number | null;
  selectionLine: number | null;
  failureLine: number | null;
  declaredCount: number | null;
  mods: { id: string; version: string; line: number; nested: boolean }[];
  omittedCount: number;
};

function startup(status: FabricStartup['status'], overrides: Partial<FabricStartup> = {}): FabricStartup {
  return {
    status, file: 'logs/latest.log', modifiedAt: null, gameVersion: null, loaderVersion: null,
    startupLine: null, selectionLine: null, failureLine: null, declaredCount: null, mods: [], omittedCount: 0,
    ...overrides,
  };
}

async function setup(page: Page) {
  await installMockTauri(page);
  await page.goto('/');
  const heading = page.getByRole('heading', { name: 'Minecraft Java Edition', exact: true });
  await page.getByRole('button').filter({ has: heading }).click();
  await page.getByRole('tab', { name: '诊断', exact: true }).click();
  await expect(heading).toBeVisible();
  await page.evaluate(() => {
    const target = window as any;
    const mock = target.__AMM_TEST__;
    const transport = target.__TAURI_INTERNALS__;
    mock.fabricStartup = null;
    mock.fabricFindings = null;
    const invoke = transport.invoke;
    transport.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      if (command !== 'diagnose_game') return invoke(command, args);
      mock.calls.push({ command, args });
      return {
        status: 'unknown', causes: [], unknowns: [], repairSuggestions: [], evidence: [],
        fabricStartup: mock.fabricStartup,
        ...mock.fabricFindings,
      };
    };
  });
}

async function diagnose(page: Page, report: FabricStartup) {
  await page.evaluate(value => { (window as any).__AMM_TEST__.fabricStartup = value; }, report);
  await page.getByRole('button', { name: '开始诊断', exact: true }).click();
  return page.getByRole('region', { name: 'Fabric 启动证据', exact: true });
}

async function setTheme(page: Page, theme: 'light' | 'dark') {
  const html = page.locator('html');
  if (await html.getAttribute('data-theme') !== theme) {
    await page.getByRole('button', { name: theme === 'dark' ? '切换暗色主题' : '切换亮色主题', exact: true }).click();
  }
  await expect(html).toHaveAttribute('data-theme', theme);
}

test('shows selected Fabric Mods with keyboard disclosure, source lines, and responsive layout in both themes', async ({ page }) => {
  await setup(page);
  const longVersion = `0.16.10+fabric-${'0123456789abcdef'.repeat(7)}`;
  const section = await diagnose(page, startup('selection_reported', {
    modifiedAt: '2026-11-17T12:34:56Z', gameVersion: '1.21.1', loaderVersion: '0.16.10',
    startupLine: 1, selectionLine: 2, declaredCount: 2,
    mods: [
      { id: 'sodium', version: '0.6.0', line: 3, nested: false },
      { id: 'fabric-api', version: longVersion, line: 4, nested: true },
    ],
  }));

  await expect(section.getByText('日志记录了 Mod 选择清单', { exact: true })).toBeVisible();
  await expect(section.getByText('这是日志中的启动记录，不代表当前运行状态或初始化成功。', { exact: true })).toBeVisible();
  await expect(section.getByText('logs/latest.log:1', { exact: true })).toBeVisible();
  await expect(section.getByText('清单第 2 行', { exact: true })).toBeVisible();
  const disclosure = section.locator('summary').filter({ hasText: /^启动时选中的 Mod（2）$/ });
  await expect(disclosure).toBeVisible();
  await expect(section.getByRole('table', { name: 'Fabric Mod 选择清单', exact: true })).toBeHidden();
  await disclosure.press('Enter');

  const table = section.getByRole('table', { name: 'Fabric Mod 选择清单', exact: true });
  await expect(table).toBeVisible();
  const fabricApi = table.getByRole('row').filter({ hasText: 'fabric-api' });
  await expect(fabricApi.getByRole('cell').nth(0)).toContainText('内嵌');
  await expect(fabricApi.getByRole('cell').nth(1)).toHaveText(longVersion);
  await expect(fabricApi.getByRole('cell').nth(2)).toHaveText('4');

  for (const theme of ['light', 'dark'] as const) {
    await setTheme(page, theme);
    expect(await page.locator('main').evaluate(main => main.scrollWidth <= main.clientWidth + 1)).toBe(true);
    expect(await section.evaluate(element => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
  }

  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls as { command: string; args: unknown }[]);
  expect(calls.filter(call => call.command === 'diagnose_game')).toEqual([
    { command: 'diagnose_game', args: { gameId: 'game-fixture', options: null } },
  ]);
  expect(calls.filter(call => /^(plan_|apply_|write_|launch_)/.test(call.command))).toEqual([]);
});

test('keeps partial, unavailable, unrecognized, ambiguous, and failed records distinct', async ({ page }) => {
  await setup(page);
  const states: {
    report: FabricStartup;
    label: string;
    line?: string;
    disclosure?: string;
    omitted?: string;
  }[] = [
    {
      report: startup('partial_selection', {
        startupLine: 1, selectionLine: 2, declaredCount: 4,
        mods: [{ id: 'fabric-api', version: '0.16.10', line: 3, nested: false }], omittedCount: 3,
      }),
      label: '清单不完整', line: '清单第 2 行', disclosure: '日志中可读的 Mod（1）', omitted: '已省略 3 项',
    },
    { report: startup('unavailable'), label: '启动日志不可用' },
    { report: startup('unrecognized'), label: '未识别到 Fabric 启动记录' },
    {
      report: startup('startup_observed', { startupLine: 1, gameVersion: '1.21.1', loaderVersion: '0.16.10' }),
      label: '日志记录了启动版本', line: 'logs/latest.log:1',
    },
    { report: startup('ambiguous'), label: '无法区分启动记录' },
    { report: startup('resolution_failed', { startupLine: 1, failureLine: 3 }), label: '日志记录了解析失败', line: '失败第 3 行' },
  ];

  for (const state of states) {
    const section = await diagnose(page, state.report);
    await expect(section.getByText(state.label, { exact: true })).toBeVisible();
    if (state.line) await expect(section.getByText(state.line, { exact: true })).toBeVisible();
    if (state.omitted) await expect(section.getByText(state.omitted, { exact: true })).toBeVisible();
    if (state.disclosure) {
      const disclosure = section.locator('summary').filter({ hasText: new RegExp(`^${state.disclosure}$`) });
      await disclosure.press('Enter');
      await expect(section.getByRole('table', { name: 'Fabric Mod 选择清单', exact: true })).toBeVisible();
      await expect(section.getByText('日志记录了 Mod 选择清单', { exact: true })).toHaveCount(0);
    } else {
      await expect(section.locator('details')).toHaveCount(0);
    }
    await expect(section.getByText('这是日志中的启动记录，不代表当前运行状态或初始化成功。', { exact: true })).toBeVisible();
  }

  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls as { command: string }[]);
  expect(calls.filter(call => call.command === 'diagnose_game')).toHaveLength(states.length);
  expect(calls.filter(call => /^(plan_|apply_|write_|launch_)/.test(call.command))).toEqual([]);
});

test('shows an exact Fabric dependency observation without offering an automatic repair', async ({ page }) => {
  await setup(page);
  const summary = 'Fabric 日志记录：amm_smoke_probe 1.0.0 所需的依赖 amm_smoke_required 版本不匹配；要求及已发现版本见原始证据';
  const text = "Mod 'AMM Smoke Probe' (amm_smoke_probe) 1.0.0 requires version 2.0.0 or later of mod 'AMM Smoke Dependency' (amm_smoke_required), but only the wrong version is present: 1.0.0!";
  await page.evaluate(({ summary, text }) => {
    (window as any).__AMM_TEST__.fabricFindings = {
      status: 'findings',
      causes: [{ kind: 'dependency', basis: 'observed', summary, evidenceIds: ['e1'] }],
      evidence: [{ id: 'e1', file: '/fixtures/instance/logs/latest.log', line: 12, text, source: 'fabric_dependency' }],
      repairSuggestions: [{ kind: 'dependency', description: '核对依赖名称、来源、版本约束和启用状态。', requiresConfirmation: true, fileOpsInput: null }],
    };
  }, { summary, text });
  const section = await diagnose(page, startup('resolution_failed', { startupLine: 1, failureLine: 3 }));
  await expect(section.getByText('失败第 3 行', { exact: true })).toBeVisible();
  await expect(page.getByRole('heading', { name: summary, exact: true })).toBeVisible();
  await expect(page.getByText('/fixtures/instance/logs/latest.log:12', { exact: true })).toBeVisible();
  await expect(page.getByText(text, { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '预览修复', exact: true })).toHaveCount(0);
  expect(await page.locator('main').evaluate(main => main.scrollWidth <= main.clientWidth + 1)).toBe(true);
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls as { command: string }[]);
  expect(calls.filter(call => /^(plan_|apply_|write_|launch_)/.test(call.command))).toEqual([]);
});
