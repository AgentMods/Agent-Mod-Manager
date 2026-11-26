import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const gameId = "game-fixture";
const source = {
  sourceId: "modrinth",
  projectId: "ProjectA",
  versionId: "OldVer01",
  version: "1.0.0",
};
const previousArchive = {
  artifactId: "old-archive", gameId, gameName: "Minecraft Java Edition", name: "Example Mod",
  filename: "example-old.jar", version: "1.0.0", sha256: "a".repeat(64), projectPage: "https://modrinth.com/mod/ProjectA",
  ...source,
};
const versions = [
  {version_id: "NewVer01", version: "1.1.0", name: "1.1.0", release_channel: "release", published_at: "2026-10-02T00:00:00Z", game_versions: [], loaders: []},
  {version_id: "OldVer01", version: "1.0.0", name: "1.0.0", release_channel: "release", published_at: "2026-10-01T00:00:00Z", game_versions: [], loaders: []},
];
const available = {status: {source_id: "modrinth", state: "available", message: ""}, versions, has_more: false};
const resolution = {
  status: {source_id: "modrinth", state: "available", message: ""},
  download: {resolution_id: "resolution-1", source_id: "modrinth", project_id: "ProjectA", version_id: "NewVer01", version: "1.1.0", filename: "example.jar"},
};
const downloadedEntry = {
  artifactId: "new-archive", gameId, gameName: "Minecraft Java Edition", name: "Example Mod",
  filename: "example.jar", version: "1.1.0", sha256: "b".repeat(64), projectPage: "https://modrinth.com/mod/ProjectA",
  sourceId: "modrinth", projectId: "ProjectA", versionId: "NewVer01",
};

async function installUpdateFixture(
  page: Page,
  installed: boolean,
  sourceEntry: Record<string, any> = previousArchive,
  versionPage: Record<string, any> = available,
  installHasProvenance = true,
  installStatus = "enabled",
) {
  await installMockTauri(page, {modLibrary: [sourceEntry]});
  await page.addInitScript(({gameId, installed, sourceEntry, installHasProvenance, installStatus}) => {
    const target = window as any;
    const transport = target.__TAURI_INTERNALS__;
    const originalInvoke = transport.invoke;
    const state = {library: [structuredClone(sourceEntry)]};
    const oldInstall = {
      id: "install-old", gameId, name: "Example Mod", version: "1.0.0", source: "modrinth", status: installStatus,
      installRoot: "/sandbox/minecraft/mods", files: [], installedAt: "2026-11-18T00:00:00Z", tagIds: [],
      ...(installHasProvenance && {downloadOrigin: {artifactId: sourceEntry.artifactId, sourceId: sourceEntry.sourceId, projectId: sourceEntry.projectId, versionId: sourceEntry.versionId}}),
    };
    transport.invoke = async (command: string, args: Record<string, any> = {}, options?: unknown) => {
      target.__AMM_TEST__.calls.push({command, args});
      if (command === "list_mod_library") return structuredClone(state.library);
      if (command === "list_installs") return installed ? [structuredClone(oldInstall)] : [];
      if (command === "list_mod_versions") return structuredClone((window as any).__MOD_UPDATE_FIXTURE__.available);
      if (command === "resolve_mod") return structuredClone((window as any).__MOD_UPDATE_FIXTURE__.resolution);
      if (command === "download_mod") {
        state.library = [sourceEntry, structuredClone((window as any).__MOD_UPDATE_FIXTURE__.downloadedEntry)];
        return {artifactId: "new-archive", gameId, filename: "example.jar"};
      }
      return originalInvoke(command, args, options);
    };
    target.__MOD_UPDATE_STATE__ = state;
  }, {gameId, installed, sourceEntry, installHasProvenance, installStatus});
  await page.addInitScript(({available, resolution, downloadedEntry}) => {
    (window as any).__MOD_UPDATE_FIXTURE__ = {available, resolution, downloadedEntry};
  }, {available: versionPage, resolution, downloadedEntry});
}

async function openGame(page: Page) {
  await page.getByRole("button", {name: "游戏库", exact: true}).click();
  await page.getByRole("button").filter({has: page.getByRole("heading", {name: "Minecraft Java Edition", exact: true})}).click();
  await expect(page.getByTestId("game-detail-page")).toBeVisible();
}

function row(page: Page, text: string) {
  return page.getByTestId("game-detail-page").getByRole("table", {name: "Mod 管理", exact: true})
    .getByRole("row").filter({hasText: text});
}

