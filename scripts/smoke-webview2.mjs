import assert from 'node:assert/strict';
import { execFile, spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp } from 'node:fs/promises';
import { createServer } from 'node:net';
import { isAbsolute, join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const loopback = '127.0.0.1';
const packagedUrl = /^https?:\/\/tauri\.localhost(?:[/?#]|$)/;
const timeout = 20_000;
const archivePicker = fileURLToPath(new URL('./select-windows-smoke-archive.ps1', import.meta.url));

async function loopbackPort() {
  const listener = createServer();
  const ready = once(listener, 'listening');
  listener.listen(0, loopback);
  await ready;
  const { port } = listener.address();
  await new Promise((resolve, reject) => listener.close(error => error ? reject(error) : resolve()));
  return port;
}

class WebView2Session {
  constructor(page, close, chooseArchive) {
    this.page = page;
    this.close = close;
    this.chooseArchive = chooseArchive;
  }

  url() {
    return this.page.url();
  }

  async visible(xpath) {
    const first = this.page.locator(`xpath=${xpath}`).filter({ visible: true }).first();
    await first.waitFor({ state: 'visible', timeout });
    return first;
  }

  count(xpath) {
    return this.page.locator(`xpath=${xpath}`).count();
  }

  async click(xpath) {
    await (await this.visible(xpath)).click({ timeout });
  }

  async fill(xpath, text) {
    const input = await this.visible(xpath);
    await input.fill(text, { timeout });
    assert.equal(await input.inputValue({ timeout }), text);
  }

  async inputValue(xpath) {
    return (await this.visible(xpath)).inputValue({ timeout });
  }

  async enabled(xpath) {
    return (await this.visible(xpath)).isEnabled({ timeout });
  }

  async screenshot(path) {
    await this.page.screenshot({ path, type: 'png', timeout });
    console.log(`Screenshot: ${path}`);
  }

  async alerts() {
    return (await this.page.getByRole('alert').allTextContents()).slice(0, 4);
  }
}

async function waitForBrowser(app, port) {
  const deadline = performance.now() + 45_000;
  while (performance.now() < deadline) {
    assert.equal(app.exitCode, null, `Packaged app exited during WebView2 startup: ${app.exitCode}`);
    assert.equal(app.signalCode, null, 'Packaged app terminated during WebView2 startup');
    const response = await fetch(`http://${loopback}:${port}/json/version`, {
      signal: AbortSignal.timeout(1000), redirect: 'error',
    }).catch(() => undefined);
    if (response?.ok) {
      const { webSocketDebuggerUrl } = await response.json();
      const url = new URL(webSocketDebuggerUrl);
      assert.ok(url.protocol === 'ws:' && url.hostname === loopback && url.port === String(port) &&
        url.pathname.startsWith('/devtools/browser/') && !url.username && !url.password,
      'Expected a browser debugging endpoint on the selected loopback port');
      return url.href;
    }
    await delay(150);
  }
  throw new Error('Timed out waiting for the installed app to start WebView2');
}

export async function startWebView2(application, scratch) {
  assert.equal(process.platform, 'win32');
  const shell = process.env.AMM_WINDOWS_SMOKE_POWERSHELL;
  assert.ok(shell && isAbsolute(shell), 'The installer smoke must supply the absolute PowerShell path');
  const { chromium } = await import('@playwright/test');
  const port = await loopbackPort();
  // Set test-only flags on this child. Do not enable debugging in shipped settings.
  const env = Object.fromEntries(Object.entries(process.env).filter(([name]) =>
    !/^(AMM_|WEBVIEW2_)/i.test(name)));
  // Keep unrelated AMM_ variables isolated while explicitly disabling analytics in the app.
  env.AMM_DISABLE_ANALYTICS = '1';
  env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-address=${loopback} --remote-debugging-port=${port}`;
  env.WEBVIEW2_USER_DATA_FOLDER = await mkdtemp(join(scratch, 'webview2-'));
  const app = spawn(application, [], { cwd: scratch, env, stdio: ['ignore', 'inherit', 'inherit'] });
  const exited = new Promise(resolve => {
    app.once('error', resolve);
    app.once('exit', resolve);
  });
  let browser;
  async function chooseArchive(path) {
    assert.ok(app.pid && app.exitCode === null && app.signalCode === null, 'The fixture app is no longer running');
    await exec(shell, ['-NoLogo', '-NoProfile', '-NonInteractive', '-File', archivePicker,
      '-AppPid', String(app.pid), '-ArchivePath', path], { timeout: 25_000, windowsHide: true });
  }

  async function close() {
    try {
      await browser?.close();
    } finally {
      if (app.pid && app.exitCode === null && app.signalCode === null) {
        try {
          await exec(join(env.SystemRoot ?? 'C:\\Windows', 'System32', 'taskkill.exe'),
            ['/PID', String(app.pid), '/T', '/F'], { timeout: 8_000, windowsHide: true });
        } catch (error) {
          if (app.exitCode === null && app.signalCode === null) throw error;
        }
        await Promise.race([exited, delay(5_000, null, { ref: false })]);
        assert.ok(app.exitCode !== null || app.signalCode !== null, 'The fixture app did not exit');
      }
    }
  }
  try {
    await once(app, 'spawn');
    browser = await chromium.connectOverCDP(await waitForBrowser(app, port), { timeout });
    const deadline = performance.now() + timeout;
    while (performance.now() < deadline) {
      const page = browser.contexts().flatMap(context => context.pages()).find(page => packagedUrl.test(page.url()));
      if (page) return new WebView2Session(page, close, chooseArchive);
      await delay(150);
    }
    throw new Error('WebView2 did not load the packaged Tauri origin');
  } catch (error) {
    await close().catch(cleanup => console.error(`WebView2 cleanup failed: ${cleanup.message}`));
    throw error;
  }
}
