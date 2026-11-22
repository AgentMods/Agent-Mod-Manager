import { expect, test, type Locator, type Page, type TestInfo } from '@playwright/test';
import { installMockTauri } from './mock-tauri';

async function setup(page: Page, theme: 'light' | 'dark') {
  await page.emulateMedia({ colorScheme: theme });
  await installMockTauri(page);
  await page.goto('/');
  await page.getByRole('button', {name: '游戏库', exact: true}).click();
  await page.evaluate(() => {
    const target = window as any;
    const transport = target.__TAURI_INTERNALS__;
    const invoke = transport.invoke;
    const mock = target.__AMM_TEST__;
    const previous = 'old-release';
    const transactionId = 'txn-replacement-fixture';
    const versionId = 'V'.repeat(128);
    const gamePath = '/sandbox/minecraft/mods/';
    const downloadPath = '/sandbox/data/downloads/fixture/Fixture.jar';
    const trashPath = '/sandbox/data/trash/replacement/payload';
    const fingerprint = { kind: 'file', size: 1024, sha256: 'b'.repeat(64) };
    const plan = {
      id: transactionId, summary: `替换 Downloaded Fixture：OLD001 → ${versionId}`, total_bytes: 3072,
      created_at: '2026-11-18T00:00:00Z', requires_review: true,
      entries: [
        { operation: { op: 'copy', src: downloadPath, dst: `${gamePath}shared.jar` }, impact: 'overwrite', before: fingerprint, source: fingerprint },
        { operation: { op: 'copy', src: downloadPath, dst: `${gamePath}added.jar` }, impact: 'create', before: null, source: fingerprint },
        { operation: { op: 'move', src: `${gamePath}retired.jar`, dst: trashPath }, impact: 'move', before: null, source: fingerprint },
      ],
    };
    const origin = { artifactId: 'artifact-old', sourceId: 'modrinth', projectId: 'fixture', versionId: 'OLD001' };
    let rolledBack = false;
    const controls = mock.replacements = {
      supersedeReplacement: false, failRollback: false, blockGame: '', release: undefined as (() => void) | undefined,
    };

    transport.invoke = async (command: string, args: Record<string, any> = {}) => {
      if (command === 'rollback_transaction') {
        mock.calls.push({ command, args });
        if (args.transactionId !== transactionId || rolledBack) throw new Error('不是当前替换事务');
        if (controls.failRollback) throw new Error('回收站载荷已变化，不能回滚');
        rolledBack = true;
        return null;
      }
      const result = await invoke(command, args);
      if (command === 'get_file_plan' && args.planId === transactionId) return plan;
      if (command !== 'list_installs') return result;
      if (controls.blockGame === args.gameId) {
        controls.blockGame = '';
        await new Promise<void>(resolve => { controls.release = resolve; });
        controls.release = undefined;
      }
      if (args.gameId !== 'game-fixture') return [];

      const [base] = result;
      const old = { ...base, id: previous, name: '旧版 Fixture', downloadOrigin: origin,
        status: rolledBack ? 'enabled' : 'superseded', metadata: { transactionId: 'txn-old-install', modType: 'java-mod' } };
      return [old, { ...base, id: 'replacement-release', name: '新版 Fixture',
        status: controls.supersedeReplacement ? 'superseded' : rolledBack ? 'rolled_back' : 'enabled',
        downloadOrigin: { ...origin, versionId }, metadata: { replacesInstallId: previous, transactionId, modType: 'java-mod' } }];
    };
  });
}

async function openGameMods(page: Page) {
  await page.getByRole('button', { name: '游戏库', exact: true }).click();
  await page.getByRole('button').filter({ hasText: 'Minecraft Java Edition' }).click();
  await page.getByRole('tab', {name: 'Mod 管理', exact: true}).click();
}

async function calls(page: Page, command: string) {
  return page.evaluate(name => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === name).map((call: any) => call.args), command);
}

