'use strict';

const { STATUS_CODES } = require('node:http');
const { isAbsolute } = require('node:path');

const ENDPOINT = 'https://www.bing.com/search';
const MAX_REQUEST_BYTES = 8 * 1024;
const MAX_OUTPUT_BYTES = 32 * 1024;
const LAUNCH_TIMEOUT_MS = 2500;
const NAVIGATION_TIMEOUT_MS = 12000;
const RESULTS_TIMEOUT_MS = 3500;
const SEARCH_TIMEOUT_MS = 25000;
const decodeUtf8 = new TextDecoder('utf-8', { fatal: true });
const resultSelector = '#b_results > li.b_algo h2 a[href], #b_results > .b_no';
const documentPolicy = [
  { name: 'Content-Security-Policy', value: "default-src 'none'; style-src 'unsafe-inline' https://bing.com https://*.bing.com; base-uri 'none'; form-action 'none'" },
  { name: 'Connection-Allowlist', value: '("https://*.bing.com/rp/*" "https://*.bing.com/rs/*" "https://bing.com/rp/*" "https://bing.com/rs/*");redirects=block;webrtc=block' },
  { name: 'Referrer-Policy', value: 'no-referrer' },
  { name: 'X-DNS-Prefetch-Control', value: 'off' },
];
const challengeSelector = '#b_captcha, #b_captcha_container, #captcha, #challenge-form, #cf-challenge-running, input[name="captcha"], iframe[src*="/captcha" i]';

function empty(status = 'unavailable') {
  return { status, results: [], truncated: false };
}

async function launchBrowser(signal, executablePath) {
  const { chromium } = require('playwright-core');
  if (executablePath !== undefined && !isAbsolute(executablePath)) throw new Error('Browser path must be absolute');
  const candidates = executablePath ? [{ executablePath }] : [
    ...(process.platform === 'win32' ? [{ channel: 'msedge' }] : []),
    { channel: 'chrome' },
    ...(process.platform === 'darwin' ? [{ channel: 'msedge' }] : []),
    ...(process.platform === 'linux' ? ['/usr/bin/chromium', '/usr/bin/chromium-browser']
      .map(executablePath => ({ executablePath })) : []),
    { channel: 'chromium' },
    {}, // Playwright can also install just its Chromium headless shell.
  ];
  for (const candidate of candidates) {
    signal.throwIfAborted();
    try {
      return await chromium.launch({
        ...candidate,
        headless: true,
        timeout: LAUNCH_TIMEOUT_MS,
        chromiumSandbox: process.platform !== 'linux', // Linux may not allow user namespaces.
        handleSIGINT: false,
        handleSIGTERM: false,
        handleSIGHUP: false,
        // Leave Chromium's system proxy and DNS settings intact.
        // Hold back rel=prerender's browser-process fetch, which bypasses page CDP.
        args: ['--dns-prefetch-disable', '--enable-features=ConnectionAllowlists,NoStatePrefetchHoldback,SkipListAccountsWithoutGaiaCookies'],
      });
    } catch {
      // Playwright tears down failed launches. Do not try another after cancellation.
    }
  }
  throw new Error('No supported Chromium browser could be launched');
}

function isBingUrl(url) {
  return url?.protocol === 'https:' && !url.username && !url.password && !url.port &&
    /^(?:[a-z\d-]+\.)?bing\.com$/i.test(url.hostname);
}

async function restrictRequests(cdp, searchUrl) {
  const { frameTree } = await cdp.send('Page.getFrameTree');
  const mainFrame = frameTree.frame.id;
  const navigations = new Set();
  // Unlike Playwright route.continue(), CDP pauses *every* HTTP redirect hop.
  // An allowed URL can never implicitly grant permission to its Location target.
  cdp.on('Fetch.requestPaused', async event => {
    const { requestId, request, frameId, resourceType, redirectedRequestId } = event;
    if (event.responseStatusCode !== undefined) {
      const { responseStatusCode, responseStatusText, responseHeaders = [] } = event;
      const replaces = /^(?:connection-allowlist(?:-report-only)?|link|refresh|referrer-policy|x-dns-prefetch-control|report-to|reporting-endpoints|nel)$/i;
      // Retain the live body and status. Strip header resource hints and constrain
      // renderer connections (including speculative preconnect outside Fetch).
      const headers = responseHeaders.filter(({ name }) => !replaces.test(name));
      try {
        await cdp.send('Fetch.continueResponse', {
          requestId, responseCode: responseStatusCode,
          responsePhrase: responseStatusText || STATUS_CODES[responseStatusCode] || 'Response',
          responseHeaders: [...headers, ...documentPolicy],
        });
      } catch {
        await cdp.send('Fetch.failRequest', { requestId, errorReason: 'BlockedByClient' }).catch(() => {});
      }
      return;
    }
    const url = URL.parse(request.url);
    const purpose = Object.keys(request.headers).some(name => /^(?:sec-)?purpose$/i.test(name));
    let allow = false;
    if (frameId === mainFrame && request.method === 'GET' && !purpose && isBingUrl(url)) {
      if (resourceType === 'Document') {
        const initial = navigations.size === 0 && url.href === searchUrl.href;
        const regional = navigations.has(redirectedRequestId) && navigations.size < 5 &&
          url.pathname === '/search' && url.searchParams.getAll('q').length === 1 &&
          url.searchParams.get('q') === searchUrl.searchParams.get('q');
        allow = initial || regional;
        if (allow) navigations.add(requestId);
      } else if (resourceType === 'Stylesheet') {
        allow = /^\/(?:rp|rs)\//.test(url.pathname);
      }
    }
    await cdp.send(allow ? 'Fetch.continueRequest' : 'Fetch.failRequest',
      allow ? { requestId, interceptResponse: resourceType === 'Document' } : { requestId, errorReason: 'BlockedByClient' }).catch(() => {});
  });
  await cdp.send('Fetch.enable', { patterns: [{ requestStage: 'Request', urlPattern: '*' }] });
}

