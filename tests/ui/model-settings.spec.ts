import { expect, test, type Locator, type Page } from "@playwright/test";
import type { AppSettings } from "../../src/lib/types";
import { installMockTauri } from "./mock-tauri";

async function openModelSettings(page: Page) {
  await installMockTauri(page);
  await page.goto("/");
  await page.getByRole("button", {name: "设置", exact: true}).click();
  const section = page.getByRole("region", {name: "模型列表", exact: true});
  const table = section.getByRole("table", {name: "模型列表", exact: true});
  await expect(table).toBeVisible();
  return table;
}

function requestModel(table: Locator, id: string) {
  return table.getByRole("textbox", {name: `${id} 实际请求模型`, exact: true});
}

async function addModel(page: Page, table: Locator, id: string) {
  await page.getByRole("button", {name: "手动添加", exact: true}).click();
  const input = requestModel(table, "新模型");
  await expect(input).toBeFocused();
  await expect(input).toHaveValue("");
  await input.fill(id);
  await expect(requestModel(table, id)).toHaveValue(id);
}

function settingsWrites(page: Page) {
  return page.evaluate(() => {
    const {calls} = (window as typeof window & {
      __AMM_TEST__: {calls: {command: string; args: {settings: AppSettings}}[]};
    }).__AMM_TEST__;
    return calls.filter((call) => call.command === "set_settings").map((call) => call.args.settings);
  });
}

function modelOrder(table: Locator) {
  return table.getByRole("textbox", {name: / 实际请求模型$/}).evaluateAll(
    (inputs) => inputs.map((input) => (input as HTMLInputElement).value),
  );
}

test("手动追加模型后可配置显示名、上下文和全部思考等级", async ({page}) => {
  const table = await openModelSettings(page);
  await addModel(page, table, "fixture-thinking");

  const displayName = table.getByRole("textbox", {name: "fixture-thinking 菜单显示名", exact: true});
  await displayName.fill("推理模型");
  const budget = table.getByRole("spinbutton", {name: "fixture-thinking 上下文预算", exact: true});
  await expect(budget).toHaveValue("128000");
  await budget.fill("160000");

  await table.getByRole("button", {name: "fixture-thinking 思考等级", exact: true}).click();
  const menu = page.getByRole("menu");
  const options = menu.getByRole("menuitemcheckbox");
  const levels = ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"];
  await expect(options).toHaveCount(levels.length);
  for (const level of levels) {
    await expect(menu.getByRole("menuitemcheckbox", {name: new RegExp(`（${level}）$`)})).toHaveAttribute(
      "aria-checked", ["low", "medium", "high"].includes(level) ? "true" : "false",
    );
  }
  await menu.getByRole("menuitemcheckbox", {name: /（ultra）$/}).click();
  await page.keyboard.press("Escape");
  await expect(menu).toBeHidden();

  const defaultModel = page.getByRole("combobox", {name: "默认模型", exact: true});
  await expect(defaultModel.getByRole("option", {name: "推理模型", exact: true})).toHaveAttribute("value", "fixture-thinking");
  await defaultModel.selectOption({label: "推理模型"});
  await expect(defaultModel).toHaveValue("fixture-thinking");
  await page.getByRole("button", {name: "保存设置", exact: true}).click();
  await expect.poll(() => settingsWrites(page)).toEqual([expect.objectContaining({
    model: "fixture-thinking",
    context_budget: 160000,
    models: [
      {id: "mock-model", display_name: "mock-model", context_budget: 32768, reasoning_levels: []},
      {id: "fixture-thinking", display_name: "推理模型", context_budget: 160000, reasoning_levels: ["low", "medium", "high", "ultra"]},
    ],
  })]);
});

