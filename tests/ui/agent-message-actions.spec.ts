import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const original = "请检查模组清单";
const reply = "已检查 **3** 个模组。";

async function openConversation(page: Page) {
  await installMockTauri(page);
  await page.addInitScript(({ original, reply }) => {
    window.addEventListener("DOMContentLoaded", () => {
      const fixture = (window as any).__AMM_TEST__;
      const tauri = (window as any).__TAURI_INTERNALS__;
      const invoke = tauri.invoke;
      const message = (id: string, role: string, content: string) => ({
        id, role, content, reasoning: null, tool_calls: null, tool_call_id: null,
      });
      const sourceMessages = [message("user-source", "user", original), message("assistant-source", "assistant", reply)];
      const branchMessages = new Map<string, ReturnType<typeof message>[]>();

      Object.defineProperty(navigator.clipboard, "writeText", {
        value: async (text: string) => { fixture.clipboardText = text; },
      });
      tauri.invoke = async (command: string, args: Record<string, any> = {}) => {
        if (command === "get_conversation") {
          const detail = await invoke(command, args);
          const messages = args.id === "conversation-fixture" ? sourceMessages : branchMessages.get(args.id);
          return messages ? { ...detail, messages } : detail;
        }
        if (command === "restart_turn") {
          fixture.calls.push({ command, args });
          const content = args.edit ? args.request.content : original;
          const branch = await invoke("create_conversation", { title: "消息分支", gameId: "game-fixture" });
          branchMessages.set(branch.id, [
            message("user-branch", "user", content),
            message("assistant-branch", "assistant", "重新生成的回复"),
          ]);
          return branch.id;
        }
        return invoke(command, args);
      };
    }, { once: true });
  }, { original, reply });
  await page.goto("/");
  await page.getByRole("button", { name: /^Agent( 对话)?$/ }).click();
  await page.getByRole("button", { name: "检查 Minecraft 模组", exact: true }).click();
}

const restartRequest = (page: Page) => page.evaluate(() => (
  (window as any).__AMM_TEST__.calls.find((call: any) => call.command === "restart_turn")?.args
));

function footer(page: Page, role: "user" | "assistant") {
  return page.locator(`article.chat-message.${role} [data-slot="message-footer"]`);
}

test("message footers copy the original source and retry the selected answer", async ({ page }) => {
  await openConversation(page);
  for (const [role, text] of [["user", original], ["assistant", reply]] as const) {
    await footer(page, role).getByRole("button", { name: "复制消息" }).click();
    await expect(footer(page, role).getByRole("button", { name: "已复制" })).toBeVisible();
    expect(await page.evaluate(() => (window as any).__AMM_TEST__.clipboardText)).toBe(text);
  }
  await footer(page, "assistant").getByRole("button", { name: "重试", exact: true }).click();
  await expect(page.getByText("重新生成的回复")).toBeVisible();
  expect(await restartRequest(page)).toMatchObject({ messageId: "assistant-source", edit: false, request: { conversationId: "conversation-fixture" } });
  await page.getByRole("button", { name: "检查 Minecraft 模组", exact: true }).click();
  await expect(page.locator('[data-message-id="assistant-source"]')).toHaveText("已检查 3 个模组。");
});

test("editing loads the selected user text and sends the replacement on confirmation", async ({ page }) => {
  await openConversation(page);
  const composer = page.getByRole("textbox", { name: "告诉 Agent 你想做什么" });
  await composer.fill("未发送草稿");
  const edit = footer(page, "user").getByRole("button", { name: "编辑消息" });
  await edit.click();
  await expect(composer).toBeFocused();
  await expect(composer).toHaveValue(original);
  await page.getByRole("button", { name: "取消编辑" }).click();
  await expect(composer).toHaveValue("未发送草稿");
  await edit.click();
  await composer.fill("请只检查已启用的模组");
  await page.getByRole("button", { name: "发送消息" }).click();
  await expect(page.locator('[data-message-id="user-branch"]')).toHaveText("请只检查已启用的模组");
  expect(await restartRequest(page)).toMatchObject({ messageId: "user-source", edit: true, request: { content: "请只检查已启用的模组" } });
});
