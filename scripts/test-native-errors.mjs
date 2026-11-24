import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { after, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { createServer } from 'vite';

const root = fileURLToPath(new URL('../', import.meta.url));
const readCatalog = name => JSON.parse(readFileSync(new URL(`../src-tauri/locales/${name}.json`, import.meta.url), 'utf8'));
const nativeMessages = readCatalog('native');
const vite = await createServer({
  root,
  configFile: false,
  appType: 'custom',
  optimizeDeps: { noDiscovery: true },
  server: { middlewareMode: true, watch: null, ws: false },
});
after(() => vite.close());
globalThis.document = { documentElement: { lang: '' } };
const { applyLanguageSettings, languages } = await vite.ssrLoadModule('/src/lib/i18n.ts');
const { formatNativeError } = await vite.ssrLoadModule('/src/lib/native-error.ts');
const useLanguage = locale => applyLanguageSettings({ locale, effective_locale: locale });
const wrap = (key, locale, detail) => key === 'error.raw' ? detail :
  nativeMessages[key][locale].replace('{detail}', () => detail);
const error = (key, detail) => ({
  key, args: key === 'error.raw' ? {} : { detail }, message: wrap(key, 'zh-CN', detail),
});

test('existing specific keys and structured Error causes still render', () => {
  useLanguage('en');
  const native = { key: 'locale.unsupported', args: {}, message: nativeMessages['locale.unsupported']['zh-CN'] };
  assert.equal(formatNativeError(new Error(native.message, { cause: native })), nativeMessages[native.key].en);
  assert.equal(formatNativeError({ key: 'error.invalid', message: 'readable fallback' }), 'readable fallback');
  assert.equal(formatNativeError({ key: '__proto__', message: 'readable fallback' }), 'readable fallback');
});

test('unmatched paths, vendor logs, and legacy strings remain verbatim', () => {
  useLanguage('en');
  const path = 'C:\\Mods\\资料\\{detail}\\$&\\archive.zip';
  const log = 'HTTP 429\r\nProvider: 账户无效 {platform} $&';
  assert.equal(formatNativeError(error('error.path_escape', path)), `Path is outside the allowed location: ${path}`);
  assert.equal(formatNativeError(error('error.raw', log)), log);
  assert.equal(formatNativeError(error('error.io', log)), wrap('error.io', 'en', log));
  assert.equal(formatNativeError(log), log);
  assert.equal(formatNativeError(error('error.raw', '__proto__')), '__proto__');
});

test('the production renderer translates all exact raw and wrapped detail entries in each locale', () => {
  const detailCatalog = readCatalog('error-details');
  assert.ok(Object.keys(detailCatalog).some(key => key !== 'error.raw'), 'Expected a wrapped category');
  assert.ok(Object.keys(detailCatalog['error.raw'] ?? {}).length, 'Expected raw detail entries');
  for (const locale of Object.keys(languages)) {
    useLanguage(locale);
    for (const [key, entries] of Object.entries(detailCatalog)) {
      for (const [source, translations] of Object.entries(entries)) {
        const expectedDetail = locale === 'zh-CN' ? source : translations[locale];
        assert.equal(typeof expectedDetail, 'string', `${locale} ${key}: ${source}`);
        assert.equal(formatNativeError(error(key, source)), wrap(key, locale, expectedDetail), `${locale} ${key}: ${source}`);
      }
    }
  }
});

test('no translation crosses category boundaries or matches only part of a detail', () => {
  useLanguage('en');
  const detailCatalog = readCatalog('error-details');
  for (const [key, entries] of Object.entries(detailCatalog)) {
    const source = Object.keys(entries)[0];
    assert.equal(formatNativeError(error('error.io', source)), wrap('error.io', 'en', source));
    for (const diagnostic of [` ${source}`, `${source} `, `Provider error: ${source}\nC:/Mods/{detail}`]) {
      assert.equal(formatNativeError(error(key, diagnostic)), wrap(key, 'en', diagnostic));
    }
    assert.equal(formatNativeError(source), source, 'Unstructured strings are not looked up');
  }
});
