import { expect, test, type Page } from "@playwright/test";
import { installMockTauri } from "./mock-tauri";

const pngBase64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j2ioAAAAASUVORK5CYII=";
const toBuffer = (value: string, encoding?: string) => (globalThis as any).Buffer.from(value, encoding);
const pngFile = {name: "screenshot.png", mimeType: "image/png", buffer: toBuffer(pngBase64, "base64")};
const pngDataUrl = `data:image/png;base64,${pngBase64}`;

type ConversionOutcome =
  | {kind: "resolve"; markdown: string}
  | {kind: "reject"; message: string}
  | {kind: "consent"; message: string}
  | {kind: "defer"; ignoreCancel?: boolean};

async function installAttachmentMock(page: Page, outcomes: ConversionOutcome[] = [], holdBegin = false) {
  await installMockTauri(page);
  await page.addInitScript(({conversionOutcomes, holdBegin}) => {
    window.addEventListener("DOMContentLoaded", () => {
      const target = window as any;
      const fixture = target.__AMM_TEST__;
      const tauri = target.__TAURI_INTERNALS__;
      const originalInvoke = tauri.invoke;
      const resolvers = new Map<number, {id: string; resolve: (markdown: string) => void; cancel: () => void}>();
      const registered = new Set<string>();
      const settled: boolean[] = [];
      let conversionIndex = 0;

      target.__AMM_ATTACHMENT_TEST__ = {
        settled,
        finishBegin: undefined as (() => void) | undefined,
        resolve(index: number, markdown: string) {
          resolvers.get(index)?.resolve(markdown);
        },
      };

      tauri.invoke = async (command: string, args: any = {}, options?: Record<string, any>) => {
        if (command === "begin_agent_attachment") {
          fixture.calls.push({command, args});
          if (holdBegin) await new Promise<void>((resolve) => { target.__AMM_ATTACHMENT_TEST__.finishBegin = resolve; });
          registered.add(args.conversionId);
          return null;
        }
        if (command === "cancel_agent_attachment") {
          fixture.calls.push({command, args});
          registered.delete(args.conversionId);
          for (const entry of resolvers.values()) if (entry.id === args.conversionId) entry.cancel();
          return null;
        }
        if (command !== "convert_agent_attachment") return originalInvoke(command, args, options);

        const id = options?.headers["x-attachment-conversion-id"];
        if (!registered.delete(id)) throw {code: "cancelled", message: "附件转换已取消"};
        const index = conversionIndex++;
        fixture.calls.push({command, args, options});
        const outcome = conversionOutcomes[index] ?? {kind: "resolve", markdown: "# Converted attachment"};
        try {
          if (outcome.kind === "reject") throw new Error(outcome.message);
          if (outcome.kind === "consent") throw {code: "consent_required", message: outcome.message};
          if (outcome.kind === "defer") {
            return await new Promise<string>((resolve, reject) => resolvers.set(index, {
              id, resolve,
              cancel: () => { if (!outcome.ignoreCancel) reject({code: "cancelled", message: "附件转换已取消"}); },
            }));
          }
          return outcome.markdown;
        } finally {
          resolvers.delete(index);
          settled[index] = true;
        }
      };
    }, {once: true});
  }, {conversionOutcomes: outcomes, holdBegin});
}

const markitdownPreset = {
  id: "markitdown",
  name: "MarkItDown MCP",
  transport: "stdio",
  command: "uvx",
  args: ["--prerelease", "allow", "markitdown-mcp@latest"],
  url: null,
  env_secret_refs: {},
  bearer_secret_ref: null,
  enabled: false,
  builtin: true,
};

type MarkitdownMockOptions = {holdConnect?: boolean; failConnect?: string};