function resultUrl(raw, pageUrl) {
  if (!raw || raw.length > 8192 || /[\s\\\p{Cc}]/u.test(raw)) return null;
  const parsed = URL.parse(raw, pageUrl);
  if (!parsed) return null;
  let url;
  if (isBingUrl(parsed) && parsed.pathname === '/ck/a') {
    // Only decode the organic relay's a1/base64url target; never navigate to it.
    const targets = parsed.searchParams.getAll('u');
    const match = /^a1([A-Za-z0-9_-]+)={0,2}$/.exec(targets[0] ?? '');
    if (targets.length !== 1 || !match) return null;
    const bytes = Buffer.from(match[1], 'base64url');
    if (bytes.toString('base64url') !== match[1]) return null;
    try {
      const target = decodeUtf8.decode(bytes);
      if (!/^https?:\/\//i.test(target) || /[\s\\\p{Cc}]/u.test(target)) return null;
      url = URL.parse(target);
    } catch {
      return null;
    }
  } else if (/^(?:https?:)?\/\//i.test(raw)) {
    url = parsed;
  }
  if (!url || !['https:', 'http:'].includes(url.protocol) || url.username || url.password ||
      Buffer.byteLength(url.href) > 2048) return null;
  if (/(?:^|\.)bing\.com\.?$/i.test(url.hostname) &&
      /^\/(?:$|(?:ck|aclick|alink|link|search)(?:\/|$))/i.test(url.pathname)) return null;
  return url.href;
}

function boundedText(raw, max) {
  const text = raw.replace(/\s+/gu, ' ').replace(/[\p{Cc}\u200b\u200e\u200f\u202a-\u202e\u2066-\u2069]/gu, '').trim();
  if (Buffer.byteLength(text) <= max) return [text, false];
  let size = 0;
  let output = '';
  for (const char of text) {
    size += Buffer.byteLength(char);
    if (size > max) break;
    output += char;
  }
  return [output.trimEnd(), true];
}

async function readResults(page, count) {
  const observation = await page.evaluate(({ challengeSelector }) => {
    if (document.querySelector(challengeSelector) ||
        /^(?:one last step|just a moment|verify (?:that )?you are human|access denied|service unavailable)(?:[.!…\s]|$)/i.test(document.title.trim())) return null;
    const visible = element => element?.checkVisibility({ visibilityProperty: true }) &&
      !element.closest('[hidden],[aria-hidden="true" i]');
    const text = element => visible(element) ? element.innerText.slice(0, 4096) : '';
    const list = document.querySelector('#b_results');
    if (!visible(list)) return null;
    const hits = [];
    for (const row of list.querySelectorAll(':scope > li.b_algo:not(.b_ad)')) {
      if (hits.length === 100) break;
      if (!visible(row)) continue;
      const link = row.querySelector('h2 a[href]');
      const snippet = row.querySelector('.b_caption p, .b_snippet, p[class*="b_lineclamp"]');
      hits.push({ href: link?.getAttribute('href')?.slice(0, 8193), title: text(link), snippet: text(snippet) });
    }
    return { hits, noResults: [...list.querySelectorAll(':scope > .b_no')].some(visible) };
  }, { challengeSelector });
  if (!observation) return empty();
  const output = empty('available');
  const seen = new Set();
  for (const hit of observation.hits) {
    const url = resultUrl(hit.href, page.url());
    const [title, titleCut] = boundedText(hit.title, 512);
    if (!url || !title) continue;
    const key = url.split('#', 1)[0];
    if (seen.has(key)) continue;
    if (output.results.length === count) {
      output.truncated = true;
      break;
    }
    seen.add(key);
    const [snippet, snippetCut] = boundedText(hit.snippet, 1536);
    output.truncated ||= titleCut || snippetCut;
    output.results.push({ title, url, snippet });
  }
  if (!output.results.length && (!observation.noResults || observation.hits.length)) return empty();
  while (Buffer.byteLength(JSON.stringify(output)) > MAX_OUTPUT_BYTES) {
    output.results.pop();
    output.truncated = true;
  }
  return output;
}

async function searchBing(input, {
  signal = new AbortController().signal,
  executablePath,
  launch = launchBrowser,
  newSession = (context, page) => context.newCDPSession(page),
} = {}) {
  // The caller enforces public-query policy. Here only check the wire shape and bounds.
  const query = input?.query;
  const count = input?.count;
  if (typeof query !== 'string' || !query.trim() || [...query].length > 512 ||
      !Number.isInteger(count) || count < 1 || count > 10 || signal.aborted) return empty();
  const work = AbortSignal.any([signal, AbortSignal.timeout(SEARCH_TIMEOUT_MS)]);
  const url = new URL(ENDPOINT);
  url.searchParams.set('q', query.trim());
  let browser;
  let closing;
  const closeBrowser = () => closing ??= browser.close().catch(() => {});
  try {
    try {
      browser = await launch(work, executablePath);
    } catch {
      return empty(work.aborted ? 'unavailable' : 'needs_configuration');
    }
    work.addEventListener('abort', closeBrowser, { once: true });
    work.throwIfAborted();
    const context = await browser.newContext({
      javaScriptEnabled: false,
      acceptDownloads: false,
      serviceWorkers: 'block',
    });
    const page = await context.newPage();
    await restrictRequests(await newSession(context, page), url);
    const response = await page.goto(url.href, { waitUntil: 'domcontentloaded', timeout: NAVIGATION_TIMEOUT_MS });
    if (response?.status() !== 200 || !/^text\/html\b/i.test(response.headers()['content-type'] ?? '') ||
        !isBingUrl(URL.parse(page.url()))) return empty();
    await page.locator(`${resultSelector}, ${challengeSelector}`).first()
      .waitFor({ state: 'attached', timeout: RESULTS_TIMEOUT_MS }).catch(() => {});
    return await readResults(page, count);
  } catch {
    return empty();
  } finally {
    work.removeEventListener('abort', closeBrowser);
    if (browser) await closeBrowser();
  }
}

// The caller writes a UTF-8 JSON line and keeps the pipe open until completion.
// EOF cancels processing, including when Playwright is still starting up.
async function runCli(search = searchBing) {
  const controller = new AbortController();
  const { stdin, stdout } = process;
  let cancel;
  const request = new Promise(resolve => {
    cancel = () => {
      controller.abort();
      resolve(null);
    };
    let pending = Buffer.alloc(0);
    let gotLine = false;
    const nonWhitespace = bytes => bytes.some(byte => ![9, 10, 13, 32].includes(byte));
    stdin.on('data', chunk => {
      if (gotLine) {
        if (nonWhitespace(chunk)) cancel();
        return;
      }
      const newline = chunk.indexOf(10);
      const line = newline < 0 ? chunk : chunk.subarray(0, newline);
      if (pending.length + line.length > MAX_REQUEST_BYTES) return cancel();
      pending = Buffer.concat([pending, line]);
      if (newline < 0) return;
      gotLine = true;
      if (nonWhitespace(chunk.subarray(newline + 1))) return cancel();
      try {
        resolve(JSON.parse(decodeUtf8.decode(pending)));
      } catch {
        cancel();
      }
    });
    for (const event of ['end', 'error', 'close']) stdin.once(event, cancel);
    for (const name of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.once(name, cancel);
    stdout.once('error', cancel);
  });
  let output;
  try {
    const input = await request;
    if (!controller.signal.aborted) output = await search(input, { signal: controller.signal });
  } catch {
    // Failure details, query and browser logs must not appear in the JSON stream.
  } finally {
    for (const name of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.removeListener(name, cancel);
    stdin.destroy();
  }
  const line = `${JSON.stringify(!controller.signal.aborted && output ? output : empty())}\n`;
  // On Windows a pending pipe read may keep the event loop alive after destroy().
  // Playwright is already closed; exit only after flushing the complete reply.
  await new Promise(resolve => stdout.write(line, resolve));
  process.exit(0);
}

if (require.main === module) {
  const args = process.argv.slice(2);
  void runCli((input, options) => {
    if (!args.length) return searchBing(input, options);
    if (args.length === 2 && args[0] === '--executable-path') {
      return searchBing(input, { ...options, executablePath: args[1] });
    }
    return empty('needs_configuration');
  });
}
module.exports = { launchBrowser, runCli, searchBing };
