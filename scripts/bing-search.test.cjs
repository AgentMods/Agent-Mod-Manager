'use strict';

const assert = require('node:assert/strict');
const { spawn } = require('node:child_process');
const { once } = require('node:events');
const { createServer } = require('node:http');
const { test } = require('node:test');
const { searchBing } = require('./bing-search.cjs');

const query = 'Fabric 中文 & site:fabricmc.net # docs';
const search = new URL('https://www.bing.com/search');
search.searchParams.set('q', query);
const input = { query, count: 2 };
const empty = status => ({ status, results: [], truncated: false });
const card = (url, title, snippet = '') => `<li class="b_algo"><h2><a href="${url}">${title}</a></h2><div class="b_caption"><p>${snippet}</p></div></li>`;
const list = (...rows) => `<ol id="b_results">${rows.join('')}</ol>`;
const relay = target => `/ck/a?u=a1${Buffer.from(target).toString('base64url')}`;
const first = 'https://fabricmc.net/use/installer/';

// A real Chromium page and the real request guard are used throughout. Only
// the test's CDP session supplies synthetic responses to permitted requests.
async function fixtureSession(context, page, respond, events = []) {
  const cdp = await context.newCDPSession(page);
  const requests = new Map();
  const fixtures = new Map();
  let handlePause;
  cdp.on('Fetch.requestPaused', event => requests.set(event.requestId, event));
  const fulfill = (requestId, status, headers, body) => cdp.send('Fetch.fulfillRequest', {
    requestId, responseCode: status, responseHeaders: headers,
    body: Buffer.from(body).toString('base64'),
  });
  return {
    on(name, handler) {
      if (name !== 'Fetch.requestPaused') throw new Error('Unexpected CDP event');
      handlePause = handler;
      cdp.on(name, handler);
    },
    async send(method, options) {
      if (method === 'Fetch.continueResponse') {
        return fulfill(options.requestId, options.responseCode, options.responseHeaders, fixtures.get(options.requestId));
      }
      if (!['Fetch.continueRequest', 'Fetch.failRequest'].includes(method)) return cdp.send(method, options);
      const { requestId } = options;
      const original = requests.get(requestId);
      const url = original.request.url;
      const allowed = method === 'Fetch.continueRequest';
      events.push({ url, allowed });
      if (!allowed) return cdp.send(method, options);
      const fixture = await respond(url);
      if (!fixture) return cdp.send('Fetch.failRequest', { requestId, errorReason: 'BlockedByClient' });
      const { status = 200, body = '', headers = {} } = fixture;
      const responseHeaders = Object.entries({ 'Content-Type': 'text/html; charset=utf-8', ...headers })
        .map(([name, value]) => ({ name, value }));
      if (!options.interceptResponse) return fulfill(requestId, status, responseHeaders, body);
      // A synthetic CDP response skips the network response event. Feed the same
      // pause to the production handler, then fulfill with its final headers.
      fixtures.set(requestId, body);
      await handlePause({ ...original, responseStatusCode: status, responseHeaders });
    },
  };
}

function fixtureSearch(respond, events = [], options = {}) {
  return searchBing(input, {
    ...options,
    newSession: (context, page) => fixtureSession(context, page, respond, events),
  });
}

