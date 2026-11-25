import { expect, test, type Locator, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

function calls(page: Page, command: string) {
  return page.evaluate((name) => (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === name), command);
}

async function openApp(page: Page, options: Parameters<typeof installMockTauri>[1] = {}) {
  await installMockTauri(page, {holdAgentTurn: true, ...options});
  await page.goto("/");
}

function composerInput(page: Page) {
  return page.locator("textarea[aria-label]").last();
}

async function datalistValues(input: Locator) {
  return input.evaluate((element) => {
    const listId = element.getAttribute("list");
    const list = listId ? document.getElementById(listId) as HTMLDataListElement | null : null;
    return Array.from(list?.options ?? [], (option) => option.value);
  });
}

async function expectFolderDialogOptions(page: Page, count: number) {
  const dialogs = await calls(page, "plugin:dialog|open");
  expect(dialogs).toHaveLength(count);
  for (const call of dialogs) {
    expect(call.args.options).toMatchObject({directory: true, multiple: false});
  }
}

async function expectSelectedText(input: Locator, text: string) {
  await expect.poll(() => input.evaluate((element) => {
    const field = element as HTMLTextAreaElement;
    return field.value.slice(field.selectionStart, field.selectionEnd);
  })).toBe(text);
}

test("使用模板时可输入目录或选择文件夹，取消不覆盖输入，Escape 不改草稿", async ({page}) => {
  await openApp(page);
  const input = composerInput(page);
  const presets = page.getByRole("group", {name: "预设提示词"});

  await presets.getByRole("button", {name: "添加游戏", exact: true}).click();
  let dialog = page.getByRole("dialog", {name: "使用模板", exact: true});
  await expect(dialog).toBeVisible();
  const gameDirectory = dialog.getByRole("textbox", {name: "游戏目录", exact: true});
  await gameDirectory.fill("/sandbox/typed-game");
  await dialog.getByRole("button", {name: "选择文件夹", exact: true}).click();
  await expect(gameDirectory).toHaveValue("/sandbox/new-game");
  await gameDirectory.fill("/sandbox/keep-this-game");
  await dialog.getByRole("button", {name: "选择文件夹", exact: true}).click();
  await expect(gameDirectory).toHaveValue("/sandbox/keep-this-game");
  await expectFolderDialogOptions(page, 2);

  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(input).toHaveValue("");

  await presets.getByRole("button", {name: "迁移旧管理器的 Mod", exact: true}).click();
  dialog = page.getByRole("dialog", {name: "使用模板", exact: true});
  await expect(dialog).toBeVisible();
  const oldDirectory = dialog.getByRole("textbox", {name: "旧 Mod 存储目录", exact: true});
  await oldDirectory.fill("/sandbox/old-manager-mods");
  await dialog.getByRole("button", {name: "选择文件夹", exact: true}).click();
  await expect(oldDirectory).toHaveValue("/sandbox/old-manager-mods");
  await expectFolderDialogOptions(page, 3);
  await dialog.getByRole("button", {name: "填入草稿", exact: true}).click();

  await expect(dialog).toBeHidden();
  await expect(input).toHaveValue("将 /sandbox/old-manager-mods 中的 Mod 从旧管理器迁移到新管理器。");
  await expect(input).toBeFocused();
  await expect(presets).toBeHidden();
  expect(await calls(page, "create_conversation")).toHaveLength(0);
  expect(await calls(page, "start_turn")).toHaveLength(0);
});

test("安装预设直接填入并选中游戏占位符，可直接替换且不会自动提交", async ({page}) => {
  await openApp(page);
  const input = composerInput(page);
  await page.getByRole("group", {name: "预设提示词"})
    .getByRole("button", {name: "查找并安装 Mod", exact: true}).click();
  const template = "帮我安装一些好玩的[游戏名]mod";
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(input).toHaveValue(template);
  await expect(input).toBeFocused();
  await expectSelectedText(input, "[游戏名]");
  await page.keyboard.insertText("Minecraft Java Edition");
  await expect(input).toHaveValue("帮我安装一些好玩的Minecraft Java Editionmod");
  expect(await calls(page, "create_conversation")).toHaveLength(0);
  expect(await calls(page, "start_turn")).toHaveLength(0);
  expect(await calls(page, "list_mod_library")).toHaveLength(0);
});

test("点击「使用模板」仅建议游戏名，填入时替换占位符且不发送", async ({page}) => {
  await openApp(page);
  const input = composerInput(page);
  await page.getByRole("group", {name: "预设提示词"})
    .getByRole("button", {name: "查找并安装 Mod", exact: true}).click();
  const template = "帮我安装一些好玩的[游戏名]mod";
  await expect(input).toHaveValue(template);

  await page.getByRole("button", {name: "使用模板", exact: true}).click();
  const dialog = page.getByRole("dialog", {name: "使用模板", exact: true});
  await expect(dialog).toBeVisible();
  const gameName = dialog.getByLabel("游戏名", {exact: true});
  await expect(dialog.getByRole("combobox", {name: "游戏名", exact: true})).toHaveCount(1);
  await expect(dialog.getByLabel("Mod 名称", {exact: true})).toHaveCount(0);
  await expect(gameName).toHaveAttribute("list", /.+/);
  await expect.poll(() => datalistValues(gameName)).toContain("Minecraft Java Edition");
  expect(await calls(page, "list_mod_library")).toHaveLength(0);

  await gameName.fill("Minecraft Java Edition");
  await dialog.getByRole("button", {name: "填入草稿", exact: true}).click();
  await expect(dialog).toBeHidden();
  await expect(input).toHaveValue("帮我安装一些好玩的Minecraft Java Editionmod");
  await expect(input).toBeFocused();
  expect(await calls(page, "create_conversation")).toHaveLength(0);
  expect(await calls(page, "start_turn")).toHaveLength(0);
  expect(await calls(page, "list_mod_library")).toHaveLength(0);
});

test("直接编辑或取消未填写的模板都保留提示词", async ({page}) => {
  await openApp(page);
  const input = composerInput(page);
  await page.getByRole("group", {name: "预设提示词"})
    .getByRole("button", {name: "查找并安装 Mod", exact: true}).click();
  const template = "帮我安装一些好玩的[游戏名]mod";
  await expect(input).toHaveValue(template);

  const assist = page.getByRole("button", {name: "使用模板", exact: true});
  await assist.click();
  let dialog = page.getByRole("dialog", {name: "使用模板", exact: true});
  await expect(dialog.getByRole("button", {name: "填入草稿", exact: true})).toBeDisabled();
  await dialog.getByRole("button", {name: "直接编辑提示词", exact: true}).click();
  await expect(dialog).toBeHidden();
  await expect(input).toHaveValue(template);

  await assist.click();
  dialog = page.getByRole("dialog", {name: "使用模板", exact: true});
  await expect(dialog).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(input).toHaveValue(template);
  expect(await calls(page, "create_conversation")).toHaveLength(0);
  expect(await calls(page, "start_turn")).toHaveLength(0);
});

test("英文安装预设使用指定模板并选中游戏名占位符", async ({page}) => {
  await openApp(page, {appLocale: "en"});
  await page.getByRole("group", {name: "Preset prompts"})
    .getByRole("button", {name: "Find and install a mod", exact: true}).click();
  const input = composerInput(page);
  const prompt = "Help me install some fun mods for [game name].";
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(input).toHaveValue(prompt);
  await expect(input).toBeFocused();
  await expectSelectedText(input, "[game name]");
  expect(await calls(page, "create_conversation")).toHaveLength(0);
  expect(await calls(page, "start_turn")).toHaveLength(0);
});