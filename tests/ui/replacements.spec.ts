import { expect, test, type Locator, type Page, type TestInfo } from '@playwright/test';
import { installMockTauri } from './mock-tauri';

async function setup(page: Page, theme: 'light' | 'dark' = 'light', initialState: 'original' | 'replaced' = 'original') {
  await page.emulateMedia({ colorScheme: theme });
  await installMockTauri(page);
  await page.goto('/');
  await page.evaluate(initial => {
    const target = window as any;
    const transport = target.__TAURI_INTERNALS__;
    const invoke = transport.invoke;
    const mock = target.__AMM_TEST__;
    mock.seedDownload = true;
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
    let state: 'original' | 'replaced' | 'rolled_back' = initial;
    const controls = mock.replacements = {
      sameVersion: false, extraCandidateStatus: '', hideCandidates: false, supersedeReplacement: false,
      failRead: false, failRollback: false, blockGame: '', release: undefined as (() => void) | undefined,
      nexus: false, legacyNexusActive: true,
    };

    transport.invoke = async (command: string, args: Record<string, any> = {}) => {
      if (command === 'apply_plan' && args.planId === transactionId) {
        mock.calls.push({ command, args });
        state = 'replaced';
        return plan;
      }
      if (command === 'rollback_transaction') {
        mock.calls.push({ command, args });
        if (args.transactionId !== transactionId || state !== 'replaced') throw new Error('不是当前替换事务');
        if (controls.failRollback) throw new Error('回收站载荷已变化，不能回滚');
        state = 'rolled_back';
        return null;
      }
      const result = await invoke(command, args);
      if (command === 'list_games') return [...result, { ...result[0], id: 'other-game', name: '另一个游戏' }];
      if (command === 'get_game_adapter') return { ...result, modTypes: [...result.modTypes, { id: 'other-mod', name: '其他类型', root: 'mods', notes: '' }] };
      if (command === 'list_downloads' && controls.nexus) return result.map((record: Record<string, unknown>) => ({ ...record, sourceId: 'nexus', projectId: 'game-domain/123' }));
      if (command === 'get_file_plan' && args.planId === transactionId) return plan;
      if (command === 'plan_install' && args.request.replaceInstallId) {
        if (args.request.gameId !== 'game-fixture' || args.request.replaceInstallId !== previous) throw new Error('不是选中的旧版本');
        if (controls.extraCandidateStatus) throw new Error('同一游戏还有此项目的其他安装，请先解决重复版本');
        return { plan, warnings: [] };
      }
      if (command !== 'list_installs') return result;
      if (controls.blockGame === args.gameId) {
        controls.blockGame = '';
        await new Promise<void>(resolve => { controls.release = resolve; });
        controls.release = undefined;
      }
      if (controls.failRead) throw new Error('无法读取安装记录');
      if (args.gameId !== 'game-fixture') return [];

      const [base] = result;
      const old = { ...base, id: previous, name: '旧版 Fixture', downloadOrigin: origin,
        status: state === 'replaced' ? 'superseded' : 'enabled', metadata: { transactionId: 'txn-old-install', modType: 'java-mod' } };
      if (controls.nexus) return [
        { ...old, status: controls.legacyNexusActive ? 'enabled' : 'superseded', downloadOrigin: { ...origin, sourceId: 'nexus', projectId: '123' } },
        { ...old, id: 'different-domain', downloadOrigin: { ...origin, sourceId: 'nexus', projectId: 'other-domain/123' } },
      ];
      if (state !== 'original') return [old, { ...base, id: 'replacement-release', name: '新版 Fixture',
        status: controls.supersedeReplacement ? 'superseded' : state === 'replaced' ? 'enabled' : 'rolled_back',
        downloadOrigin: { ...origin, versionId }, metadata: { replacesInstallId: previous, transactionId, modType: 'java-mod' } }];

      return [
        ...(controls.hideCandidates || controls.sameVersion ? [] : [old]),
        ...(controls.extraCandidateStatus ? [{ ...old, id: 'other-release', name: '另一个旧版', status: controls.extraCandidateStatus, downloadOrigin: { ...origin, versionId: 'PRIOR002' } }] : []),
        ...(controls.sameVersion ? [{ ...old, id: 'same-release', downloadOrigin: { ...origin, versionId } }] : []),
        { ...old, id: 'already-replaced', status: 'superseded' },
        { ...old, id: 'legacy-label', downloadOrigin: null, metadata: { sourceModId: 'fixture' } },
        { ...old, id: 'wrong-project', downloadOrigin: { ...origin, projectId: 'Fixture' } },
        { ...old, id: 'wrong-source', downloadOrigin: { ...origin, sourceId: 'curseforge' } },
        { ...old, id: 'wrong-game', gameId: 'other-game' },
      ];
    };
  }, initialState);
}

