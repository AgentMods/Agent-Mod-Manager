// Validate UI translations against the strings used by the application and each other.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const root = fileURLToPath(new URL('../', import.meta.url));
const catalogDir = path.join(root, 'src/locales/translations');
const locales = ['en', 'de', 'fr', 'es', 'it', 'ja', 'ko', 'pt-BR', 'ru'];
const tokens = text => [...new Set(Array.from(text.matchAll(/\{(\w+)\}/g), match => match[1]))].sort();
// Native detail translations are exact, category-scoped literals. The catalog
// is mandatory for production builds; Vite's glob alone allows zero matches.
const detailPath = path.join(root, 'src-tauri/locales/error-details.json');
assert.ok(fs.existsSync(detailPath), 'Missing required catalog: src-tauri/locales/error-details.json');
const detailCatalog = JSON.parse(fs.readFileSync(detailPath, 'utf8'));
const nativeCatalog = JSON.parse(fs.readFileSync(path.join(root, 'src-tauri/locales/native.json'), 'utf8'));
const isRecord = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const externalCategories = new Set(['error.db', 'error.io', 'error.net', 'error.json']);
assert.ok(isRecord(detailCatalog) && Object.keys(detailCatalog).length, 'Native detail catalog must not be empty');
let detailCount = 0;
for (const [category, entries] of Object.entries(detailCatalog)) {
  const nativeSource = nativeCatalog[category]?.['zh-CN'];
  const wrapped = category.startsWith('error.') && !externalCategories.has(category) &&
    typeof nativeSource === 'string' && nativeSource.includes('{detail}');
  assert.ok(category === 'error.raw' || wrapped, `Invalid native detail category: ${category}`);
  assert.ok(isRecord(entries) && Object.keys(entries).length, `Empty native detail category: ${category}`);
  for (const [source, translations] of Object.entries(entries)) {
    assert.ok(source.trim(), `Empty source text in ${category}`);
    assert.ok(isRecord(translations), `Missing translation object for ${category}: ${source}`);
    assert.deepEqual(Object.keys(translations).sort(), [...locales].sort(),
      `Incorrect locale set for ${category}: ${source}`);
    for (const [locale, value] of Object.entries(translations)) {
      assert.ok(typeof value === 'string' && value.trim(), `Empty ${locale} translation for ${category}: ${source}`);
      assert.deepEqual(tokens(value), tokens(source), `Literal token mismatch (${locale}) for ${category}: ${source}`);
    }
    detailCount++;
  }
}
console.log(`Validated ${detailCount} native error details in ${locales.length} translated languages.`);

const required = new Set();
const sourceDir = path.join(root, 'src');
for (const name of fs.readdirSync(sourceDir, { recursive: true }).filter(name => /\.tsx?$/.test(name))) {
  const source = fs.readFileSync(path.join(sourceDir, name), 'utf8');
  const file = ts.createSourceFile(name, source, ts.ScriptTarget.Latest, true);
  function visit(node) {
    if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === 'tr') {
      const [message] = node.arguments;
      if (message && ts.isStringLiteralLike(message)) required.add(message.text);
    }
    ts.forEachChild(node, visit);
  }
  visit(file);
}
let baseline;
for (const locale of locales) {
  const catalog = JSON.parse(fs.readFileSync(path.join(catalogDir, `${locale}.json`), 'utf8'));
  for (const key of required) assert.ok(Object.hasOwn(catalog, key), `Missing ${locale} translation: ${key}`);
  const keys = Object.keys(catalog).sort();
  if (baseline) assert.deepEqual(keys, baseline, `Different message keys in ${locale}`);
  baseline ??= keys;
  for (const [source, translated] of Object.entries(catalog)) {
    assert.ok(typeof translated === 'string' && translated.trim(), `Empty ${locale} translation: ${source}`);
    assert.deepEqual(tokens(translated), tokens(source), `Placeholder mismatch (${locale}): ${source}`);
  }
}
console.log(`Validated ${baseline.length} messages in all ${locales.length + 1} UI languages, including the Chinese source.`);