async function installMarkitdownMock(page: Page, {holdConnect = false, failConnect}: MarkitdownMockOptions) {
  await page.addInitScript(({preset, holdConnect, failConnect}) => {
    window.addEventListener("DOMContentLoaded", () => {
      const target = window as any;
      const fixture = target.__AMM_TEST__;
      const tauri = target.__TAURI_INTERNALS__;
      const originalInvoke = tauri.invoke.bind(tauri);
      let config = preset;
      const connection = {failConnect, finishConnect: undefined as (() => void) | undefined};
      target.__AMM_MARKITDOWN_TEST__ = connection;
      tauri.invoke = async (command: string, args: any = {}, ...rest: unknown[]) => {
        if (command === "mcp_list") {
          fixture.calls.push({command, args});
          return [config];
        }
        if (command === "mcp_update") {
          fixture.calls.push({command, args});
          config = args.config;
          return null;
        }
        if (command === "mcp_connect") {
          fixture.calls.push({command, args});
          if (holdConnect) await new Promise<void>((resolve) => { connection.finishConnect = resolve; });
          if (connection.failConnect) throw new Error(connection.failConnect);
          return {serverId: args.expectedConfig.id, tools: []};
        }
        return originalInvoke(command, args, ...rest);
      };
    }, {once: true});
  }, {preset: markitdownPreset, holdConnect, failConnect});
}

async function openAgent(page: Page, outcomes: ConversionOutcome[] = [], markitdown?: MarkitdownMockOptions, holdBegin = false) {
  await installAttachmentMock(page, outcomes, holdBegin);
  if (markitdown) await installMarkitdownMock(page, markitdown);
  await page.goto("/");
  await page.getByRole("button", {name: /^Agent( 对话)?$/}).click();
  await expect(page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true})).toBeVisible();
}

const commandCalls = (page: Page, command: string) => page.evaluate((name) => (
  (window as any).__AMM_TEST__.calls.filter((call: any) => call.command === name)
), command);

test("附件按钮可访问且支持多选，图片原样作为唯一消息发送", async ({page}) => {
  await openAgent(page);

  const addButton = page.getByRole("button", {name: "添加附件", exact: true});
  const fileInput = page.locator('input[type="file"]');
  await expect(addButton).toBeVisible();
  await expect(fileInput).toHaveCount(1);
  await expect(fileInput).toHaveAttribute("multiple", "");

  const chooserEvent = page.waitForEvent("filechooser");
  await addButton.click();
  const chooser = await chooserEvent;
  expect(chooser.isMultiple()).toBe(true);
  await chooser.setFiles(pngFile);

  await expect(page.getByLabel("待发送附件")).toBeVisible();
  await expect(page.getByText("screenshot.png", {exact: true})).toBeVisible();
  await expect(page.getByRole("button", {name: "启动 MarkItDown MCP", exact: true})).toHaveCount(0);
  const sendButton = page.getByRole("button", {name: "发送消息", exact: true});
  await expect(sendButton).toBeEnabled();
  await sendButton.click();

  await expect.poll(async () => (await commandCalls(page, "start_turn")).length).toBe(1);
  const request = (await commandCalls(page, "start_turn"))[0].args.request;
  expect(request.content).toBe("");
  expect(request.attachments).toEqual([{kind: "image", name: "screenshot.png", dataUrl: pngDataUrl}]);
  expect(await commandCalls(page, "convert_agent_attachment")).toEqual([]);
  await expect(page.getByLabel("消息附件")).toBeVisible();
  await expect(page.getByRole("button", {name: "预览 screenshot.png", exact: true})).toBeVisible();
});

