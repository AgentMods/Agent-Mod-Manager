import { expect, test, type Page } from "@playwright/test";
import type { Game } from "../../src/lib/types";
import { installMockTauri } from "./mock-tauri";

const ordinaryUnc = String.raw`\\Server\Mods\Game`;
const verbatimUnc = String.raw`\\?\UNC\Server\Mods\Game`;
const verbatimDrive = String.raw`\\?\C:\Games\Fixture`;
const ordinaryDrive = String.raw`C:\Games\Fixture`;
const unix = "/sandbox/Game\\Data ";

function argsFor(page: Page, command: string) {
  return page.evaluate((command) => (window as any).__AMM_TEST__.calls
    .filter((call: any) => call.command === command).map((call: any) => call.args), command);
}

async function openLibrary(page: Page, gameRootPath?: string, withExternalAdapter = false) {
  await installMockTauri(page, { gameRootPath, withExternalAdapter });
  await page.goto("/");
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Minecraft Java Edition", exact: true })).toBeVisible();
}

function candidate(name: string, rootPath: string, registeredGameId: string | null = null) {
  return { name, rootPath, registeredGameId, status: "candidate", confidence: 0, suggestedAdapterSlug: null, platform: "unknown", warnings: [] };
}

const scanCandidates = [
  candidate("已登记", ordinaryUnc),
  candidate("已登记的另一拼法", verbatimUnc),
  candidate("大小写不同的共享游戏", String.raw`\\Server\Mods\game`),
  candidate("Unix 大写", unix),
  candidate("Unix 小写", "/sandbox/game\\Data "),
  candidate("从扫描添加", verbatimDrive),
  candidate("普通拼法", ordinaryDrive),
  candidate("保留尾空格", `${verbatimDrive} `),
  candidate("服务端已登记", String.raw`D:\Games\Existing`, "existing-by-server"),
];

async function installDiscoveryResponses(page: Page) {
  const scannedGame: Game = {
    id: "game-from-scan", name: "Fixture", slug: "unknown", rootPath: ordinaryDrive, platform: "unknown",
    adapterSlug: null, adapterVersion: null, confidence: 0, coverPath: null, steamAppId: null,
    installedModCount: 0, enabledModCount: 0, metadata: {},
    createdAt: "2026-11-21T00:00:00Z", updatedAt: "2026-11-21T00:00:00Z",
  };
  await page.evaluate(({ scanCandidates, verbatimUnc, verbatimDrive, unix, scannedGame }) => {
    const target = window as any;
    const transport = target.__TAURI_INTERNALS__;
    const invoke = transport.invoke;
    let registered = false;
    transport.invoke = async (command: string, args: any = {}) => {
      if (command === "list_games" && registered) return [...await invoke(command, args), scannedGame];
      if (command === "scan_games" || command === "identify_game" || command === "register_game" ||
          (command === "plugin:dialog|open" && args.options?.title === "选择游戏目录")) {
        target.__AMM_TEST__.calls.push({ command, args });
        if (command === "scan_games") return { candidates: scanCandidates, warnings: [{ path: verbatimUnc, message: "检查路径" }], directoriesChecked: 9, entriesChecked: 9, truncated: false };
        if (command === "identify_game" && args.directory === unix) return scanCandidates[3];
        if (command === "register_game" && args.directory === verbatimDrive) {
          registered = true;
          return scannedGame;
        }
        if (command === "plugin:dialog|open") return verbatimUnc;
        throw new Error("目录未按原样传递");
      }
      return invoke(command, args);
    };
  }, { scanCandidates, verbatimUnc, verbatimDrive, unix, scannedGame });
}

