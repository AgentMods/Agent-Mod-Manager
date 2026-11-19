import { expect, test, type Page } from '@playwright/test';
import { installMockTauri } from './mock-tauri';

const fileName = `${'longExternalDependency'.repeat(8)}.jar`;
const declarations = [
  {dependency_type: 'required', project_id: 'DepMod01', version_id: 'PinVers1', file_name: null},
  {dependency_type: 'optional', project_id: 'DepMod02', version_id: null, file_name: null},
  {dependency_type: 'incompatible', project_id: null, version_id: 'ConfVers', file_name: null},
  {dependency_type: 'embedded', project_id: null, version_id: null, file_name: fileName},
];

async function resolveFixture(page: Page, dependencies: typeof declarations | null) {
  await installMockTauri(page);
  await page.goto('/');
  await page.evaluate(dependencies => {
    const target = window as any;
    const invoke = target.__TAURI_INTERNALS__.invoke;
    const status = {source_id: 'modrinth', state: 'available', message: 'Modrinth'};
    const mod = {source_id: 'modrinth', project_id: 'Sample01', game_id: 'minecraft', name: 'Modrinth Fixture', summary: 'Fixture description', author: null, homepage_url: 'https://modrinth.com/mod/Sample01', icon_url: null, downloads: 42};
    target.__TAURI_INTERNALS__.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      if (!['list_mod_sources', 'search_mods', 'resolve_mod'].includes(command)) return invoke(command, args);
      target.__AMM_TEST__.calls.push({command, args});
      if (command === 'list_mod_sources') return [status];
      if (command === 'search_mods') return {mods: [mod], sources: [status]};
      return {status, homepage_url: mod.homepage_url, dependencies, download: {
        source_id: mod.source_id, project_id: mod.project_id, version_id: 'Version1', name: mod.name,
        version: '1.0.0', filename: 'sample.jar', size: 650, resolution_id: 'sample-resolution',
        hashes: {sha512: 'a'.repeat(128)}, homepage_url: mod.homepage_url, warnings: [],
      }};
    };
  }, dependencies);
  await page.getByRole('button', {name: 'Mod 发现', exact: true}).click();
  await page.getByLabel('搜索 Mod', {exact: true}).fill('Fixture');
  await page.getByRole('button', {name: '搜索', exact: true}).click();
  await page.getByRole('button').filter({has: page.getByRole('heading', {name: 'Modrinth Fixture', exact: true})}).click();
  return page.getByTestId('discover-page').locator('aside');
}

test('Modrinth lists the selected version declarations without triggering file actions', async ({page}) => {
  const inspector = await resolveFixture(page, declarations);
  await inspector.getByText('来源声明的直接依赖（4）', {exact: true}).click();
  for (const label of ['必需', '可选', '不兼容', '已内嵌', '项目 DepMod01', '版本 PinVers1', '版本 ConfVers', fileName]) {
    await expect(inspector.getByText(label, {exact: true})).toBeVisible();
  }
  const dark = page.getByRole('button', {name: '切换暗色主题', exact: true});
  const light = page.getByRole('button', {name: '切换亮色主题', exact: true});
  if (await light.isVisible()) await light.click();
  for (const theme of ['light', 'dark']) {
    if (theme === 'dark') await dark.click();
    const fits = await inspector.locator('.source-dependencies').evaluate(panel =>
      panel.scrollWidth <= panel.clientWidth + 1 && panel.getBoundingClientRect().right <= innerWidth + 1,
    );
    expect(fits, `dependency details fit in ${theme} mode`).toBe(true);
  }
  const writes = await page.evaluate(() => (window as any).__AMM_TEST__.calls.filter((call: any) => ['download_mod', 'plan_install', 'apply_plan'].includes(call.command)));
  expect(writes).toHaveLength(0);
});

for (const [name, dependencies, label] of [
  ['explicitly empty', [], '来源未声明直接依赖'],
  ['unavailable', null, '来源依赖未知'],
] as const) {
  test(`Modrinth displays ${name} dependency metadata without conflating states`, async ({page}) => {
    const inspector = await resolveFixture(page, dependencies === null ? null : [...dependencies]);
    await expect(inspector.getByText(label, {exact: true})).toBeVisible();
    await expect(inspector.getByRole('button', {name: '下载归档', exact: true})).toBeVisible();
  });
}