async function recordDialog(dialog: Locator, info: TestInfo, name: string) {
  expect(await dialog.evaluate(element => {
    const box = element.getBoundingClientRect();
    const body = element.querySelector('[data-slot="modal-body"]')!;
    return box.left >= 0 && box.right <= innerWidth + 1 && body.scrollWidth <= body.clientWidth + 1;
  })).toBe(true);
  await dialog.screenshot({ path: info.outputPath(`${name}.png`) });
}

test('rollback reads the original replacement, then confirms separately and refreshes rows', async ({ page }, info) => {
  await setup(page, 'dark');
  await openGameMods(page);

  const old = page.getByRole('row').filter({ hasText: '旧版 Fixture' });
  const replacement = page.getByRole('row').filter({ hasText: '新版 Fixture' });
  const openRollback = replacement.getByRole('button', { name: '预览回滚替换', exact: true });
  await expect(old).toContainText('已被替换');
  await expect(old.getByRole('button', { name: '使用 Agent 卸载', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: '预览回滚替换', exact: true })).toHaveCount(1);
  await openRollback.click();

  const rollback = page.getByRole('dialog', { name: '回滚版本替换', exact: true });
  const confirm = rollback.getByRole('button', { name: '确认回滚', exact: true });
  await expect(rollback).toContainText('原替换事务：');
  const effects = rollback.getByRole('table', { name: '回滚文件影响', exact: true });
  await expect(effects.getByRole('row').nth(1)).toContainText('/sandbox/minecraft/mods/retired.jar');
  await expect(effects.getByRole('row').nth(1)).toContainText('移回原位置');
  await expect(effects.getByRole('row').nth(2)).toContainText('移出新增文件');
  await expect(effects.getByRole('row').nth(3)).toContainText('还原覆盖前文件');
  await expect(rollback.getByRole('button', { name: '批准并执行', exact: true })).toHaveCount(0);
  expect(await calls(page, 'get_file_plan')).toEqual([{ planId: 'txn-replacement-fixture' }]);
  expect(await calls(page, 'rollback_transaction')).toHaveLength(0);
  await recordDialog(rollback, info, 'dark-replacement-rollback');
  await rollback.getByRole('button', { name: '取消', exact: true }).click();
  expect(await calls(page, 'rollback_transaction')).toHaveLength(0);

  await openRollback.click();
  await page.evaluate(() => { (window as any).__AMM_TEST__.replacements.failRollback = true; });
  await confirm.click();
  await expect(rollback.getByRole('alert')).toContainText('回收站载荷已变化，不能回滚');
  await page.evaluate(() => { (window as any).__AMM_TEST__.replacements.failRollback = false; });
  await confirm.click();
  await expect(rollback).toBeHidden();
  await expect(old).toContainText('已安装');
  await expect(replacement).toContainText('已回滚');
  await expect(openRollback).toHaveCount(0);
  expect(await calls(page, 'rollback_transaction')).toEqual([{ transactionId: 'txn-replacement-fixture' }, { transactionId: 'txn-replacement-fixture' }]);
  expect(await calls(page, 'apply_plan')).toHaveLength(0);
});

test('rollback refuses a replacement superseded while its original plan is open', async ({ page }) => {
  await setup(page, 'light');
  await openGameMods(page);
  await page.getByRole('button', { name: '预览回滚替换', exact: true }).click();
  const rollback = page.getByRole('dialog', { name: '回滚版本替换', exact: true });
  const confirm = rollback.getByRole('button', { name: '确认回滚', exact: true });
  await page.evaluate(() => { (window as any).__AMM_TEST__.replacements.blockGame = 'game-fixture'; });
  await confirm.click();
  await expect(rollback.getByRole('button', { name: '取消', exact: true })).toBeDisabled();
  await expect.poll(() => page.evaluate(() => !!(window as any).__AMM_TEST__.replacements.release)).toBe(true);
  await page.evaluate(() => {
    const controls = (window as any).__AMM_TEST__.replacements;
    controls.supersedeReplacement = true;
    controls.release();
  });
  await expect(rollback.getByRole('alert')).toContainText('安装记录已变化，不能继续回滚此替换。');
  await expect(confirm).toBeDisabled();
  expect(await calls(page, 'rollback_transaction')).toHaveLength(0);
});
