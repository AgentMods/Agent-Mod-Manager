import { expect, test } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

type BrowseSourceCase = {
  id: string;
  title: string;
  sourceId: string;
  sourceName: string;
  gameId: string;
  providerGameId: string;
  projectId: string;
  catalogSourceId?: string | null;
  legacyIdentity?: boolean;
  currentGameId: string | null;
  origin?: { kind: string } | null;
  assistantBefore?: boolean;
};

const sourceCases: BrowseSourceCase[] = [
  {
    id: "nexus-history",
    title: "Nexus 安装历史",
    sourceId: "nexusmods",
    sourceName: "Nexus Mods",
    gameId: "game-stardew",
    providerGameId: "stardewvalley",
    projectId: "stardewvalley/12345",
    catalogSourceId: "nexus",
    currentGameId: null,
    origin: { kind: "user" },
    assistantBefore: true,
  },
  {
    id: "thunderstore-history",
    title: "Thunderstore 安装历史",
    sourceId: "thunderstore",
    sourceName: "Thunderstore",
    gameId: "game-valheim",
    providerGameId: "valheim",
    projectId: "Northstar/Quiet-Islands",
    catalogSourceId: "thunderstore",
    currentGameId: "game-stardew",
    origin: null,
  },
  {
    id: "three-dm-history",
    title: "3DM 安装历史",
    sourceId: "glossmod_3dm",
    sourceName: "3DM",
    gameId: "game-stardew",
    providerGameId: "22",
    projectId: "7654321",
    legacyIdentity: true,
    currentGameId: null,
  },
  {
    id: "mod-inn-history",
    title: "Mod 客栈安装历史",
    sourceId: "glossmod_http",
    sourceName: "Mod客栈",
    gameId: "game-valheim",
    providerGameId: "valheim",
    projectId: "314159",
    catalogSourceId: null,
    currentGameId: "game-stardew",
    origin: { kind: "user" },
  },
];

const legacyStockWorkflow = "用原厂来源的附件或版本列表确认版本，对同一作品调用 download_file 或 download_version。只收到可信下载回执后，用 file_received_mod 归档模型收到的精确文件名；若无法验证、账号拒绝或只能从网站手动下载，明确停止并提示用户到原作品页选择并手动导入。";
const stockWorkflow = "已有可信下载回执时，用 file_received_mod 归档回执中的精确文件名；省略 gameName、modName 和 projectPage。否则用原厂附件或版本列表确认版本，对同一作品调用 download_file 或 download_version。若无法验证、账号拒绝或只能从网站手动下载，明确停止并提示用户到原作品页选择并手动导入。下载工具直接返回 artifactId 时，使用该已归档原包。";
const threeDmWorkflow = "用原厂 3DM MCP get-mod-detail 查询作品，id 取 projectId 的整数值。用返回的 download.resolution_id 调用 download_mod。若不能验证官方直链，提示用户在原作品页选择文件并手动导入。";
const legacyThreeDmWorkflow = "优先用原厂 3DM MCP get-mod-detail 核对作品资源；只使用返回的会话 resolution_id 调用 download_mod，不传 URL。若不能验证官方直链，提示用户在原作品页选择文件并手动导入。";
const modInnWorkflow = "用官方 Mod 客栈 list_work_files 查询此 work_id，核对附件、版本与 entitlement；只使用官方 get_work_file_download 下载本作品的附件。";
const fixedEnding = "\n可先用 list_mod_library 检查已有的本作品归档。用 match_mod_install 检查已归档原包，再用 install_library_mod 审批原生安装计划。兼容性、版本、映射或用户意图不明确时用 ask_user；如果无法完成就停下说明，不根据下载成功或模型文本宣称已安装。明确停止于一个经验证的安装事务。";
const nativePrefix = "用户从浏览卡片发起了本机安装。下列 JSON 由原生浏览结果生成，不来自翻译、作品文案或模型推断：";
const nativeScope = "\n只处理这一个来源作品和登记游戏，不要搜索替代品、打开任意 URL 或安装其他依赖。";

function instructionFor(source: BrowseSourceCase, overrides: { catalogSourceId?: string | null; append?: string; currentStock?: boolean } = {}) {
  const identity = source.legacyIdentity
    ? {
      gameId: source.gameId,
      projectId: source.projectId,
      providerGameId: source.providerGameId,
      sourceId: source.sourceId,
    }
    : {
      catalogSourceId: overrides.catalogSourceId ?? source.catalogSourceId,
      gameId: source.gameId,
      projectId: source.projectId,
      providerGameId: source.providerGameId,
      sourceId: source.sourceId,
    };
  const workflow = source.sourceId === "glossmod_http" ? modInnWorkflow
    : source.sourceId === "glossmod_3dm" ? source.legacyIdentity ? legacyThreeDmWorkflow : threeDmWorkflow
      : overrides.currentStock ? stockWorkflow : legacyStockWorkflow;
  return `${nativePrefix}${JSON.stringify(identity)}${nativeScope}${workflow}${fixedEnding}${overrides.append ?? ""}`;
}

