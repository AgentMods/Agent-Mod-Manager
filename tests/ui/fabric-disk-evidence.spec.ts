import { expect, test, type Page } from '@playwright/test';
import { installMockTauri } from './mock-tauri';

type FabricDisk = {
  directory: string;
  status: 'scanned' | 'missing' | 'partial' | 'unavailable';
  jars: {
    file: string;
    metadataStatus: 'present' | 'absent' | 'invalid' | 'limited' | 'unreadable';
    id: string | null;
    version: string | null;
    ownership: 'recorded' | 'unrecorded' | 'ambiguous' | 'unverified';
    logRelation: {
      status: 'exact' | 'version_differs' | 'ambiguous' | 'not_listed' | 'unverified';
      line: number | null;
      version: string | null;
    };
  }[];
};

function disk(status: FabricDisk['status'], jars: FabricDisk['jars'] = []): FabricDisk {
  return { directory: 'mods/', status, jars };
}

async function setup(page: Page) {
  await installMockTauri(page);
  await page.goto('/');
  await page.getByRole('button', {name: '游戏库', exact: true}).click();
  const heading = page.getByRole('heading', { name: 'Minecraft Java Edition', exact: true });
  await page.getByRole('button').filter({ has: heading }).click();
  await page.getByRole('tab', { name: '诊断', exact: true }).click();
  await expect(heading).toBeVisible();
  await page.evaluate(() => {
    const target = window as any;
    const mock = target.__AMM_TEST__;
    const transport = target.__TAURI_INTERNALS__;
    mock.fabricDisk = null;
    const invoke = transport.invoke;
    transport.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      if (command !== 'diagnose_game') return invoke(command, args);
      mock.calls.push({ command, args });
      return {
        status: 'unknown', causes: [], unknowns: [], repairSuggestions: [], evidence: [],
        fabricDisk: mock.fabricDisk,
      };
    };
  });
}

async function diagnose(page: Page, report: FabricDisk) {
  await page.evaluate(value => { (window as any).__AMM_TEST__.fabricDisk = value; }, report);
  await page.getByRole('button', { name: '开始诊断', exact: true }).click();
  return page.getByRole('region', { name: 'Fabric 磁盘证据', exact: true });
}

async function setTheme(page: Page, theme: 'light' | 'dark') {
  const html = page.locator('html');
  if (await html.getAttribute('data-theme') !== theme) {
    await page.getByRole('button', { name: theme === 'dark' ? '切换暗色主题' : '切换亮色主题', exact: true }).click();
  }
  await expect(html).toHaveAttribute('data-theme', theme);
}

async function expectNoWriteCalls(page: Page, expectedDiagnoses: number) {
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls as { command: string; args: unknown }[]);
  expect(calls.filter(call => call.command === 'diagnose_game')).toHaveLength(expectedDiagnoses);
  expect(calls.filter(call => /^(plan_|apply_|write_|launch_|trash_|restore_|purge_|rollback_|snapshot_|register_|set_|delete_|remove_|import_|install_|uninstall_)/.test(call.command))).toEqual([]);
}

