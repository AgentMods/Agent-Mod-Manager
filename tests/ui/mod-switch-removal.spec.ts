import { expect, test, type Page } from "@playwright/test";
import type { AgentEvent } from "../../src/lib/types";
import { installMockTauri } from "./mock-tauri";

const gameId = "game-fixture";
const installId = "install-fixture";
const forgetTool = "forget_uninstalled_mod";
const storePath = "/src/lib/mod-switch-store.ts";
const agentPath = "/src/lib/agent-store.ts";
const scoped = { game_id: gameId, install_id: installId };
const removed = { status: "removed", gameId, installId };
const started = (name: string, arguments_: Record<string, unknown>, toolCallId = "call") =>
  ({ kind: "tool-start", payload: { name, arguments: arguments_, toolCallId } });
const result = (name: string, receipt: Record<string, unknown>, toolCallId = "call", status = "done") =>
  ({ kind: "tool-result", payload: { name, toolCallId, result: receipt, status } });
const row = (status: string, id = installId) => ({ id, gameId, status });
const notice = (page: Page, text: string | RegExp) => page.getByRole("status").filter({ hasText: text });
const success = "「目标 Mod」已从管理列表删除。";

type Readback = { rows: ReturnType<typeof row>[]; hold?: boolean } | { error: string };
type Turn = { installId: string; name: string; body: Pick<AgentEvent, "kind" | "payload">[]; done?: string };
const target = (body: Turn["body"], done?: string): Turn => ({ installId, name: "目标 Mod", body, done });

async function track(page: Page, turns: Turn[], readbacks: Readback[]) {
  const bindings = await page.evaluate(async ({ readbacks, turns, gameId, storePath, agentPath }) => {
    const modSwitch = await import(storePath) as typeof import("../../src/lib/mod-switch-store");
    const agent = await import(agentPath) as typeof import("../../src/lib/agent-store");
    const window_ = window as any;
    const tauri = window_.__TAURI_INTERNALS__;
    const invoke = tauri.invoke;
    let reads = 0;
    let release: () => void;
    const held = new Promise<void>(resolve => { release = resolve; });
    tauri.invoke = async (command: string, args: { gameId?: string }, options: unknown) => {
      if (command !== "list_installs" || args.gameId !== gameId) return invoke(command, args, options);
      const readback = readbacks[Math.min(reads++, readbacks.length - 1)];
      if ("error" in readback) throw new Error(readback.error);
      if (readback.hold) await held;
      return readback.rows;
    };
    const ids: string[] = [];
    let duplicateTurnAllowed = false;
    const outputs = turns.map(({ installId, name, body, done }, index) => {
      const conversationId = `removal-conversation-${index}`;
      const turnId = `removal-turn-${index}`;
      const id = modSwitch.reserveModSwitch({ kind: "remove", gameId, installId, name });
      if (!id) throw new Error("Could not reserve removal");
      if (index > 0) duplicateTurnAllowed ||= modSwitch.bindModSwitch(id, "removal-conversation-0", "removal-turn-0");
      const bound = modSwitch.bindModSwitch(id, conversationId, turnId);
      ids.push(id);
      const events = [{ kind: "turn-start", payload: {} }, ...body, { kind: "done", payload: { status: done ?? "done" } }]
        .map((event, seq): AgentEvent => ({ ...event, seq: seq + 100, conversationId, turnId, messageId: null, createdAt: "" }));
      // The regular cache listener should not own the target write. Results for
      // other rows must still reach it, even when they use the same native tool.
      return { events, bound, otherResults: modSwitch.withoutModSwitchWrites(events)
        .filter(event => event.kind === "tool-result").map(event => event.payload.toolCallId) };
    });
    window_.__modRemovalTest = {
      reads: () => reads,
      release: () => release(),
      status: (index: number) => modSwitch.useModSwitch.getState().pending[ids[index]]?.status ?? null,
      retry: (index: number) => modSwitch.retryModSwitchReadback(ids[index]),
      replay: async (index: number) => {
        const { events } = outputs[index];
        window_.__AMM_TEST__.seedEvents(events);
        await agent.replayConversation(events[0].conversationId);
      },
    };
    return { bound: outputs.map(output => output.bound), duplicateTurnAllowed, otherResults: outputs.map(output => output.otherResults) };
  }, { readbacks, turns, gameId, storePath, agentPath });
  expect(bindings.bound).toEqual(turns.map(() => true));
  expect(bindings.duplicateTurnAllowed).toBe(false);
  return {
    otherResults: bindings.otherResults,
    reads: () => page.evaluate(() => (window as any).__modRemovalTest.reads()),
    release: () => page.evaluate(() => (window as any).__modRemovalTest.release()),
    status: (index = 0) => page.evaluate(i => (window as any).__modRemovalTest.status(i), index),
    retry: (index = 0) => page.evaluate(i => (window as any).__modRemovalTest.retry(i), index),
    replay: (index = 0) => page.evaluate(i => (window as any).__modRemovalTest.replay(i), index),
  };
}