test("文档转换失败后可重选，保留草稿和已添加图片并按约定发送", async ({page}) => {
  const markdown = "# Manual\n\nKeep the original formatting.";
  await openAgent(page, [
    {kind: "reject", message: "conversion failed"},
    {kind: "resolve", markdown},
  ]);

  const composer = page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true});
  await composer.fill("请结合图片阅读这份说明");
  const fileInput = page.locator('input[type="file"]');
  const document = {name: "manual.html", mimeType: "text/html", buffer: toBuffer("source document")};
  await fileInput.setInputFiles([pngFile, document]);

  await page.waitForFunction(() => (window as any).__AMM_ATTACHMENT_TEST__?.settled[0] === true);
  await expect(page.getByLabel("待发送附件")).toBeVisible();
  await expect(page.getByRole("button", {name: "发送消息", exact: true})).toBeDisabled();
  expect(await commandCalls(page, "start_turn")).toEqual([]);

  await page.getByRole("button", {name: "重试 manual.html", exact: true}).click();
  await page.waitForFunction(() => (window as any).__AMM_ATTACHMENT_TEST__?.settled[1] === true);
  await expect(composer).toHaveValue("请结合图片阅读这份说明");
  await expect(page.getByText("screenshot.png", {exact: true})).toBeVisible();
  await expect(page.getByText("manual.html", {exact: true})).toBeVisible();
  await expect(page.getByRole("button", {name: "发送消息", exact: true})).toBeEnabled();

  const conversions = await page.evaluate(() => (window as any).__AMM_TEST__.calls
    .filter((call: any) => call.command === "convert_agent_attachment")
    .map((call: any) => ({
      name: call.options.headers["x-attachment-name"],
      bytes: Array.from(new Uint8Array(call.args as ArrayBuffer)),
    })));
  const attempts = (await commandCalls(page, "begin_agent_attachment")).map((call) => call.args.conversionId);
  expect(new Set(attempts).size).toBe(2);
  expect(attempts).toEqual((await commandCalls(page, "convert_agent_attachment")).map((call) => call.options.headers["x-attachment-conversion-id"]));
  expect(conversions).toEqual([
    {name: encodeURIComponent("manual.html"), bytes: Array.from(new TextEncoder().encode("source document"))},
    {name: encodeURIComponent("manual.html"), bytes: Array.from(new TextEncoder().encode("source document"))},
  ]);

  await page.getByRole("button", {name: "发送消息", exact: true}).click();
  await expect.poll(async () => (await commandCalls(page, "start_turn")).length).toBe(1);
  const request = (await commandCalls(page, "start_turn"))[0].args.request;
  expect(request.content).toBe("请结合图片阅读这份说明");
  expect(request.attachments).toEqual([
    {kind: "image", name: "screenshot.png", dataUrl: pngDataUrl},
    {kind: "document", name: "manual.html", markdown},
  ]);
  await expect(page.getByLabel("消息附件")).toBeVisible();
  await expect(page.getByRole("button", {name: "预览 screenshot.png", exact: true})).toBeVisible();
  await expect(page.getByRole("button", {name: "预览 manual.html", exact: true})).toBeVisible();
});

test("大图片、文档和多附件不会阻止消息发送", async ({page}) => {
  await openAgent(page);
  const composer = page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true});
  const text = "长".repeat(45_000) + "结尾";
  await composer.fill(text);
  await page.locator('input[type="file"]').evaluate((input: HTMLInputElement, png) => {
    const MiB = 1024 * 1024;
    const pngBytes = Uint8Array.from(atob(png), (char) => char.charCodeAt(0));
    const files = new DataTransfer();
    const payload = new Uint8Array(5 * MiB);
    for (let i = 0; i < 4; i++) {
      files.items.add(new File([pngBytes, payload], `large-${i}.png`, {type: "image/png"}));
    }
    for (let i = 0; i < 7; i++) {
      files.items.add(new File([pngBytes], `small-${i}.png`, {type: "image/png"}));
    }
    files.items.add(new File(["# Large", new Uint8Array(10 * MiB).fill(120)], "large.md", {type: "text/markdown"}));
    input.files = files.files;
    input.dispatchEvent(new Event("change", {bubbles: true}));
  }, pngBase64);

  const send = page.getByRole("button", {name: "发送消息", exact: true});
  await expect(page.getByLabel("待发送附件").getByRole("button", {name: /^移除 /})).toHaveCount(12);
  await expect(send).toBeEnabled({timeout: 15_000});
  await send.click();
  await expect.poll(() => page.evaluate(() => {
    const calls = (window as any).__AMM_TEST__.calls;
    const sent = calls.find((call: any) => call.command === "start_turn")?.args.request;
    const converted = calls.find((call: any) => call.command === "convert_agent_attachment");
    return sent && {
      textLength: sent.content.length,
      textSuffix: sent.content.slice(-2),
      attachments: sent.attachments.length,
      bigImages: sent.attachments.filter((entry: any) => entry.kind === "image" && entry.dataUrl.length > 5 * 1024 * 1024 * 4 / 3).length,
      bigDoc: converted.args.byteLength > 10 * 1024 * 1024,
      lastAttachment: sent.attachments.at(-1).name,
    };
  })).toEqual({textLength: text.length, textSuffix: "结尾", attachments: 12, bigImages: 4, bigDoc: true, lastAttachment: "large.md"});
});