test('compares historical exact and differing versions, ownership, duplicate IDs, and keyboard disclosure', async ({ page }) => {
  await setup(page);
  const section = await diagnose(page, disk('scanned', [
    {
      file: 'sodium.jar', metadataStatus: 'present', id: 'sodium', version: '0.6.0', ownership: 'recorded',
      logRelation: { status: 'exact', line: 37, version: '0.6.0' },
    },
    {
      file: 'indium.jar', metadataStatus: 'present', id: 'indium', version: '1.0.36', ownership: 'unrecorded',
      logRelation: { status: 'version_differs', line: 45, version: '1.0.35' },
    },
    {
      file: 'fabric-api-copy-a.jar', metadataStatus: 'present', id: 'fabric-api', version: '0.115.0', ownership: 'ambiguous',
      logRelation: { status: 'ambiguous', line: null, version: null },
    },
    {
      file: `fabric-api-${'long-evidence-name-'.repeat(4)}.jar`, metadataStatus: 'present', id: 'fabric-api', version: '0.115.0', ownership: 'unverified',
      logRelation: { status: 'ambiguous', line: null, version: null },
    },
  ]));

  const rows = section.getByRole('list', { name: 'Fabric JAR 对照清单', exact: true });
  const exactRow = rows.getByRole('listitem').filter({ hasText: 'sodium.jar' });
  const differingRow = rows.getByRole('listitem').filter({ hasText: 'indium.jar' });
  await expect(exactRow.getByText('ID 与版本一致', { exact: true })).toBeVisible();
  await expect(exactRow.getByText('第 37 行', { exact: true })).toBeVisible();
  await expect(differingRow.getByText('日志版本不同', { exact: true })).toBeVisible();
  await expect(differingRow.getByText('第 45 行 · 1.0.35', { exact: true })).toBeVisible();
  await expect(section.getByText('有启用记录', { exact: true })).toBeVisible();
  await expect(section.getByText('无对应启用记录', { exact: true })).toBeVisible();
  await expect(section.getByText('多条启用记录', { exact: true })).toBeVisible();
  await expect(section.getByText('管理记录未核实', { exact: true })).toBeVisible();
  await expect(section.getByText('多个 JAR 声明相同 ID', { exact: true })).toHaveCount(2);

  const disclosure = section.locator('summary').filter({ hasText: /^磁盘 JAR 对照（4）$/ });
  await expect(rows).toBeVisible();
  await disclosure.press('Enter');
  await expect(rows).toBeHidden();
  await disclosure.press('Space');
  await expect(rows).toBeVisible();

  for (const theme of ['light', 'dark'] as const) {
    await setTheme(page, theme);
    expect(await page.locator('main').evaluate(main => main.scrollWidth <= main.clientWidth + 1)).toBe(true);
    expect(await section.evaluate(element => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
  }
  await expectNoWriteCalls(page, 1);
});

test('distinguishes incomplete evidence, unsupported metadata, and empty, missing, or unavailable scans', async ({ page }) => {
  await setup(page);
  const partial = await diagnose(page, disk('partial', [
    {
      file: 'duplicate-a.jar', metadataStatus: 'present', id: 'duplicate-id', version: '1.0.0', ownership: 'recorded',
      logRelation: { status: 'ambiguous', line: null, version: null },
    },
    {
      file: 'duplicate-b.jar', metadataStatus: 'present', id: 'duplicate-id', version: '1.0.0', ownership: 'unrecorded',
      logRelation: { status: 'ambiguous', line: null, version: null },
    },
    {
      file: 'old-format.jar', metadataStatus: 'absent', id: null, version: null, ownership: 'unverified',
      logRelation: { status: 'unverified', line: null, version: null },
    },
    {
      file: 'unsupported.jar', metadataStatus: 'invalid', id: null, version: null, ownership: 'unverified',
      logRelation: { status: 'unverified', line: null, version: null },
    },
    {
      file: 'large.jar', metadataStatus: 'limited', id: null, version: null, ownership: 'unverified',
      logRelation: { status: 'unverified', line: null, version: null },
    },
    {
      file: 'unreadable.jar', metadataStatus: 'unreadable', id: null, version: null, ownership: 'unverified',
      logRelation: { status: 'unverified', line: null, version: null },
    },
  ]));
  await expect(partial.getByText('仅检查了部分证据', { exact: true })).toBeVisible();
  await expect(partial.getByText('未找到 Fabric 元数据', { exact: true })).toBeVisible();
  await expect(partial.getByText('归档或元数据不受支持', { exact: true })).toBeVisible();
  await expect(partial.getByText('超过读取上限', { exact: true })).toBeVisible();
  await expect(partial.getByText('无法安全读取', { exact: true })).toBeVisible();
  await expect(partial.getByText('多个 JAR 声明相同 ID', { exact: true })).toHaveCount(2);

  const empty = await diagnose(page, disk('scanned'));
  await expect(empty.getByText('未发现可检查的直属 JAR。', { exact: true })).toBeVisible();
  await expect(empty.locator('details')).toHaveCount(0);

  const missing = await diagnose(page, disk('missing'));
  await expect(missing.getByText('未找到 mods 目录', { exact: true })).toBeVisible();
  await expect(missing.getByText('未发现可检查的直属 JAR。', { exact: true })).toHaveCount(0);

  const unavailable = await diagnose(page, disk('unavailable'));
  await expect(unavailable.getByText('无法核实 mods 目录', { exact: true })).toBeVisible();
  await expect(unavailable.getByText('未发现可检查的直属 JAR。', { exact: true })).toHaveCount(0);
  await expectNoWriteCalls(page, 4);
});