test.beforeEach(async ({ page }) => {
  await installMockTauri(page);
  await page.goto("/");
  await expect(page.getByRole("button", { name: "游戏库", exact: true })).toBeVisible();
});

test("removal waits for list readback even when a scoped receipt committed before the turn was cancelled", async ({ page }) => {
  const tracked = await track(page, [target([
    started(forgetTool, { ...scoped, install_id: "other-install" }, "other"),
    result(forgetTool, { ...removed, installId: "other-install" }, "other"),
    started(forgetTool, scoped), result(forgetTool, removed),
  ], "cancelled")], [{ rows: [row("enabled", "another-install")], hold: true }]);
  expect(tracked.otherResults).toEqual([["other"]]);
  await tracked.replay();
  await expect.poll(tracked.reads).toBe(1);
  expect(await tracked.status()).toBe("verifying");
  await expect(notice(page, success)).toHaveCount(0);
  await tracked.release();
  await expect(notice(page, success)).toBeVisible();
  expect(await tracked.status()).toBeNull();
});

test("a scoped receipt does not report success while the exact record is still on the list", async ({ page }) => {
  const tracked = await track(page, [target([started(forgetTool, scoped), result(forgetTool, removed)])], [{ rows: [row("uninstalled")] }]);
  await tracked.replay();
  await expect(notice(page, /收到删除回执，但该记录仍在管理列表中/)).toBeVisible();
  await expect(notice(page, success)).toHaveCount(0);
});

test("independent removal turns keep intermediate uninstall and rollback results distinct", async ({ page }) => {
  const other = "other-install";
  const tracked = await track(page, [target([
    started("uninstall_mod", scoped), result("uninstall_mod", { status: "committed", gameId, installId }),
  ]), { installId: other, name: "第二个 Mod", body: [
    started("rollback_transaction", { ...scoped, install_id: other, transaction_id: "transaction" }),
    result("rollback_transaction", { status: "rolled_back", gameId, installId: other, transactionId: "transaction" }),
  ] }], [
    { rows: [row("uninstalled"), row("enabled", other)], hold: true },
    { rows: [row("uninstalled"), row("rolled_back", other)] },
  ]);
  await tracked.replay();
  await expect.poll(tracked.reads).toBe(1);
  await tracked.replay(1);
  expect(await tracked.status(1)).toBe("verifying");
  expect(await tracked.reads()).toBe(1);
  await tracked.release();
  await expect(notice(page, /未执行「目标 Mod」的删除.*当前为已卸载，仍在管理列表中/)).toBeVisible();
  await expect(notice(page, /未执行「第二个 Mod」的删除.*当前为已回滚，仍在管理列表中/)).toBeVisible();
  expect(await tracked.reads()).toBe(2);
  await expect(notice(page, /已从管理列表删除/)).toHaveCount(0);
});

