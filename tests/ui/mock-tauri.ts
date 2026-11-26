import type { Page } from "@playwright/test";
import type { GameCandidate } from "../../src/lib/types";

type WorkspaceGameFixture = {id: string; name: string};
type GameLaunchOptionFixture = {id: string; label: string | null};
type GameModLauncherFixture = {name: string; path: string; args: string[]};
type AskUserQuestionFixture = {question: string; options?: string[]; multiSelect?: boolean};
type AskUserPreviewFixture = {title?: string; questions: AskUserQuestionFixture[]} | {question: string};

export const PLUGIN_CREATED_AT_FIXTURE = "2026-11-12T08:15:00Z";
export const PLUGIN_UPDATED_AT_FIXTURE = "2026-11-20T16:45:00Z";

// Test-only IPC transport. It is never imported by the shipped application.
export async function installMockTauri(page: Page, {
  modLibrary = [], onboarding = false, onboardingStarted = false, failRegistration = false, emptyLibrary = false,
  dashboardSponsors = ["Fixture Sponsor"],
  dashboardStats,
  gameRootPath = "/sandbox/minecraft", gameLaunchPath, gameSteamAppId = null, gameModLauncher, gameLaunchOptions, failGameLaunchOptions = false, failGameLaunchMethods = [], gameWithoutAdapter = false, gameWithoutMods = false, adapterDisabled = false, withExternalAdapter = false, holdAgentTurn = false,
  gamePlanConflicts = [],
  pluginDateFixture = "valid",
  failInstalledPluginRemoval = null,
  duplicatePluginDraftSlug = false, failPluginDraftDeletion = false, seedPublishedPluginRecord = false,
  providerModels = ["fixture-thinking", "fixture-standard"],
  configuredModels,
  appLocale,
  systemLocale = "zh-CN",
  askUser,
  storage,
  workspaceGames = [],
  conversationFixtures,
  appUpdate,
  failAppUpdateRoute = false,
  pendingGameTools = [], mcpCredentialStatuses = {}, failMcpCredentialStatusIds = [], failFirstMcpSecretWrites = [], holdNextMcpCredentialStatus = false,
}: {
  modLibrary?: Array<{artifactId: string; gameId: string | null; gameName: string; name: string; filename: string; version: string | null; sha256: string; projectPage?: string | null}>;
  onboarding?: boolean;
  onboardingStarted?: boolean;
  emptyLibrary?: boolean;
  dashboardSponsors?: string[];
  dashboardStats?: {managedGameCount: number; installedModCount: number; totalModCount: number};
  failRegistration?: boolean;
  gameRootPath?: string;
  gameLaunchPath?: string;
  gameSteamAppId?: string | null;
  gameModLauncher?: GameModLauncherFixture;
  gameLaunchOptions?: GameLaunchOptionFixture[];
  failGameLaunchOptions?: boolean;
  failGameLaunchMethods?: string[];
  gameWithoutAdapter?: boolean;
  gameWithoutMods?: boolean;
  adapterDisabled?: boolean;
  withExternalAdapter?: boolean;
  holdAgentTurn?: boolean;
  gamePlanConflicts?: Array<Record<string, unknown>>;
  pluginDateFixture?: "valid" | "null-and-missing";
  failInstalledPluginRemoval?: "once" | "always" | null;
  duplicatePluginDraftSlug?: boolean;
  failPluginDraftDeletion?: boolean;
  seedPublishedPluginRecord?: boolean;
  providerModels?: string[];
  configuredModels?: import("../../src/lib/types").AgentModelSettings[];
  appLocale?: string | null;
  systemLocale?: string | null;
  askUser?: {preview: AskUserPreviewFixture; approvalId?: string; toolCallId?: string; deferResult?: boolean};
  workspaceGames?: WorkspaceGameFixture[];
  conversationFixtures?: Array<Record<string, unknown>>;
  pendingGameTools?: import("../../src/lib/mcp-connection").McpConfig[];
  mcpCredentialStatuses?: Record<string, Record<string, boolean>>;
  failMcpCredentialStatusIds?: string[];
  failFirstMcpSecretWrites?: string[];
  holdNextMcpCredentialStatus?: boolean;
  appUpdate?: {
    status?: 'idle' | 'checking' | 'downloading' | 'ready' | 'upToDate' | 'error' | 'unsupported';
    version?: string;
    message?: string;
    checkResult?: {status: 'idle' | 'checking' | 'downloading' | 'ready' | 'upToDate' | 'error' | 'unsupported'; version?: string; message?: string};
    pollStatuses?: {status: 'idle' | 'checking' | 'downloading' | 'ready' | 'upToDate' | 'error' | 'unsupported'; version?: string; message?: string}[];
  };
  failAppUpdateRoute?: boolean;
  storage?: {
    dataRoot: string;
    parent: string;
    previousError?: string;
    rejectFirstMigration?: boolean;
    holdMigration?: boolean;
    nextBootstrap?: {dataRoot?: string; storageMigrationError?: string | null};
  };
} = {}) {
  // The public-looking manifest image is served offline in browser tests.
  await page.route("https://covers.example.org/minecraft-java.jpg", route => route.fulfill({contentType: "image/png", path: "public/favicon.png"}));
  await page.addInitScript(({modLibrary, onboarding, onboardingStarted, failRegistration, emptyLibrary, dashboardSponsors, dashboardStats, gameRootPath, gameLaunchPath, gameSteamAppId, gameModLauncher, gameLaunchOptions, failGameLaunchOptions, failGameLaunchMethods, gameWithoutAdapter, gameWithoutMods, adapterDisabled, withExternalAdapter, holdAgentTurn, gamePlanConflicts, pluginDateFixture, pluginCreatedAt, pluginUpdatedAt, failInstalledPluginRemoval, duplicatePluginDraftSlug, failPluginDraftDeletion, seedPublishedPluginRecord, providerModels, configuredModels, appLocale, systemLocale, askUser, storage, workspaceGames, conversationFixtures, appUpdate, failAppUpdateRoute, pendingGameTools, mcpCredentialStatuses, failMcpCredentialStatusIds, failFirstMcpSecretWrites, holdNextMcpCredentialStatus}) => {
    type Args = Record<string, any>;
    const now = "2026-11-18T00:00:00Z";
    const draftUpdatedAtField = pluginDateFixture === "valid" ? {updatedAt: pluginUpdatedAt} : {};
    const installedUpdatedAtField = pluginDateFixture === "valid" ? {updatedAt: pluginUpdatedAt} : {updatedAt: null};
    const callbacks = new Map<number, (value: any) => void>();
    const listeners = new Map<number, { event: string; handler: number }>();
    let counter = 0;
    let seq = 0;
    let migrationAttempts = 0;
    let releaseStorageMigration: (() => void) | undefined;
    let updateStatus = {status: appUpdate?.status ?? 'idle', version: appUpdate?.version, message: appUpdate?.message};
    const updatePollStatuses = structuredClone(appUpdate?.pollStatuses ?? []);
    let updatePollingStarted = false;
    const credentialStatuses = structuredClone(mcpCredentialStatuses);
    const failedCredentialStatusIds = new Set(failMcpCredentialStatusIds);
    const failedSecretWrites = new Set(failFirstMcpSecretWrites);
    let holdCredentialStatus = holdNextMcpCredentialStatus;
    let releaseCredentialStatus: (() => void) | undefined;
    const storageBootstrapKey = "amm.test.storageBootstrapAfterMigration";
    const restoredStorage = storage?.nextBootstrap ? JSON.parse(sessionStorage.getItem(storageBootstrapKey) ?? "{}") : {};
    let pendingGameConflicts = structuredClone(gamePlanConflicts ?? []);
    const calls: Array<{ command: string; args: Args }> = [];
    const savedQuestionnaire = askUser ? JSON.parse(sessionStorage.getItem("amm.test.questionnaire") ?? "null") : null;
    const events: any[] = savedQuestionnaire?.events ?? [];
    seq = events.at(-1)?.seq ?? seq;
    const replayedEvents: any[] = [];
    const initialGame: Args = {
      id: "game-fixture", name: "Minecraft Java Edition", slug: "minecraft-java",
      rootPath: gameRootPath, platform: "windows", adapterSlug: withExternalAdapter && !gameWithoutAdapter ? "minecraft-java" : null,
      adapterVersion: withExternalAdapter && !gameWithoutAdapter ? "1.0.0" : null, confidence: 1, coverPath: null,
      lastScannedAt: now, installedModCount: gameWithoutMods ? 0 : 1, enabledModCount: gameWithoutMods ? 0 : 1,
      metadata: {...(gameLaunchPath === undefined ? {} : {launchPath: gameLaunchPath}), ...(gameModLauncher ? {modLauncher: gameModLauncher} : {})},
      steamAppId: gameSteamAppId, createdAt: now, updatedAt: now,
    };
    const games: Args[] = emptyLibrary || (onboarding && !onboardingStarted) ? [] : [
      gameWithoutAdapter ? {...initialGame, adapterSlug: null, adapterVersion: null} : initialGame,
    ];
    if (games.length > 0) games.push(...workspaceGames.map((game: WorkspaceGameFixture) => ({
      ...initialGame, id: game.id, name: game.name, slug: game.id, rootPath: `/sandbox/${game.id}`,
      adapterSlug: null, adapterVersion: null, installedModCount: 0, enabledModCount: 0, metadata: {},
    })));
    const availableLaunchOptions = (game: Args): GameLaunchOptionFixture[] => {
      if (gameLaunchOptions !== undefined) return structuredClone(gameLaunchOptions);
      const options: GameLaunchOptionFixture[] = [];
      if (game.steamAppId) options.push({id: "steam", label: null});
      if (game.metadata?.launchPath) options.push({id: "direct", label: null});
      if (game.metadata?.modLauncher) options.push({id: "custom", label: game.metadata.modLauncher.name});
      return options;
    };
    const onboardingCandidates: GameCandidate[] = [
      {rootPath: "/sandbox/steam/Fixture", name: "Fixture Game", confidence: 0.92, status: "high_confidence", suggestedAdapterSlug: withExternalAdapter ? "minecraft-java" : null, registeredGameId: null, platform: "windows", warnings: ["这是一个测试扫描候选，请确认目录后再添加"]},
      {rootPath: "/sandbox/steam/Broken", name: "Broken Fixture", confidence: 0.6, status: "candidate", suggestedAdapterSlug: null, registeredGameId: null, platform: "unknown", warnings: []},
    ];
    const libraryCandidates = onboardingCandidates.map((item, i) => ({...item,
      rootPath: item.rootPath.replace("/sandbox/steam", "/sandbox/new-game"),
      confidence: i === 0 ? 1 : item.confidence,
    }));
    const conversations: Args[] = savedQuestionnaire?.conversations ?? (conversationFixtures ? structuredClone(conversationFixtures) : [{
      id: "conversation-fixture", title: "检查 Minecraft 模组", game_id: "game-fixture", category: null,
      task_summary: null, archived: false, pinned: false, message_count: 0,
      created_at: now, updated_at: now, permission_mode: null,
    }]);
    const seededMessages = conversationFixtures?.map((conversation: Record<string, unknown>) => [String(conversation.id), []] as [string, any[]])
      ?? [["conversation-fixture", []] as [string, any[]]];
    const messages = new Map<string, any[]>(savedQuestionnaire?.messages ?? seededMessages);
    const approvalId = askUser?.approvalId ?? "approval-fixture";
    const toolCallId = askUser?.toolCallId ?? "tool-fixture";
    let activeConversation = "conversation-fixture";
    let pending = false;
    let deferredQuestionResult: (() => void) | undefined;
    let restored = false;
    let purged = false;
    let pluginEnabled = withExternalAdapter && !adapterDisabled;
    let minecraftSkillEnabled = true;
    let skillRemoved = false;
    let skillEnabled = true;
    let fileTrashed = false;
    let directoryPickCount = 0;
    let modsFolderMissing = false;
    let archiveImported = false;
    let importedEntry: Args | null = null;
    let fixtureUninstalled = false;
    let fixtureRolledBack = false;
    const fixtureRollbackTransactionId = "txn-mod-rollback";
    const archiveInstallRecords: Args[] = [];
    let deployment = 0;
    const removedRecords = new Set<string>();
    const removedArchives = new Set<string>();
    let modAction: {name: "install_library_mod" | "uninstall_mod"; target: Args; preview: Args} | null = null;
    const modActionControls: {outcome: "success" | "error" | "done-without-state" | "question" | "rollback"; questionPreview: Args; failNextReadback: boolean} = {
      outcome: "success",
      questionPreview: {title: "安装前确认", questions: [{question: "请确认此 Mod 的安装目录。"}]},
      failNextReadback: false,
    };
    let identitySearch: Args | null = null;
    let publicationOutcome = 'completed';
    let publicationReconcileOutcome: 'verified' | 'unconfirmed' = 'verified';
    const noSuggestion = {status: 'candidate' as const, suggestedAdapterSlug: null, registeredGameId: null, platform: 'unknown', warnings: []};
    const unknownCandidate = {...noSuggestion, rootPath: '/sandbox/unknown-game', name: 'unknown-game', confidence: 0.7 / 9, matches: [{slug: 'baldurs-gate-3', confidence: 0.7 / 9, matchedMarkers: ['Data']}], sources: [], requiresConfirmation: true};
    const localMatch = {...libraryCandidates[0], suggestedAdapterSlug: withExternalAdapter ? libraryCandidates[0].suggestedAdapterSlug : null, warnings: []};
    const inspectedCandidates: GameCandidate[] = [
      unknownCandidate,
      {...noSuggestion, rootPath: '/sandbox/below-threshold', name: 'below-threshold', confidence: 0.799999},
      {...localMatch, rootPath: '/sandbox/at-threshold', confidence: 0.8},
      {...noSuggestion, rootPath: '/sandbox/near-tie', name: 'near-tie', confidence: 0.92, warnings: ['多个适配包具有接近的得分，需要人工选择']},
      // A directory-only probe can score higher than a scan with launcher metadata.
      {...localMatch, rootPath: onboardingCandidates[1].rootPath},
      {...localMatch, rootPath: libraryCandidates[1].rootPath},
    ];
    const publications: Args[] = seedPublishedPluginRecord ? [{
      id: 'publish-fixture-retained', state: 'completed', phase: 'completed', fileId: 42, workId: 99,
      createdAt: now, updatedAt: now,
      preview: {package: {title: '已发布 fixture-game', slug: 'fixture-game', version: '1.0.0'}},
    }] : [];
    const draft = {id: 'draft-fixture', slug: 'fixture-game', path: '/sandbox/data/staging/plugin-drafts/fixture', writeTransactionId: 'txn-draft-fixture', createdAt: pluginCreatedAt, ...draftUpdatedAtField};
    const pluginDrafts: Args[] = [draft, ...(duplicatePluginDraftSlug ? [{
      ...draft, id: 'draft-fixture-duplicate', path: '/sandbox/data/staging/plugin-drafts/fixture-duplicate',
      writeTransactionId: 'txn-draft-fixture-duplicate',
    }] : [])];
    let settings: Args = { base_url: "https://example.com/v1", model: onboarding ? "" : "mock-model", models: configuredModels, permission_mode: "request_approval", api_key_ref: "model.api_key", onboarding_completed: !onboarding };
    let composerPreferences: Args = {model: null, reasoning_effort: null, permission_mode: null};
    const appLocales: Array<[string, string]> = [
      ["zh-CN", "简体中文"], ["en", "English"], ["ja", "日本語"], ["ko", "한국어"], ["de", "Deutsch"],
      ["fr", "Français"], ["es", "Español"], ["pt-BR", "Português (Brasil)"], ["ru", "Русский"], ["it", "Italiano"],
    ];
    const appLocaleStorageKey = "amm.test.appLocale";
    const storedAppLocale = sessionStorage.getItem(appLocaleStorageKey);
    let selectedAppLocale: string | null = appLocales.some(([code]) => code === storedAppLocale)
      ? storedAppLocale
      : appLocales.some(([code]) => code === appLocale) ? appLocale : null;
    let simulatedSystemLocale = systemLocale;
    const effectiveLocale = () => selectedAppLocale ?? (
      appLocales.find(([code]) => code.split("-")[0].toLowerCase() === (simulatedSystemLocale ?? "").trim().split(/[-_.@]/)[0].toLowerCase())?.[0]
      ?? "en"
    );
    let onboardingCompleted = !onboarding;
    let onboardingInProgress = onboardingStarted;
    const plan = {
      id: "txn-fixture", summary: "还原 Example.jar", total_bytes: 1024, created_at: now,
      requires_review: true,
      entries: [{ operation: { op: "move", src: "/sandbox/data/trash/trash_fixture/payload", dst: "/sandbox/minecraft/mods/Example.jar" }, before: null,
        source: { kind: "file", size: 1024, sha256: "a".repeat(64) }, impact: "move" }],
    };
    const skillPlan = {
      ...plan, id: "txn-skill-fixture", summary: "卸载独立 Skill my-game-notes",
      entries: [{...plan.entries[0], operation: {op: "move", src: "/sandbox/data/skills/my-game-notes/SKILL.md", dst: "/sandbox/data/trash/trash_skill/payload"}}],
    };
    const filePlan = {
      ...plan, id: "txn-file-fixture", summary: "移入回收站",
      entries: [{...plan.entries[0], operation: {op: "move", src: "/sandbox/minecraft/options.txt", dst: "/sandbox/data/trash/trash_options/payload"}}],
    };
    const importPlan = {
      ...plan, id: "txn-import-fixture", summary: "导入本地归档到管理器：Fixture.jar",
      entries: [{...plan.entries[0], impact: "add", operation: {op: "copy", src: "/sandbox/data/staging/import-scratch/fixture/Fixture.jar", dst: "/sandbox/data/staging/imports/fixture/Fixture.jar"}}],
    };
    const installPlan = {
      ...plan, id: "txn-install-fixture", summary: "安装 Fixture",
      entries: [{...plan.entries[0], operation: {op: "copy", src: "/sandbox/data/staging/imports/fixture/Fixture.jar", dst: "/sandbox/minecraft/mods/Fixture.jar"}}],
    };
    const skill = () => ({name: "my-game-notes", description: "本地游戏说明", location: "/sandbox/data/skills/my-game-notes/SKILL.md", source: "external", kind: "standalone", valid: true, enabled: skillEnabled, issues: []});
    const systemSkills = [
      {name: "system-adapter-authoring", description: "创建或更新游戏适配包。", instruction: "先确认目标游戏和适配需求。"},
      {name: "system-web-search", description: "查找游戏、模组和加载器资料。", instruction: "仅发送公开的搜索关键词。"},
      {name: "system-mod-download", description: "从来源定位版本并下载 Mod。", instruction: "先核对目标版本和加载器。"},
      {name: "system-mod-commission", description: "引导玩家到 Mod 客栈创建委托。", instruction: "先区分查找现有作品和定制需求。"},
      {name: "system-mod-install", description: "从游戏分类库安装指定 Mod。", instruction: "先核对本地归档和安装规则。"},
      {name: "system-mod-uninstall", description: "卸载已登记的 Mod。", instruction: "先核对安装记录和文件归属。"},
      {name: "system-mod-organize", description: "安装成功后由管理器自动整理 Mod 展示名与分类标签。", instruction: "根据已知资料生成展示名和分类标签。"},
    ].map(({name, description, instruction}) => ({
      skill: {name, description, location: `plugins/skills/${name}/SKILL.md`, source: "system", kind: "system", valid: true, enabled: true, issues: []},
      markdown: `---\nname: ${name}\ndescription: ${description}\n---\n\n${instruction}`,
    }));
    const minecraftPlugin = {
      slug: "minecraft-java", name: "Minecraft Java Edition", gameName: "Minecraft Java Edition",
      version: "1.0.0", description: "Minecraft 模组安装指引", source: "external", validated: true,
      installPath: "/sandbox/data/plugins/games/minecraft-java", coverUrl: "https://covers.example.org/minecraft-java.jpg",
      ...installedUpdatedAtField,
    };
    const installedPlugins: Args[] = withExternalAdapter ? [{...minecraftPlugin, enabled: pluginEnabled}] : [];
    const cloudPluginSlugs = new Set<string>();
    const withCloudEvidence = (plugin: Args) => ({...plugin, hasCloudEvidence: cloudPluginSlugs.has(plugin.slug)});
    let pendingPluginDraft: Args | null = null;
    let pluginDraftWritePlan: Args | null = null;
    let pluginInstallPlan: Args | null = null;
    let pendingAdapterJson = "";
    let pendingSkillMarkdown = "";
    let nextPluginDraft = 0;
    const minecraftSkill = () => ({
      skill: {
        name: minecraftPlugin.slug, description: minecraftPlugin.description, location: `${minecraftPlugin.installPath}/SKILL.md`,
        source: minecraftPlugin.source, kind: "external", valid: true, enabled: pluginEnabled && minecraftSkillEnabled, issues: [],
      },
      markdown: "---\nname: minecraft-java\ndescription: Minecraft 模组安装指引\n---\n\n# Minecraft Java Edition 适配指导\n\n核对加载器版本和模组依赖。",
    });
    const persistOnly = (kind: string, payload: any, conversationId = activeConversation, messageId: string | null = "assistant-fixture") => {
      const value = { seq: ++seq, turnId: "turn-fixture", conversationId, messageId, kind, payload, createdAt: now };
      events.push(value);
      if (kind === "done") pending = false;
      return value;
    };
    const emitEvent = (event: string, payload: unknown) => {
      for (const [id, listener] of listeners) {
        if (listener.event === event) callbacks.get(listener.handler)?.({ event, id, payload });
      }
    };
    const emit = (kind: string, payload: any, conversationId = activeConversation, messageId: string | null = "assistant-fixture") => {
      const value = persistOnly(kind, payload, conversationId, messageId);
      emitEvent(`agent://${kind}`, value);
      return value;
    };
    const delta = (text: string) => {
      const message = messages.get(activeConversation)?.find((m) => m.id === "assistant-fixture");
      if (message) message.content += text;
      emit("delta", { content: text, reasoning: "" });
    };
    const finish = ({approved, answer}: {approved: boolean; answer?: string | null}, prompted = true) => {
      pending = false;
      if (prompted) emit("approval-resolved", { approvalId, status: approved ? "approved" : "rejected" });
      const publishResult = () => {
        const questionAction = !!modAction && modActionControls.outcome === "question";
        if (askUser || questionAction) {
          const preview = askUser?.preview ?? modActionControls.questionPreview;
          const submitted = approved ? "questions" in preview ? JSON.parse(answer!) : answer : null;
          emit("tool-result", {toolCallId, status: "done", result: {answer: submitted}});
        } else {
          let installReceipt: Args | undefined;
          const actionSucceeded = approved && (modActionControls.outcome === "success"
            || (modActionControls.outcome === "rollback" && modAction?.name === "uninstall_mod"));
          if (actionSucceeded && modAction?.name === "uninstall_mod") {
            if (modAction.target.installId === "install-fixture") {
              if (!fixtureUninstalled && !fixtureRolledBack) {
                fixtureUninstalled = modActionControls.outcome !== "rollback";
                fixtureRolledBack = modActionControls.outcome === "rollback";
                initialGame.installedModCount = Math.max(0, initialGame.installedModCount - 1);
                initialGame.enabledModCount = Math.max(0, initialGame.enabledModCount - 1);
              }
            }
            else {
              const record = archiveInstallRecords.find(item => item.id === modAction!.target.installId);
              if (record && record.status !== "uninstalled") {
                record.status = modActionControls.outcome === "rollback" ? "rolled_back" : "uninstalled";
                if (modActionControls.outcome === "rollback") record.metadata.transactionId = fixtureRollbackTransactionId;
                initialGame.installedModCount = Math.max(0, initialGame.installedModCount - 1);
                initialGame.enabledModCount = Math.max(0, initialGame.enabledModCount - 1);
              }
            }
          } else if (actionSucceeded && modAction?.name === "install_library_mod") {
            const artifactId = modAction.target.artifactId;
            const entry = [...modLibrary, importedEntry].find(item => item?.artifactId === artifactId);
            if (!entry) throw new Error("unknown library entry");
            // Native library installs create a new deployment/transaction, including for reinstalls.
            const installId = `install-${++deployment}-${artifactId}`;
            const transactionId = `txn-${installId}`;
            archiveInstallRecords.push({id: installId, gameId: initialGame.id, name: entry.name, version: entry.version,
              source: entry.sourceId ?? "local", status: "enabled", installRoot: `${gameRootPath}/mods`, files: [],
              metadata: {libraryEntryId: artifactId, transactionId}, installedAt: now});
            initialGame.installedModCount += 1;
            initialGame.enabledModCount += 1;
            installReceipt = {installId, transactionId, fileCount: 1};
          }
          if (modActionControls.outcome === "error" && approved) {
            emit("tool-result", {toolCallId, ...(modAction && {name: modAction.name}), status: "error",
              result: {error: "测试模拟 Mod 操作执行失败"}});
          } else if (approved && modActionControls.outcome === "rollback") {
            emit("tool-result", {toolCallId, name: "rollback_transaction", status: "done", result: {
              status: "rolled_back", gameId: modAction?.target.gameId, installId: modAction?.target.installId,
              transactionId: fixtureRollbackTransactionId,
            }});
          } else {
            emit("tool-result", {toolCallId, ...(modAction && {name: modAction.name}), status: approved ? "done" : "denied",
              result: {status: approved ? "committed" : "denied", ...(installReceipt ?? (modActionControls.outcome === "done-without-state"
                ? {installId: "missing-install", transactionId: "missing-txn"} : {}))}});
          }
        }
        delta((askUser || modActionControls.outcome === "question")
          ? approved ? "\n回答已提交。" : "\n回答已跳过。"
          : approved ? modActionControls.outcome === "error" ? "\n操作失败。"
            : modActionControls.outcome === "rollback" ? "\n回滚已完成。" : "\n检查完成，文件计划已执行。" : "\n未执行文件操作。");
        emit("usage", { prompt_tokens: 100, completion_tokens: 20, total_tokens: 120 });
        emit("done", { status: "done" }, activeConversation, null);
        if (askUser) sessionStorage.setItem("amm.test.questionnaire", JSON.stringify({events, conversations, messages: [...messages]}));
      };
      if (askUser?.deferResult) deferredQuestionResult = publishResult;
      else publishResult();
    };
    const invoke = async (command: string, args: Args = {}) => {
      calls.push({command, args: command === "list_agent_models" ? {baseUrl: args.baseUrl, authenticated: !!args.apiKey} : args});
      if (command === "plugin:event|listen") {
        const id = ++counter; listeners.set(id, { event: args.event, handler: args.handler }); return id;
      }
      if (command === "plugin:event|unlisten") { listeners.delete(args.eventId); return null; }
      if (command === "plugin:window|is_maximized") return false;
      if (command.startsWith("plugin:window|") || command.startsWith("plugin:opener|")) return null;
      switch (command) {
        case "get_bootstrap_state": {
          const bootstrap = { dataRoot: storage?.dataRoot ?? "/sandbox/data", storageMigrationError: storage?.previousError ?? null,
            recoveredTransactions: [], recoveryError: null, onboardingCompleted, isFirstRun: !onboardingCompleted && games.length === 0,
            resumeOnboarding: !onboardingCompleted && onboardingInProgress };
          return {...bootstrap, ...restoredStorage};
        }
        case "plugin:path|join": return args.paths.join("\\");
        case "migrate_storage": {
          if (!storage || args.parent !== storage.parent) throw new Error("unexpected storage location");
          const attempt = ++migrationAttempts;
          if (storage.holdMigration) await new Promise<void>((resolve) => { releaseStorageMigration = () => resolve(); });
          if (attempt === 1 && storage.rejectFirstMigration) throw new Error("所选位置已包含 Agent Mod Manager 文件夹");
          if (storage.nextBootstrap) {
            try { sessionStorage.setItem(storageBootstrapKey, JSON.stringify(storage.nextBootstrap)); }
            catch { /* The migration call still reports its configured result. */ }
          }
          return null;
        }
        case "plugin:dialog|open": {
          if (storage && args.options?.title === "选择存储位置") return storage.parent;
          if (!args.options?.directory) return "/sandbox/incoming/Fixture.jar";
          return ++directoryPickCount === 1 ? "/sandbox/new-game" : null;
        }
        case "list_games": return structuredClone(games);
        case "list_dashboard_sponsors": return structuredClone(dashboardSponsors);
        case "get_dashboard_stats": return structuredClone(dashboardStats ?? {
          managedGameCount: games.length,
          installedModCount: games.reduce((count, game) => count + Number(game.installedModCount ?? 0), 0),
          totalModCount: games.reduce((count, game) => count + Number(game.installedModCount ?? 0), 0),
        });
        case "list_game_conflicts": return structuredClone(pendingGameConflicts
          .filter((conflict: any) => !Array.isArray(conflict.blockedChanges) || conflict.blockedChanges.includes(args.change))
          .map((conflict: any) => {
            const {blockedChanges: _blockedChanges, ...wireConflict} = conflict;
            return wireConflict;
          }));
        case "resolve_game_conflict": {
          const index = pendingGameConflicts.findIndex((conflict: any) => conflict.id === args.conflictId
            && (!Array.isArray(conflict.blockedChanges) || conflict.blockedChanges.includes(args.change)));
          if (index < 0) throw new Error("待处理任务不存在或已解除");
          pendingGameConflicts.splice(index, 1);
          return null;
        }
        case "identify_game": {
          const candidate = inspectedCandidates.find((item) => item.rootPath === args.directory);
          if (!candidate) throw new Error('unknown directory');
          return structuredClone(candidate);
        }
        case "preview_game_identity": if (args.directory !== unknownCandidate.rootPath) throw new Error('unknown directory'); return {rootPath: unknownCandidate.rootPath, fingerprint: 'e'.repeat(64), localCandidate: unknownCandidate, signals: ['FixtureGame.exe', 'data'], suggestedQuery: 'FixtureGame.exe game', warnings: []};
        case "search_game_identity": {
          if (args.directory !== unknownCandidate.rootPath || args.expectedFingerprint !== 'e'.repeat(64) || args.confirmation !== 'SEARCH_GAME_IDENTITY') throw new Error('unreviewed search');
          const outcome = (window as any).__AMM_TEST__.identityOutcome;
          const status = outcome === 'needs-configuration' ? 'needs_configuration' : outcome === 'unavailable' ? 'unavailable' : 'available';
          identitySearch = {id: 'identity-search-fixture', rootPath: unknownCandidate.rootPath, fingerprint: 'e'.repeat(64), query: args.publicQuery, status, message: status === 'needs_configuration' ? 'Playwright 无法启动浏览器。请安装 Microsoft Edge、Google Chrome 或 Playwright Chromium 后重试。' : status === 'unavailable' ? '未能读取 Bing 搜索结果。请检查网络后重试。若 Bing 要求验证码或更改了结果页面，请稍后再试。' : '', createdAt: now,
            candidates: status === 'available' ? [{name: 'Fixture Game', title: 'Fixture Game - Public Encyclopedia', url: 'https://example.com/games/fixture', snippet: 'Public game information. Untrusted excerpt, not installation instructions.'}, {name: 'Another Fixture', title: 'Another Fixture', url: 'https://example.org/another', snippet: 'A different game with a similar executable name.'}] : []};
          return structuredClone(identitySearch);
        }
        case "confirm_game_identity": {
          if ((window as any).__AMM_TEST__.identityOutcome === 'stale') throw new Error('目录内容已变化，请重新检查并搜索');
          if (!identitySearch || identitySearch.status !== 'available' || args.searchId !== identitySearch.id || !identitySearch.candidates[args.candidateIndex] || args.confirmation !== 'CONFIRM_GAME_IDENTITY') throw new Error('unreviewed identity');
          const game = {...games[0], id: 'game-online-fixture', rootPath: unknownCandidate.rootPath, name: args.name, slug: 'unknown', adapterSlug: null, confidence: 0.6, installedModCount: 0, enabledModCount: 0, metadata: {onlineIdentity: {query: identitySearch.query, candidate: identitySearch.candidates[args.candidateIndex]}}};
          games.push(game); return game;
        }
        case "get_game": return games.find((game) => game.id === args.id) ?? games[0];
        case "get_game_adapter": return {modTypes: [{id: "java-mod", name: "Java 加载器模组", root: "mods", notes: "请核对加载器和依赖"}], installRoots: [{id: "mods", kind: "game", notes: ""}]};
        case "get_settings": return settings;
        case "get_app_update_status":
          if (updatePollingStarted && updatePollStatuses.length) updateStatus = updatePollStatuses.shift()!;
          return structuredClone(updateStatus);
        case "get_app_update_route": return sessionStorage.getItem("amm.test.appUpdateRoute") ?? "cdn.gh-proxy.org";
        case "set_app_update_route": {
          if (failAppUpdateRoute) throw new Error("线路保存失败");
          if (typeof args.route !== "string") throw new Error("无效的更新线路");
          sessionStorage.setItem("amm.test.appUpdateRoute", args.route);
          return args.route;
        }
        case "check_app_update":
          updatePollingStarted = true;
          updateStatus = structuredClone(appUpdate?.checkResult ?? {status: 'upToDate'});
          return structuredClone(updateStatus);
        case "get_app_locale": return {locale: selectedAppLocale, effective_locale: effectiveLocale()};
        case "set_app_locale": {
          if (args.locale !== null && !appLocales.some(([code]) => code === args.locale)) {
            throw new Error("不支持的应用语言");
          }
          selectedAppLocale = args.locale;
          if (selectedAppLocale === null) sessionStorage.removeItem(appLocaleStorageKey);
          else sessionStorage.setItem(appLocaleStorageKey, selectedAppLocale);
          return {locale: selectedAppLocale, effective_locale: effectiveLocale()};
        }
        case "list_agent_models": return providerModels;
        case "get_agent_model_metadata": {
          if (args.baseUrl === "https://offline.example/v1") throw new Error("metadata unavailable");
          return args.modelIds.flatMap((id: string) => {
            if (id === "fixture-thinking") return [{id, reasoningLevels: ["low", "medium", "high"]}];
            if (id === "fixture-standard") return [{id, reasoningLevels: []}];
            return [];
          });
        }
        case "set_settings": settings = args.settings; onboardingCompleted = settings.onboarding_completed; return null;
        case "get_composer_preferences": return composerPreferences;
        case "set_composer_preferences": composerPreferences = args.preferences; return null;
        case "set_secret": {
          if (failedSecretWrites.has(args.key)) {
            failedSecretWrites.delete(args.key);
            throw new Error("模拟后端写入失败");
          }
          for (const status of Object.values(credentialStatuses)) {
            if (Object.hasOwn(status, args.key)) status[args.key] = true;
          }
          return null;
        }
        case "start_onboarding": onboardingInProgress = true; return null;
        case "set_onboarding_completed": onboardingCompleted = !!args.completed; settings = {...settings, onboarding_completed: onboardingCompleted}; return null;
        case "list_conversations": return conversations.filter((c) => c.archived === !!args.archived);
        case "create_conversation": {
          const workspaceGameIds = "workspaceGameIds" in args ? args.workspaceGameIds : args.gameId == null ? null : [args.gameId];
          const conversation = { ...conversations[0], id: `conversation-${++counter}`, title: args.title || "新对话", game_id: args.gameId ?? null, workspace_game_ids: structuredClone(workspaceGameIds), category: args.category ?? null, archived: false, message_count: 0 };
          conversations.push(conversation); messages.set(conversation.id, []); return structuredClone(conversation);
        }
        case "set_conversation_workspace": {
          const conversation = conversations.find((item) => item.id === args.id);
          if (!conversation) throw new Error("未找到对话");
          conversation.workspace_game_ids = structuredClone(args.gameIds);
          conversation.game_id = Array.isArray(args.gameIds) && args.gameIds.length === 1 ? args.gameIds[0] : null;
          conversation.updated_at = now;
          return structuredClone(conversation);
        }
        case "get_conversation": return { conversation: conversations.find((c) => c.id === args.id), messages: messages.get(args.id) ?? [] };
        case "archive_conversation": {
          const conversation = conversations.find((c) => c.id === args.id);
          if (conversation) conversation.archived = args.archived;
          return null;
        }
        case "set_conversation_permission": return null;
        case "replay_agent_events": {
          const replayed = events.filter((event) => event.conversationId === args.conversationId && event.seq > args.after).slice(0, args.limit || 2000);
          replayedEvents.push(...replayed);
          return replayed;
        }
        case "start_turn": {
          modAction = null;
          const conversation = conversations.find(c => c.id === args.request.conversationId);
          const permissionMode = args.request.permissionMode ?? conversation?.permission_mode ?? settings.permission_mode;
          const origin = args.origin ?? {kind: "user"};
          // Single-target tool simulation. The batch protocol fixture may ask
          // to replay only its first target without altering persisted content.
          const target = origin.kind === "mod_request" && (!origin.batch || args.mockFirstBatchTarget)
            ? origin.targets[0] : undefined;
          if (target) {
            const game = conversation?.game_id;
            if (game !== target.gameId || game !== initialGame.id) {
              throw new Error("Mod action requires exact game context");
            }
            if (target.artifactId) {
              const entry = [...modLibrary, importedEntry].find(entry => entry?.artifactId === target.artifactId);
              if (!entry || entry.gameId !== game) throw new Error("library artifact is not bound to the chosen game");
              modAction = {name: "install_library_mod", target, preview: {
                installRoot: "/sandbox/minecraft/mods", plan: {...installPlan, summary: `安装 ${entry.filename}`,
                  entries: [{...installPlan.entries[0], operation: {...installPlan.entries[0].operation,
                    dst: `/sandbox/minecraft/mods/${entry.filename}`}}]},
              }};
            } else {
              const archiveInstall = archiveInstallRecords.find(item => item.id === target.installId && item.status === "enabled");
              if ((target.installId === "install-fixture" && (fixtureUninstalled || fixtureRolledBack)) || (target.installId !== "install-fixture" && !archiveInstall)) {
                throw new Error("unrecognized installation");
              }
              modAction = {name: "uninstall_mod", target, preview: {...plan, summary: "卸载 Example（install-fixture）"}};
            }
          }
          activeConversation = args.request.conversationId;
          const history = messages.get(activeConversation) ?? [];
          history.push({ id: "user-fixture", role: "user", content: args.request.content,
            origin: origin.kind === "mod_request" ? {...origin, originalContent: args.request.content} : origin,
            ...(args.request.attachments?.length ? { attachments: args.request.attachments } : {}),
            reasoning: null, tool_calls: null, tool_call_id: null });
          history.push({ id: "assistant-fixture", role: "assistant", content: "", reasoning: null, tool_calls: null, tool_call_id: null });
          messages.set(activeConversation, history);
          // Deliberately emit before invoke resolves: a listener installed after
          // start_turn would lose this delta without durable replay.
          emit("turn-start", { permissionMode }, activeConversation, null);
          delta("正在检查模组文件。");
          pending = true;
          if (!holdAgentTurn) setTimeout(() => {
            if (!pending) return;
            let modArgs: Args | undefined;
            if (modAction?.name === "install_library_mod") {
              const {gameId, artifactId} = modAction.target;
              emit("tool-start", {toolCallId: "tool-match-library", name: "match_mod_install", arguments: {game_id: gameId, artifact_id: artifactId}});
              emit("tool-result", {toolCallId: "tool-match-library", name: "match_mod_install", status: "done", result: {status: "matched"}});
              modArgs = {request: {gameId, artifactId, mode: "adapter"}};
            } else if (modAction?.name === "uninstall_mod") {
              const inspectArgs = {game_id: modAction.target.gameId, install_id: modAction.target.installId};
              const rollbackAction = modActionControls.outcome === "rollback";
              emit("tool-start", {toolCallId: "tool-inspect-install", name: "inspect_install", arguments: inspectArgs});
              emit("tool-result", {toolCallId: "tool-inspect-install", name: "inspect_install", status: "done",
                result: {reconciliation: {recommendedAction: rollbackAction ? "rollback_install" : "safe_uninstall",
                  ...(rollbackAction && {transactionId: fixtureRollbackTransactionId})}}});
              modArgs = rollbackAction ? {...inspectArgs, transaction_id: fixtureRollbackTransactionId} : inspectArgs;
            }
            const questionAction = !!modAction && modActionControls.outcome === "question";
            const rollbackAction = !!modAction && modAction.name === "uninstall_mod" && modActionControls.outcome === "rollback";
            const name = questionAction ? "ask_user" : rollbackAction ? "rollback_transaction" : modAction?.name ?? (askUser ? "ask_user" : "apply_file_ops");
            emit("tool-start", { toolCallId, providerCallId: "provider-fixture", name,
              arguments: modArgs ?? askUser?.preview ?? (questionAction ? modActionControls.questionPreview : { plan_id: plan.id }) });
            if (name !== "ask_user" && permissionMode === "full_trust") finish({ approved: true }, false);
            else emit("approval-request", { approvalId, toolCallId, title: name,
              preview: questionAction ? modActionControls.questionPreview : modAction?.preview ?? askUser?.preview ?? plan });
          }, 120);
          return "turn-fixture";
        }
        case "respond_approval": if (pending) finish(args.response); return null;
        case "cancel_turn": pending = false; emit("done", { status: "cancelled" }, activeConversation, null); return null;
        case "list_trash": return purged || (restored && !args.includeRestored) ? [] : [{ id: "trash_fixture", original_path: "/sandbox/minecraft/mods/Example.jar", trash_path: "/sandbox/data/trash/trash_fixture/payload", transaction_id: "txn-trash", size: 1024, is_dir: false, game_id: "game-fixture", label: "Example.jar", reason: "停用测试模组", created_at: now, restored_at: restored ? now : null, exists: !restored }];
        case "purge_trash": if (args.confirmation !== "PURGE_TRASH") throw new Error("confirmation missing"); purged = true; return 1024;
        case "restore_trash": return plan;
        case "get_file_plan": return args.planId === skillPlan.id ? skillPlan : args.planId === filePlan.id ? filePlan : plan;
        case "apply_plan": {
          if (pluginDraftWritePlan?.id === args.planId && pendingPluginDraft) {
            pluginDrafts.push(structuredClone(pendingPluginDraft));
            const completed = pluginDraftWritePlan;
            pluginDraftWritePlan = null;
            pendingPluginDraft = null;
            return completed;
          }
          if (pluginInstallPlan?.id === args.planId) {
            const adapter = JSON.parse(pendingAdapterJson || "{}");
            const installed = {
              slug: adapter.slug, name: adapter.name, gameName: adapter.name, version: adapter.version,
              description: "外部测试适配包", source: "external", validated: true, enabled: true,
              installPath: `/sandbox/data/plugins/games/${adapter.slug}`, coverUrl: null,
            };
            const existing = installedPlugins.findIndex(plugin => plugin.slug === installed.slug);
            if (existing >= 0) installedPlugins.splice(existing, 1, installed);
            else installedPlugins.push(installed);
            if (installed.slug === minecraftPlugin.slug) pluginEnabled = true;
            const completed = pluginInstallPlan;
            pluginInstallPlan = null;
            return completed;
          }
          if (args.planId === importPlan.id) {archiveImported = true; return importPlan;}
          if (args.planId === skillPlan.id) {skillRemoved = true; return skillPlan;}
          if (args.planId === filePlan.id) {fileTrashed = true; return filePlan;}
          if (args.planId !== plan.id) throw new Error("unknown plan");
          restored = true; return plan;
        }
        case "list_snapshots": return [];
        case "list_installs": {
          if (modActionControls.failNextReadback) {
            modActionControls.failNextReadback = false;
            throw new Error("测试模拟安装状态回读失败");
          }
          return [
            ...(removedRecords.has("install-fixture") ? [] : [{ id: "install-fixture", gameId: "game-fixture", name: "Example", version: "1.0.0", source: "modrinth", status: fixtureRolledBack ? "rolled_back" : fixtureUninstalled ? "uninstalled" : "enabled", installRoot: "/sandbox/minecraft/mods", files: [], metadata: fixtureRolledBack ? {transactionId: fixtureRollbackTransactionId} : {}, installedAt: now, tagIds: [] }]),
            ...archiveInstallRecords.filter(record => !removedRecords.has(record.id)).map(record => ({...record, tagIds: record.tagIds ?? []})),
          ];
        }
        case "list_mod_tags": return [];
        case "list_user_roots": return {};
        case "list_downloads": return (window as any).__AMM_TEST__.seedDownload ? [{artifact: {id: 'artifact-fixture', path: '/sandbox/data/downloads/fixture/Fixture.jar', filename: 'Fixture.jar', size: 1024, sha256: 'b'.repeat(64), integrity_verified: true, requires_review: true, warnings: []}, sourceId: 'modrinth', projectId: 'fixture', versionId: 'V'.repeat(128), name: 'Downloaded Fixture', version: '1.0.0', createdAt: now}] : [];
        case "plan_import_archive": if (args.path !== "/sandbox/incoming/Fixture.jar") throw new Error("unknown archive"); return {filename: "Fixture.jar", artifactPath: "/sandbox/data/staging/imports/fixture/Fixture.jar", sha256: "b".repeat(64), size: 1024, plan: importPlan, warnings: []};
        case "remove_managed_mod": {
          if (args.gameId !== initialGame.id) throw new Error("unknown game");
          if (args.target.kind === "archive") removedArchives.add(args.target.id);
          else if (args.target.kind === "record" && ((args.target.id === "install-fixture" && fixtureUninstalled)
            || archiveInstallRecords.some(record => record.id === args.target.id && record.status === "uninstalled"))) removedRecords.add(args.target.id);
          else throw new Error("record is still installed");
          return null;
        }
        case "list_mod_library": {
          const game = games.find(game => game.id === args.gameId);
          if (!game) return [];
          const gameKey = (name: string) => name.trim().toLowerCase().replace(/\s+/g, " ");
          return [...modLibrary, ...(importedEntry ? [importedEntry] : [])].filter(entry =>
            !removedArchives.has(entry.artifactId) && (entry.gameId === game.id || (entry.gameId === null && gameKey(entry.gameName) === gameKey(game.name))));
        }
        case "bind_mod_library_entry": {
          const entry = modLibrary.find(entry => entry.artifactId === args.artifactId);
          if (!entry || args.gameId !== initialGame.id || (entry.gameId !== null && entry.gameId !== args.gameId)) {
            throw new Error("archive unavailable for the chosen game");
          }
          entry.gameId = args.gameId;
          return entry;
        }
        case "register_imported_mod": {
          if (args.gameId !== initialGame.id || args.transactionId !== importPlan.id || !archiveImported) throw new Error("archive not committed for the chosen game");
          importedEntry = {artifactId: "artifact-import-fixture", gameId: args.gameId, gameName: initialGame.name, name: "Fixture", filename: "Fixture.jar", version: null, sha256: "b".repeat(64), projectPage: null};
          return importedEntry;
        }
        case "mcp_pending_game_tools": return structuredClone(pendingGameTools);
        case "mcp_list": return [];
        case "mcp_credential_status": {
          if (failedCredentialStatusIds.has(args.id)) throw new Error("模拟读取配置状态失败");
          if (holdCredentialStatus) {
            holdCredentialStatus = false;
            await new Promise<void>((resolve) => { releaseCredentialStatus = resolve; });
          }
          return structuredClone(credentialStatuses[args.id] ?? {});
        }
        case "mcp_startup_requests": return [];
        case "mcp_connections": return [];
        case "mcp_server_status": return {startupRequested: false, running: false, stopping: false, endpoint: null, tokenRef: "mcp.local_server.token", activeCalls: 0};
        case "list_skills": return [
          ...systemSkills.map(({skill}) => ({...skill})),
          ...installedPlugins.map(plugin => ({name: plugin.slug, description: plugin.description, location: `${plugin.installPath}/SKILL.md`, source: "external", kind: "external", valid: true, enabled: plugin.enabled, issues: []})),
          skillRemoved ? {...skill(), enabled: false, valid: false, issues: ["Skill 已移入回收站"]} : skill(),
        ];
        case "get_skill": {
          const system = systemSkills.find(({skill}) => skill.name === args.name);
          if (system) return structuredClone(system);
          const installed = installedPlugins.find(plugin => plugin.slug === args.name);
          if (installed?.slug === minecraftPlugin.slug) return minecraftSkill();
          if (installed) return {skill: {name: installed.slug, description: installed.description, location: `${installed.installPath}/SKILL.md`, source: "external", kind: "external", valid: true, enabled: installed.enabled, issues: []}, markdown: pendingSkillMarkdown || "---\nname: external-fixture\ndescription: External fixture\n---\n\nTest-only package."};
          if (args.name !== "my-game-notes" || skillRemoved) throw new Error("unknown Skill");
          return {skill: skill(), markdown: "---\nname: my-game-notes\ndescription: 本地游戏说明\n---\n\n只读取管理中的游戏目录。"};
        }
        case "set_skill_enabled": {
          if (args.name === minecraftPlugin.slug && installedPlugins.some(plugin => plugin.slug === minecraftPlugin.slug)) {
            if (args.enabled && !pluginEnabled) throw new Error("适配包未启用");
            minecraftSkillEnabled = args.enabled;
            return minecraftSkill().skill;
          }
          const system = systemSkills.find(({skill}) => skill.name === args.name);
          if (system) {
            system.skill.enabled = args.enabled;
            return {...system.skill};
          }
          if (args.name !== "my-game-notes" || skillRemoved) throw new Error("unknown Skill");
          skillEnabled = args.enabled;
          return skill();
        }
        case "plan_uninstall_skill": if (args.name !== "my-game-notes" || skillRemoved) throw new Error("unknown Skill"); return {plan: skillPlan};
        case "list_plugin_drafts": return structuredClone(pluginDrafts);
        case "write_plugin_draft": {
          const packageSlug = String(args.slug ?? "").trim();
          let adapter: Args;
          try { adapter = JSON.parse(String(args.adapterJson ?? "")); }
          catch { throw new Error("无效输入: adapter.json 不是有效 JSON"); }
          if (!packageSlug || adapter.slug !== packageSlug || !String(args.skillMarkdown ?? "").trim()) throw new Error("无效输入: 草稿内容不完整");
          pendingAdapterJson = String(args.adapterJson);
          pendingSkillMarkdown = String(args.skillMarkdown);
          const id = `draft-external-${++nextPluginDraft}`;
          pendingPluginDraft = {id, slug: packageSlug, path: `/sandbox/data/staging/plugin-drafts/${packageSlug}`, writeTransactionId: `txn-draft-${packageSlug}`, createdAt: pluginCreatedAt, ...draftUpdatedAtField};
          pluginDraftWritePlan = {
            id: `txn-write-${packageSlug}`, summary: `保存外部适配包草稿 ${packageSlug}`, total_bytes: 512, created_at: now,
            requires_review: true,
            entries: [{operation: {op: "copy", src: `/sandbox/data/staging/plugin-drafts/.scratch/${packageSlug}`, dst: pendingPluginDraft.path}, before: null,
              source: {kind: "file", size: 512, sha256: "a".repeat(64)}, impact: "copy"}],
          };
          return {draft: structuredClone(pendingPluginDraft), plan: structuredClone(pluginDraftWritePlan)};
        }
        case "validate_plugin": {
          const target = pluginDrafts.find(item => item.id === args.draftId);
          if (!target) throw new Error("unknown draft");
          const isFixture = target.id === draft.id || target.id === "draft-fixture-duplicate";
          const adapter = isFixture ? {schemaVersion: 1, slug: target.slug, name: "Fixture Game", version: "1.0.0", platforms: ["windows"], markers: [], installRoots: [], modTypes: [], archiveRules: [], loadOrder: {kind: "none", paths: [], notes: ""}, savePaths: [], logPaths: [], uninstallNotes: ["保留用户文件"], sources: [{title: "Fixture", url: "https://example.com", checkedAt: "2026-11-21"}]} : JSON.parse(pendingAdapterJson || "{}");
          const skill = {description: isFixture ? "Public fixture" : "外部测试适配包"};
          if (pendingPluginDraft?.id === target.id) pendingPluginDraft = structuredClone(target);
          return {draft: structuredClone(target), transactionStatus: "committed", valid: true, package: {adapter, skill}, hashes: {adapterSha256: "a".repeat(64), skillSha256: "b".repeat(64)}, issues: [], warnings: []};
        }
        case "install_plugin": {
          const target = pluginDrafts.find(item => item.id === args.draftId);
          if (!target) throw new Error("unknown draft");
          pluginInstallPlan = {
            id: `txn-install-${target.slug}`, summary: `安装外部适配包 ${target.slug}`, total_bytes: 512, created_at: now,
            requires_review: true,
            entries: [{operation: {op: "copy", src: target.path, dst: `/sandbox/data/plugins/games/${target.slug}`}, before: null,
              source: {kind: "directory", size: 512, sha256: "c".repeat(64)}, impact: "copy"}],
          };
          if (target.id === pendingPluginDraft?.id) pendingPluginDraft = structuredClone(target);
          return {plan: structuredClone(pluginInstallPlan)};
        }
        case "delete_plugin_draft": {
          const index = pluginDrafts.findIndex(item => item.id === args.draftId);
          if (index < 0) throw new Error('unknown draft');
          if (failPluginDraftDeletion) throw new Error('模拟删除草稿失败');
          pluginDrafts.splice(index, 1);
          return null;
        }
        case "get_plugin_publication": return structuredClone(publications.find(item => item.id === args.id));
        case "list_plugin_publications": return structuredClone(publications);
        case "prepare_plugin_publication": {
          if (args.draftId !== draft.id) throw new Error('unknown draft');
          const record = {
            id: 'publish_fixture', state: 'intended', phase: 'prepared', fileId: null, workId: null, createdAt: now, updatedAt: now,
            preview: {digest: 'd'.repeat(64), package: {...args.metadata, slug: draft.slug, version: '1.0.0', destination: 'https://www.glossmod.com/api/v1/mcp', workStatus: 'published', archiveName: 'fixture-game-1.0.0.zip', archiveSha256: 'c'.repeat(64), archiveSize: 512,
              files: [{name: 'adapter.json', size: 100, sha256: 'a'.repeat(64), text: '{"slug":"fixture-game","version":"1.0.0"}'}, {name: 'SKILL.md', size: 80, sha256: 'b'.repeat(64), text: '---\nname: fixture-game\ndescription: Public fixture\n---\nReview before installing.'}]}}
          };
          publications.splice(0, publications.length, record); return structuredClone(record);
        }
        case "publish_plugin": {
          const record = publications.find(record => record.id === args.id);
          if (!record || args.digest !== record.preview.digest || args.confirmation !== 'PUBLISH_PLUGIN' || record.state !== 'intended') throw new Error('unapproved or stale publication');
          if (publicationOutcome === 'unknown-create') {record.state = 'unknown'; record.phase = 'createUploadPending'; throw new Error('远程结果不确定，不会自动重发');}
          record.fileId = 42;
          if (publicationOutcome === 'unknown-complete') {record.state = 'unknown'; record.phase = 'completeUploadPending'; throw new Error('远程结果不确定，不会自动重发');}
          if (publicationOutcome === 'unknown-publish') {record.state = 'unknown'; record.phase = 'publishWorkPending'; throw new Error('远程结果不确定，不会自动重发');}
          if (publicationOutcome === 'scan') {record.phase = 'awaitingScan'; return structuredClone(record);}
          record.state = 'completed'; record.phase = 'completed'; record.workId = 99; return structuredClone(record);
        }
        case "reconcile_plugin_publication": {
          if (Object.keys(args).length !== 1 || typeof args.id !== 'string') throw new Error('invalid reconciliation arguments');
          const record = publications.find(item => item.id === args.id);
          if (!record || publicationReconcileOutcome === 'unconfirmed' || record.fileId == null) return 'unconfirmed';
          if (record.phase === 'awaitingScan' || record.phase === 'completeUploadPending') {
            record.state = 'intended'; record.phase = 'uploadReady'; record.updatedAt = now;
            return 'verified';
          }
          if (record.phase === 'publishWorkPending') {
            record.state = 'completed'; record.phase = 'completed'; record.workId = 99; record.updatedAt = now;
            return 'verified';
          }
          return 'unconfirmed';
        }
        case "set_plugin_enabled": {
          const installed = installedPlugins.find(plugin => plugin.slug === args.slug);
          if (!installed) throw new Error("unknown plugin");
          installed.enabled = args.enabled;
          if (installed.slug === minecraftPlugin.slug) pluginEnabled = args.enabled;
          return withCloudEvidence(installed);
        }
        case "list_plugins": return structuredClone(installedPlugins.map(withCloudEvidence));
        case "remove_installed_plugin": {
          const index = installedPlugins.findIndex(plugin => plugin.slug === args.slug);
          if (index < 0) throw new Error("unknown plugin");
          const shouldFail = failInstalledPluginRemoval === "always"
            || (failInstalledPluginRemoval === "once" && !(window as any).__AMM_TEST__.installedPluginRemovalFailed);
          if (shouldFail) {
            (window as any).__AMM_TEST__.installedPluginRemovalFailed = true;
            throw new Error("模拟移除适配包失败");
          }
          installedPlugins.splice(index, 1);
          return { transactionId: `txn-remove-${args.slug}` };
        }
        case "list_manager_plugin_games": return {games: [], totalGames: 0, skippedEntries: 0};
        case "search_mods": return { mods: [{ source_id: "modrinth", project_id: "example", game_id: "minecraft", name: "Example Performance Mod", summary: "测试来源响应", author: "Fixture Author", homepage_url: "https://example.com/mod", icon_url: null, downloads: 1024 }], sources: [{ source_id: "modrinth", state: "available", message: "" }] };
        case "read_dir": return [{ name: "mods", path: "/sandbox/minecraft/mods", is_dir: true, size: 0, modified: now }, ...fileTrashed ? [] : [{ name: "options.txt", path: "/sandbox/minecraft/options.txt", is_dir: false, size: 128, modified: now }]];
        case "read_file": if (fileTrashed && args.path === "/sandbox/minecraft/options.txt") throw new Error("file moved to trash"); return { lines: [{ line: 1, text: "fixture log" }], truncated: false };
        case "trash_paths": if (JSON.stringify(args.request.paths) !== JSON.stringify(["/sandbox/minecraft/options.txt"])) throw new Error("unknown file"); return filePlan;
        case "register_game": {
          const scanned = [...onboardingCandidates, ...libraryCandidates].find((item) => item.rootPath === args.directory);
          const verified = inspectedCandidates.find((item) => item.rootPath === args.directory) ?? scanned;
          if (!verified) throw new Error("unknown candidate");
          if (failRegistration && scanned?.name === "Broken Fixture") throw new Error("目录已变化，请重新扫描");
          const suggested = verified.suggestedAdapterSlug;
          const adapterSlug = args.fromDiscovery === true
            ? suggested === args.adapterSlug ? suggested : null
            : args.adapterSlug ?? suggested;
          const name = adapterSlug ? verified.name : verified.rootPath.split("/").at(-1)!;
          const id = `game-${name.toLowerCase().replaceAll(" ", "-")}`;
          const game = {...initialGame, id, name, rootPath: verified.rootPath, adapterSlug, adapterVersion: adapterSlug ? initialGame.adapterVersion : null,
            slug: adapterSlug ?? "unknown", platform: adapterSlug ? verified.platform : "unknown", confidence: adapterSlug ? verified.confidence : 0,
            installedModCount: 0, enabledModCount: 0};
          if (!games.some((item) => item.id === game.id)) games.push(game);
          verified.registeredGameId = game.id;
          if (scanned) scanned.registeredGameId = game.id;
          return game;
        }
        case "scan_games": {
          const candidates = args.options?.userRoots?.includes("/sandbox/new-game") ? libraryCandidates : onboarding ? onboardingCandidates : [];
          return { candidates: structuredClone(candidates), warnings: [], directoriesChecked: 3, entriesChecked: 18, truncated: false };
        }
        // Launch selects by game ID and method; file paths are supplied only when editing launcher settings.
        case "launch_game": {
          const game = games.find((item) => item.id === args.id);
          if (!game) throw new Error(`未找到: 游戏: ${args.id}`);
          if (typeof args.method === "string") {
            if (!availableLaunchOptions(game).some((option) => option.id === args.method)) throw new Error(`无效输入: 不支持的启动方式: ${args.method}`);
            if (failGameLaunchMethods.includes(args.method)) throw new Error(`模拟启动失败: ${args.method}`);
            return null;
          }
          if (!game.metadata?.launchPath && !game.steamAppId) throw new Error("尚未设置启动方式，请在“编辑”中选择游戏可执行文件");
          return null;
        }
        case "list_game_launch_options": {
          const game = games.find((item) => item.id === args.id);
          if (!game) throw new Error(`未找到: 游戏: ${args.id}`);
          if (failGameLaunchOptions) throw new Error("模拟读取启动方式失败");
          return availableLaunchOptions(game);
        }
        case "set_game_mod_launcher": {
          const game = games.find((item) => item.id === args.id);
          if (!game) throw new Error(`未找到: 游戏: ${args.id}`);
          if (args.launcher === null) {
            const {modLauncher: _removed, ...metadata} = game.metadata ?? {};
            game.metadata = metadata;
          } else {
            const launcher = args.launcher;
            if (!launcher || typeof launcher.name !== "string" || typeof launcher.path !== "string" || !Array.isArray(launcher.args)) {
              throw new Error("无效输入: 启动器配置格式无效");
            }
            game.metadata = {...(game.metadata ?? {}), modLauncher: structuredClone(launcher)};
          }
          game.updatedAt = now;
          return structuredClone(game);
        }
        case "open_game_location": {
          if (!games.some((item) => item.id === args.id)) throw new Error(`未找到: 游戏: ${args.id}`);
          if (args.target !== "root" && args.target !== "mods") throw new Error("无效输入: 打开目录的目标只能是 root 或 mods");
          if (args.target === "mods" && modsFolderMissing) throw new Error("无法定位 Mod 目录：Mod 目录不存在或不是目录: /sandbox/minecraft/mods");
          return null;
        }
        case "set_game_adapter": {
          const game = games.find((item) => item.id === args.id);
          if (!game) throw new Error(`未找到: 游戏: ${args.id}`);
          if (args.adapterSlug !== minecraftPlugin.slug || !installedPlugins.some(plugin => plugin.slug === minecraftPlugin.slug && plugin.enabled)) throw new Error(`适配包不可用: ${args.adapterSlug}`);
          game.adapterSlug = minecraftPlugin.slug;
          game.adapterVersion = minecraftPlugin.version;
          game.updatedAt = now;
          return structuredClone(game);
        }
        case "remove_game_adapter": {
          const game = games.find((item) => item.id === args.id);
          if (!game) throw new Error(`未找到: 游戏: ${args.id}`);
          if (pendingGameConflicts.some((conflict: any) => !Array.isArray(conflict.blockedChanges) || conflict.blockedChanges.includes("select_adapter"))) {
            throw new Error("冲突: 仍有未完成的任务阻止适配包变更");
          }
          game.adapterSlug = null;
          game.adapterVersion = null;
          game.updatedAt = now;
          return structuredClone(game);
        }
        case "update_game": {
          const game = games.find((item) => item.id === args.id);
          if (!game) throw new Error(`未找到: 游戏: ${args.id}`);
          const name = String(args.name ?? "").trim();
          if (!name || /\p{Cc}/u.test(name)) throw new Error("无效输入: 游戏名称去除首尾空白后不能为空，且不能含有控制字符");
          const launchPath = typeof args.launchPath === "string" ? args.launchPath.trim() : "";
          if (launchPath && !launchPath.startsWith(`${game.rootPath}/`)) throw new Error("路径越界: 启动程序必须位于该游戏的根目录内");
          game.name = name;
          const {launchPath: _previous, ...rest} = game.metadata ?? {};
          game.metadata = launchPath ? {...rest, launchPath} : rest;
          return structuredClone(game);
        }
        case "remove_game": {
          const index = games.findIndex((item) => item.id === args.id);
          if (index < 0) throw new Error(`未找到: 游戏: ${args.id}`);
          if (games[index].installedModCount > 0) throw new Error(`冲突: 该游戏仍有 ${games[index].installedModCount} 个已安装的 Mod，请先在游戏详情中卸载后再移除`);
          if (pendingGameConflicts.some((conflict: any) => conflict.blockedChanges?.includes("remove"))) throw new Error("冲突: 仍有未完成的文件任务阻止移除游戏");
          games.splice(index, 1);
          return null;
        }        default: throw new Error(`Unimplemented test IPC: ${command}`);
      }
    };
    const target = window as any;
    target.isTauri = true;
    target.__TAURI_INTERNALS__ = {
      invoke,
      transformCallback: (callback: (value: any) => void) => { const id = ++counter; callbacks.set(id, callback); return id; },
      unregisterCallback: (id: number) => callbacks.delete(id),
      convertFileSrc: () => "/favicon.png",
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
    };
    target.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_event: string, id: number) => listeners.delete(id) };
    target.__AMM_TEST__ = {
      calls, emit, emitEvent, persistOnly, replayedEvents, events, conversations, games, modLibrary, pluginDrafts, installedPlugins, cloudPluginSlugs, modActionControls,
      setSystemLocale(locale: string | null) { simulatedSystemLocale = locale; },
      publishQuestionResult() {
        deferredQuestionResult?.();
        deferredQuestionResult = undefined;
      },
      finishStorageMigration() {
        releaseStorageMigration?.();
        releaseStorageMigration = undefined;
      },
      finishMcpCredentialStatus() {
        releaseCredentialStatus?.();
        releaseCredentialStatus = undefined;
      },
      get pending() { return pending; },
      get modsFolderMissing() { return modsFolderMissing; },
      set modsFolderMissing(value: boolean) { modsFolderMissing = value; },
      get publicationOutcome() { return publicationOutcome; },
      set publicationOutcome(value: string) { publicationOutcome = value; },
      get publicationReconcileOutcome() { return publicationReconcileOutcome; },
      set publicationReconcileOutcome(value: 'verified' | 'unconfirmed') { publicationReconcileOutcome = value; },
      seedEvents(values: any[]) {
        events.push(...values);
        seq = Math.max(seq, ...values.map((event) => event.seq).filter(Number.isSafeInteger));
      },
    };
  }, {modLibrary, onboarding, onboardingStarted, failRegistration, emptyLibrary, dashboardSponsors, dashboardStats, gameRootPath, gameLaunchPath, gameSteamAppId, gameModLauncher, gameLaunchOptions, failGameLaunchOptions, failGameLaunchMethods, gameWithoutAdapter, gameWithoutMods, adapterDisabled, withExternalAdapter, holdAgentTurn, gamePlanConflicts, pluginDateFixture, pluginCreatedAt: PLUGIN_CREATED_AT_FIXTURE, pluginUpdatedAt: PLUGIN_UPDATED_AT_FIXTURE, failInstalledPluginRemoval, duplicatePluginDraftSlug, failPluginDraftDeletion, seedPublishedPluginRecord, providerModels, configuredModels, appLocale, systemLocale, askUser, storage, workspaceGames, conversationFixtures, appUpdate, failAppUpdateRoute, pendingGameTools, mcpCredentialStatuses, failMcpCredentialStatusIds, failFirstMcpSecretWrites, holdNextMcpCredentialStatus});
}