async function openUpdateDialog(page: Page, name: string, installed = false) {
  const modRow = installed
    ? page.getByTestId("game-detail-page").getByRole("table", {name: "Mod 管理", exact: true})
      .getByRole("row", {name: new RegExp(`^${name} \\d+ 个文件`)})
    : row(page, name);
  await modRow.getByRole("button", {name: "更多", exact: true}).click();
  await page.getByRole("menuitem", {name: "更新", exact: true}).click();
  return page.getByRole("dialog", {name: "更新 Mod", exact: true});
}

function calls(page: Page, ...commands: string[]) {
  return page.evaluate(names => (window as any).__AMM_TEST__.calls.filter((call: any) => names.includes(call.command)), commands);
}

for (const installed of [true, false]) {
  test(`${installed ? "installed" : "pending"} Mod checks source versions only after explicit confirmation`, async ({page}) => {
    await installUpdateFixture(page, installed);
    await page.goto("/");
    await openGame(page);
    const dialog = await openUpdateDialog(page, "Example Mod");
    await expect(dialog).toContainText("Example Mod");
    await expect(dialog).toContainText("modrinth");
    await expect(dialog).toContainText("当前版本");
    await expect(dialog).toContainText("1.0.0");
    expect(await calls(page, "list_mod_versions", "resolve_mod", "download_mod")).toEqual([]);

    await dialog.getByRole("button", {name: "检查更新", exact: true}).click();
    await expect(dialog).toContainText("可更新版本");
    await expect(dialog).toContainText("1.1.0");
    expect((await calls(page, "list_mod_versions")).map(call => call.args)).toEqual([{
      request: {source_id: "modrinth", project_id: "ProjectA", offset: 0, limit: 50},
    }]);
    expect(await calls(page, "resolve_mod", "download_mod")).toEqual([]);
    await expect(dialog.getByRole("button", {name: installed ? "下载并审核替换" : "下载到库", exact: true})).toBeVisible();
  });
}

test("downloading a pending update resolves the selected version and reads the new archive back", async ({page}) => {
  await installUpdateFixture(page, false);
  await page.goto("/");
  await openGame(page);
  const dialog = await openUpdateDialog(page, "Example Mod");
  await dialog.getByRole("button", {name: "检查更新", exact: true}).click();
  await expect(dialog).toContainText("可更新版本");
  await dialog.getByRole("button", {name: "下载到库", exact: true}).click();

  expect((await calls(page, "resolve_mod")).map(call => call.args)).toEqual([{
    request: {source_id: "modrinth", project_id: "ProjectA", version_id: "NewVer01"},
  }]);
  const downloads = await calls(page, "download_mod");
  expect(downloads).toHaveLength(1);
  expect(downloads[0].args).toMatchObject({resolutionId: "resolution-1", classification: {gameId}});
  expect(downloads[0].args.taskId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i);
  await expect.poll(async () => (await calls(page, "list_mod_library")).length).toBeGreaterThan(1);
  expect(await calls(page, "start_turn", "create_conversation")).toEqual([]);
  expect((await calls(page, "list_mod_library")).at(-1)?.args).toEqual({gameId});
});

test("downloading an installed update starts an approval-scoped replacement for the matching install", async ({page}) => {
  await installUpdateFixture(page, true);
  await page.goto("/");
  await openGame(page);
  const dialog = await openUpdateDialog(page, "Example Mod");
  await dialog.getByRole("button", {name: "检查更新", exact: true}).click();
  await expect(dialog).toContainText("可更新版本");
  await dialog.getByRole("button", {name: "下载并审核替换", exact: true}).click();

  await expect.poll(async () => (await calls(page, "start_turn")).some((call: any) =>
    call.args.request?.content?.includes(JSON.stringify({gameId, artifactId: "new-archive", replaceInstallId: "install-old"}))))
    .toBe(true);
  const starts = await calls(page, "start_turn");
  const replacement = starts.find((call: any) => call.args.request?.content?.includes(
    JSON.stringify({gameId, artifactId: "new-archive", replaceInstallId: "install-old"})));
  expect(replacement.args.request.permissionMode).toBe("request_approval");
  expect((await calls(page, "create_conversation")).some((call: any) => call.args.gameId === gameId && call.args.category === "install")).toBe(true);
});

