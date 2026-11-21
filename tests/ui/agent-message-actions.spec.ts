import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const conversationId = "conversation-fixture";
const original = "请检查模组清单";
const reply = "已检查 **3** 个模组。";
const regeneratedReply = "重新生成的回复";
const priorUser = "上一轮用户请求";
const priorReply = "上一轮 Agent 回复";
const laterUser = "后续轮次请求";
const laterReply = "后续轮次回复";

async function openConversation(page: Page) {
  await installMockTauri(page);
  await page.addInitScript(({ original, reply, priorUser, priorReply, laterUser, laterReply, regeneratedReply }) => {
    window.addEventListener("DOMContentLoaded", () => {
      const fixture = (window as any).__AMM_TEST__;
      const tauri = (window as any).__TAURI_INTERNALS__;
      const invoke = tauri.invoke;
      const message = (id: string, role: string, content: string) => ({
        id, role, content, reasoning: null, tool_calls: null, tool_call_id: null,
      });
      let messages = [
        message("user-prior", "user", priorUser),
        message("assistant-prior", "assistant", priorReply),
        message("user-target", "user", original),
        message("assistant-target", "assistant", reply),
        message("user-later", "user", laterUser),
        message("assistant-later", "assistant", laterReply),
      ];

      Object.defineProperty(navigator.clipboard, "writeText", {
        value: async (text: string) => { fixture.clipboardText = text; },
      });
      tauri.invoke = async (command: string, args: Record<string, any> = {}) => {
        if (command === "get_conversation") {
          const detail = await invoke(command, args);
          return args.id === "conversation-fixture" ? { ...detail, messages: structuredClone(messages) } : detail;
        }
        if (command === "restart_turn") {
          fixture.calls.push({ command, args });
          if (args.request.conversationId !== "conversation-fixture") throw new Error("restart must stay in the selected conversation");
          const target = messages.findIndex((item) => item.id === args.messageId);
          if (target < 0) throw new Error(`unknown restart target: ${args.messageId}`);
          const replacement = args.edit
            ? [
              message("user-target", "user", args.request.content),
              message("assistant-new", "assistant", regeneratedReply),
            ]
            : [message("assistant-new", "assistant", regeneratedReply)];
          messages = [
            ...messages.slice(0, target),
            ...replacement,
          ];
          return "conversation-fixture";
        }
        return invoke(command, args);
      };
    }, { once: true });
  }, { original, reply, priorUser, priorReply, laterUser, laterReply, regeneratedReply });
  await page.goto("/");
  await page.getByRole("button", { name: /^Agent( 对话)?$/ }).click();
  await page.getByRole("button", { name: "检查 Minecraft 模组", exact: true }).click();
}

const restartRequest = (page: Page) => page.evaluate(() => (
  (window as any).__AMM_TEST__.calls.find((call: any) => call.command === "restart_turn")?.args
));

function footer(page: Page, messageId: string) {
  return page.locator("article.chat-message").filter({ has: page.locator(`[data-message-id="${messageId}"]`) })
    .locator('[data-slot="message-footer"]');
}

async function expectConversationUnchangedInSidebar(page: Page) {
  const conversation = page.getByRole("button", { name: "检查 Minecraft 模组", exact: true });
  await expect(conversation).toHaveAttribute("aria-current", "true");
  await expect(page.locator('ul[aria-label="最近对话"] > li')).toHaveCount(1);
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.conversations.map((item: any) => item.id)))
    .toEqual([conversationId]);
}

test("message footers copy messages and retry in place from the selected turn", async ({ page }) => {
  await openConversation(page);
  for (const [messageId, text] of [["user-target", original], ["assistant-target", reply]] as const) {
    await footer(page, messageId).getByRole("button", { name: "复制消息" }).click();
    await expect(footer(page, messageId).getByRole("button", { name: "已复制" })).toBeVisible();
    expect(await page.evaluate(() => (window as any).__AMM_TEST__.clipboardText)).toBe(text);
  }

  await footer(page, "assistant-target").getByRole("button", { name: "重试", exact: true }).click();
  await expect(page.locator('[data-message-id="assistant-new"]')).toHaveText(regeneratedReply);
  await expect(page.locator('[data-message-id="assistant-target"]')).toHaveCount(0);
  await expect(page.locator('[data-message-id="user-prior"]')).toHaveText(priorUser);
  await expect(page.locator('[data-message-id="assistant-prior"]')).toHaveText(priorReply);
  await expect(page.locator('[data-message-id="user-target"]')).toHaveText(original);
  await expect(page.locator('[data-message-id="user-later"], [data-message-id="assistant-later"]')).toHaveCount(0);
  await expect(page.locator("article.chat-message")).toHaveCount(4);
  expect(await restartRequest(page)).toMatchObject({
    messageId: "assistant-target", edit: false, request: { conversationId },
  });
  await expectConversationUnchangedInSidebar(page);
});

test("editing replaces the selected turn in place and cancelling restores the composer draft", async ({ page }) => {
  await openConversation(page);
  const composer = page.getByRole("textbox", { name: "告诉 Agent 你想做什么" });
  await composer.fill("未发送草稿");
  const edit = footer(page, "user-target").getByRole("button", { name: "编辑消息" });
  await edit.click();
  await expect(composer).toBeFocused();
  await expect(composer).toHaveValue(original);
  await page.getByRole("button", { name: "取消编辑" }).click();
  await expect(composer).toHaveValue("未发送草稿");

  await edit.click();
  await composer.fill("请只检查已启用的模组");
  await page.getByRole("button", { name: "发送消息" }).click();
  await expect(page.locator('[data-message-id="user-target"]')).toHaveText("请只检查已启用的模组");
  await expect(page.locator('[data-message-id="assistant-new"]')).toHaveText(regeneratedReply);
  await expect(page.locator('[data-message-id="assistant-target"]')).toHaveCount(0);
  await expect(page.locator('[data-message-id="user-prior"]')).toHaveText(priorUser);
  await expect(page.locator('[data-message-id="assistant-prior"]')).toHaveText(priorReply);
  await expect(page.locator('[data-message-id="user-later"], [data-message-id="assistant-later"]')).toHaveCount(0);
  await expect(page.locator("article.chat-message")).toHaveCount(4);
  expect(await restartRequest(page)).toMatchObject({
    messageId: "user-target", edit: true, request: { conversationId, content: "请只检查已启用的模组" },
  });
  await expectConversationUnchangedInSidebar(page);
});
