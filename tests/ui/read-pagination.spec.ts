import { expect, test } from '@playwright/test';
import { installMockTauri } from './mock-tauri';

const cursor = '634dc3f5-866a-4f80-b9de-e4ebe5ec6763';

test('游戏库仅保留当前候选批次，并可结束扫描', async ({ page }) => {
  await installMockTauri(page);
  await page.goto('/');
  await page.getByRole('button', { name: '游戏库', exact: true }).click();
  await page.evaluate((cursor) => {
    const browser = window as any;
    const invoke = browser.__TAURI_INTERNALS__.invoke;
    browser.__TAURI_INTERNALS__.invoke = async (command: string, args: any) => {
      if (command !== 'scan_games') return invoke(command, args);
      browser.__AMM_TEST__.calls.push({ command, args });
      const request = args.options.page;
      const second = request.cursor === cursor;
      return {
        candidates: request.cancel ? [] : [{
          rootPath: second ? '/page-two' : '/page-one', name: second ? '第二批候选游戏' : '首批候选游戏',
          confidence: 0, status: 'unidentified', suggestedAdapterSlug: null,
          platform: 'unknown', matches: [], sources: [], requiresConfirmation: true,
          registeredGameId: null, warnings: [],
        }],
        warnings: [], entriesChecked: 123456, directoriesChecked: 10002, truncated: false, nextCursor: cursor,
      };
    };
  }, cursor);
  await page.getByRole('button', { name: '扫描游戏', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: '扫描游戏', exact: true });
  await expect(dialog.getByText('首批候选游戏')).toBeVisible();
  await dialog.getByRole('button', { name: '下一批候选' }).click();
  await expect(dialog.getByText('第二批候选游戏')).toBeVisible();
  await expect(dialog.getByText('首批候选游戏')).toHaveCount(0);
  await dialog.getByRole('button', { name: '停止扫描' }).click();
  await expect(dialog.getByRole('button', { name: '下一批候选' })).toHaveCount(0);
  const options = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === 'scan_games').map((call: any) => call.args.options.page));
  expect(options).toEqual([{ requestId: expect.any(String) }, { cursor }, { requestId: cursor, cancel: true }]);
});

test('诊断续页保留游戏范围和绝对日志行号', async ({ page }) => {
  await installMockTauri(page, { withExternalAdapter: true });
  await page.goto('/');
  await page.getByRole('button', { name: '游戏库', exact: true }).click();
  await page.evaluate((cursor) => {
    const browser = window as any;
    const invoke = browser.__TAURI_INTERNALS__.invoke;
    browser.__TAURI_INTERNALS__.invoke = async (command: string, args: any) => {
      if (command !== 'diagnose_game') return invoke(command, args);
      browser.__AMM_TEST__.calls.push({ command, args });
      const second = args.options.page.cursor === cursor;
      return {
        status: 'findings', causes: [], unknowns: [], repairSuggestions: [],
        evidence: [{ file: '/game/logs/latest.log', line: second ? 12001 : 1, text: second ? '第二批诊断内容' : '首批诊断内容' }],
        logs: { files: [], nextCursor: second ? null : cursor },
      };
    };
  }, cursor);
  await page.getByRole('button').filter({ has: page.getByRole('heading', { name: 'Minecraft Java Edition', exact: true }) }).click();
  await page.getByRole('tab', { name: '诊断' }).click();
  await page.getByRole('button', { name: '开始诊断' }).click();
  await expect(page.getByText('首批诊断内容')).toBeVisible();
  await page.getByRole('button', { name: '下一批诊断' }).click();
  await expect(page.getByText('第二批诊断内容')).toBeVisible();
  await expect(page.getByText(/12001/)).toBeVisible();
  await expect(page.getByText('首批诊断内容')).toHaveCount(0);
  await expect(page.getByRole('button', { name: '下一批诊断' })).toHaveCount(0);
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === 'diagnose_game'));
  expect(calls[1].args).toEqual({ gameId: 'game-fixture', options: { page: { cursor } } });
});