test("移除转换中的附件会取消对应请求并继续下一个附件", async ({page}) => {
  await openAgent(page, [{kind: "defer"}]);
  const fileInput = page.locator('input[type="file"]');
  await fileInput.setInputFiles([
    {name: "first.html", mimeType: "text/html", buffer: toBuffer("<p>First</p>")},
    {name: "second.html", mimeType: "text/html", buffer: toBuffer("<p>Second</p>")},
  ]);
  await expect.poll(async () => (await commandCalls(page, "convert_agent_attachment")).length).toBe(1);
  await page.getByRole("button", {name: "移除 first.html", exact: true}).click();
  await expect(page.getByRole("button", {name: "移除 first.html", exact: true})).toHaveCount(0);
  const firstCall = (await commandCalls(page, "convert_agent_attachment"))[0];
  await expect.poll(() => commandCalls(page, "cancel_agent_attachment")).toEqual([
    {command: "cancel_agent_attachment", args: {conversionId: firstCall.options.headers["x-attachment-conversion-id"]}},
  ]);
  await expect(page.getByRole("button", {name: "发送消息", exact: true})).toBeEnabled();
  expect((await commandCalls(page, "convert_agent_attachment")).map((call) => call.options.headers["x-attachment-name"]))
    .toEqual(["first.html", "second.html"]);
});

test("转换未完成时切换会话会发送取消，迟到结果不会进入新会话", async ({page}) => {
  await openAgent(page, [{kind: "defer", ignoreCancel: true}]);
  const fileInput = page.locator('input[type="file"]');
  await fileInput.setInputFiles({name: "late.html", mimeType: "text/html", buffer: toBuffer("late result")});
  await page.waitForFunction(() => (window as any).__AMM_ATTACHMENT_TEST__?.settled.length === 0
    && (window as any).__AMM_TEST__.calls.some((call: any) => call.command === "convert_agent_attachment"));
  await expect(page.getByRole("button", {name: "发送消息", exact: true})).toBeDisabled();
  expect(await commandCalls(page, "start_turn")).toEqual([]);

  await page.getByRole("button", {name: "新建对话", exact: true}).click();
  await expect(page.locator(".chat-header h1")).toHaveText("Agent 对话");
  const conversionId = (await commandCalls(page, "begin_agent_attachment"))[0].args.conversionId;
  await expect.poll(() => commandCalls(page, "cancel_agent_attachment")).toEqual([
    {command: "cancel_agent_attachment", args: {conversionId}},
  ]);
  await page.evaluate(() => (window as any).__AMM_ATTACHMENT_TEST__.resolve(0, "# Late result"));
  await page.waitForFunction(() => (window as any).__AMM_ATTACHMENT_TEST__?.settled[0] === true);

  await expect(page.getByText("late.html", {exact: true})).toHaveCount(0);
  expect(await commandCalls(page, "start_turn")).toEqual([]);
});

test("注册尚未完成时移除附件，只取消其预留，不再上传", async ({page}) => {
  await openAgent(page, [], undefined, true);
  await page.locator('input[type="file"]').setInputFiles({name: "reserved.html", mimeType: "text/html", buffer: toBuffer("reserved")});
  await expect.poll(async () => (await commandCalls(page, "begin_agent_attachment")).length).toBe(1);
  const conversionId = (await commandCalls(page, "begin_agent_attachment"))[0].args.conversionId;
  await page.getByRole("button", {name: "移除 reserved.html", exact: true}).click();
  await page.evaluate(() => (window as any).__AMM_ATTACHMENT_TEST__.finishBegin());
  await expect.poll(() => commandCalls(page, "cancel_agent_attachment")).toEqual([
    {command: "cancel_agent_attachment", args: {conversionId}},
  ]);
  expect(await commandCalls(page, "convert_agent_attachment")).toEqual([]);
});