test("键盘和原生拖拽可排序，删除默认模型后按新顺序回退", async ({page}) => {
  const table = await openModelSettings(page);
  await addModel(page, table, "fixture-alpha");
  await addModel(page, table, "fixture-beta");
  await table.getByRole("textbox", {name: "fixture-beta 菜单显示名", exact: true}).fill("备用模型");

  const defaultModel = page.getByRole("combobox", {name: "默认模型", exact: true});
  await defaultModel.selectOption("fixture-alpha");
  const move = table.getByRole("button", {name: "移动 备用模型", exact: true});
  await move.press("ArrowUp");
  await expect.poll(() => modelOrder(table)).toEqual(["mock-model", "fixture-beta", "fixture-alpha"]);
  await expect(move).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect.poll(() => modelOrder(table)).toEqual(["mock-model", "fixture-alpha", "fixture-beta"]);
  await expect(move).toBeFocused();

  await move.dragTo(table.getByRole("button", {name: "移动 mock-model", exact: true}));
  await expect.poll(() => modelOrder(table)).toEqual(["fixture-beta", "mock-model", "fixture-alpha"]);
  await expect(defaultModel).toHaveValue("fixture-alpha");

  await table.getByRole("button", {name: "删除 fixture-alpha", exact: true}).click();
  await expect.poll(() => modelOrder(table)).toEqual(["fixture-beta", "mock-model"]);
  await expect(defaultModel).toHaveValue("fixture-beta");
  await page.getByRole("button", {name: "保存设置", exact: true}).click();
  await expect.poll(() => settingsWrites(page)).toEqual([expect.objectContaining({
    model: "fixture-beta",
    models: [
      {id: "fixture-beta", display_name: "备用模型", context_budget: 32768, reasoning_levels: []},
      {id: "mock-model", display_name: "mock-model", context_budget: 32768, reasoning_levels: []},
    ],
  })]);
});

test("设置中的空行和重复 ID 可行内修正，删除最后一行后可保存空列表", async ({page}) => {
  const table = await openModelSettings(page);
  const save = page.getByRole("button", {name: "保存设置", exact: true});
  const defaultModel = page.getByRole("combobox", {name: "默认模型", exact: true});
  await expect(defaultModel).toHaveCount(0);

  await page.getByRole("button", {name: "手动添加", exact: true}).click();
  const row = table.getByRole("row").last();
  const input = row.getByRole("textbox", {name: / 实际请求模型$/});
  await expect(input).toBeFocused();
  await expect(save).toBeDisabled();
  await input.fill("mock-model");
  await expect.poll(() => modelOrder(table)).toEqual(["mock-model", "mock-model"]);
  await expect(save).toBeDisabled();
  expect(await settingsWrites(page)).toEqual([]);

  await input.fill("fixture-beta");
  await expect(save).toBeEnabled();
  await defaultModel.selectOption("fixture-beta");
  await requestModel(table, "fixture-beta").fill("fixture-edited");
  await table.getByRole("textbox", {name: "fixture-edited 菜单显示名", exact: true}).fill("已编辑模型");
  await expect(defaultModel).toHaveValue("fixture-edited");
  await save.click();
  const editedSettings = expect.objectContaining({
    model: "fixture-edited",
    models: expect.arrayContaining([expect.objectContaining({id: "fixture-edited", display_name: "已编辑模型"})]),
  });
  await expect.poll(() => settingsWrites(page)).toEqual([editedSettings]);

  await table.getByRole("button", {name: "删除 已编辑模型", exact: true}).click();
  await expect(defaultModel).toHaveCount(0);
  await table.getByRole("button", {name: "删除 mock-model", exact: true}).click();
  await expect(requestModel(table, "mock-model")).toHaveCount(0);
  await expect(save).toBeEnabled();
  await save.click();
  await expect.poll(() => settingsWrites(page)).toEqual([editedSettings, expect.objectContaining({model: "", models: []})]);
});

test("首次引导需要一个有效模型才能保存并继续", async ({page}) => {
  await installMockTauri(page, {onboarding: true});
  await page.goto("/");
  await page.getByRole("button", {name: "稍后再扫"}).click();
  const save = page.getByRole("button", {name: "保存并继续", exact: true});
  await expect(save).toBeDisabled();

  const table = page.getByRole("table", {name: "模型列表", exact: true});
  await addModel(page, table, "fixture-standard");
  await expect(save).toBeEnabled();
  await table.getByRole("button", {name: "删除 fixture-standard", exact: true}).click();
  await expect(save).toBeDisabled();
  expect(await settingsWrites(page)).toEqual([]);
});
