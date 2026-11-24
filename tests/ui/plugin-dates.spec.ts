import { expect, test, type Locator, type Page } from "@playwright/test";
import { installMockTauri, PLUGIN_CREATED_AT_FIXTURE, PLUGIN_UPDATED_AT_FIXTURE } from "./mock-tauri";

async function openPluginCenter(page: Page, fixture: Parameters<typeof installMockTauri>[1] = {}) {
  await installMockTauri(page, fixture);
  await page.goto("/");
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button", {name: "插件中心", exact: true}).click();
  await expect(page.getByTestId("plugins-page")).toBeVisible();
}

async function columnIndex(table: Locator, name: string) {
  const header = table.getByRole("columnheader", {name, exact: true});
  await expect(header).toBeVisible();
  const index = await header.evaluate(element => Array.from(element.parentElement?.children ?? []).indexOf(element));
  expect(index, `missing table column: ${name}`).toBeGreaterThanOrEqual(0);
  return index;
}

async function expectFormattedTime(page: Page, locator: Locator, timestamp: string) {
  const expected = await page.evaluate(value => new Date(value).toLocaleString(document.documentElement.lang || "zh-CN", {dateStyle: "short", timeStyle: "short"}), timestamp);
  await expect(locator).toHaveAttribute("datetime", timestamp);
  await expect(locator).toHaveText(expected);
}

async function expectNarrowTableUsable(page: Page, table: Locator) {
  const container = table.locator("xpath=..");
  const metrics = await container.evaluate(element => {
    const rect = element.getBoundingClientRect();
    return {
      clientWidth: element.clientWidth,
      scrollWidth: element.scrollWidth,
      overflowX: getComputedStyle(element).overflowX,
      right: rect.right,
      viewportWidth: window.innerWidth,
      documentWidth: document.documentElement.scrollWidth,
    };
  });
  expect(["auto", "scroll"]).toContain(metrics.overflowX);
  expect(metrics.right).toBeLessThanOrEqual(metrics.viewportWidth + 1);
  expect(metrics.documentWidth).toBeLessThanOrEqual(metrics.viewportWidth + 1);

  if (metrics.scrollWidth > metrics.clientWidth) {
    await container.evaluate(element => { element.scrollLeft = element.scrollWidth; });
    await expect(table.getByRole("columnheader", {name: "操作", exact: true})).toBeInViewport();
  }
}

test("installed and draft tables show distinct creation and update times, including selected package details", async ({page}, testInfo) => {
  await openPluginCenter(page, {withExternalAdapter: true});
  const pluginPage = page.getByTestId("plugins-page");

  const installedTable = pluginPage.getByRole("table");
  const installedRow = installedTable.getByRole("row").filter({hasText: "Minecraft Java Edition"});
  await expect(installedRow).toBeVisible();
  const installedUpdatedAt = installedRow.getByRole("cell").nth(await columnIndex(installedTable, "更新时间")).locator("time");
  await expectFormattedTime(page, installedUpdatedAt, PLUGIN_UPDATED_AT_FIXTURE);

  await installedRow.getByRole("cell").first().getByRole("button").click();
  const details = pluginPage.locator("aside dl");
  const detailsUpdatedAt = details.locator("time");
  await expect(details.locator("dt", {hasText: "更新时间"})).toHaveText("更新时间");
  await expectFormattedTime(page, detailsUpdatedAt, PLUGIN_UPDATED_AT_FIXTURE);

  await page.getByRole("tab", {name: /^草稿/}).click();
  const draftsTable = pluginPage.getByRole("table");
  const draftRow = draftsTable.getByRole("row").filter({hasText: "fixture-game"});
  await expect(draftRow).toBeVisible();
  const createdAtCell = draftRow.getByRole("cell").nth(await columnIndex(draftsTable, "创建时间"));
  const updatedAtCell = draftRow.getByRole("cell").nth(await columnIndex(draftsTable, "更新时间"));
  const expectedCreatedAt = await page.evaluate(value => new Date(value).toLocaleString(document.documentElement.lang || "zh-CN", {dateStyle: "short", timeStyle: "short"}), PLUGIN_CREATED_AT_FIXTURE);
  await expect(createdAtCell).toHaveText(expectedCreatedAt);
  await expect(updatedAtCell.locator("time")).toHaveAttribute("datetime", PLUGIN_UPDATED_AT_FIXTURE);
  await expectFormattedTime(page, updatedAtCell.locator("time"), PLUGIN_UPDATED_AT_FIXTURE);
  expect(expectedCreatedAt).not.toBe(await updatedAtCell.innerText());

  if (testInfo.project.name === "narrow-390") {
    await expectNarrowTableUsable(page, draftsTable);
    await page.getByRole("tab", {name: "已安装", exact: true}).click();
    await expectNarrowTableUsable(page, pluginPage.getByRole("table"));
  }
});

test("null and missing updatedAt render a dash in tables and selected package details", async ({page}) => {
  await openPluginCenter(page, {withExternalAdapter: true, pluginDateFixture: "null-and-missing"});
  const pluginPage = page.getByTestId("plugins-page");

  const installedTable = pluginPage.getByRole("table");
  const installedRow = installedTable.getByRole("row").filter({hasText: "Minecraft Java Edition"});
  await expect(installedRow).toBeVisible();
  const installedCell = installedRow.getByRole("cell").nth(await columnIndex(installedTable, "更新时间"));
  await expect(installedCell).toHaveText("—");
  await expect(installedCell.locator("time")).toHaveCount(0);

  await installedRow.getByRole("cell").first().getByRole("button").click();
  const details = pluginPage.locator("aside dl");
  const detailsUpdateTerm = details.locator("dt", {hasText: "更新时间"});
  await expect(detailsUpdateTerm).toHaveText("更新时间");
  await expect(detailsUpdateTerm.locator("xpath=following-sibling::dd[1]")).toHaveText("—");
  await expect(details.locator("time")).toHaveCount(0);

  await page.getByRole("tab", {name: /^草稿/}).click();
  const draftsTable = pluginPage.getByRole("table");
  const draftRow = draftsTable.getByRole("row").filter({hasText: "fixture-game"});
  await expect(draftRow).toBeVisible();
  const draftCell = draftRow.getByRole("cell").nth(await columnIndex(draftsTable, "更新时间"));
  await expect(draftCell).toHaveText("—");
  await expect(draftCell.locator("time")).toHaveCount(0);
  await expect(pluginPage.getByText("Invalid Date", {exact: true})).toHaveCount(0);
});
