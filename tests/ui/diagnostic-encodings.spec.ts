import { expect, test, type Page } from '@playwright/test';
import { installMockTauri } from './mock-tauri';

async function setup(page: Page) {
  await page.emulateMedia({ colorScheme: 'light' });
  await installMockTauri(page);
  await page.goto('/');
  await page.getByRole('button', {name: '游戏库', exact: true}).click();
  await expect(page.getByRole('heading', { name: 'Minecraft Java Edition', exact: true })).toBeVisible();
  await page.evaluate(async () => {
    const target = window as any;
    const mock = target.__AMM_TEST__;
    const transport = target.__TAURI_INTERNALS__;
    const invoke = transport.invoke;
    // The shared transport returns its fixture game list by reference.
    const games = await invoke('list_games');
    games.push({ ...games[0], id: 'other-game', name: '另一个游戏', rootPath: '/sandbox/other' });

    transport.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      if (command !== 'diagnose_game') return invoke(command, args);
      mock.calls.push({ command, args });
      const base = { status: 'unknown', causes: [], repairSuggestions: [], evidence: [] };
      if (args.gameId === 'other-game') return { ...base, unknowns: ['此游戏没有可用日志。'] };

      const encoding = (args.options as { legacyLogEncoding: string } | null)?.legacyLogEncoding;
      if (encoding === 'shift_jis') {
        await new Promise<void>(resolve => { mock.releaseDiagnosis = resolve; });
        return { ...base, unknowns: ['原游戏的延迟诊断。'] };
      }

      const legacy = encoding === 'gbk';
      const legacyFile = '/sandbox/minecraft/logs/legacy.log';
      return {
        ...base,
        unknowns: [legacy ? '未验证加载器兼容性。' : '旧版日志编码无法确认。'],
        evidence: legacy ? [{ file: legacyFile, line: 4, text: '旧版日志中的依赖线索。' }] : [],
        logs: { files: [
          { file: '/sandbox/minecraft/logs/latest.log', kind: 'log', status: 'read', encoding: 'utf-8', truncated: false, notes: [] },
          { file: 'C:\\sandbox\\logs\\boot.log', kind: 'log', status: 'read', encoding: 'utf-16le', truncated: true, notes: ['仅读取文件前缀。'] },
          { file: legacyFile, kind: 'log', status: legacy ? 'read' : 'unknown_encoding', encoding: legacy ? encoding : null, truncated: false,
            notes: [legacy ? '按请求的旧编码回退读取。' : '未指定旧编码回退。'] },
          { file: '/sandbox/minecraft/order.txt', kind: 'load_order', status: 'unknown_encoding', encoding: null, truncated: false, notes: ['加载顺序文件不使用日志回退。'] },
        ] },
      };
    };
  });
}

async function openDiagnosis(page: Page, game: string) {
  const heading = page.getByRole('heading', { name: game, exact: true });
  await page.getByRole('button').filter({ has: heading }).click();
  await page.getByRole('tab', { name: '诊断', exact: true }).click();
  await expect(heading).toBeVisible();
}

test('log fallback is optional, clears old evidence, and reports the actual source encodings', async ({ page }) => {
  await setup(page);
  await openDiagnosis(page, 'Minecraft Java Edition');
  const selector = page.getByRole('combobox', { name: '日志编码', exact: true });
  const run = page.getByRole('button', { name: '开始诊断', exact: true });
  const evidence = page.getByRole('complementary').filter({ has: page.getByRole('heading', { name: '诊断证据' }) });
  const sources = evidence.getByText('文件读取状态（4）', { exact: true });
  await expect(selector).toHaveValue('');
  expect(await selector.locator('option').evaluateAll(options => options.map(option => (option as HTMLOptionElement).value)))
    .toEqual(['', 'windows-1252', 'windows-1251', 'gbk', 'big5', 'shift_jis']);
  const before = await page.evaluate(() => (window as any).__AMM_TEST__.calls.length);
  await run.click();
  await expect(page.getByText('旧版日志编码无法确认。', { exact: true })).toBeVisible();
  await sources.click();
  await expect(evidence.getByText('日志 · 编码未知', { exact: true })).toBeVisible();
  await expect(evidence.getByText('未指定旧编码回退。', { exact: true })).toBeVisible();
  await expect(evidence.getByText('boot.log', { exact: true })).toBeVisible();
  await expect(evidence).not.toContainText('C:\\sandbox');

  await selector.selectOption('gbk');
  await expect(evidence).toHaveCount(0);
  await expect(page.getByText('旧版日志编码无法确认。', { exact: true })).toHaveCount(0);
  await expect(page.getByRole('heading', { name: '尚未运行诊断' })).toBeVisible();
  await run.click();
  await expect(page.getByText('未验证加载器兼容性。', { exact: true })).toBeVisible();
  await expect(evidence.getByText('旧版日志中的依赖线索。', { exact: true })).toBeVisible();
  await sources.click();
  for (const text of ['日志 · 已读取 · gbk', '按请求的旧编码回退读取。', '日志 · 已读取 · utf-8',
    '日志 · 已读取 · utf-16le · 已截断', '仅读取文件前缀。', '加载顺序 · 编码未知', '加载顺序文件不使用日志回退。']) {
    await expect(evidence.getByText(text, { exact: true })).toBeVisible();
  }
  for (const theme of ['light', 'dark']) {
    if (theme === 'dark') await page.getByRole('button', { name: '切换暗色主题', exact: true }).click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    expect(await page.locator('main').evaluate(main => main.scrollWidth <= main.clientWidth + 1)).toBe(true);
  }

  await selector.selectOption('');
  await expect(evidence).toHaveCount(0);
  await run.click();
  await expect(page.getByText('旧版日志编码无法确认。', { exact: true })).toBeVisible();
  expect(await page.evaluate(start => (window as any).__AMM_TEST__.calls.slice(start), before)).toEqual([
    { command: 'diagnose_game', args: { gameId: 'game-fixture', options: null } },
    { command: 'diagnose_game', args: { gameId: 'game-fixture', options: { legacyLogEncoding: 'gbk' } } },
    { command: 'diagnose_game', args: { gameId: 'game-fixture', options: null } },
  ]);
});

test('pending diagnosis locks the option and cannot populate another game', async ({ page }) => {
  await setup(page);
  await openDiagnosis(page, 'Minecraft Java Edition');
  const selector = page.getByRole('combobox', { name: '日志编码', exact: true });
  const run = page.getByRole('button', { name: '开始诊断', exact: true });
  await selector.selectOption('shift_jis');
  await run.click();
  await expect(page.getByRole('button', { name: '正在诊断…', exact: true })).toBeDisabled();
  await expect(selector).toBeDisabled();

  await page.getByRole('button', { name: '返回游戏库', exact: true }).click();
  await openDiagnosis(page, '另一个游戏');
  await expect(selector).toBeEnabled();
  await expect(selector).toHaveValue('');
  await run.click();
  await expect(page.getByText('此游戏没有可用日志。', { exact: true })).toBeVisible();
  await page.evaluate(() => (window as any).__AMM_TEST__.releaseDiagnosis());
  await expect(page.getByText('原游戏的延迟诊断。', { exact: true })).toHaveCount(0);
  await expect(page.getByText('此游戏没有可用日志。', { exact: true })).toBeVisible();
  await expect(page.getByText(/^文件读取状态/)).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: { command: string }) => call.command === 'diagnose_game').map((call: { args: unknown }) => call.args))).toEqual([
    { gameId: 'game-fixture', options: { legacyLogEncoding: 'shift_jis' } },
    { gameId: 'other-game', options: null },
  ]);
});
