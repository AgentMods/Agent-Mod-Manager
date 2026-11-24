import { expect, test } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const legacyRemoval = String.raw`我已在游戏详情中明确选择卸载并从管理列表删除这一条安装记录。目标（精确 ID）：{"gameId":"game-fixture","installId":"安装/\"例\""}。请先读取 system-mod-uninstall Skill，再用 inspect_install 核对该记录，按 reconciliation.recommendedAction 和 Skill 处理：仅在 safe_uninstall 时用 uninstall_mod；在 rollback_install 或 rollback_replacement 时，用报告中的事务 ID（transaction_id）和相同的 game_id、install_id 申请 rollback_transaction；存在冲突时先说明。不要按名称匹配或操作其他安装记录。卸载或回滚成功后，再用 forget_uninstalled_mod 对相同游戏和记录执行从管理列表删除。`;

const translated = "The player selected these exact archives from the game details page. Install the first Mod, then the second.";
const syntheticOrigin = {
  kind: "mod_request", version: 1, action: "install", batch: true,
  targets: [{ gameId: "game-fixture", artifactId: "archive/alpha" }, { gameId: "game-fixture", artifactId: "archive/例" }],
  originalContent: translated,
};

test("request folding accepts validated provenance or exact legacy templates, not edits or lookalikes", async ({ page }) => {
  await installMockTauri(page);
  await page.goto("/");
  const results = await page.evaluate(async ({ legacyRemoval, translated, syntheticOrigin }) => {
    const { parseModRequest, createModRequestMessage } = await import("/src/lib/mod-request.ts");
    const { applyLanguageSettings } = await import("/src/lib/i18n.ts");
    applyLanguageSettings({ locale: "en", effective_locale: "en" });
    const legacy = { role: "user", content: legacyRemoval };
    const synthetic = { role: "user", content: translated, origin: syntheticOrigin };
    const structured = [
      synthetic,
      { ...synthetic, role: "assistant" },
      { ...synthetic, content: translated + "\nExtra user instructions." },
      { ...synthetic, origin: { ...syntheticOrigin, version: 2 } },
      { ...synthetic, origin: { ...syntheticOrigin, targets: [syntheticOrigin.targets[0], syntheticOrigin.targets[0]] } },
      { ...synthetic, origin: { ...syntheticOrigin, targets: [{ gameId: "other-game", artifactId: "xyz" }, syntheticOrigin.targets[1]] } },
      { ...synthetic, origin: { ...syntheticOrigin, targets: [{ gameId: "game-fixture", installId: "xyz" }] } },
      { ...synthetic, origin: { ...syntheticOrigin, batch: false } },
    ];
    const old = [
      legacy,
      { ...legacy, origin: { kind: "user" } },
      { ...legacy, origin: { kind: "edited" } },
      { ...legacy, origin: { kind: "adapter_prompt" } },
      { ...legacy, attachments: [{ kind: "document", name: "extra.md", markdown: "Extra instructions" }] },
      { ...legacy, content: legacyRemoval + " " },
      { ...legacy, content: legacyRemoval.replace('{"gameId":', '{ "gameId":') },
      { ...legacy, content: legacyRemoval.replace('{"gameId":', '{"gameId":"duplicate", "gameId":') },
    ];
    const exactTarget = { gameId: 'game-"x"', artifactId: 'archive/{name}\n例' };
    const generated = createModRequestMessage("install", [exactTarget], false);
    return { parsed: [...old, ...structured].map(parseModRequest), generated, exactTarget };
  }, { legacyRemoval, translated, syntheticOrigin });
  expect(results.parsed).toEqual([
    { kind: "remove", batch: false, gameId: "game-fixture", ids: ['安装/"例"'] },
    ...Array(7).fill(null),
    { kind: "install", batch: true, gameId: "game-fixture", ids: ["archive/alpha", "archive/例"] },
    ...Array(7).fill(null),
  ]);
  expect(results.generated.content).toContain(JSON.stringify(results.exactTarget));
  expect(results.generated.content).toContain("match_mod_install");
  expect(results.generated.content).toContain("install_library_mod");
  expect(results.generated.origin.targets).toEqual([results.exactTarget]);
});

test("a persisted request folds across a locale change and retains its original input for editing", async ({ page }) => {
  await installMockTauri(page);
  await page.addInitScript(({ translated, syntheticOrigin }) => {
    const target = window as any;
    const wrap = (transport: any) => {
      if (!transport) return;
      const invoke = transport.invoke;
      transport.invoke = async (command: string, args: Record<string, unknown>) => {
        const result = await invoke(command, args);
        if (command !== "get_conversation" || args.id !== "conversation-fixture") return result;
        return { ...result, messages: [
          { id: "generated-mod", role: "user", content: translated, origin: syntheticOrigin },
          { id: "edited-mod", role: "user", content: translated + " Keep the backup.", origin: syntheticOrigin },
        ] };
      };
    };
    let transport = target.__TAURI_INTERNALS__;
    wrap(transport);
    Object.defineProperty(target, "__TAURI_INTERNALS__", {
      configurable: true,
      get: () => transport,
      set: value => { transport = value; wrap(value); },
    });
  }, { translated, syntheticOrigin });
  await page.goto("/");
  await page.getByRole("button", { name: "检查 Minecraft 模组", exact: true }).click();
  const card = page.locator('[data-message-id="generated-mod"]').filter({ has: page.locator("details") });
  await expect(card).toContainText("2 项");
  await expect(page.locator('[data-message-id="edited-mod"]')).toContainText("Keep the backup.");
  await page.evaluate(async () => {
    const { applyLanguageSettings } = await import("/src/lib/i18n.ts");
    applyLanguageSettings({ locale: "en", effective_locale: "en" });
  });
  await expect(card).toContainText("2 items");
  await expect(card).toBeVisible();
  await page.evaluate(async () => {
    const { applyLanguageSettings } = await import("/src/lib/i18n.ts");
    applyLanguageSettings({ locale: "zh-CN", effective_locale: "zh-CN" });
  });
  await card.locator("summary").click();
  await expect(card.getByText("archive/例", { exact: true }).last()).toBeVisible();
  await card.locator("xpath=ancestor::article").getByRole("button", { name: "编辑消息", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "告诉 Agent 你想做什么", exact: true })).toHaveValue(translated);
});
