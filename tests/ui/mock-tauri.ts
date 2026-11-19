import type { Page } from "@playwright/test";

// Test-only IPC transport. It is never imported by the shipped application.
export async function installMockTauri(page: Page, {fomodSelection = "requires_user", onboarding = false, onboardingStarted = false, failRegistration = false}: {fomodSelection?: "safe" | "requires_user"; onboarding?: boolean; onboardingStarted?: boolean; failRegistration?: boolean} = {}) {
  await page.addInitScript(({fomodSelection, onboarding, onboardingStarted, failRegistration}) => {
    type Args = Record<string, any>;
    const now = "2026-11-18T00:00:00Z";
    const callbacks = new Map<number, (value: any) => void>();
    const listeners = new Map<number, { event: string; handler: number }>();
    let counter = 0;
    let seq = 0;
    const calls: Array<{ command: string; args: Args }> = [];
    const events: any[] = [];
    const initialGame: Args = {
      id: "game-fixture", name: "Minecraft Java Edition", slug: "minecraft-java",
      rootPath: "/sandbox/minecraft", platform: "windows", adapterSlug: "minecraft-java",
      adapterVersion: "1.0.0", confidence: 1, coverPath: null, steamAppId: null,
      lastScannedAt: now, installedModCount: 1, enabledModCount: 1, metadata: {}, createdAt: now, updatedAt: now,
    };
    const games: Args[] = onboarding && !onboardingStarted ? [] : [initialGame];
    const onboardingCandidates: Args[] = [
      {rootPath: "/sandbox/steam/Fixture", name: "Fixture Game", confidence: 0.92, status: "identified", suggestedAdapterSlug: "minecraft-java", registeredGameId: null, platform: "windows", warnings: ["这是一个测试扫描候选，请确认目录后再登记"]},
      {rootPath: "/sandbox/steam/Broken", name: "Broken Fixture", confidence: 0.61, status: "ambiguous", suggestedAdapterSlug: null, registeredGameId: null, platform: "unknown", warnings: []},
    ];
    const conversations = [{
      id: "conversation-fixture", title: "检查 Minecraft 模组", game_id: "game-fixture",
      task_summary: null, archived: false, pinned: false, message_count: 0,
      created_at: now, updated_at: now, permission_mode: null,
    }];
    const messages = new Map<string, any[]>();
    messages.set("conversation-fixture", []);
    let activeConversation = "conversation-fixture";
    let pending = false;
    let restored = false;
    let purged = false;
    let pluginEnabled = true;
    let skillRemoved = false;
    let skillEnabled = true;
    let fileTrashed = false;
    let directoryPickCount = 0;
    let modsFolderMissing = false;
    let archiveImported = false;
    let identitySearch: Args | null = null;
    let publicationOutcome = 'completed';
    let publicationReconcileOutcome: 'verified' | 'unconfirmed' = 'verified';
    const unknownCandidate = {rootPath: '/sandbox/unknown-game', name: 'unknown-game', confidence: 0, status: 'unidentified', suggestedAdapterSlug: null, registeredGameId: null, platform: 'unknown', warnings: [], matches: [], sources: [], requiresConfirmation: true};
    const publications: Args[] = [];
    const draft = {id: 'draft-fixture', slug: 'fixture-game', path: '/sandbox/data/staging/plugin-drafts/fixture', writeTransactionId: 'txn-draft-fixture', createdAt: now};
    let settings: Args = { base_url: "https://example.com/v1", model: onboarding ? "" : "mock-model", context_budget: 32768, permission_mode: "request_approval", api_key_ref: "model.api_key", onboarding_completed: !onboarding };
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
      ...plan, id: "txn-import-fixture", summary: "导入本地归档到管理器",
      entries: [{...plan.entries[0], operation: {op: "copy", src: "/sandbox/data/staging/import-scratch/fixture/Fixture.jar", dst: "/sandbox/data/staging/imports/fixture/Fixture.jar"}}],
    };
    const installPlan = {
      ...plan, id: "txn-install-fixture", summary: "安装 Fixture",
      entries: [{...plan.entries[0], operation: {op: "copy", src: "/sandbox/data/staging/imports/fixture/Fixture.jar", dst: "/sandbox/minecraft/mods/Fixture.jar"}}],
    };
    const skill = () => ({name: "my-game-notes", description: "本地游戏说明", location: "/sandbox/data/skills/my-game-notes/SKILL.md", source: "external", kind: "standalone", valid: true, enabled: skillEnabled, issues: []});
    const emit = (kind: string, payload: any, conversationId = activeConversation, messageId: string | null = "assistant-fixture") => {
      const value = { seq: ++seq, turnId: "turn-fixture", conversationId, messageId, kind, payload, createdAt: now };
      events.push(value);
      for (const [id, listener] of listeners) {
        if (listener.event === `agent://${kind}`) callbacks.get(listener.handler)?.({ event: listener.event, id, payload: value });
      }
      return value;
    };
    const delta = (text: string) => {
      const message = messages.get(activeConversation)?.find((m) => m.id === "assistant-fixture");
      if (message) message.content += text;
      emit("delta", { content: text, reasoning: "" });
    };
    const finish = (approved: boolean) => {
      pending = false;
      emit("approval-resolved", { approvalId: "approval-fixture", status: approved ? "approved" : "rejected" });
      emit("tool-result", { toolCallId: "tool-fixture", status: approved ? "done" : "denied", result: { status: approved ? "committed" : "denied" } });
      delta(approved ? "\n检查完成，文件计划已执行。" : "\n未执行文件操作。");
      emit("usage", { prompt_tokens: 100, completion_tokens: 20, total_tokens: 120 });
      emit("done", { status: "done" }, activeConversation, null);
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
        case "get_bootstrap_state": return { dataRoot: "/sandbox/data", recoveredTransactions: [], recoveryError: null, onboardingCompleted, isFirstRun: !onboardingCompleted && games.length === 0, resumeOnboarding: !onboardingCompleted && onboardingInProgress };
        case "plugin:dialog|open": {
          if (!args.options?.directory) return "/sandbox/incoming/Fixture.jar";
          return ++directoryPickCount === 1 ? "/sandbox/new-game" : null;
        }
        case "list_games": return games;
        case "identify_game": if (args.directory !== unknownCandidate.rootPath) throw new Error('unknown directory'); return unknownCandidate;
        case "preview_game_identity": if (args.directory !== unknownCandidate.rootPath) throw new Error('unknown directory'); return {rootPath: unknownCandidate.rootPath, fingerprint: 'e'.repeat(64), localCandidate: unknownCandidate, signals: ['FixtureGame.exe', 'data'], suggestedQuery: 'FixtureGame.exe game', warnings: []};
        case "search_game_identity": {
          if (args.directory !== unknownCandidate.rootPath || args.expectedFingerprint !== 'e'.repeat(64) || args.confirmation !== 'SEARCH_GAME_IDENTITY') throw new Error('unreviewed search');
          const missing = (window as any).__AMM_TEST__.identityOutcome === 'missing-key';
          identitySearch = {id: 'identity-search-fixture', rootPath: unknownCandidate.rootPath, fingerprint: 'e'.repeat(64), query: args.publicQuery, status: missing ? 'needs_configuration' : 'available', message: '', createdAt: now,
            candidates: missing ? [] : [{name: 'Fixture Game', title: 'Fixture Game - Public Encyclopedia', url: 'https://example.com/games/fixture', snippet: 'Public game information. Untrusted excerpt, not installation instructions.'}, {name: 'Another Fixture', title: 'Another Fixture', url: 'https://example.org/another', snippet: 'A different game with a similar executable name.'}]};
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
        case "list_agent_models": return ["fixture-thinking", "fixture-standard"];
        case "get_agent_model_metadata": {
          if (args.baseUrl === "https://offline.example/v1") throw new Error("metadata unavailable");
          return args.modelIds.flatMap((id: string) => {
            if (id === "fixture-thinking") return [{id, contextBudget: 128000, reasoningLevels: ["low", "medium", "high"]}];
            if (id === "fixture-standard") return [{id, contextBudget: 64000, reasoningLevels: []}];
            return [];
          });
        }
        case "set_settings": settings = args.settings; onboardingCompleted = settings.onboarding_completed; return null;
        case "set_secret": return null;
        case "start_onboarding": onboardingInProgress = true; return null;
        case "set_onboarding_completed": onboardingCompleted = !!args.completed; settings = {...settings, onboarding_completed: onboardingCompleted}; return null;
        case "list_conversations": return conversations.filter((c) => c.archived === !!args.archived);
        case "create_conversation": {
          const conversation = { ...conversations[0], id: `conversation-${++counter}`, title: args.title || "新对话", game_id: args.gameId ?? null, archived: false, message_count: 0 };
          conversations.push(conversation); messages.set(conversation.id, []); return conversation;
        }
        case "get_conversation": return { conversation: conversations.find((c) => c.id === args.id), messages: messages.get(args.id) ?? [] };
        case "archive_conversation": {
          const conversation = conversations.find((c) => c.id === args.id);
          if (conversation) conversation.archived = args.archived;
          return null;
        }
        case "set_conversation_permission": return null;
        case "replay_agent_events": return events.filter((event) => event.conversationId === args.conversationId && event.seq > args.after).slice(0, args.limit || 2000);
        case "start_turn": {
          activeConversation = args.request.conversationId;
          const history = messages.get(activeConversation) ?? [];
          history.push({ id: "user-fixture", role: "user", content: args.request.content, reasoning: null, tool_calls: null, tool_call_id: null });
          history.push({ id: "assistant-fixture", role: "assistant", content: "", reasoning: null, tool_calls: null, tool_call_id: null });
          messages.set(activeConversation, history);
          // Deliberately emit before invoke resolves: a listener installed after
          // start_turn would lose this delta without durable replay.
          emit("turn-start", {}, activeConversation, null);
          delta("正在检查模组文件。");
          pending = true;
          setTimeout(() => {
            if (!pending) return;
            emit("tool-start", { toolCallId: "tool-fixture", providerCallId: "provider-fixture", name: "apply_file_ops", arguments: { plan_id: plan.id } });
            emit("approval-request", { approvalId: "approval-fixture", toolCallId: "tool-fixture", title: "apply_file_ops", preview: plan });
          }, 120);
          return "turn-fixture";
        }
        case "respond_approval": if (pending) finish(!!args.response.approved); return null;
        case "cancel_turn": pending = false; emit("done", { status: "cancelled" }, activeConversation, null); return null;
        case "list_trash": return purged || (restored && !args.includeRestored) ? [] : [{ id: "trash_fixture", original_path: "/sandbox/minecraft/mods/Example.jar", trash_path: "/sandbox/data/trash/trash_fixture/payload", transaction_id: "txn-trash", size: 1024, is_dir: false, game_id: "game-fixture", label: "Example.jar", reason: "停用测试模组", created_at: now, restored_at: restored ? now : null, exists: !restored }];
        case "purge_trash": if (args.confirmation !== "PURGE_TRASH") throw new Error("confirmation missing"); purged = true; return 1024;
        case "restore_trash": return plan;
        case "get_file_plan": return args.planId === skillPlan.id ? skillPlan : args.planId === filePlan.id ? filePlan : plan;
        case "apply_plan": {
          if (args.planId === importPlan.id) {archiveImported = true; return importPlan;}
          if (args.planId === installPlan.id && archiveImported) return installPlan;
          if (args.planId === skillPlan.id) {skillRemoved = true; return skillPlan;}
          if (args.planId === filePlan.id) {fileTrashed = true; return filePlan;}
          if (args.planId !== plan.id) throw new Error("unknown plan");
          restored = true; return plan;
        }
        case "list_snapshots": return [];
        case "list_installs": return [{ id: "install-fixture", gameId: "game-fixture", name: "Example", version: "1.0.0", source: "modrinth", status: "enabled", installRoot: "/sandbox/minecraft/mods", files: [], metadata: {}, installedAt: now }];
        case "list_user_roots": return {};
        case "list_downloads": return (window as any).__AMM_TEST__.seedDownload ? [{artifact: {id: 'artifact-fixture', path: '/sandbox/data/downloads/fixture/Fixture.jar', filename: 'Fixture.jar', size: 1024, sha256: 'b'.repeat(64), integrity_verified: true, requires_review: true, warnings: []}, sourceId: 'modrinth', projectId: 'fixture', versionId: 'V'.repeat(128), name: 'Downloaded Fixture', version: '1.0.0', createdAt: now}] : [];
        case "plan_import_archive": if (args.path !== "/sandbox/incoming/Fixture.jar") throw new Error("unknown archive"); return {filename: "Fixture.jar", artifactPath: "/sandbox/data/staging/imports/fixture/Fixture.jar", sha256: "b".repeat(64), size: 1024, plan: importPlan, warnings: []};
        case "preview_fomod": {
          if (!["/sandbox/data/staging/imports/fixture/Fixture.jar", "/sandbox/data/downloads/fixture/Fixture.jar"].includes(args.artifactPath)) throw new Error("unknown FOMOD artifact");
          const requiredMappings = [{source: "Data/base.esp", destination: "base.esp", priority: null}];
          function option(name: string, source: string, destination: string, description: string | null = null) {
            return {name, description, image: null, mappings: [{source, destination, priority: null}], hasConditions: false};
          }
          const groups = fomodSelection === "safe" ? [
            {
              name: "Assets", selectionType: "SelectAll", hasConditions: false,
              options: [
                option("Terrain textures", "Data/terrain.dds", "textures/terrain.dds"),
                option("Interface icons", "Data/icons.dds", "textures/ui/icons.dds"),
              ],
            },
            // Distinct groups can have the same name. The wire indices identify each one.
            {
              name: "Assets", selectionType: "SelectExactlyOne", hasConditions: false,
              options: [option("Core plugin", "Data/core.esp", "core.esp")],
            },
          ] : [
            {
              name: "Textures", selectionType: "SelectExactlyOne", hasConditions: false,
              options: [
                option("HD", "Data/hd.dds", "textures/fixture.dds", "High resolution"),
                option("SD", "Data/sd.dds", "textures/fixture.dds", "Standard resolution"),
              ],
            },
          ];
          const automaticSelection = fomodSelection === "safe" ? {
            status: "safe",
            selectedOptions: groups.flatMap((group, groupIndex) => group.options.map((option, optionIndex) => ({
              group: group.name, groupIndex, option: option.name, optionIndex,
            }))),
            mappings: [...requiredMappings, ...groups.flatMap(group => group.options.flatMap(option => option.mappings))],
            warnings: [],
          } : {
            status: "requires_user",
            selectedOptions: [],
            mappings: requiredMappings,
            warnings: ["FOMOD 组 Textures 不能由安全规则唯一选择；需要用户确认"],
          };
          return {
            artifactPath: args.artifactPath,
            artifactSha256: "b".repeat(64),
            configPath: "fomod/ModuleConfig.xml",
            moduleName: "Fixture FOMOD",
            requiredMappings, groups, automaticSelection,
            warnings: groups.filter(group => group.selectionType === "SelectExactlyOne")
              .map(group => `FOMOD 组 ${group.name} 使用 SelectExactlyOne 选择约束；安装前仍需用户确认`),
          };
        }
        case "plan_install": {
          if (args.request.artifactPath === '/sandbox/data/downloads/fixture/Fixture.jar' && (window as any).__AMM_TEST__.seedDownload) {
            if (args.request.modType !== 'java-mod' || args.request.expectedSha256 !== 'b'.repeat(64)) throw new Error('unverified download or invalid mod type');
            return {plan: {...installPlan, summary: `安装 Downloaded Fixture（modrinth，项目 ID：fixture，版本 ID：${'V'.repeat(128)}）到 Minecraft`, entries: [{...installPlan.entries[0], operation: {...installPlan.entries[0].operation, src: args.request.artifactPath}}]}, warnings: []};
          }
          if (!archiveImported || args.request.artifactPath !== "/sandbox/data/staging/imports/fixture/Fixture.jar") throw new Error("archive not imported"); return {plan: installPlan, warnings: []};
        }
        case "mcp_list": return [];
        case "mcp_connections": return [];
        case "mcp_server_status": return {running: false, stopping: false, endpoint: null, tokenRef: "mcp.local_server.token", activeCalls: 0};
        case "list_skills": return [skillRemoved ? {...skill(), enabled: false, valid: false, issues: ["Skill 已移入回收站"]} : skill()];
        case "get_skill": if (args.name !== "my-game-notes" || skillRemoved) throw new Error("unknown Skill"); return {skill: skill(), markdown: "---\nname: my-game-notes\ndescription: 本地游戏说明\n---\n\n只读取已登记游戏目录。"};
        case "set_skill_enabled": if (args.name !== "my-game-notes" || skillRemoved) throw new Error("unknown Skill"); skillEnabled = args.enabled; return skill();
        case "plan_uninstall_skill": if (args.name !== "my-game-notes" || skillRemoved) throw new Error("unknown Skill"); return {plan: skillPlan};
        case "list_plugin_drafts": return [draft];
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
        case "set_plugin_enabled": if (args.slug !== "minecraft-java") throw new Error("unknown plugin"); pluginEnabled = args.enabled; return {slug: "minecraft-java", enabled: pluginEnabled};
        case "list_plugins": return [{ slug: "minecraft-java", name: "Minecraft Java Edition", gameName: "Minecraft Java Edition", version: "1.0.0", description: "Minecraft 模组安装指引", source: "builtin", enabled: pluginEnabled, validated: true, installPath: "/sandbox/data/plugins/games/minecraft-java" }];
        case "list_mod_sources": return [{ source_id: "modrinth", state: "available", message: "" }, { source_id: "curseforge", state: "missing_key", message: "需要配置 API Key" }];
        case "search_mods": return { mods: [{ source_id: "modrinth", project_id: "example", game_id: "minecraft", name: "Example Performance Mod", summary: "测试来源响应", author: "Fixture Author", homepage_url: "https://example.com/mod", icon_url: null, downloads: 1024 }], sources: [{ source_id: "modrinth", state: "available", message: "" }] };
        case "read_dir": return [{ name: "mods", path: "/sandbox/minecraft/mods", is_dir: true, size: 0, modified: now }, ...fileTrashed ? [] : [{ name: "options.txt", path: "/sandbox/minecraft/options.txt", is_dir: false, size: 128, modified: now }]];
        case "read_file": if (fileTrashed && args.path === "/sandbox/minecraft/options.txt") throw new Error("file moved to trash"); return { lines: [{ line: 1, text: "fixture log" }], truncated: false };
        case "trash_paths": if (JSON.stringify(args.request.paths) !== JSON.stringify(["/sandbox/minecraft/options.txt"])) throw new Error("unknown file"); return filePlan;
        case "register_game": {
          const candidate = onboardingCandidates.find((item) => item.rootPath === args.directory);
          if (!candidate) throw new Error("unknown candidate");
          if (failRegistration && candidate.name === "Broken Fixture") throw new Error("目录已变化，请重新扫描");
          const id = `game-${candidate.name.toLowerCase().replaceAll(" ", "-")}`;
          const game = {...initialGame, id, name: candidate.name, rootPath: candidate.rootPath, adapterSlug: args.adapterSlug ?? null, installedModCount: 0, enabledModCount: 0};
          if (!games.some((item) => item.id === game.id)) games.push(game);
          candidate.registeredGameId = game.id;
          return game;
        }
        case "scan_games": return { candidates: onboarding ? structuredClone(onboardingCandidates) : [], warnings: [], directoriesChecked: 3, entriesChecked: 18, truncated: false };
        // Game-card actions. The native commands resolve every path from the game id, so the mock never accepts a path from the UI.
        case "launch_game": {
          const game = games.find((item) => item.id === args.id);
          if (!game) throw new Error(`未找到: 游戏: ${args.id}`);
          if (!game.metadata?.launchPath && !game.steamAppId) throw new Error("尚未设置启动方式，请在“编辑”中选择游戏可执行文件");
          return null;
        }
        case "open_game_location": {
          if (!games.some((item) => item.id === args.id)) throw new Error(`未找到: 游戏: ${args.id}`);
          if (args.target !== "root" && args.target !== "mods") throw new Error("无效输入: 打开目录的目标只能是 root 或 mods");
          if (args.target === "mods" && modsFolderMissing) throw new Error("无法定位 Mod 目录：Mod 目录不存在或不是目录: /sandbox/minecraft/mods");
          return null;
        }
        case "update_game": {
          const game = games.find((item) => item.id === args.id);
          if (!game) throw new Error(`未找到: 游戏: ${args.id}`);
          const name = String(args.name ?? "").trim();
          if (!name || name.length > 80) throw new Error("无效输入: 游戏名称去除首尾空白后须为 1 到 80 个字符，且不能含有控制字符");
          const launchPath = typeof args.launchPath === "string" ? args.launchPath.trim() : "";
          if (launchPath && !launchPath.startsWith(`${game.rootPath}/`)) throw new Error("路径越界: 启动程序必须位于该游戏的已登记根目录内");
          game.name = name;
          const {launchPath: _previous, ...rest} = game.metadata ?? {};
          game.metadata = launchPath ? {...rest, launchPath} : rest;
          return structuredClone(game);
        }
        case "remove_game": {
          const index = games.findIndex((item) => item.id === args.id);
          if (index < 0) throw new Error(`未找到: 游戏: ${args.id}`);
          if (games[index].installedModCount > 0) throw new Error(`冲突: 该游戏仍有 ${games[index].installedModCount} 个已安装的 Mod，请先在游戏详情中卸载后再移除`);
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
      calls, emit, events, conversations,
      get pending() { return pending; },
      get modsFolderMissing() { return modsFolderMissing; },
      set modsFolderMissing(value: boolean) { modsFolderMissing = value; },
      get publicationOutcome() { return publicationOutcome; },
      set publicationOutcome(value: string) { publicationOutcome = value; },
      get publicationReconcileOutcome() { return publicationReconcileOutcome; },
      set publicationReconcileOutcome(value: 'verified' | 'unconfirmed') { publicationReconcileOutcome = value; },
    };
  }, {fomodSelection, onboarding, onboardingStarted, failRegistration});
}