test("unknown source keeps the local import path and performs no source checks or downloads", async ({page}) => {
  const local = {...previousArchive, artifactId: "local-archive", sourceId: "local", projectId: null, versionId: null, projectPage: null};
  await installUpdateFixture(page, false, local);
  await page.goto("/");
  await openGame(page);
  const dialog = await openUpdateDialog(page, "Example Mod");
  await expect(dialog).toContainText("无法确认此 Mod 的来源项目");
  await expect(dialog).toContainText("通过此游戏的「导入本地归档」将其加入库");
  await expect(dialog.getByRole("button", {name: "用 Agent 核对来源", exact: true})).toHaveCount(0);
  expect(await calls(page, "list_mod_versions", "resolve_mod", "download_mod")).toEqual([]);
});

test("latest published version does not offer a downgrade", async ({page}) => {
  const latest = {...available, versions: [versions[1]]};
  await installUpdateFixture(page, false, previousArchive, latest);
  await page.goto("/");
  await openGame(page);
  const dialog = await openUpdateDialog(page, "Example Mod");
  await dialog.getByRole("button", {name: "检查更新", exact: true}).click();
  await expect(dialog).toContainText("未找到更新的匹配正式版。");
  await expect(dialog).toContainText("当前版本");
  expect(await calls(page, "resolve_mod", "download_mod")).toEqual([]);
});

test("older versions and earlier publication dates are not offered as updates", async ({page}) => {
  const staleAndOlder = {...available, versions: [
    {...versions[0], published_at: "2026-09-30T00:00:00Z"},
    versions[1],
    {...versions[1], version_id: "OlderVer", version: "0.9.0", name: "0.9.0", published_at: "2026-10-03T00:00:00Z"},
  ]};
  await installUpdateFixture(page, false, previousArchive, staleAndOlder);
  await page.goto("/");
  await openGame(page);
  const dialog = await openUpdateDialog(page, "Example Mod");
  await dialog.getByRole("button", {name: "检查更新", exact: true}).click();
  await expect(dialog).toContainText("未找到更新的匹配正式版。");
  await expect(dialog).not.toContainText("1.1.0");
  await expect(dialog).not.toContainText("0.9.0");
  expect(await calls(page, "resolve_mod", "download_mod")).toEqual([]);
});

for (const invalidDate of ["2026-02-30T00:00:00Z", "2026-10-02T00:00:00"]) {
  test(`an invalid or timezone-free candidate date is treated as unverified (${invalidDate})`, async ({page}) => {
    const uncertain = {...available, versions: [
      {...versions[0], published_at: invalidDate}, versions[1],
    ]};
    await installUpdateFixture(page, false, previousArchive, uncertain);
    await page.goto("/");
    await openGame(page);
    const dialog = await openUpdateDialog(page, "Example Mod");
    await dialog.getByRole("button", {name: "检查更新", exact: true}).click();
    await expect(dialog).toContainText("无法确认有可安全升级的版本。");
    await expect(dialog).not.toContainText("未找到更新的匹配正式版。");
    await expect(dialog.getByRole("button", {name: "下载到库", exact: true})).toHaveCount(0);
    expect(await calls(page, "resolve_mod", "download_mod")).toEqual([]);
  });
}

test("a newer version missing one of the current loaders is not reported as latest or downloadable", async ({page}) => {
  const loaderPage = {...available, versions: [
    {...versions[0], game_versions: ["1.21.1"], loaders: ["fabric"]},
    {...versions[1], game_versions: ["1.21.1"], loaders: ["fabric", "neoforge"]},
  ]};
  await installUpdateFixture(page, false, previousArchive, loaderPage);
  await page.goto("/");
  await openGame(page);
  const dialog = await openUpdateDialog(page, "Example Mod");
  await dialog.getByRole("button", {name: "检查更新", exact: true}).click();
  await expect(dialog).toContainText("无法确认有可安全升级的版本。");
  await expect(dialog).not.toContainText("未找到更新的匹配正式版。");
  await expect(dialog.getByRole("button", {name: "下载到库", exact: true})).toHaveCount(0);
  expect(await calls(page, "resolve_mod", "download_mod")).toEqual([]);
});