test('isolated Chromium reads visible organic results in Bing order; no result or prefetch traffic', { timeout: 30000 }, async t => {
  const requests = [];
  const canary = createServer((request, response) => { requests.push(request.url); response.end(); });
  let connections = 0;
  canary.on('connection', () => { ++connections; });
  canary.listen(0, '127.0.0.1');
  await once(canary, 'listening');
  t.after(() => new Promise(resolve => canary.close(resolve)));
  const target = `http://127.0.0.1:${canary.address().port}`;
  const css = 'https://r.bing.com/rp/search.css';
  const events = [];
  const body = `<!doctype html><meta charset="utf-8"><title>Bing</title>
    <base href="${target}/">
    <link rel="stylesheet" href="${css}">
    <link rel="preconnect" href="${target}"><link rel="dns-prefetch" href="//outside.example.net">
    <link rel="prefetch" href="${target}/prefetch"><link rel="prerender" href="${target}/prerender">
    <link rel="preload" as="fetch" href="${target}/preload" crossorigin>
    <iframe src="${target}/frame"></iframe><img src="${target}/image">
    <script>fetch('${target}/script')</script>
    ${list(
      '<li class="b_algo b_ad"><h2><a href="https://bad.example.net/ad">Ad</a></h2></li>',
      card('https://bad.example.net/hidden', '<span class="secret">CSS-hidden result</span>'),
      card(relay('javascript:alert(1)'), 'Invalid relay'),
      card('/ck/a?u=a1abc&amp;u=a1def', 'Ambiguous relay'),
      card('https://name:secret@fabricmc.net/', 'Credential URL'),
      card(relay(first), 'Fabric &amp;  Loader<span hidden> noise</span>', 'Official&nbsp; <em>installer</em><br> for Minecraft.'),
      card(`${first}#duplicate`, 'Duplicate'),
      card('//docs.fabricmc.net/', 'Fabric 文档', `你好 ${'😀'.repeat(500)}`),
      card('https://modrinth.com/', 'Lower ranked'),
    )}`;
  const result = await fixtureSearch(url => {
    if (url === search.href) return { body, headers: { Link: `<${target}/header>; rel=preconnect` } };
    if (url === css) return { headers: { 'Content-Type': 'text/css' }, body: `.secret { display:none } body { background-image: url('${target}/css') }` };
  }, events);
  assert.equal(result.status, 'available');
  assert.equal(result.truncated, true);
  assert.deepEqual(result.results[0], {
    title: 'Fabric & Loader', url: first, snippet: 'Official installer for Minecraft.',
  });
  assert.equal(result.results[1].url, 'https://docs.fabricmc.net/');
  assert.equal(result.results[1].title, 'Fabric 文档');
  assert.match(result.results[1].snippet, /^你好 😀+$/u);
  assert(Buffer.byteLength(result.results[1].snippet) <= 1536);
  assert.deepEqual(events.filter(event => event.allowed).map(event => event.url), [search.href, css]);
  assert.deepEqual(requests, [], 'no result site or speculative HTTP fetch reached the canary');
  assert.equal(connections, 0, 'no speculative TCP connection reached the canary');
});

test('permits a Bing regional redirect only when the search and query remain the same', { timeout: 30000 }, async () => {
  const regional = new URL(search.href);
  regional.hostname = 'cn.bing.com';
  regional.searchParams.set('setlang', 'zh-CN');
  const events = [];
  const result = await fixtureSearch(url => {
    if (url === search.href) return { status: 302, headers: { Location: regional.href } };
    if (url === regional.href) return { body: list(card(relay(first), 'Fabric')) };
  }, events);
  assert.deepEqual(result, { status: 'available', truncated: false, results: [{ title: 'Fabric', url: first, snippet: '' }] });
  assert.deepEqual(events.filter(event => event.allowed).map(event => event.url), [search.href, regional.href]);
});

for (const [label, redirect] of [
  ['off-domain', 'https://www.bing.com.example.net/search?q=Fabric'],
  ['on-domain tracking', 'https://www.bing.com/ck/a?u=a1abc'],
  ['on-domain query replacement', 'https://cn.bing.com/search?q=different'],
]) {
  test(`fails closed on ${label} redirect`, { timeout: 30000 }, async () => {
    const events = [];
    const result = await fixtureSearch(url => {
      if (url === search.href) return { status: 302, headers: { Location: redirect } };
    }, events);
    assert.deepEqual(result, empty('unavailable'));
    assert.deepEqual(events, [{ url: search.href, allowed: true }, { url: redirect, allowed: false }]);
  });
}

for (const [label, fixture, expected] of [
  ['explicit empty page', { body: list('<li class="b_no">No results found.</li>') }, 'available'],
  ['captcha precedence', { body: `<div id="b_captcha"></div>${list(card(first, 'Stale result'))}` }, 'unavailable'],
  ['unknown layout', { body: list('<li>Unrecognized layout</li>') }, 'unavailable'],
  ['rate-limited response', { status: 429, body: list(card(first, 'Not a result')) }, 'unavailable'],
]) {
  test(`recognizes ${label}`, { timeout: 30000 }, async () => {
    const result = await fixtureSearch(url => url === search.href ? fixture : undefined);
    assert.deepEqual(result, empty(expected));
  });
}

function worker(t, args) {
  const child = spawn(process.execPath, args, { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
  const closed = once(child, 'close');
  // If an assertion fails before the exchange completes, ask the helper to close normally.
  t.after(() => child.stdin.destroy());
  child.stdout.setEncoding('utf8');
  child.stderr.setEncoding('utf8');
  let stdout = '';
  let stderr = '';
  child.stdout.on('data', data => { stdout += data; });
  child.stderr.on('data', data => { stderr += data; });
  return {
    child,
    async output() {
      assert.deepEqual(await closed, [0, null], stderr);
      assert.match(stdout, /^[^\n]+\n$/);
      return { json: JSON.parse(stdout), stderr };
    },
  };
}

test('CLI keeps stdin open during work, writes one JSON line, and exits', { timeout: 30000 }, async t => {
  const { child, output } = worker(t, [require.resolve('./bing-search.cjs'), '--executable-path', 'not-absolute']);
  const line = Buffer.from(`${JSON.stringify(input)}\n`);
  const split = line.indexOf(Buffer.from('中')) + 1;
  child.stdin.write(line.subarray(0, split));
  child.stdin.write(line.subarray(split));
  const result = await output();
  assert.deepEqual(result, { json: empty('needs_configuration'), stderr: '' });
});

test('CLI EOF cancels a live, paused navigation and disconnects the browser before exit', { timeout: 30000 }, async t => {
  const code = `
    const { launchBrowser, runCli, searchBing } = require(${JSON.stringify(require.resolve('./bing-search.cjs'))});
    const fixtureSession = ${fixtureSession.toString()};
    runCli((request, options) => searchBing(request, {
      ...options,
      launch: async (...args) => {
        const browser = await launchBrowser(...args);
        browser.on('disconnected', () => process.stderr.write('browser-disconnected\\n'));
        return browser;
      },
      newSession: (context, page) => fixtureSession(context, page, () => {
        process.stderr.write('navigation-paused\\n');
        return new Promise(() => {});
      }),
    }));`;
  const { child, output } = worker(t, ['-e', code]);
  let navigationPaused = false;
  let diagnostic = '';
  child.stderr.on('data', data => {
    diagnostic += data;
    if (!navigationPaused && diagnostic.includes('navigation-paused\n')) {
      navigationPaused = true;
      child.stdin.end();
    }
  });
  child.stdin.write(`${JSON.stringify(input)}\n`);
  const result = await output();
  assert.deepEqual(result.json, empty('unavailable'));
  assert.equal(result.stderr, 'navigation-paused\nbrowser-disconnected\n');
});