async function openInstall(page: Page) {
  await page.getByRole('button', { name: 'Mod 发现', exact: true }).click();
  await page.getByRole('button', { name: '安装预览', exact: true }).click();
  return page.getByRole('dialog', { name: '下载归档安装预览', exact: true });
}

async function chooseGame(dialog: Locator, game = 'game-fixture') {
  await dialog.getByLabel('目标游戏', { exact: true }).selectOption(game);
  await dialog.getByLabel('适配包 Mod 类型', { exact: true }).selectOption('java-mod');
}

async function openGameMods(page: Page) {
  await page.getByRole('button', { name: '游戏库', exact: true }).click();
  await page.getByRole('button').filter({ hasText: 'Minecraft Java Edition' }).click();
  await page.getByRole('tab', { name: '已装 Mod', exact: true }).click();
}

function replaceChoice(dialog: Locator) {
  return dialog.getByLabel('要替换的已装版本', { exact: true });
}

function generate(dialog: Locator) {
  return dialog.getByRole('button', { name: '生成变更预览', exact: true });
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

test('replacement is explicit and only exact eligible installs enter the shared preview', async ({ page }, info) => {
  await setup(page);
  const dialog = await openInstall(page);
  expect(await calls(page, 'list_installs')).toHaveLength(0);
  await chooseGame(dialog);
  const choice = replaceChoice(dialog);
  await expect(choice).toBeVisible();
  expect(await choice.locator('option').evaluateAll(options => options.map(option => (option as HTMLOptionElement).value))).toEqual(['', 'old-release']);
  await expect(choice).toHaveValue('');
  await expect(dialog).toContainText('该项目已有其他版本');
  await expect(generate(dialog)).toBeDisabled();
  expect(await calls(page, 'plan_install')).toHaveLength(0);
  await choice.selectOption('old-release');
  await expect(dialog).toContainText('已选版本 ID：OLD001');
  const type = dialog.getByLabel('适配包 Mod 类型', { exact: true });
  await type.selectOption('other-mod');
  await expect(dialog).toContainText('原版本使用「Java 加载器模组」类型，请选择相同的 Mod 类型。');
  await expect(generate(dialog)).toBeDisabled();
  expect(await calls(page, 'plan_install')).toHaveLength(0);
  await type.selectOption('java-mod');
  await recordDialog(dialog, info, 'light-replacement-selector');
  await generate(dialog).click();
  const review = page.getByRole('dialog', { name: '确认文件变更', exact: true });
  await expect(review).toContainText('/sandbox/data/trash/replacement/payload');
  await expect(review).toContainText('/sandbox/minecraft/mods/shared.jar');
  await expect(review).toContainText('/sandbox/minecraft/mods/added.jar');
  expect(await calls(page, 'plan_install')).toMatchObject([{ request: {
    replaceInstallId: 'old-release', expectedSha256: 'b'.repeat(64), source: 'modrinth', sourceModId: 'fixture',
  } }]);
  expect(await calls(page, 'apply_plan')).toHaveLength(0);
  await review.getByRole('button', { name: '取消', exact: true }).click();
  expect(await calls(page, 'apply_plan')).toHaveLength(0);
});

test('duplicate active versions block replacement even when one version is partially restored', async ({ page }) => {
  await setup(page);
  const dialog = await openInstall(page);
  for (const status of ['enabled', 'partially_restored']) {
    await page.evaluate(value => { (window as any).__AMM_TEST__.replacements.extraCandidateStatus = value; }, status);
    await chooseGame(dialog);
    await expect(dialog).toContainText('该项目有多个有效安装记录，请先处理重复版本，才能替换。');
    await expect(replaceChoice(dialog)).toHaveCount(0);
    await expect(generate(dialog)).toBeDisabled();
    expect(await calls(page, 'plan_install')).toHaveLength(0);
    await dialog.getByLabel('目标游戏', { exact: true }).selectOption('');
  }
});

test('Nexus replacement does not infer a game domain for a legacy numeric project', async ({ page }) => {
  await setup(page);
  await page.evaluate(() => { (window as any).__AMM_TEST__.replacements.nexus = true; });
  const dialog = await openInstall(page);
  await chooseGame(dialog);
  await expect(dialog).toContainText('旧 ID 不含游戏域名，无法确认是否为同一项目');
  await expect(replaceChoice(dialog)).toHaveCount(0);
  await expect(generate(dialog)).toBeDisabled();
  expect(await calls(page, 'plan_install')).toHaveLength(0);

  await dialog.getByLabel('目标游戏', { exact: true }).selectOption('');
  await page.evaluate(() => { (window as any).__AMM_TEST__.replacements.legacyNexusActive = false; });
  await chooseGame(dialog);
  await expect(dialog).toContainText('普通安装');
  await expect(replaceChoice(dialog)).toHaveCount(0);
  await expect(generate(dialog)).toBeEnabled();
});

test('identical installed version is blocked and an unrelated game uses the ordinary request', async ({ page }, info) => {
  await setup(page, 'dark');
  await page.evaluate(() => { (window as any).__AMM_TEST__.replacements.sameVersion = true; });
  const dialog = await openInstall(page);
  await chooseGame(dialog);
  await expect(dialog).toContainText('该项目的此版本已安装，不能重复安装。');
  await expect(generate(dialog)).toBeDisabled();
  await expect(replaceChoice(dialog)).toHaveCount(0);
  expect(await calls(page, 'plan_install')).toHaveLength(0);
  await page.evaluate(() => { (window as any).__AMM_TEST__.replacements.sameVersion = false; });
  await dialog.getByLabel('目标游戏', { exact: true }).selectOption('');
  await chooseGame(dialog);
  await replaceChoice(dialog).selectOption('old-release');
  await recordDialog(dialog, info, 'dark-replacement-selector');
  await chooseGame(dialog, 'other-game');
  await expect(dialog).toContainText('普通安装');
  await expect(replaceChoice(dialog)).toHaveCount(0);
  await generate(dialog).click();
  await expect(page.getByRole('dialog', { name: '确认文件变更', exact: true })).toBeVisible();
  const [ordinary] = await calls(page, 'plan_install');
  expect(ordinary.request).toMatchObject({ gameId: 'other-game', modType: 'java-mod', expectedSha256: 'b'.repeat(64) });
  expect(ordinary.request).not.toHaveProperty('replaceInstallId');
});

test('a failed lookup and a disappearing target never fall back to a normal install', async ({ page }) => {
  await setup(page);
  await page.evaluate(() => { (window as any).__AMM_TEST__.replacements.failRead = true; });
  const dialog = await openInstall(page);
  await chooseGame(dialog);
  await expect(dialog.getByRole('alert')).toContainText('无法读取安装记录');
  await expect(generate(dialog)).toBeDisabled();
  await page.evaluate(() => { (window as any).__AMM_TEST__.replacements.failRead = false; });
  await dialog.getByRole('button', { name: '重试读取已装版本', exact: true }).click();
  await replaceChoice(dialog).selectOption('old-release');
  await page.evaluate(() => { (window as any).__AMM_TEST__.replacements.blockGame = 'game-fixture'; });
  await generate(dialog).click();
  await expect(dialog.getByLabel('目标游戏', { exact: true })).toBeDisabled();
  await expect(dialog.getByRole('button', { name: '取消', exact: true })).toBeDisabled();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeVisible();
  await expect.poll(() => page.evaluate(() => !!(window as any).__AMM_TEST__.replacements.release)).toBe(true);
  await page.evaluate(() => {
    const controls = (window as any).__AMM_TEST__.replacements;
    controls.hideCandidates = true;
    controls.release();
  });
  await expect(dialog.getByRole('alert')).toContainText('安装记录已变化');
  await expect(generate(dialog)).toBeDisabled();
  expect(await calls(page, 'plan_install')).toHaveLength(0);
  await dialog.getByRole('button', { name: '清除已失效的替换选择', exact: true }).click();
  await expect(generate(dialog)).toBeEnabled();
});

test('rollback reads the original replacement, then confirms separately and refreshes rows', async ({ page }, info) => {
  await setup(page, 'dark');
  const dialog = await openInstall(page);
  await chooseGame(dialog);
  await replaceChoice(dialog).selectOption('old-release');
  await generate(dialog).click();
  await page.getByRole('button', { name: '批准并执行', exact: true }).click();
  await openGameMods(page);

  const old = page.getByRole('row').filter({ hasText: '旧版 Fixture' });
  const replacement = page.getByRole('row').filter({ hasText: '新版 Fixture' });
  const openRollback = replacement.getByRole('button', { name: '预览回滚替换', exact: true });
  await expect(old).toContainText('已被替换');
  await expect(old.getByRole('button', { name: '预览卸载', exact: true })).toBeDisabled();
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
  await expect(old).toContainText('已启用');
  await expect(replacement).toContainText('已回滚');
  await expect(openRollback).toHaveCount(0);
  expect(await calls(page, 'rollback_transaction')).toEqual([{ transactionId: 'txn-replacement-fixture' }, { transactionId: 'txn-replacement-fixture' }]);
  expect(await calls(page, 'apply_plan')).toEqual([{ planId: 'txn-replacement-fixture' }]);
});

test('rollback refuses a replacement superseded while its original plan is open', async ({ page }) => {
  await setup(page, 'light', 'replaced');
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