function conversation(id: string, title: string, category: string | null, gameId: string | null, messageCount: number) {
  const timestamp = "2026-11-25T00:00:00Z";
  return {
    id, title, category, game_id: gameId, task_summary: null,
    archived: false, pinned: false, message_count: messageCount,
    created_at: timestamp, updated_at: timestamp, permission_mode: null,
  };
}

function userMessage(id: string, content: string, origin?: { kind: string } | null) {
  return {
    id, role: "user", content,
    ...(origin === undefined ? {} : { origin }),
    reasoning: null, tool_calls: null, tool_call_id: null,
  };
}

async function installSavedChatFixture(page: Parameters<typeof installMockTauri>[0], conversations: Record<string, unknown>[], messages: Record<string, unknown>[][]) {
  const messagesByConversation = Object.fromEntries(conversations.map((item, index) => [item.id, messages[index]]));
  await installMockTauri(page, {
    workspaceGames: [
      { id: "game-stardew", name: "Stardew Valley" },
      { id: "game-valheim", name: "Valheim" },
    ],
    conversationFixtures: conversations,
  });
  await page.addInitScript((savedMessages) => {
    const target = window as any;
    const wrap = (transport: any) => {
      if (!transport || transport.__browseChatFixtureWrapped) return;
      const invoke = transport.invoke.bind(transport);
      transport.invoke = async (command: string, args: Record<string, unknown> = {}) => {
        const result = await invoke(command, args);
        if (command !== "get_conversation" || typeof args.id !== "string") return result;
        return { ...result, messages: structuredClone(savedMessages[args.id] ?? []) };
      };
      transport.__browseChatFixtureWrapped = true;
    };
    let transport = target.__TAURI_INTERNALS__;
    wrap(transport);
    Object.defineProperty(target, "__TAURI_INTERNALS__", {
      configurable: true,
      enumerable: true,
      get: () => transport,
      set: (value: any) => { transport = value; wrap(value); },
    });
  }, messagesByConversation);
}

async function enterAgent(page: Parameters<typeof installMockTauri>[0]) {
  await page.goto("/");
  await page.getByRole("button", { name: /^Agent( 对话)?$/ }).click();
}

async function openConversation(page: Parameters<typeof installMockTauri>[0], title: string, category: "install" | "ordinary") {
  if (category === "install") {
    const group = page.getByRole("button", { name: "安装 Mod", exact: true });
    if (await group.getAttribute("aria-expanded") !== "true") await group.click();
    await page.locator('ul[aria-label="安装 Mod"]').getByRole("button", { name: title, exact: true }).click();
  } else {
    await page.locator('ul[aria-label="最近对话"]').getByRole("button", { name: title, exact: true }).click();
  }
  await expect(page.locator(".chat-header h1")).toHaveText(title);
}

async function setTheme(page: Parameters<typeof installMockTauri>[0], theme: "light" | "dark") {
  const html = page.locator("html");
  if (await html.getAttribute("data-theme") !== theme) {
    await page.getByRole("button", { name: theme === "dark" ? "切换暗色主题" : "切换亮色主题", exact: true }).click();
  }
  await expect(html).toHaveAttribute("data-theme", theme);
}

