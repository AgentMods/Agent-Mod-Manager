import { expect, test, type Page } from '@playwright/test';
import { installMockTauri } from './mock-tauri';

type BethesdaEvidence = {
  status: 'scanned' | 'partial' | 'unavailable';
  plugins: {
    name: string;
    path: string;
    enabled: boolean;
    listLine: number;
    headerStatus: 'parsed' | 'missing' | 'unreadable' | 'invalid' | 'limited';
    masters: string[];
    missingMasters: string[];
    unlistedMasters: string[];
    notes: string[];
  }[];
  notes: string[];
};

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
    mock.bethesdaPlugins = null;
    const invoke = transport.invoke;
    transport.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      if (command !== 'diagnose_game') return invoke(command, args);
      mock.calls.push({ command, args });
      return {
        status: 'unknown', causes: [], unknowns: [], repairSuggestions: [], evidence: [],
        bethesdaPlugins: mock.bethesdaPlugins,
      };
    };
  });
}

async function diagnose(page: Page, report: BethesdaEvidence) {
  await page.evaluate(value => { (window as any).__AMM_TEST__.bethesdaPlugins = value; }, report);
  await page.getByRole('button', { name: '开始诊断', exact: true }).click();
  return page.getByRole('region', { name: 'Bethesda 插件头证据', exact: true });
}

async function setTheme(page: Page, theme: 'light' | 'dark') {
  const html = page.locator('html');
  if (await html.getAttribute('data-theme') !== theme) {
    await page.getByRole('button', { name: theme === 'dark' ? '切换暗色主题' : '切换亮色主题', exact: true }).click();
  }
  await expect(html).toHaveAttribute('data-theme', theme);
}

test('shows bounded Bethesda TES4 masters, missing dependencies, and responsive themes', async ({ page }) => {
  await setup(page);
  const section = await diagnose(page, {
    status: 'partial',
    plugins: [
      { name: 'Skyrim.esm', path: 'Data/Skyrim.esm', enabled: true, listLine: 2, headerStatus: 'parsed', masters: [], missingMasters: [], unlistedMasters: [], notes: [] },
      { name: 'NeedsMaster.esp', path: 'Data/NeedsMaster.esp', enabled: true, listLine: 3, headerStatus: 'parsed', masters: ['Skyrim.esm', 'MissingMaster.esm'], missingMasters: ['MissingMaster.esm'], unlistedMasters: [], notes: [] },
      { name: `Invalid-${'very-long-plugin-name-'.repeat(4)}.esp`, path: 'Data/Invalid.esp', enabled: true, listLine: 4, headerStatus: 'invalid', masters: [], missingMasters: [], unlistedMasters: [], notes: ['TES4 头无效'] },
    ],
    notes: ['只读取 Plugins.txt 声明的 loose 插件。'],
  });

  await expect(section.getByText('证据不完整', { exact: true })).toBeVisible();
  await expect(section.getByText('已检查 3 个启用插件', { exact: true })).toBeVisible();
  await expect(section.getByText('发现 1 个缺失主文件', { exact: true })).toBeVisible();
  await expect(section.getByRole('list', { name: 'Bethesda 插件头依赖清单', exact: true })).toBeVisible();
  const missing = section.getByRole('listitem').filter({ hasText: 'NeedsMaster.esp' });
  await expect(missing.getByText('MissingMaster.esm', { exact: true })).toBeVisible();
  await expect(missing.getByText('缺少主文件', { exact: true })).toBeVisible();
  await expect(section.getByText('TES4 头无效', { exact: true })).toBeVisible();

  const disclosure = section.locator('summary').filter({ hasText: /^TES4 主文件依赖（3）$/ });
  await disclosure.press('Enter');
  await expect(section.getByRole('list', { name: 'Bethesda 插件头依赖清单', exact: true })).toBeHidden();
  await disclosure.press('Space');
  await expect(section.getByRole('list', { name: 'Bethesda 插件头依赖清单', exact: true })).toBeVisible();

  for (const theme of ['light', 'dark'] as const) {
    await setTheme(page, theme);
    expect(await page.locator('main').evaluate(main => main.scrollWidth <= main.clientWidth + 1)).toBe(true);
    expect(await section.evaluate(element => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
  }

  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls as { command: string; args: unknown }[]);
  expect(calls.filter(call => call.command === 'diagnose_game')).toHaveLength(1);
  expect(calls.filter(call => /^(plan_|apply_|write_|launch_|trash_|restore_|purge_|rollback_|snapshot_|register_|set_|delete_|remove_|import_|install_|uninstall_)/.test(call.command))).toEqual([]);
});

test('shows disabled and late-listed master evidence without treating it as effective loader state', async ({ page }) => {
  await setup(page);
  const section = await diagnose(page, {
    status: 'partial',
    plugins: [
      { name: 'NeedsLate.esp', path: 'Data/NeedsLate.esp', enabled: true, listLine: 2, loadOrder: 1, headerStatus: 'parsed', masters: ['Late.esm'], missingMasters: [], unlistedMasters: [], disabledMasters: [], mastersAfterPlugin: ['Late.esm'], notes: ['主文件在当前 Plugins.txt 中排在插件之后'] },
      { name: 'NeedsDisabled.esp', path: 'Data/NeedsDisabled.esp', enabled: true, listLine: 4, loadOrder: 2, headerStatus: 'parsed', masters: ['Disabled.esm'], missingMasters: [], unlistedMasters: [], disabledMasters: ['Disabled.esm'], mastersAfterPlugin: [], notes: ['主文件已列出但未启用'] },
    ],
    notes: [],
  });

  await expect(section.getByText('主文件顺序异常 1 个', { exact: true })).toBeVisible();
  await expect(section.getByText('未启用主文件 1 个', { exact: true })).toBeVisible();
  await expect(section.getByText('启用顺序 #1', { exact: true })).toBeVisible();
  await expect(section.getByText('顺序异常', { exact: true })).toBeVisible();
  await expect(section.getByText('未启用', { exact: true })).toBeVisible();
  await expect(section.getByText('加载顺序需核对', { exact: true })).toHaveCount(2);
  await expect(section).toContainText('不代表 MO2/VFS、归档内容或游戏当前加载状态');
});