test("离开 Agent 页面会取消在途附件转换", async ({page}) => {
  await openAgent(page, [{kind: "defer"}]);
  await page.locator('input[type="file"]').setInputFiles({name: "closing.html", mimeType: "text/html", buffer: toBuffer("closing")});
  await expect.poll(async () => (await commandCalls(page, "convert_agent_attachment")).length).toBe(1);
  const conversionId = (await commandCalls(page, "begin_agent_attachment"))[0].args.conversionId;
  await page.getByRole("button", {name: "设置", exact: true}).click();
  await expect.poll(() => commandCalls(page, "cancel_agent_attachment")).toEqual([
    {command: "cancel_agent_attachment", args: {conversionId}},
  ]);
  await page.waitForFunction(() => (window as any).__AMM_ATTACHMENT_TEST__.settled[0] === true);
});

test("添加文档不会授权启动，点击 MarkItDown 按钮后直接连接并自动重试多文件", async ({page}) => {
  const firstMarkdown = "# First document";
  const secondMarkdown = "# Second document";
  await openAgent(page, [
    {kind: "consent", message: "需要确认启动 MarkItDown MCP"},
    {kind: "consent", message: "需要确认启动 MarkItDown MCP"},
    {kind: "resolve", markdown: firstMarkdown},
    {kind: "resolve", markdown: secondMarkdown},
  ], {holdConnect: true});

  const composer = page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true});
  await composer.fill("请读取这两份文件");
  const fileInput = page.locator('input[type="file"]');
  await fileInput.setInputFiles([
    pngFile,
    {name: "first.html", mimeType: "text/html", buffer: toBuffer("first source")},
    {name: "second.html", mimeType: "text/html", buffer: toBuffer("second source")},
  ]);

  const launchButton = page.getByRole("button", {name: "启动 MarkItDown MCP", exact: true});
  await expect(launchButton).toBeVisible();
  await expect(page.getByRole("button", {name: "发送消息", exact: true})).toBeDisabled();
  await expect.poll(async () => (await commandCalls(page, "convert_agent_attachment")).length).toBe(2);

  expect(await commandCalls(page, "mcp_list")).toEqual([]);
  expect(await commandCalls(page, "mcp_update")).toEqual([]);
  expect(await commandCalls(page, "mcp_connect")).toEqual([]);

  await launchButton.click();
  await expect(launchButton).toBeDisabled();
  await expect(launchButton).toHaveAttribute("aria-busy", "true");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect.poll(async () => (await commandCalls(page, "mcp_connect")).length).toBe(1);
  expect(await commandCalls(page, "convert_agent_attachment")).toHaveLength(2);
  await page.evaluate(() => (window as any).__AMM_MARKITDOWN_TEST__.finishConnect());

  await expect.poll(async () => (await commandCalls(page, "convert_agent_attachment")).length).toBe(4);
  await expect(launchButton).toHaveCount(0);
  await expect(page.getByRole("button", {name: "发送消息", exact: true})).toBeEnabled();
  const enabledPreset = {...markitdownPreset, enabled: true};
  expect(await page.evaluate(() => (window as any).__AMM_TEST__.calls
    .filter((call: any) => ["mcp_update", "mcp_connect"].includes(call.command))
    .map((call: any) => ({command: call.command, args: call.args})))).toEqual([
    {command: "mcp_update", args: {config: enabledPreset}},
    {command: "mcp_connect", args: {expectedConfig: enabledPreset, acceptsUnsandboxedStdio: true}},
  ]);

  await page.getByRole("button", {name: "发送消息", exact: true}).click();
  await expect.poll(async () => (await commandCalls(page, "start_turn")).length).toBe(1);
  const request = (await commandCalls(page, "start_turn"))[0].args.request;
  expect(request.content).toBe("请读取这两份文件");
  expect(request.attachments).toEqual([
    {kind: "image", name: "screenshot.png", dataUrl: pngDataUrl},
    {kind: "document", name: "first.html", markdown: firstMarkdown},
    {kind: "document", name: "second.html", markdown: secondMarkdown},
  ]);
});