for (const status of ["superseded", "rolled_back", "planned"]) {
  test(`${status} history can add an update to the library without starting an Agent`, async ({page}) => {
    await installUpdateFixture(page, true, previousArchive, available, true, status);
    await page.goto("/");
    await openGame(page);
    const dialog = await openUpdateDialog(page, "Example Mod");
    await dialog.getByRole("button", {name: "检查更新", exact: true}).click();
    await expect(dialog).toContainText("可更新版本");
    await dialog.getByRole("button", {name: "下载到库", exact: true}).click();
    await expect(dialog).toContainText("新版已加入此游戏的库，未安装。");
    expect(await calls(page, "create_conversation", "start_turn")).toEqual([]);
    expect((await calls(page, "resolve_mod")).map(call => call.args)).toEqual([{
      request: {source_id: "modrinth", project_id: "ProjectA", version_id: "NewVer01"},
    }]);
    expect(await calls(page, "download_mod")).toHaveLength(1);
  });
}

test("an unsupported source response is surfaced without resolving or downloading", async ({page}) => {
  const unsupported = {...available, status: {source_id: "modrinth", state: "unsupported", message: "此来源不支持版本查询"}, versions: []};
  await installUpdateFixture(page, false, previousArchive, unsupported);
  await page.goto("/");
  await openGame(page);
  const dialog = await openUpdateDialog(page, "Example Mod");
  await dialog.getByRole("button", {name: "检查更新", exact: true}).click();
  await expect(dialog).toContainText("此来源不支持版本查询");
  expect((await calls(page, "list_mod_versions")).map(call => call.args)).toEqual([{
    request: {source_id: "modrinth", project_id: "ProjectA", offset: 0, limit: 50},
  }]);
  expect(await calls(page, "resolve_mod", "download_mod")).toEqual([]);
});

test("a known Nexus project can be checked by Agent without invoking catalog or download commands", async ({page}) => {
  const nexus = {
    ...previousArchive, artifactId: "nexus-archive", sourceId: "nexus", projectId: "example/42", versionId: "100",
    projectPage: "https://www.nexusmods.com/example/mods/42",
  };
  await installUpdateFixture(page, false, nexus);
  await page.goto("/");
  await openGame(page);
  const dialog = await openUpdateDialog(page, "Example Mod");
  await expect(dialog.getByRole("link", {name: "打开原发布页", exact: true})).toHaveAttribute("href", nexus.projectPage);
  await dialog.getByRole("button", {name: "用 Agent 核对来源", exact: true}).click();

  const expectedTarget = {
    project: {sourceId: "nexus", projectId: "example/42", versionId: "100"},
    projectPage: nexus.projectPage,
    currentVersion: "1.0.0",
  };
  await expect.poll(async () => (await calls(page, "start_turn")).some((call: any) =>
    call.args.request?.content?.includes(JSON.stringify(expectedTarget)))).toBe(true);
  expect((await calls(page, "create_conversation")).map(call => call.args)).toContainEqual({
    gameId, title: "核对 Mod 来源：Example Mod",
  });
  const agentCall = (await calls(page, "start_turn")).find((call: any) => call.args.request?.content?.includes(JSON.stringify(expectedTarget)));
  expect(agentCall.args.request.permissionMode).toBe("request_approval");
  expect(agentCall.args.request.content).toContain("不要获取下载链接或下载文件");
  expect(await calls(page, "list_mod_versions", "resolve_mod", "download_mod")).toEqual([]);
});

test("an installed record without a verified download origin cannot be auto-replaced", async ({page}) => {
  await installUpdateFixture(page, true, previousArchive, available, false);
  await page.goto("/");
  await openGame(page);
  const dialog = await openUpdateDialog(page, "Example Mod", true);
  await expect(dialog).toContainText("不能自动更新");
  await expect(dialog.getByRole("button", {name: "下载并审核替换", exact: true})).toHaveCount(0);
  expect(await calls(page, "list_mod_versions", "resolve_mod", "download_mod")).toEqual([]);
});

test("an exact HTTPS project page remains available as a manual website action", async ({page}) => {
  await installUpdateFixture(page, false);
  await page.goto("/");
  await openGame(page);
  await row(page, "Example Mod").getByRole("button", {name: "更多", exact: true}).click();
  const website = page.getByRole("menuitem", {name: "网站", exact: true});
  await expect(website).toBeEnabled();
  await website.click();
  await expect.poll(async () => (await calls(page, "plugin:opener|open_url")).length).toBeGreaterThanOrEqual(1);
  expect((await calls(page, "plugin:opener|open_url")).at(-1)?.args).toMatchObject({url: "https://modrinth.com/mod/ProjectA"});
  expect(await calls(page, "list_mod_versions", "resolve_mod", "download_mod")).toEqual([]);
});