test("持久化的游览安装请求按原来源和作品身份折叠成卡片", async ({ page }, testInfo) => {
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  const conversations = sourceCases.map((source) => conversation(source.id, source.title, "install", source.currentGameId, source.assistantBefore ? 2 : 1));
  const savedMessages = sourceCases.map((source) => [
    ...(source.assistantBefore ? [{ id: `${source.id}-assistant`, role: "assistant", content: "已记录原始请求。", reasoning: null, tool_calls: null, tool_call_id: null }] : []),
    userMessage(`${source.id}-request`, instructionFor(source), source.origin),
  ]);
  await installSavedChatFixture(page, conversations, savedMessages);
  await enterAgent(page);

  for (const source of sourceCases) {
    await openConversation(page, source.title, "install");
    const card = page.getByTestId("mod-request-card");
    await expect(card).toBeVisible();
    await expect(card.getByText("安装 Mod", { exact: true })).toBeVisible();
    await expect(card.getByText("游览", { exact: true })).toBeVisible();
    await expect(card.getByText(source.sourceName, { exact: true })).toBeVisible();
    await expect(card.getByText(`项目 ID：${source.projectId}`, { exact: true })).toBeVisible();
    await expect(card.getByText(source.gameId === "game-stardew" ? "Stardew Valley" : "Valheim", { exact: true })).toBeVisible();

    const details = card.locator("details");
    await card.getByText("查看标识", { exact: true }).click();
    await expect(details).toHaveJSProperty("open", true);
    for (const identifier of [source.gameId, source.sourceId, source.providerGameId, source.projectId]) {
      await expect(details.getByText(identifier, { exact: true }).first()).toBeVisible();
    }
    if (source.catalogSourceId) await expect(details.getByText(source.catalogSourceId, { exact: true }).first()).toBeVisible();
    await card.getByText("查看标识", { exact: true }).click();
    await expect(details).toHaveJSProperty("open", false);

    if (source === sourceCases[0] && testInfo.project.name === "desktop-1280") {
      await setTheme(page, "dark");
      await expect(details).toHaveJSProperty("open", false);
      await page.screenshot({ path: testInfo.outputPath("browse-chat-request-dark-collapsed.png") });

      const savedContent = instructionFor(source);
      const message = card.locator("xpath=ancestor::article");
      const actions = message.getByRole("group", { name: "消息操作", exact: true });
      await actions.getByRole("button", { name: "复制消息", exact: true }).click();
      await expect(actions.getByRole("button", { name: "已复制", exact: true })).toBeVisible();
      const copiedContent = await page.evaluate(() => navigator.clipboard.readText());
      expect(copiedContent.replace(/\r\n?/g, "\n")).toBe(savedContent.replace(/\r\n?/g, "\n"));

      await actions.getByRole("button", { name: "编辑消息", exact: true }).click();
      const composer = page.getByRole("textbox", { name: "告诉 Agent 你想做什么", exact: true });
      await expect(composer).toHaveValue(savedContent);
      await page.getByRole("button", { name: "取消编辑", exact: true }).click();
      await expect(card).toBeVisible();
    }

    for (const theme of ["light", "dark"] as const) {
      await setTheme(page, theme);
      await expect(card).toBeVisible();
      const fits = await card.evaluate((element) => element.scrollWidth <= element.clientWidth + 1);
      expect(fits, `${source.sourceName} card should fit in ${theme} theme`).toBe(true);
      const bounds = await card.boundingBox();
      expect(bounds).not.toBeNull();
      expect(bounds!.x).toBeGreaterThanOrEqual(0);
      expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(testInfo.project.use.viewport!.width as number + 1);
    }
    await setTheme(page, "light");
  }

  const unrelatedLookups = await page.evaluate(() => (window as any).__AMM_TEST__.calls
    .map((call: { command: string }) => call.command)
    .filter((command: string) => [
      "browse_get_page", "browse_open_project", "browse_install_task", "list_browse_install_tasks",
      "list_installs", "list_mod_library",
    ].includes(command)));
  expect(unrelatedLookups).toEqual([]);
});

test("新版 stock 安装提示在聊天中显示请求卡片", async ({ page }) => {
  const source = sourceCases[0];
  const id = "current-stock-install";
  const title = "新版 Nexus 安装";
  await installSavedChatFixture(page, [conversation(id, title, "install", source.gameId, 1)], [[
    userMessage(`${id}-request`, instructionFor(source, { currentStock: true }), { kind: "user" }),
  ]]);
  await enterAgent(page);
  await openConversation(page, title, "install");
  const request = page.locator(`[data-message-id="${id}-request"]`);
  const card = page.getByTestId("mod-request-card").and(request);
  await expect(card.getByText(source.sourceName, { exact: true })).toBeVisible();
  await expect(card.getByText(`项目 ID：${source.projectId}`, { exact: true })).toBeVisible();
  await expect(request).not.toContainText(nativePrefix);
  await expect(request).not.toContainText("file_received_mod");
});

test("普通会话、编辑来源、后续用户消息和身份不完整的请求保留原文", async ({ page }) => {
  const nexus = sourceCases[0];
  const normalTitle = "普通会话粘贴的模板";
  const editedTitle = "编辑过的安装消息";
  const laterTitle = "后续轮次中的模板";
  const partialTitle = "附加文字的模板";
  const identityTitle = "来源身份不匹配";
  const conversations = [
    conversation("ordinary", normalTitle, null, "game-stardew", 1),
    conversation("edited", editedTitle, "install", "game-stardew", 1),
    conversation("later", laterTitle, "install", "game-stardew", 2),
    conversation("partial", partialTitle, "install", "game-stardew", 1),
    conversation("identity", identityTitle, "install", "game-stardew", 1),
  ];
  const fullPrompt = instructionFor(nexus);
  const savedMessages = [
    [userMessage("ordinary-request", fullPrompt, { kind: "user" })],
    [userMessage("edited-request", fullPrompt, { kind: "edited" })],
    [
      userMessage("later-first-user", "这条是安装会话中的首条用户消息。", { kind: "user" }),
      userMessage("later-request", fullPrompt, { kind: "user" }),
    ],
    [userMessage("partial-request", instructionFor(nexus, { currentStock: true, append: "\n用户追加说明。" }), { kind: "user" })],
    [userMessage("identity-request", instructionFor(nexus, { catalogSourceId: "thunderstore" }), { kind: "user" })],
  ];
  await installSavedChatFixture(page, conversations, savedMessages);
  await enterAgent(page);

  for (const [title, category, messageId] of [
    [normalTitle, "ordinary", "ordinary-request"],
    [editedTitle, "install", "edited-request"],
    [laterTitle, "install", "later-request"],
    [partialTitle, "install", "partial-request"],
    [identityTitle, "install", "identity-request"],
  ] as const) {
    await openConversation(page, title, category);
    await expect(page.getByTestId("mod-request-card")).toHaveCount(0);
    const original = page.locator(`[data-message-id="${messageId}"]`);
    await expect(original).toBeVisible();
    await expect(original).toContainText("用户从浏览卡片发起了本机安装。");
  }
});