test("扫描候选合并旧路径别名，UNC 与 Unix 大小写不合并，发送原始路径", async ({ page }) => {
  await openLibrary(page, verbatimUnc);
  await installDiscoveryResponses(page);
  const library = page.getByTestId("library-page");
  await page.getByRole("button", { name: "扫描游戏", exact: true }).click();
  const discovery = page.getByRole("dialog", { name: "扫描游戏", exact: true });
  const row = (name: string) => discovery.locator('[data-slot="item"]').filter({ has: page.getByText(name, { exact: true }) });
  await expect(discovery).toBeVisible();
  await expect(discovery.getByRole("status").filter({ hasText: "已检查" })).toContainText("已检查 9 个目录、9 个条目");
  await expect(row("Minecraft Java Edition")).toBeVisible();
  expect(await argsFor(page, "scan_games")).toEqual([{ options: { userRoots: [], includeCommonRoots: true, page: { requestId: expect.any(String) } } }]);
  await page.keyboard.press("Escape");
  await expect(discovery).toBeHidden();
  await expect(library).toBeVisible();
  await page.getByRole("button", { name: "扫描游戏", exact: true }).click();
  await expect(discovery).toBeVisible();
  await expect.poll(async () => (await argsFor(page, "scan_games")).length).toBe(2);
  expect(await argsFor(page, "scan_games")).toEqual([
    { options: { userRoots: [], includeCommonRoots: true, page: { requestId: expect.any(String) } } },
    { options: { userRoots: [], includeCommonRoots: true, page: { requestId: expect.any(String) } } },
  ]);
  await expect(row("Minecraft Java Edition").getByRole("button", { name: "管理中" })).toBeDisabled();
  await expect(row("Minecraft Java Edition").getByText(ordinaryUnc, { exact: true })).toBeVisible();
  await expect(row("已登记的另一拼法")).toHaveCount(0);
  await expect(row("普通拼法")).toHaveCount(0);
  for (const name of ["大小写不同的共享游戏", "Unix 大写", "Unix 小写", "保留尾空格"]) {
    await expect(row(name).getByRole("button", { name: "添加", exact: true })).toBeEnabled();
  }
  await expect(row("服务端已登记").getByRole("button", { name: "管理中" })).toBeDisabled();
  await discovery.getByRole("button", { name: "选择游戏目录", exact: true }).click();
  await expect(discovery.getByLabel("游戏目录", { exact: true })).toHaveValue(verbatimUnc);
  await expect(discovery.getByRole("status").filter({ hasText: "扫描目录：" })).toContainText(`扫描目录：${ordinaryUnc}`);
  const warnings = discovery.getByText("扫描警告（1）", { exact: true });
  const warning = discovery.getByText(`${ordinaryUnc}：检查路径`, { exact: true });
  await expect(warnings).toBeVisible();
  await expect(warning).toBeHidden();
  await warnings.click();
  await expect(warning).toBeVisible();
  await warnings.press("Enter");
  await expect(warning).toBeHidden();
  expect((await argsFor(page, "scan_games")).at(-1)).toEqual({ options: { userRoots: [verbatimUnc], includeCommonRoots: false, page: { requestId: expect.any(String) } } });

  await discovery.getByLabel("游戏目录", { exact: true }).fill(unix);
  await discovery.getByRole("button", { name: "检查单个游戏", exact: true }).click();
  await expect(row("Unix 大写")).toBeVisible();
  expect(await argsFor(page, "identify_game")).toEqual([{ directory: unix }]);
  await discovery.getByRole("button", { name: "扫描此目录", exact: true }).click();
  await row("从扫描添加").getByRole("button", { name: "添加", exact: true }).click();
  await expect(page.getByText("已将「Fixture」添加到游戏库", { exact: true })).toBeVisible();
  await expect(discovery).toBeVisible();
  await expect(row("Fixture").getByRole("button", { name: "管理中", exact: true })).toBeDisabled();
  expect((await argsFor(page, "scan_games")).at(-1)).toEqual({ options: { userRoots: [unix], includeCommonRoots: false, page: { requestId: expect.any(String) } } });
  expect(await argsFor(page, "register_game")).toEqual([{ directory: verbatimDrive, adapterSlug: null, fromDiscovery: true }]);
  await page.keyboard.press("Escape");
  await expect(discovery).toBeHidden();
  await expect(library).toBeVisible();
});