for (const [label, call, receipt] of [
  ["wrong call game", { ...scoped, game_id: "another-game" }, removed],
  ["wrong call install", { ...scoped, install_id: "another-install" }, removed],
  ["wrong receipt game", scoped, { ...removed, gameId: "another-game" }],
  ["wrong receipt install", scoped, { ...removed, installId: "another-install" }],
  ["missing receipt scope", scoped, { status: "removed" }],
  ["wrong receipt status", scoped, { ...removed, status: "committed" }],
  ["no tool result", scoped, null],
] as const) {
  test(`removal is not confirmed by list absence alone: ${label}`, async ({ page }) => {
    const tracked = await track(page, [target([started(forgetTool, call), ...(receipt ? [result(forgetTool, receipt)] : [])])], [{ rows: [] }]);
    await tracked.replay();
    await expect(notice(page, /管理列表中已无该记录，但没有有效删除回执，无法确认是否由本次任务删除/)).toBeVisible();
    await expect(notice(page, success)).toHaveCount(0);
  });
}

test("a rejected removal reports that the now-uninstalled record remains", async ({ page }) => {
  const tracked = await track(page, [target([
    started("uninstall_mod", scoped, "uninstall"), result("uninstall_mod", { status: "committed" }, "uninstall"),
    started(forgetTool, scoped),
    { kind: "approval-request", payload: { approvalId: "review", toolCallId: "call" } },
    { kind: "approval-resolved", payload: { approvalId: "review", status: "rejected" } },
    result(forgetTool, { error: "用户拒绝了此操作" }, "call", "error"),
  ])], [{ rows: [row("uninstalled")] }]);
  await tracked.replay();
  await expect(notice(page, /已拒绝删除「目标 Mod」.*当前为已卸载，仍在管理列表中/)).toBeVisible();
  await expect(notice(page, success)).toHaveCount(0);
});

test("a missing-files inspection without record removal does not claim success", async ({ page }) => {
  const tracked = await track(page, [target([
    started("inspect_install", scoped),
    result("inspect_install", { gameId, installId, reconciliation: { recommendedAction: "forget_missing_files" } }),
  ])], [{ rows: [row("enabled")] }]);
  await tracked.replay();
  await expect(notice(page, /安装文件已不在，Agent 未执行删除。该记录仍在管理列表中/)).toBeVisible();
});

test("failed list readback stays retryable without restarting the removal turn", async ({ page }) => {
  const tracked = await track(page, [target([started(forgetTool, scoped), result(forgetTool, removed)])], [
    { error: "fixture list unavailable" }, { rows: [] },
  ]);
  await tracked.replay();
  await expect(notice(page, /最新管理列表：fixture list unavailable。可重试读取/)).toBeVisible();
  expect(await tracked.status()).toBe("readback_failed");
  await expect(notice(page, success)).toHaveCount(0);
  await tracked.retry();
  await expect(notice(page, success)).toBeVisible();
  expect(await tracked.reads()).toBe(2);
  expect(await tracked.status()).toBeNull();
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls.some((call: { command: string }) => call.command === "start_turn"))).toBe(false);
});

test("a tracked deployed row displays deletion in progress and disables its switch", async ({ page }) => {
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await page.getByRole("button").filter({ has: page.getByRole("heading", { name: "Minecraft Java Edition", exact: true }) }).click();
  const table = page.getByTestId("game-detail-page").getByRole("table", { name: "Mod 管理", exact: true });
  const deployed = table.getByRole("row").filter({ hasText: "Example" });
  const toggle = deployed.getByRole("switch", { name: "使用 Agent 卸载", exact: true });
  await expect(toggle).toBeEnabled();
  await track(page, [target([])], [{ rows: [] }]);
  await expect(deployed).toContainText("删除中…");
  await expect(toggle).toBeDisabled();
  await expect(toggle).toHaveAttribute("aria-busy", "true");
});