test("MarkItDown 启动期间切换会话，不会自动重试新会话中的附件", async ({page}) => {
  await openAgent(page, [
    {kind: "consent", message: "需要确认启动 MarkItDown MCP"},
    {kind: "consent", message: "需要确认启动 MarkItDown MCP"},
  ], {holdConnect: true});
  const fileInput = page.locator('input[type="file"]');
  const launchButton = page.getByRole("button", {name: "启动 MarkItDown MCP", exact: true});
  await fileInput.setInputFiles({name: "old.html", mimeType: "text/html", buffer: toBuffer("old")});
  await launchButton.click();
  await expect.poll(async () => (await commandCalls(page, "mcp_connect")).length).toBe(1);

  await page.getByRole("button", {name: "新建对话", exact: true}).click();
  await expect(page.getByText("old.html", {exact: true})).toHaveCount(0);
  await fileInput.setInputFiles({name: "new.html", mimeType: "text/html", buffer: toBuffer("new")});
  await expect(launchButton).toBeDisabled();
  await page.evaluate(() => (window as any).__AMM_MARKITDOWN_TEST__.finishConnect());
  await expect(launchButton).toBeEnabled();
  await expect(page.getByText("等待启动 MarkItDown MCP", {exact: true})).toBeVisible();
  expect(await commandCalls(page, "convert_agent_attachment")).toHaveLength(2);
});

test("MarkItDown 启动失败会在聊天中显示原因，可再次点击启动", async ({page}) => {
  const failure = "MarkItDown 连接失败：fixture";
  await openAgent(page, [
    {kind: "consent", message: "需要确认启动 MarkItDown MCP"},
    {kind: "resolve", markdown: "# Retried document"},
  ], {failConnect: failure});
  await page.locator('input[type="file"]').setInputFiles({name: "retry.html", mimeType: "text/html", buffer: toBuffer("retry source")});
  const launchButton = page.getByRole("button", {name: "启动 MarkItDown MCP", exact: true});
  await launchButton.click();
  await expect(page.getByRole("alert").filter({hasText: failure})).toBeVisible();
  await expect(launchButton).toBeEnabled();
  expect(await commandCalls(page, "convert_agent_attachment")).toHaveLength(1);

  await page.evaluate(() => { (window as any).__AMM_MARKITDOWN_TEST__.failConnect = undefined; });
  await launchButton.click();
  await expect(page.getByRole("button", {name: "发送消息", exact: true})).toBeEnabled();
  await expect(page.getByRole("alert").filter({hasText: failure})).toHaveCount(0);
  expect(await commandCalls(page, "mcp_connect")).toHaveLength(2);
  expect(await commandCalls(page, "convert_agent_attachment")).toHaveLength(2);
});

test("剪贴板图片添加为附件，普通文本粘贴仍写入草稿", async ({page, context}) => {
  await openAgent(page);
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const composer = page.getByRole("textbox", {name: "告诉 Agent 你想做什么", exact: true});
  await page.evaluate(async () => navigator.clipboard.writeText("来自剪贴板的文字"));
  await composer.focus();
  await page.keyboard.press("Control+V");
  await expect(composer).toHaveValue("来自剪贴板的文字");

  await page.evaluate(async () => {
    const canvas = document.createElement("canvas");
    canvas.width = 1;
    canvas.height = 1;
    const blob = await new Promise<Blob>((resolve, reject) => {
      canvas.toBlob((result) => result ? resolve(result) : reject(new Error("canvas PNG encoding failed")), "image/png");
    });
    await navigator.clipboard.write([new ClipboardItem({"image/png": blob})]);
  });
  await composer.focus();
  await page.keyboard.press("Control+V");

  await expect(composer).toHaveValue("来自剪贴板的文字");
  const pendingImage = page.getByLabel("待发送附件").locator("img").first();
  await expect(pendingImage).toBeVisible();
  expect(await pendingImage.getAttribute("src")).toMatch(/^data:image\/png;base64,/);
  await expect(page.getByRole("button", {name: "启动 MarkItDown MCP", exact: true})).toHaveCount(0);
  expect(await commandCalls(page, "convert_agent_attachment")).toEqual([]);
});
