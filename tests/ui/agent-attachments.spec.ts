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
  | {kind: "defer"};

async function installAttachmentMock(page: Page, outcomes: ConversionOutcome[] = []) {
  await installMockTauri(page);
  await page.addInitScript((conversionOutcomes) => {
    window.addEventListener("DOMContentLoaded", () => {
      const target = window as any;
      const fixture = target.__AMM_TEST__;
      const tauri = target.__TAURI_INTERNALS__;
      const originalInvoke = tauri.invoke;
      const resolvers = new Map<number, (markdown: string) => void>();
      const settled: boolean[] = [];
      let conversionIndex = 0;

      target.__AMM_ATTACHMENT_TEST__ = {
        settled,
        resolve(index: number, markdown: string) {
          resolvers.get(index)?.(markdown);
        },
      };

      tauri.invoke = async (command: string, args: any = {}, options?: Record<string, any>) => {
        if (command !== "convert_agent_attachment") return originalInvoke(command, args, options);

        const index = conversionIndex++;
        fixture.calls.push({command, args, options});
        const outcome = conversionOutcomes[index] ?? {kind: "resolve", markdown: "# Converted attachment"};
        try {
          if (outcome.kind === "reject") throw new Error(outcome.message);
          if (outcome.kind === "consent") throw {code: "consent_required", message: outcome.message};
          if (outcome.kind === "defer") {
            return await new Promise<string>((resolve) => resolvers.set(index, resolve));
          }
          return outcome.markdown;
        } finally {
          settled[index] = true;
        }
      };
    }, {once: true});
  }, outcomes);
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

async function openAgent(page: Page, outcomes: ConversionOutcome[] = [], markitdown?: MarkitdownMockOptions) {
  await installAttachmentMock(page, outcomes);
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
  const document = {name: "manual.txt", mimeType: "text/plain", buffer: toBuffer("source document")};
  await fileInput.setInputFiles([pngFile, document]);

  await page.waitForFunction(() => (window as any).__AMM_ATTACHMENT_TEST__?.settled[0] === true);
  await expect(page.getByLabel("待发送附件")).toBeVisible();
  await expect(page.getByRole("button", {name: "发送消息", exact: true})).toBeDisabled();
  expect(await commandCalls(page, "start_turn")).toEqual([]);

  await page.getByRole("button", {name: "重试 manual.txt", exact: true}).click();
  await page.waitForFunction(() => (window as any).__AMM_ATTACHMENT_TEST__?.settled[1] === true);
  await expect(composer).toHaveValue("请结合图片阅读这份说明");
  await expect(page.getByText("screenshot.png", {exact: true})).toBeVisible();
  await expect(page.getByText("manual.txt", {exact: true})).toBeVisible();
  await expect(page.getByRole("button", {name: "发送消息", exact: true})).toBeEnabled();

  const conversions = await page.evaluate(() => (window as any).__AMM_TEST__.calls
    .filter((call: any) => call.command === "convert_agent_attachment")
    .map((call: any) => ({
      name: call.options.headers["x-attachment-name"],
      bytes: Array.from(new Uint8Array(call.args as ArrayBuffer)),
    })));
  expect(conversions).toEqual([
    {name: encodeURIComponent("manual.txt"), bytes: Array.from(new TextEncoder().encode("source document"))},
    {name: encodeURIComponent("manual.txt"), bytes: Array.from(new TextEncoder().encode("source document"))},
  ]);

  await page.getByRole("button", {name: "发送消息", exact: true}).click();
  await expect.poll(async () => (await commandCalls(page, "start_turn")).length).toBe(1);
  const request = (await commandCalls(page, "start_turn"))[0].args.request;
  expect(request.content).toBe("请结合图片阅读这份说明");
  expect(request.attachments).toEqual([
    {kind: "image", name: "screenshot.png", dataUrl: pngDataUrl},
    {kind: "document", name: "manual.txt", markdown},
  ]);
  await expect(page.getByLabel("消息附件")).toBeVisible();
  await expect(page.getByRole("button", {name: "预览 screenshot.png", exact: true})).toBeVisible();
  await expect(page.getByRole("button", {name: "预览 manual.txt", exact: true})).toBeVisible();
});

test("转换未完成时切换会话，迟到结果不会进入新会话", async ({page}) => {
  await openAgent(page, [{kind: "defer"}]);
  const fileInput = page.locator('input[type="file"]');
  await fileInput.setInputFiles({name: "late.md", mimeType: "text/markdown", buffer: toBuffer("late result")});
  await page.waitForFunction(() => (window as any).__AMM_ATTACHMENT_TEST__?.settled.length === 0
    && (window as any).__AMM_TEST__.calls.some((call: any) => call.command === "convert_agent_attachment"));
  await expect(page.getByRole("button", {name: "发送消息", exact: true})).toBeDisabled();
  expect(await commandCalls(page, "start_turn")).toEqual([]);

  await page.getByRole("button", {name: "新建对话", exact: true}).click();
  await expect(page.locator(".chat-header h1")).toHaveText("Agent 对话");
  await page.evaluate(() => (window as any).__AMM_ATTACHMENT_TEST__.resolve(0, "# Late result"));
  await page.waitForFunction(() => (window as any).__AMM_ATTACHMENT_TEST__?.settled[0] === true);

  await expect(page.getByText("late.md", {exact: true})).toHaveCount(0);
  expect(await commandCalls(page, "start_turn")).toEqual([]);
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
    {name: "first.md", mimeType: "text/markdown", buffer: toBuffer("first source")},
    {name: "second.md", mimeType: "text/markdown", buffer: toBuffer("second source")},
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
    {kind: "document", name: "first.md", markdown: firstMarkdown},
    {kind: "document", name: "second.md", markdown: secondMarkdown},
  ]);
});

test("MarkItDown 启动期间切换会话，不会自动重试新会话中的附件", async ({page}) => {
  await openAgent(page, [
    {kind: "consent", message: "需要确认启动 MarkItDown MCP"},
    {kind: "consent", message: "需要确认启动 MarkItDown MCP"},
  ], {holdConnect: true});
  const fileInput = page.locator('input[type="file"]');
  const launchButton = page.getByRole("button", {name: "启动 MarkItDown MCP", exact: true});
  await fileInput.setInputFiles({name: "old.md", mimeType: "text/markdown", buffer: toBuffer("old")});
  await launchButton.click();
  await expect.poll(async () => (await commandCalls(page, "mcp_connect")).length).toBe(1);

  await page.getByRole("button", {name: "新建对话", exact: true}).click();
  await expect(page.getByText("old.md", {exact: true})).toHaveCount(0);
  await fileInput.setInputFiles({name: "new.md", mimeType: "text/markdown", buffer: toBuffer("new")});
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
  await page.locator('input[type="file"]').setInputFiles({name: "retry.md", mimeType: "text/markdown", buffer: toBuffer("retry source")});
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
