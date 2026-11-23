import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const gameId = "game-fixture";
const otherGameId = "other-game";
const installId = "install-hash-fixture";
const artifactSha256 = "a".repeat(64);

async function installOrganizationEventFixture(page: Page, delaySubscription = false) {
  await installMockTauri(page, { workspaceGames: [{ id: otherGameId, name: "Other Game" }] });
  await page.addInitScript(({ targetGameId, targetInstallId, digest }) => {
    const target = window as any;
    const initial = {
      id: targetInstallId,
      gameId: targetGameId,
      name: "Sakura - Texture Pack",
      version: "1.0.0",
      source: "local",
      status: "enabled",
      installRoot: "/sandbox/minecraft/mods",
      files: [],
      metadata: { artifactSha256: digest },
      installedAt: "2026-11-18T00:00:00Z",
      tagIds: [],
    };
    const organized = {
      ...initial,
      name: "Sakura - 材质包",
      tagIds: ["tag-other"],
    };
    const state = { organized: false };
    const record = (command: string, args: Record<string, unknown>) => target.__AMM_TEST__.calls.push({ command, args });
    const transport = target.__TAURI_INTERNALS__;
    const originalInvoke = transport.invoke;
    transport.invoke = async (command: string, args: Record<string, any> = {}, options?: unknown) => {
      if (command === "list_installs") {
        record(command, args);
        return args.gameId === targetGameId
          ? [structuredClone(state.organized ? organized : initial)]
          : [];
      }
      if (command === "list_mod_tags") {
        record(command, args);
        return args.gameId === targetGameId && state.organized
          ? [{ id: "tag-other", name: "Other", color: "#6366F1" }]
          : [];
      }
      if (["save_mod_tag", "delete_mod_tag", "set_mod_tags"].includes(command)) {
        record(command, args);
        return null;
      }
      return originalInvoke(command, args, options);
    };
    target.__MOD_ORGANIZATION_EVENT_TEST__ = {
      organize() { state.organized = true; },
      emit(id: string, recordId: string) {
        target.__AMM_TEST__.emitEvent("mod://organized", { gameId: id, installId: recordId });
      },
    };
  }, { targetGameId: gameId, targetInstallId: installId, digest: artifactSha256 });

  if (delaySubscription) {
    await page.addInitScript(() => {
      const target = window as any;
      const transport = target.__TAURI_INTERNALS__;
      const originalInvoke = transport.invoke;
      transport.invoke = (command: string, args: Record<string, any> = {}, options?: unknown) => {
        if (command === "plugin:event|listen" && args.event === "mod://organized") {
          return new Promise((resolve, reject) => {
            target.__MOD_ORGANIZATION_EVENT_TEST__.releaseSubscription = () => {
              Promise.resolve(originalInvoke(command, args, options)).then(resolve, reject);
            };
          });
        }
        return originalInvoke(command, args, options);
      };
    });
  }
}

async function openGame(page: Page, name: string) {
  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await page.getByRole("button").filter({ has: page.getByRole("heading", { name, exact: true }) }).click();
  await expect(page.getByTestId("game-detail-page")).toBeVisible();
  await page.getByRole("tab", { name: "Mod 管理", exact: true }).click();
}

function installRow(page: Page, name: string) {
  return page.getByTestId("game-detail-page").getByRole("table", { name: "Mod 管理", exact: true })
    .getByRole("row").filter({ hasText: name });
}

async function organizeAndEmit(page: Page) {
  await page.evaluate(() => {
    const fixture = (window as any).__MOD_ORGANIZATION_EVENT_TEST__;
    fixture.organize();
    fixture.emit("game-fixture", "install-hash-fixture");
  });
}

test("organized event refreshes the install name and Other tag; repeated events trigger no writes", async ({ page }) => {
  await installOrganizationEventFixture(page);
  await page.goto("/");
  await openGame(page, "Minecraft Java Edition");

  await expect(installRow(page, "Sakura - Texture Pack")).toBeVisible();
  const before = await page.evaluate(() => (window as any).__AMM_TEST__.calls.length);
  await organizeAndEmit(page);

  const row = installRow(page, "Sakura - 材质包");
  await expect(row).toBeVisible();
  await expect(row.getByText("Other", { exact: true })).toBeVisible();
  await expect.poll(async () => page.evaluate((start) => (window as any).__AMM_TEST__.calls.slice(start)
    .filter((call: any) => ["list_installs", "list_mod_tags"].includes(call.command)), before)).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ command: "list_installs", args: { gameId } }),
        expect.objectContaining({ command: "list_mod_tags", args: { gameId } }),
      ]),
    );

  await page.evaluate(() => {
    const fixture = (window as any).__MOD_ORGANIZATION_EVENT_TEST__;
    fixture.emit("game-fixture", "install-hash-fixture");
    fixture.emit("game-fixture", "install-hash-fixture");
  });
  await expect(row).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__AMM_TEST__.calls);
  expect(calls.filter((call: any) => ["save_mod_tag", "delete_mod_tag", "set_mod_tags"].includes(call.command))).toEqual([]);
});

test("an event for a different game stays scoped while the user switches details", async ({ page }) => {
  await installOrganizationEventFixture(page);
  await page.goto("/");
  await openGame(page, "Minecraft Java Edition");
  await expect(installRow(page, "Sakura - Texture Pack")).toBeVisible();

  await page.getByRole("button", { name: "游戏库", exact: true }).click();
  await page.getByRole("button").filter({ has: page.getByRole("heading", { name: "Other Game", exact: true }) }).click();
  await expect(page.getByTestId("game-detail-page")).toBeVisible();
  const otherGameReads = await page.evaluate(() => (window as any).__AMM_TEST__.calls
    .filter((call: any) => ["list_installs", "list_mod_tags"].includes(call.command) && call.args.gameId === "other-game").length);

  await organizeAndEmit(page);
  await expect.poll(async () => page.evaluate(() => (window as any).__AMM_TEST__.calls
    .filter((call: any) => ["list_installs", "list_mod_tags"].includes(call.command) && call.args.gameId === "other-game").length))
    .toBe(otherGameReads);

  await openGame(page, "Minecraft Java Edition");
  const row = installRow(page, "Sakura - 材质包");
  await expect(row).toBeVisible();
  await expect(row.getByText("Other", { exact: true })).toBeVisible();
  await expect.poll(async () => page.evaluate(() => (window as any).__AMM_TEST__.calls
    .filter((call: any) => call.command === "list_installs" && call.args.gameId === "game-fixture").length))
    .toBeGreaterThan(1);
});

test("a query completed before event listener registration is refreshed when subscription becomes ready", async ({ page }) => {
  await installOrganizationEventFixture(page, true);
  await page.goto("/");
  await openGame(page, "Minecraft Java Edition");
  await expect(installRow(page, "Sakura - Texture Pack")).toBeVisible();
  await expect.poll(() => page.evaluate(() => typeof (window as any).__MOD_ORGANIZATION_EVENT_TEST__.releaseSubscription))
    .toBe("function");

  await page.evaluate(() => {
    const fixture = (window as any).__MOD_ORGANIZATION_EVENT_TEST__;
    fixture.organize();
    fixture.releaseSubscription();
  });

  const row = installRow(page, "Sakura - 材质包");
  await expect(row).toBeVisible();
  await expect(row.getByText("Other", { exact: true })).toBeVisible();
});