test("新手引导合并别名并保持原始注册路径", async ({ page }) => {
  await installMockTauri(page, { onboarding: true, onboardingStarted: true, gameRootPath: verbatimUnc });
  await page.goto("/");
  await page.getByRole("button", { name: "继续设置", exact: true }).click();
  await installDiscoveryResponses(page);
  await page.getByRole("button", { name: "下一步", exact: true }).click();
  await expect(page.getByRole("heading", { name: "配置 AI 模型服务", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "跳过配置", exact: true }).click();
  await expect(page.getByRole("heading", { name: "启用可用工具", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "下一步", exact: true }).click();
  await expect(page.getByRole("heading", { name: "扫描游戏列表", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "扫描已安装游戏", exact: true }).click();
  const candidates = page.getByRole("list", { name: "扫描候选游戏", exact: true });
  const row = (text: string) => candidates.getByRole("listitem").filter({ has: page.getByText(text, { exact: true }) });
  const existing = row(ordinaryUnc);
  await expect(existing).toHaveCount(1);
  await expect(existing.getByText("Minecraft Java Edition", { exact: true })).toBeVisible();
  await expect(existing.getByText("管理中", { exact: true })).toBeVisible();
  await expect(row("已登记的另一拼法")).toHaveCount(0);
  await expect(row("普通拼法")).toHaveCount(0);
  const picked = row(ordinaryDrive);
  await expect(picked).toHaveCount(1);
  await expect(picked.getByText("从扫描添加", { exact: true })).toBeVisible();
  await expect(picked.getByRole("button", { name: "添加", exact: true })).toBeEnabled();
  const trailingSpace = row("保留尾空格");
  await expect(trailingSpace.getByRole("button", { name: "添加", exact: true })).toBeEnabled();
  await expect(trailingSpace.getByText("管理中", { exact: true })).toHaveCount(0);
  await picked.getByRole("button", { name: "添加", exact: true }).click();
  await expect(picked.getByText("管理中", { exact: true })).toBeVisible();
  await expect(picked.getByText("Fixture", { exact: true })).toBeVisible();
  await expect(trailingSpace.getByRole("button", { name: "添加", exact: true })).toBeEnabled();
  await expect(trailingSpace.getByText("管理中", { exact: true })).toHaveCount(0);
  expect(await argsFor(page, "register_game")).toEqual([{ directory: verbatimDrive, adapterSlug: null, fromDiscovery: true }]);

  // A fresh report still contains the old discovery aliases, so the managed state must come from the library readback.
  const rescan = page.getByRole("button", { name: "重新扫描", exact: true });
  await rescan.click();
  await expect.poll(async () => (await argsFor(page, "scan_games")).length).toBe(2);
  await expect(rescan).toBeEnabled();
  await expect(picked.getByText("管理中", { exact: true })).toBeVisible();
  await expect(picked.getByText("Fixture", { exact: true })).toBeVisible();
  await expect(picked.getByRole("button", { name: "添加", exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "下一步", exact: true }).click();
  await expect(page.getByText("2 款游戏", { exact: true })).toBeVisible();
});

test("原生文件选择、预览与回收站使用原路径，复制与展示一致", async ({ page }) => {
  await openLibrary(page, verbatimUnc);
  const launchPath = `${verbatimUnc}\\Play.exe `;
  const regularFile = `${verbatimUnc}\\Options.txt`;
  const specialFile = `${verbatimUnc}\\NUL.txt`;
  const trailingFile = `${verbatimUnc}\\README.txt `;
  const paths = [regularFile, specialFile, trailingFile];
  await page.evaluate(({ verbatimUnc, paths, launchPath }) => {
    const target = window as any;
    const mock = target.__AMM_TEST__;
    const transport = target.__TAURI_INTERNALS__;
    const invoke = transport.invoke;
    Object.defineProperty(navigator.clipboard, "writeText", { value: async (text: string) => { mock.copiedPath = text; } });
    transport.invoke = async (command: string, args: any = {}) => {
      if (command === "plugin:dialog|open" && args.options?.title === "选择游戏启动程序") {
        mock.calls.push({ command, args });
        return launchPath;
      }
      if (command === "update_game") {
        mock.calls.push({ command, args });
        return null;
      }
      if (command === "read_dir" && args.path === verbatimUnc) {
        mock.calls.push({ command, args });
        return paths.map((path: string) => ({ name: path.split("\\").at(-1), path, size: 5, modified: "2026-11-21", is_dir: false }));
      }
      if (command === "read_file") {
        mock.calls.push({ command, args });
        return { lines: [{ text: "内容", line: 1 }], truncated: false };
      }
      return invoke(command, args);
    };
  }, { verbatimUnc, paths, launchPath });
  await page.getByRole("button", { name: "更多操作", exact: true }).click();
  await page.getByRole("menuitem", { name: "编辑", exact: true }).click();
  const edit = page.getByRole("dialog", { name: "编辑游戏" });
  await edit.getByRole("button", { name: "选择", exact: true }).click();
  await expect(edit.getByLabel("启动程序", { exact: true })).toHaveValue(launchPath);
  await edit.getByRole("button", { name: "保存", exact: true }).click();
  await expect(edit).toBeHidden();
  expect(await argsFor(page, "update_game")).toEqual([{ id: "game-fixture", name: "Minecraft Java Edition", launchPath }]);

  await page.getByRole("button").filter({ has: page.getByRole("heading", { name: "Minecraft Java Edition", exact: true }) }).click();
  await page.getByRole("tab", { name: "文件", exact: true }).click();
  await expect(page.getByText(ordinaryUnc, { exact: true })).toBeVisible();
  const copy = page.getByRole("button", { name: "复制文件路径", exact: true });
  for (const [name, raw, shown] of [
    ["Options.txt", regularFile, `${ordinaryUnc}\\Options.txt`],
    ["NUL.txt", specialFile, specialFile],
    ["README.txt", trailingFile, trailingFile],
  ]) {
    await page.getByRole("button", { name, exact: true }).click();
    const shownPath = page.getByText(shown, { exact: true });
    await expect(shownPath).toBeVisible();
    expect(await shownPath.textContent()).toBe(shown);
    await copy.click();
    await expect.poll(() => page.evaluate(() => (window as any).__AMM_TEST__.copiedPath)).toBe(shown);
    expect((await argsFor(page, "read_file")).at(-1)).toEqual({ path: raw });
  }
  await page.getByRole("button", { name: "移入回收站", exact: true }).click();
  expect(await argsFor(page, "read_dir")).toEqual([{ path: verbatimUnc }]);
  expect(await argsFor(page, "trash_paths")).toEqual([{ request: { paths: [trailingFile], game_id: "game-fixture", reason: "用户从文件浏览器移除" } }]);
});

for (const [parent, expected] of [
  ["/Volumes/Games\\", String.raw`/Volumes/Games\/Agent Mod Manager`],
  [`${verbatimUnc} `, `${verbatimUnc} \\Agent Mod Manager`],
]) {
  test(`迁移预览按所选路径语法追加目录：${parent}`, async ({ page }) => {
    await installMockTauri(page, { storage: { dataRoot: verbatimUnc, parent } });
    await page.goto("/");
    await page.getByRole("button", { name: "设置", exact: true }).click();
    await page.getByRole("tab", { name: "外观与存储", exact: true }).click();
    await expect(page.getByText(ordinaryUnc, { exact: true })).toBeVisible();
    await page.getByRole("button", { name: "更改存储目录", exact: true }).click();
    const dialog = page.getByRole("alertdialog", { name: "迁移存储目录" });
    await expect(dialog.getByText(expected, { exact: true })).toBeVisible();
    await dialog.getByRole("button", { name: "迁移并重启", exact: true }).click();
    expect(await argsFor(page, "migrate_storage")).toEqual([{ parent }]);
  });
}

test("Agent 审批和工具详情只转换原生结构化路径，不改文本、错误或第三方数据", async ({ page }) => {
  await installMockTauri(page, { holdAgentTurn: true });
  await page.goto("/");
  await page.getByRole("button", { name: /^Agent( 对话)?$/ }).click();
  await page.getByRole("textbox", { name: "告诉 Agent 你想做什么" }).fill(verbatimDrive);
  await page.getByRole("button", { name: "发送消息", exact: true }).click();
  await expect(page.locator('[data-message-id="assistant-fixture"]')).toContainText("正在检查模组文件。");
  const dst = `${verbatimUnc}\\target.txt`;
  const projectedDst = `${ordinaryUnc}\\target.txt`;
  const detail = `文件系统错误：${verbatimDrive}`;
  await page.evaluate(({ verbatimDrive, dst, detail }) => {
    const mock = (window as any).__AMM_TEST__;
    mock.emit("tool-start", { toolCallId: "foreign", name: "mcp__fixture__read_dir", arguments: { path: verbatimDrive } });
    mock.emit("tool-result", { toolCallId: "foreign", status: "done", result: { path: verbatimDrive } });
    mock.emit("tool-start", { toolCallId: "native-read", name: "read_dir", arguments: { path: verbatimDrive } });
    mock.emit("tool-result", { toolCallId: "native-read", status: "error", result: { error: detail } });
    mock.emit("tool-start", { toolCallId: "tool-fixture", name: "trash_paths", arguments: { paths: [verbatimDrive], reason: verbatimDrive } });
    mock.emit("approval-request", { title: "trash_paths", toolCallId: "tool-fixture", approvalId: "approval-fixture", preview: {
      id: "legacy-path-plan", summary: verbatimDrive, total_bytes: 16,
      entries: [{ operation: { op: "move", src: verbatimDrive, dst }, impact: "move", before: null, source: null }],
    } });
  }, { verbatimDrive, dst, detail });

  const approval = page.getByRole("group", { name: "操作审批", exact: true });
  await expect(page.getByRole("dialog", { name: "操作审批", exact: true })).toHaveCount(0);
  await expect(approval.getByText(projectedDst, { exact: true })).toBeVisible();
  await expect(approval.getByText(verbatimDrive, { exact: true })).toBeVisible(); // summary is text
  await approval.getByText("完整请求", { exact: true }).click();
  const preview = JSON.parse(await approval.locator("pre").innerText());
  expect(preview.entries[0].operation).toEqual({ op: "move", src: String.raw`C:\Games\Fixture`, dst: projectedDst });
  expect(preview.summary).toBe(verbatimDrive);
  const reject = approval.getByRole("button", { name: "拒绝", exact: true });
  await reject.click();
  await expect(reject).toHaveCount(0);
  expect((await argsFor(page, "respond_approval"))[0]).toMatchObject({ approvalId: "approval-fixture", response: { approved: false } });
  await expect(page.getByRole("button", { name: "trash_paths 已拒绝", exact: true })).toBeVisible();
  const encoded = (text: string) => JSON.stringify(text);
  const nativeTrace = page.getByRole("button", { name: "read_dir 执行失败", exact: true });
  await expect(nativeTrace).toHaveAttribute("aria-expanded", "true");
  await expect(page.locator("pre").filter({ hasText: `"path": ${encoded(String.raw`C:\Games\Fixture`)}` })).toBeVisible();
  await expect(page.locator("pre").filter({ hasText: `"error": ${encoded(detail)}` })).toBeVisible();
  await page.getByRole("button", { name: "mcp__fixture__read_dir 已执行", exact: true }).click();
  await expect(page.locator("pre").filter({ hasText: `"path": ${encoded(verbatimDrive)}` })).toBeVisible();
  const original = await page.evaluate(() => (window as any).__AMM_TEST__.events.find((event: any) => event.kind === "approval-request").payload.preview);
  expect(original.entries[0].operation).toEqual({ op: "move", src: verbatimDrive, dst });
});

test("本地归档选择器允许 RAR 并保留原生选择的路径", async ({ page }) => {
  await openLibrary(page);
  const path = verbatimUnc + "\\ Map Pack.RAR ";
  await page.evaluate((selected) => {
    const target = window as any;
    const invoke = target.__TAURI_INTERNALS__.invoke;
    target.__TAURI_INTERNALS__.invoke = async (command: string, args: any) => {
      if (command === "plugin:dialog|open") {
        target.__AMM_TEST__.calls.push({ command, args });
        return selected;
      }
      if (command === "plan_import_archive") {
        target.__AMM_TEST__.calls.push({ command, args });
        throw new Error("仅校验请求参数");
      }
      return invoke(command, args);
    };
  }, path);
  await page.getByRole("button").filter({ has: page.getByRole("heading", { name: "Minecraft Java Edition", exact: true }) }).click();
  await page.getByRole("button", { name: "导入本地归档", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("仅校验请求参数");
  expect(await argsFor(page, "plugin:dialog|open")).toEqual([expect.objectContaining({
    options: expect.objectContaining({
      filters: [{ name: "Mod 归档", extensions: ["zip", "jar", "mrpack", "7z", "rar", "gz", "tgz"] }],
    }),
  })]);
  expect(await argsFor(page, "plan_import_archive")).toEqual([{ path }]);
  expect(await argsFor(page, "register_imported_mod")).toHaveLength(0);
  expect(await argsFor(page, "start_turn")).toHaveLength(0);
});

test("启动程序输入框显示普通路径，未编辑的已有值和选择值按原样提交", async ({ page }) => {
  const stored = `${verbatimUnc}\\Old.exe`;
  const picked = `${verbatimUnc}\\New.exe`;
  await installMockTauri(page, { gameRootPath: verbatimUnc, gameLaunchPath: stored });
  await page.goto("/");
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await page.evaluate((picked) => {
    const target = window as any;
    const transport = target.__TAURI_INTERNALS__;
    const invoke = transport.invoke;
    transport.invoke = async (command: string, args: any) => {
      if (command === "plugin:dialog|open" && args.options?.title === "选择游戏启动程序") return picked;
      if (command === "update_game") {
        target.__AMM_TEST__.calls.push({ command, args });
        return null;
      }
      return invoke(command, args);
    };
  }, picked);
  const dialog = page.getByRole("dialog", { name: "编辑游戏", exact: true });
  const input = dialog.getByRole("textbox", { name: "启动程序", exact: true });
  const save = dialog.getByRole("button", { name: "保存", exact: true });
  async function openEditor() {
    await page.getByRole("button", { name: "更多操作", exact: true }).click();
    await page.getByRole("menuitem", { name: "编辑", exact: true }).click();
  }
  await openEditor();
  await expect(input).toHaveValue(`${ordinaryUnc}\\Old.exe`);
  await save.click();
  await expect(dialog).toBeHidden();
  await openEditor();
  await dialog.getByRole("button", { name: "选择", exact: true }).click();
  await expect(input).toHaveValue(`${ordinaryUnc}\\New.exe`);
  await save.click();
  await expect(dialog).toBeHidden();
  await openEditor();
  // Typed text is not reprojected mid-edit, so the user can include a verbatim prefix.
  const special = `${verbatimUnc}\\Typed.exe `;
  await input.fill(verbatimUnc);
  await expect(input).toHaveValue(verbatimUnc);
  await input.fill(special);
  await expect(input).toHaveValue(special);
  await save.click();
  await expect(dialog).toBeHidden();
  expect((await argsFor(page, "update_game")).map((args: any) => args.launchPath)).toEqual([stored, picked, special]);
});

test("归档计划沿用现有审批和路径展示", async ({ page }) => {
  await installMockTauri(page, { holdAgentTurn: true });
  await page.goto("/");
  await page.getByRole("button", { name: /^Agent( 对话)?$/ }).click();
  await page.getByRole("textbox", { name: "告诉 Agent 你想做什么" }).fill("预览压缩和解压计划");
  await page.getByRole("button", { name: "发送消息", exact: true }).click();
  await expect(page.locator('[data-message-id="assistant-fixture"]')).toContainText("正在检查模组文件。");
  const operations = [
    { op: "compress", src: `${verbatimDrive}\\folder`, dst: `${verbatimUnc}\\new.zip` },
    { op: "extract", src: `${verbatimDrive}\\source.7z`, dst: `${verbatimUnc}\\output` },
  ];
  await page.evaluate((operations) => {
    const mock = (window as any).__AMM_TEST__;
    const preview = {
      id: "archive-file-ops", summary: "归档操作", total_bytes: 128, requires_review: true,
      entries: operations.map((operation) => ({ operation, impact: "add", before: null, source: null })),
    };
    mock.emit("tool-start", { toolCallId: "archive-planner", name: "plan_file_ops", arguments: { operations } });
    mock.emit("tool-result", { toolCallId: "archive-planner", status: "done", result: preview });
    mock.emit("tool-start", { toolCallId: "tool-fixture", name: "apply_file_ops", arguments: { plan_id: preview.id } });
    mock.emit("approval-request", { title: "apply_file_ops", toolCallId: "tool-fixture", approvalId: "approval-fixture", preview });
  }, operations);
  const approval = page.getByRole("group", { name: "操作审批", exact: true });
  const shownDrive = String.raw`C:\Games\Fixture`;
  const expected = [
    { op: "compress", src: `${shownDrive}\\folder`, dst: `${ordinaryUnc}\\new.zip` },
    { op: "extract", src: `${shownDrive}\\source.7z`, dst: `${ordinaryUnc}\\output` },
  ];
  await expect(approval.getByText("新增：2", { exact: true })).toBeVisible();
  for (const [op, label] of [[expected[0], "压缩"], [expected[1], "解压"]] as const) {
    const cell = `td[data-operation="${op.op}"]`;
    const row = approval.getByRole("row").filter({ has: page.locator(cell) });
    await expect(row.locator(cell)).toHaveText(label);
    for (const value of [op.src, op.dst]) await expect(row.getByText(value, { exact: true })).toBeVisible();
  }
  await approval.getByText("完整请求", { exact: true }).click();
  const preview = JSON.parse(await approval.locator("pre").innerText());
  expect(preview.entries.map((entry: any) => entry.operation)).toEqual(expected);
  const original = await page.evaluate(() => (window as any).__AMM_TEST__.events.find((event: any) => event.kind === "approval-request").payload.preview);
  expect(original.entries.map((entry: any) => entry.operation)).toEqual(operations);
});
