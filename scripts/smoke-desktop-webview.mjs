import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants } from 'node:fs';
import { access, mkdir, mkdtemp, readFile, readlink, realpath, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, isAbsolute, join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { promisify } from 'node:util';
import * as zlib from 'node:zlib';
import {
  cancelDelta, cancelPrompt, conversationTitle, diagnosePrompt, diagnosisReply, firstDelta, installFilename,
  installPrompt, installReply, modelName, pathIdentity, startSmokeModel, trashPrompt, trashReply,
} from './smoke-agent-provider.mjs';

const endpoint = 'http://127.0.0.1:4444';
const platform = process.platform;
const windows = platform === 'win32';
const dataDirectoryName = 'com.glossmod.amm';
const exec = promisify(execFile);
const xdo = args => exec('xdotool', args, { timeout: 5_000 });
const elementKey = 'element-6066-11e4-a52e-4f735466cecf';
const game = 'Minecraft Java Edition';
const marker = 'amm-smoke-instance.esm';
const minecraft = '1.21.1';
const fabric = '0.16.14';
const note = 'smoke-note.txt';
const noteContents = 'amm-desktop-webview-fixture\n';
const mods = [
  { id: 'smoke_exact', disk: '1.2.3', logged: '1.2.3', line: 5 },
  { id: 'smoke_diff', disk: '2.0.0', logged: '1.9.0', line: 6 },
];

async function webdriver(method, route, body, timeout = 10_000) {
  const response = await fetch(endpoint + route, {
    method,
    headers: body === undefined ? undefined : { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(timeout),
    redirect: 'error',
  }).catch(cause => {
    throw new Error(`${method} ${endpoint}${route}: ${cause.cause?.code ?? cause.message}`, { cause });
  });
  const raw = await response.text();
  let reply;
  try {
    reply = JSON.parse(raw);
  } catch (cause) {
    throw new Error(`${method} ${route}: HTTP ${response.status}, expected WebDriver JSON: ${raw.slice(0, 250)}`, { cause });
  }
  if (!response.ok || reply?.value?.error) {
    const { error, message } = reply?.value ?? {};
    throw Object.assign(new Error(`${method} ${route}: ${error ?? response.status}: ${message ?? raw}`), { code: error });
  }
  assert.ok(reply && Object.hasOwn(reply, 'value'), `${method} ${route}: missing WebDriver value`);
  return reply.value;
}

class Session {
  constructor(id) {
    this.route = `/session/${encodeURIComponent(id)}`;
  }

  request(method, path, body) {
    return webdriver(method, this.route + path, body);
  }

  url() {
    return this.request('GET', '/url');
  }

  close() {
    return this.request('DELETE', '');
  }

  async elements(xpath) {
    const result = await this.request('POST', '/elements', { using: 'xpath', value: xpath });
    return result.map(entry => {
      assert.ok(typeof entry[elementKey] === 'string', 'Missing W3C element ID');
      return `/element/${encodeURIComponent(entry[elementKey])}`;
    });
  }

  async count(xpath) {
    return (await this.elements(xpath)).length;
  }

  async visible(xpath, enabled = false) {
    const deadline = performance.now() + 20_000;
    do {
      for (const element of await this.elements(xpath)) {
        try {
          if (await this.request('GET', `${element}/displayed`) &&
              (!enabled || await this.request('GET', `${element}/enabled`))) return element;
        } catch (error) {
          if (error.code !== 'stale element reference') throw error;
        }
      }
      await delay(120);
    } while (performance.now() < deadline);
    throw new Error(`Timed out waiting for ${enabled ? 'enabled ' : ''}visible UI: ${xpath}`);
  }

  async click(xpath) {
    const element = await this.visible(xpath, true);
    await this.request('POST', `${element}/click`, {});
  }

  async selectOption(xpath, value) {
    await this.click(`${xpath}/option[@value=${literal(value)}]`);
  }

  async fill(xpath, text) {
    const element = await this.visible(xpath, true);
    await this.request('POST', `${element}/clear`, {});
    await this.request('POST', `${element}/value`, { text });
    assert.equal(await this.request('GET', `${element}/property/value`), text);
  }

  async inputValue(xpath) {
    const element = await this.visible(xpath, true);
    return this.request('GET', `${element}/property/value`);
  }

  async enabled(xpath) {
    const element = await this.visible(xpath);
    return this.request('GET', `${element}/enabled`);
  }

  chooseArchive(path) {
    return chooseGtkArchive(path);
  }

  async screenshot(path) {
    const data = await this.request('GET', '/screenshot');
    assert.equal(typeof data, 'string', 'Expected a base64 WebDriver screenshot');
    const png = Buffer.from(data, 'base64');
    assert.equal(png.subarray(0, 8).toString('hex'), '89504e470d0a1a0a', 'Expected a PNG screenshot');
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, png);
    console.log(`Screenshot: ${path}`);
  }

  async alerts() {
    const alerts = (await this.elements('//*[@role="alert"]')).slice(0, 4);
    return Promise.all(alerts.map(element => this.request('GET', `${element}/text`)));
  }
}

// Older Node 22 releases do not have node:zlib's CRC-32 helper.
function crc32(bytes) {
  if (typeof zlib.crc32 === 'function') return zlib.crc32(bytes);
  let crc = -1;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
  }
  return (~crc) >>> 0;
}

function metadataJar(id, version) {
  // One stored ZIP member at the root. There are no classes or executable contents.
  const name = Buffer.from('fabric.mod.json');
  const data = Buffer.from(JSON.stringify({ schemaVersion: 1, id, version }));
  const local = Buffer.alloc(30);
  local.writeUInt32LE(0x04034b50, 0);
  local.writeUInt16LE(20, 4);
  local.writeUInt16LE(0x0021, 12); // DOS date: 1980-01-01
  local.writeUInt32LE(crc32(data), 14);
  local.writeUInt32LE(data.length, 18);
  local.writeUInt32LE(data.length, 22);
  local.writeUInt16LE(name.length, 26);

  const central = Buffer.alloc(46);
  central.writeUInt32LE(0x02014b50, 0);
  central.writeUInt16LE(20, 4);
  local.copy(central, 6, 4, 30);

  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(1, 8);
  end.writeUInt16LE(1, 10);
  end.writeUInt32LE(central.length + name.length, 12);
  end.writeUInt32LE(local.length + name.length + data.length, 16);
  return Buffer.concat([local, name, data, central, name, end]);
}

async function createFixture(root) {
  await mkdir(join(root, 'logs'));
  await mkdir(join(root, 'mods'));
  const log = [
    `[12:34:56] [main/INFO]: Loading Minecraft ${minecraft} with Fabric Loader ${fabric}`,
    '[12:34:57] [main/INFO]: Loading 4 mods:',
    `\t- minecraft ${minecraft}`,
    `\t- fabricloader ${fabric}`,
    ...mods.map(mod => `\t- ${mod.id} ${mod.logged}`),
    '',
  ].join('\n');
  await Promise.all([
    writeFile(join(root, 'options.txt'), 'fullscreen:false\n'),
    writeFile(join(root, marker), 'AMM smoke fixture\n'),
    writeFile(join(root, note), noteContents),
    writeFile(join(root, 'logs/latest.log'), log),
    ...mods.map(mod => writeFile(join(root, 'mods', `${mod.id}.jar`), metadataJar(mod.id, mod.disk))),
  ]);
}

function literal(text) {
  const parts = text.split("'").map(part => `'${part}'`);
  return parts.length === 1 ? parts[0] : `concat(${parts.join(`, "'", `)})`;
}

const exact = text => `normalize-space(.)=${literal(text)}`;
const pathIs = path => {
  if (!windows) return `.=${literal(path)}`;
  // Rust canonicalization can keep the Win32 extended-length prefix.
  const translated = `translate(., 'ABCDEFGHIJKLMNOPQRSTUVWXYZ\\', 'abcdefghijklmnopqrstuvwxyz/')`;
  const expected = pathIdentity(path);
  return `(${translated}=${literal(expected)} or ${translated}=${literal(`//?/${expected}`)})`;
};
const textAt = (scope, text, tag = '*') => `${scope}//${tag}[${exact(text)}]`;
const planPath = (scope, path) => `${scope}//tbody//td//span[${pathIs(path)}]`;
const main = '//main';
const button = (text, scope = main) => textAt(scope, text, 'button');
const tab = text => `${main}//button[@role="tab" and ${exact(text)}]`;
const labeledInput = text => `${main}//input[@id=${textAt(main, text, 'label')}/@for]`;
const modRow = `${main}//table[@aria-label="Mod 管理"]/tbody/tr[.//strong[${exact(installFilename)}]]`;
const gameTile = `${main}//button[.//*[@data-slot="card-title" and ${exact(game)}]]`;
const modal = title => `//*[@role="dialog" and .//*[@data-slot="dialog-title" and ${exact(title)}]]`;
const dialog = modal('确认文件变更');
const afterPlan = `${main}[not(${dialog})]`;
const nav = label => `//nav[@aria-label="主导航"]//button[${exact(label)}]`;
const iconButton = name => `//button[.//*[local-name()="svg" and contains(@class, ${literal(`lucide-${name}`)})]]`;
const composer = `${main}//textarea[@aria-label="告诉 Agent 你想做什么"]`;
const assistantAnswer = text => `${main}//article[contains(concat(' ', normalize-space(@class), ' '), ' assistant ')]//p[${exact(text)}]`;
const evidencePath = (path, suffix) => path.replace(/(?:\.png)?$/i, `.${suffix}.png`);

async function visibleText(ui, xpath) {
  const element = await ui.visible(xpath);
  return (windows ? await element.textContent() : await ui.request('GET', `${element}/text`)).trim();
}

async function startLinuxSession(application) {
  const { sessionId, capabilities } = await webdriver('POST', '/session', {
    capabilities: { alwaysMatch: { browserName: 'wry', 'tauri:options': { application } } },
  }, 60_000);
  assert.ok(typeof sessionId === 'string' && sessionId.length, 'Missing W3C session ID');
  const ui = new Session(sessionId);
  try {
    assert.equal(capabilities?.browserName, 'wry', 'Expected the native wry WebView');
    assert.equal(capabilities?.platformName, 'linux', 'Expected the Linux WebDriver platform');
    return ui;
  } catch (error) {
    await ui.close().catch(() => {});
    throw error;
  }
}

async function installExternalMinecraftAdapter(ui) {
  const fixtures = new URL('../src-tauri/tests/fixtures/game-adapters/minecraft-java/', import.meta.url);
  const adapter = JSON.parse(await readFile(new URL('adapter.json', fixtures), 'utf8'));
  // The smoke owns this marker and can therefore check unattended recognition.
  adapter.markers = [{ path: marker, kind: 'file', required: true, platforms: adapter.platforms }];
  delete adapter.coverUrl;
  const skill = await readFile(new URL('SKILL.md', fixtures), 'utf8');

  await ui.click(nav('插件中心'));
  await ui.visible(textAt(main, '暂无已安装适配包'));
  await ui.click(button('新建草稿'));
  const editor = modal('声明式适配包草稿');
  await ui.fill(`${editor}//input[@id="draft-slug"]`, adapter.slug);
  await ui.fill(`${editor}//textarea[@id="draft-adapter"]`, JSON.stringify(adapter));
  await ui.fill(`${editor}//textarea[@id="draft-skill"]`, skill.replaceAll('\r\n', '\n'));
  await ui.click(button('预览保存草稿', editor));
  await ui.click(button('批准并执行', dialog));

  const draftRow = `${main}//tr[.//strong[${exact(adapter.slug)}]]`;
  await ui.click(button('验证', draftRow));
  await ui.click(button('安装预览', modal(`验证：${adapter.slug}`)));
  await ui.click(button('批准并执行', dialog));
  await ui.click(`${afterPlan}//button[@role="tab" and ${exact('已安装')}]`);
  await ui.visible(`${main}//tr[.//strong[${exact(adapter.name)}]]`);
  await ui.click(nav('游戏库'));
}

async function registerAndOpen(ui, root) {
  const packagedUrl = windows ? /^https?:\/\/tauri\.localhost(?:[/?#]|$)/ : /^tauri:\/\/localhost(?:[/?#]|$)/;
  await ui.visible(`${main}//*[@data-testid='home-page']//h1`);
  // Package smoke always starts from a disposable app profile. Pin its persisted UI and
  // future Mod-organization language before using the existing Simplified Chinese flow.
  await ui.click(iconButton('settings'));
  const appLocale = '//*[@id="app-locale"]';
  await ui.selectOption(appLocale, 'zh-CN');
  // The native settings save completes before React applies the selected language.
  await ui.visible("/html[@lang='zh-CN']//*[@id='app-locale' and not(@disabled)]");
  assert.equal(await ui.inputValue(appLocale), 'zh-CN');
  await ui.click(nav('游戏库'));
  console.log('Pinned zh-CN through the language selector in the fresh smoke profile.');
  await installExternalMinecraftAdapter(ui);
  assert.match(await ui.url(), packagedUrl,
    'Expected assets from the packaged Tauri protocol, not a dev server');

  const scanDialog = modal('扫描游戏');
  await ui.click(button('扫描游戏'));
  // Opening the dialog starts a common-directory scan before we check the fixture.
  await ui.visible(`${scanDialog}//*[@role="status"]//p[starts-with(normalize-space(.), '已检查 ')]`);
  await ui.fill(`${scanDialog}//input[@aria-label="游戏目录"]`, root);
  await ui.click(button('检查单个游戏', scanDialog));
  // ScanGameCard uses an h3 and renders the path directly in PathText, not ItemTitle/ItemDescription.
  const candidate = `${scanDialog}//*[@data-slot="item" and .//h3[${exact(game)}] and .//span[${pathIs(root)}]]`;
  await ui.click(button('添加', candidate));

  // Registration updates the same result in place. Open details from the library card after closing the dialog.
  await ui.visible(button('管理中', candidate));
  await ui.click(button('关闭', scanDialog));
  await ui.click(`${gameTile}[not(${scanDialog})]`);
  await ui.visible(textAt(main, game, 'h1'));
}

async function diagnose(ui) {
  await ui.click(`${main}//button[@role="tab" and ${exact('诊断')}]`);
  await ui.click(button('开始诊断'));
  const startup = '//section[@aria-labelledby="fabric-evidence-heading"]';
  for (const text of [
    'Fabric 启动证据', '日志记录了 Mod 选择清单', 'logs/latest.log:1', '清单第 2 行', '日志声明 4 项',
    '这是日志中的启动记录，不代表当前运行状态或初始化成功。',
  ]) await ui.visible(textAt(startup, text));
  await ui.visible(`${startup}//dt[${exact('Minecraft')}]/following-sibling::dd[1][${exact(minecraft)}]`);
  await ui.visible(`${startup}//dt[${exact('Fabric Loader')}]/following-sibling::dd[1][${exact(fabric)}]`);
  await ui.click(textAt(startup, '启动时选中的 Mod（4）', 'summary'));
  const rows = `${startup}//table[@aria-label="Fabric Mod 选择清单"]/tbody[count(tr)=4]`;
  for (const [id, version, line] of [
    ['minecraft', minecraft, 3], ['fabricloader', fabric, 4], ...mods.map(mod => [mod.id, mod.logged, mod.line]),
  ]) {
    await ui.visible(`${rows}/tr[td[1][${exact(id)}] and td[2][${exact(version)}] and td[3][${exact(String(line))}]]`);
  }

  const disk = '//section[@aria-labelledby="fabric-disk-heading"]';
  await ui.visible(textAt(disk, 'Fabric 磁盘证据', 'h2'));
  await ui.visible(textAt(disk, 'mods/', 'p'));
  await ui.visible(textAt(disk, '磁盘 JAR 对照（2）', 'summary'));
  const jars = `${disk}//*[@role="list" and @aria-label="Fabric JAR 对照清单" and count(*[@role="listitem"])=2]`;
  for (const mod of mods) {
    const row = `${jars}/*[@role="listitem" and .//*[@data-slot="item-title" and ${exact(`${mod.id}.jar`)}]]`;
    const matches = mod.disk === mod.logged;
    await ui.visible(textAt(row, `${mod.id} · ${mod.disk}`, 'span'));
    await ui.visible(textAt(row, '无对应启用记录', 'span'));
    await ui.visible(textAt(row, matches ? 'ID 与版本一致' : '日志版本不同', 'span'));
    await ui.visible(textAt(row, `第 ${mod.line} 行${matches ? '' : ` · ${mod.logged}`}`, 'span'));
  }
}

async function moveAndRestore(ui, root) {
  const original = join(root, note);
  await ui.click(`${main}//button[@role="tab" and ${exact('文件')}]`);
  await ui.click(textAt(main, note, 'td'));
  await ui.visible(textAt(`${main}//aside`, noteContents.trim(), 'span'));
  await ui.click(button('移入回收站'));
  await ui.visible(planPath(dialog, original));
  assert.equal(await readFile(original, 'utf8'), noteContents, 'Trash preview must not move the file');
  await ui.click(button('批准并执行', dialog));
  await ui.visible(textAt(`${afterPlan}//aside`, '未选择文件', 'p'));
  await assert.rejects(access(original), { code: 'ENOENT' }, 'Trash commit must remove the original');
  await restoreNote(ui, root);
}

async function restoreNote(ui, root) {
  const original = join(root, note);
  await ui.click(nav('回收站'));
  const item = `//table[@aria-label="回收站文件"]/tbody/tr[.//span[${pathIs(original)}]]`;
  await ui.visible(textAt(item, '在回收站', 'span'));
  await ui.click(`${item}//*[@role="checkbox" and @aria-label="选择 ${note}"]`);
  await ui.click(button('恢复所选'));
  await ui.visible(planPath(dialog, original));
  await assert.rejects(access(original), { code: 'ENOENT' }, 'Restore preview must not recreate the file');
  await ui.click(button('批准并执行', dialog));
  await ui.visible(`${afterPlan}//*[@data-slot="empty-title" and ${exact('回收站为空')}]`);
  assert.equal(await readFile(original, 'utf8'), noteContents, 'Restore must preserve the original bytes');
}

async function send(ui, prompt) {
  await ui.fill(composer, prompt);
  await ui.click(`${main}//button[@aria-label="发送消息"]`);
}

async function chooseGtkArchive(path) {
  const visibleChoosers = async () => {
    try {
      const { stdout } = await xdo(['search', '--onlyvisible', '--name', '^选择本地 Mod 归档$']);
      const matches = stdout.trim().split(/\s+/);
      assert.ok(matches.length === 1 && /^\d+$/.test(matches[0]), 'Expected one visible archive chooser');
      return matches;
    } catch (error) {
      if (error.code !== 1) throw error;
      return [];
    }
  };
  const deadline = performance.now() + 10_000;
  let window;
  while (!window && performance.now() < deadline) {
    [window] = await visibleChoosers();
    if (!window) await delay(100);
  }
  assert.ok(window, 'Timed out waiting for the native GTK file chooser');
  await xdo(['windowfocus', '--sync', window]);
  await xdo(['key', '--clearmodifiers', 'ctrl+l']);
  await delay(150);
  await xdo(['type', '--clearmodifiers', '--', path]);
  // GTK enables Open asynchronously once it validates the typed path.
  for (let attempt = 0; attempt < 5; attempt++) {
    await delay(300);
    if (!(await visibleChoosers()).includes(window)) return;
    const { stdout } = await xdo(['getwindowfocus']);
    assert.equal(stdout.trim(), window, 'The archive chooser lost keyboard focus');
    await xdo(['key', '--clearmodifiers', 'Return']);
  }
  assert.ok(!(await visibleChoosers()).includes(window), 'The native chooser did not accept the fixture');
}

async function fixtureProcessAlive(pid, dataHome) {
  try {
    const processStatus = await readFile(`/proc/${pid}/status`, 'utf8');
    if (/^State:\s+Z\b/m.test(processStatus)) return false;
    const binary = await readlink(`/proc/${pid}/exe`);
    const env = await readFile(`/proc/${pid}/environ`, 'utf8');
    assert.match(binary, /\/agent-mod-manager(?: \(deleted\))?$/);
    assert.equal((await stat(`/proc/${pid}`)).uid, process.getuid());
    assert.ok(env.split('\0').includes(`XDG_DATA_HOME=${dataHome}`), 'Application is outside the smoke profile');
    assert.ok(env.split('\0').includes('AMM_DISABLE_ANALYTICS=1'),
      'Analytics opt-out did not reach the packaged application process');
    return true;
  } catch (error) {
    if (error.code === 'ENOENT' || error.code === 'ESRCH') return false;
    throw error;
  }
}

async function quitForRelaunch(ui, dataHome) {
  const { stdout } = await xdo(['search', '--onlyvisible', '--name', '^Agent Mod Manager$']);
  const windows = stdout.trim().split(/\s+/);
  assert.ok(windows.length === 1 && /^\d+$/.test(windows[0]), 'Expected one packaged main window');
  const pid = Number((await xdo(['getwindowpid', windows[0]])).stdout.trim());
  assert.ok(Number.isInteger(pid) && pid > 1 && pid !== process.pid, 'Unexpected application PID');
  assert.ok(await fixtureProcessAlive(pid, dataHome));
  await ui.close();
  await delay(200);
  if (await fixtureProcessAlive(pid, dataHome)) {
    // In extraction mode WebDriver stops the AppImage wrapper, not always its GTK child.
    try { process.kill(pid, 'SIGTERM'); }
    catch (error) { if (error.code !== 'ESRCH') throw error; }
    console.log('Stopped the isolated app process left by the WebDriver wrapper before relaunch.');
  }
  const deadline = performance.now() + 5_000;
  while (await fixtureProcessAlive(pid, dataHome)) {
    assert.ok(performance.now() < deadline, 'The first app process did not exit before relaunch');
    await delay(100);
  }
}

async function importAgentFixture(ui, root, profile) {
  const source = join(root, installFilename);
  const bytes = metadataJar('smoke_install', '1.0.0');
  const sha256 = createHash('sha256').update(bytes).digest('hex');
  const destination = join(root, 'mods', installFilename);
  await writeFile(source, bytes, { flag: 'wx' });
  await ui.click(button('导入本地归档'));
  await ui.chooseArchive(source);
  await ui.visible(textAt(dialog, `导入本地归档到管理器：${installFilename}`, 'p'));
  await ui.visible(textAt(dialog, '复制', 'td'));
  const path = await visibleText(ui, `${dialog}//tbody[count(tr)=1]/tr[td[1][@data-operation="copy"]]/td[2]/span[last()]`);
  const imports = pathIdentity(join(profile, 'staging', 'imports')) + '/';
  const reviewed = pathIdentity(path);
  assert.ok(isAbsolute(path) && reviewed.startsWith(imports), 'Reviewed import must target the isolated profile');
  const [id, filename, ...extra] = reviewed.slice(imports.length).split('/');
  assert.match(id, /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);
  assert.equal(filename, installFilename);
  assert.deepEqual(extra, []);
  await assert.rejects(access(path), { code: 'ENOENT' }, 'Import preview must not publish the managed archive');
  await assert.rejects(access(destination), { code: 'ENOENT' });
  await ui.click(button('批准并执行', dialog));
  await ui.click(`${tab('Mod 管理')}[not(${dialog})]`);
  await ui.visible(textAt(modRow, '待安装', 'span'));
  await ui.visible(`${modRow}/td[${exact('本地归档')}]`);
  assert.deepEqual(await readFile(path), bytes, 'Reviewed import must preserve archive bytes');
  await assert.rejects(access(destination), { code: 'ENOENT' }, 'Import must not install the fixture');
  return { id, library: join(profile, 'Mods'), destination, sha256, bytes };
}

async function runAgentTurns(ui, model, root, artifact, screenshot) {
  const original = join(root, note);
  await ui.click('//button[@aria-label="设置"]');
  // The separate post-install organizer is not part of these four scripted turns.
  await ui.click(tab('Skills'));
  const organizer = `${main}//button[@role="checkbox" and @aria-label="启用 安装后整理 Mod"]`;
  await ui.visible(organizer);
  if (await ui.count(`${organizer}[@aria-checked="true"]`)) await ui.click(`${organizer}[@aria-checked="true"]`);
  await ui.visible(`${organizer}[@aria-checked="false" and not(@disabled)]`);
  await ui.click(tab('AI 与模型'));
  await ui.visible(textAt(main, '暂无模型'));
  assert.equal(await ui.inputValue(`${labeledInput('API 密钥')}[@placeholder="输入服务商提供的 API Key"]`), '',
    'The Agent test needs a fresh model configuration without credentials');
  await ui.fill(labeledInput('API 地址'), model.baseUrl);
  await ui.click(button('手动添加'));
  const modelInput = `${main}//table[@aria-label="模型列表"]//input[@placeholder="输入模型 ID"]`;
  assert.equal(await ui.inputValue(modelInput), '');
  await ui.fill(modelInput, modelName);
  await ui.click(button('保存设置'));
  await ui.visible('//*[@role="status" and normalize-space(.)="模型设置已保存"]');
  await ui.click(nav('Agent'));
  const workspace = modal('选择工作区游戏');
  await ui.click(`${main}//button[@aria-label="切换工作区：全部游戏"]`);
  await ui.click(button('清空', workspace));
  await ui.click(`${workspace}//label[.//span[${exact(game)}]]//button[@role="checkbox"]`);
  await ui.click(button('应用', workspace));
  await ui.visible(`${main}//button[@aria-label=${literal(`切换工作区：${game}`)} and not(${workspace})]`);
  await send(ui, diagnosePrompt);
  await ui.visible(assistantAnswer(firstDelta));
  assert.equal(model.calls(), 1, 'The first delta must be visible before the server completes');
  model.allowDiagnosis();
  await ui.visible(assistantAnswer(diagnosisReply));
  await ui.visible(textAt(main, '19 tokens', 'span'));
  console.log('Agent: rendered an unfinished stream, then ran the real read-only Fabric diagnostic tool.');

  const approval = `${main}//*[@role="group" and @aria-label="操作审批"]`;
  await send(ui, installPrompt);
  const copy = `${approval}//tbody[count(tr)=1]/tr[td[1][@data-operation="copy"]]`;
  await ui.visible(`${copy}/td[2]/span[last()][${pathIs(artifact.destination)}]`);
  const archive = await visibleText(ui, `${copy}/td[2]/span[1]`);
  const library = pathIdentity(artifact.library) + '/';
  const reviewed = pathIdentity(archive);
  assert.ok(isAbsolute(archive) && reviewed.startsWith(library), 'Install must copy from the isolated Mod library');
  const [category, entry, segment, filename, ...extra] = reviewed.slice(library.length).split('/');
  assert.ok(category && !['.', '..'].includes(category) && entry?.endsWith(`--${artifact.id}`),
    'Install source must be the exact indexed import');
  assert.equal(segment, 'archive');
  assert.equal(filename, installFilename);
  assert.deepEqual(extra, []);
  assert.equal(createHash('sha256').update(await readFile(archive)).digest('hex'), artifact.sha256,
    'Reviewed library source must match the imported JAR');
  assert.equal(model.calls(), 3);
  await assert.rejects(access(artifact.destination), { code: 'ENOENT' }, 'Agent installation preview must not deploy');
  await ui.screenshot(evidencePath(screenshot, 'agent-install-approval'));
  await ui.click(button('批准并执行', approval));
  await ui.visible(assistantAnswer(installReply));
  await ui.visible(textAt(main, '21 tokens', 'span'));
  assert.deepEqual(await readFile(artifact.destination), artifact.bytes, 'Installed fixture must match the reviewed archive');
  console.log('Agent: reviewed and installed the inert JAR selected in the native archive chooser.');

  await send(ui, trashPrompt);
  await ui.visible(planPath(approval, original));
  assert.equal(model.calls(), 5);
  assert.equal(await readFile(original, 'utf8'), noteContents, 'Agent approval preview mutated the fixture');
  await ui.screenshot(evidencePath(screenshot, 'agent-approval'));
  await ui.click(button('批准并执行', approval));
  await ui.visible(assistantAnswer(trashReply));
  await ui.visible(textAt(main, '23 tokens', 'span'));
  await assert.rejects(access(original), { code: 'ENOENT' }, 'Approval must commit the trash transaction');

  await send(ui, cancelPrompt);
  await ui.visible(assistantAnswer(cancelDelta));
  await ui.click(`${main}//button[@aria-label="停止生成"]`);
  await model.waitForCancellation();
  await ui.visible(`${main}//button[@aria-label="发送消息"]`);
  assert.equal(await ui.count(approval), 0);
  assert.equal(await readFile(join(root, 'options.txt'), 'utf8'), 'fullscreen:false\n',
    'Incomplete cancelled tool calls must never modify game files');
  console.log('Agent: approval committed the reviewed move; cancellation closed the incomplete tool stream.');

  await ui.click(nav('历史对话'));
  await ui.click(`${main}//button[@title=${literal(`归档：${conversationTitle}`)}]`);
  await ui.visible(`${main}//*[@data-slot="empty-title" and ${exact('暂无历史对话')}]`);
  console.log(`Agent: ${model.calls()} chat completions and ${model.summaryCalls()} context summaries.`);
}

async function reopenAgentHistory(ui, model, root, screenshot) {
  const summaries = model.summaryCalls();
  // Confirm the UI choice persisted through the real native locale IPC and app relaunch.
  await ui.click(iconButton('settings'));
  assert.equal(await ui.inputValue('//*[@id="app-locale"]'), 'zh-CN',
    'The disposable native profile must retain its selected UI language after relaunch');
  await ui.click(nav('历史对话'));
  await ui.click(`${main}//button[@role="tab" and ${exact('已归档')}]`);
  await ui.click(`${main}//button[@aria-label=${literal(`打开对话：${conversationTitle}`)}]`);
  for (const answer of [diagnosisReply, installReply, trashReply, cancelDelta]) {
    await ui.visible(assistantAnswer(answer));
  }
  await ui.visible(textAt(main, '23 tokens', 'span'));
  assert.equal(await ui.enabled(composer), false, 'Archived conversation must be read-only');
  await ui.screenshot(evidencePath(screenshot, 'agent-archive'));
  await restoreNote(ui, root);
  model.assertHealthy();
  assert.equal(model.calls(), 7, 'History replay and restoration must not call the model');
  assert.equal(model.summaryCalls(), summaries, 'History replay and restoration must not invoke compression');
  console.log('Agent: reopened the archived conversation after relaunch and restored the fixture note exactly.');

  await ui.click(nav('游戏库'));
  await ui.click(gameTile);
  await ui.click(tab('Mod 管理'));
  await ui.visible(textAt(modRow, '1 个文件', 'small'));
  await ui.visible(`${modRow}/td[${exact('本地归档')}]`);
  const installed = `${modRow}/td[.//span[${exact('已安装')}]]`;
  await ui.visible(`${installed}//*[@role="switch" and @aria-label="使用 Agent 卸载" and @aria-checked="true"]`);
}

async function verifyWindowsState(profile, root) {
  const { DatabaseSync } = await import('node:sqlite');
  const db = new DatabaseSync(join(profile, 'manager.db'), { readOnly: true });
  try {
    assert.deepEqual(db.prepare('PRAGMA quick_check').all().map(row => row.quick_check), ['ok']);
    const games = db.prepare('SELECT root_path FROM games').all();
    assert.deepEqual(games.map(row => pathIdentity(row.root_path)), [pathIdentity(root)]);
    const txns = db.prepare('SELECT kind, status, count(*) AS n FROM file_transactions GROUP BY kind, status ORDER BY kind, status').all();
    assert.deepEqual(txns.map(({ kind, status, n }) => [kind, status, n]), [
      ['files', 'committed', 3], // Adapter draft, adapter deployment, and archive import.
      ['internal', 'committed', 2], // Trash and restore.
      ['library_install', 'committed', 1],
    ]);
    const installs = db.prepare('SELECT name, status, source, json_array_length(files_json) AS n FROM installs').all();
    assert.deepEqual(installs.map(({ name, status, source, n }) => [name, status, source, n]), [
      [installFilename, 'enabled', 'local', 1],
    ]);
    assert.equal(db.prepare("SELECT source FROM plugins WHERE slug='minecraft-java'").get()?.source, 'external');
    const turns = db.prepare('SELECT status FROM agent_turns ORDER BY status').all();
    assert.deepEqual(turns.map(row => row.status), ['cancelled', 'done', 'done', 'done']);
    const conversations = db.prepare('SELECT title, archived FROM conversations').all();
    assert.deepEqual(conversations.map(({ title, archived }) => [title, archived]), [[conversationTitle, 1]]);
    console.log('SQLite quick_check: ok; one fixture game, six committed transactions, one enabled local Mod, three completed turns, one cancelled turn, one archived conversation.');
  } finally {
    db.close();
  }
}

async function run() {
  assert.ok(windows || platform === 'linux', 'The native WebView smoke supports Windows and Linux');
  const [application, requestedScreenshot, ...extra] = process.argv.slice(2);
  assert.ok(application && isAbsolute(application) && !extra.length &&
    (requestedScreenshot === undefined || isAbsolute(requestedScreenshot)),
    'Usage: node scripts/smoke-desktop-webview.mjs <absolute app binary> [absolute screenshot.png]');
  const screenshot = requestedScreenshot ?? resolve(tmpdir(), `amm-desktop-webview-${process.pid}.png`);
  assert.notEqual(resolve(screenshot), resolve(application), 'Screenshot path must not be the app binary');
  await access(application, constants.X_OK);
  assert.ok((await stat(application)).isFile(), 'Application must be an executable file');
  const testAgent = windows || process.env.AMM_LINUX_AGENT_SMOKE === '1';
  const dataHome = process.env.XDG_DATA_HOME;
  const windowsProfile = process.env.AMM_WINDOWS_SMOKE_PROFILE;
  if (windows) {
    assert.ok(windowsProfile && isAbsolute(windowsProfile) && basename(windowsProfile) === dataDirectoryName,
      'Use run-windows-package-smoke.ps1 in a disposable Windows profile');
    await assert.rejects(access(windowsProfile), { code: 'ENOENT' },
      'Windows smoke refuses to open an existing app profile');
  }
  if (testAgent && !windows) {
    assert.ok(dataHome && isAbsolute(dataHome), 'Agent smoke requires a fresh isolated XDG_DATA_HOME');
    assert.ok(process.env.DISPLAY, 'Agent smoke requires its own X11 desktop');
    await assert.rejects(access(join(dataHome, dataDirectoryName, 'manager.db')), { code: 'ENOENT' },
      'Agent smoke refuses to change the model settings of an existing profile');
  }

  const scratch = await mkdtemp(join(await realpath(tmpdir()), 'amm-desktop-webview-'));
  const root = join(scratch, 'minecraft');
  const startSession = async () => windows
    ? (await import('./smoke-webview2.mjs')).startWebView2(application, scratch)
    : startLinuxSession(application);
  let ui;
  let model;
  let savedEvidence = false;
  let stage = 'creating the offline fixture';
  try {
    await mkdir(root);
    await createFixture(root);
    stage = windows ? 'starting the installed Windows app and connecting to WebView2' : 'opening a stock tauri-driver session';
    ui = await startSession();

    stage = 'inspecting, registering, and reopening the Minecraft fixture';
    await registerAndOpen(ui, root);
    stage = 'checking historical Fabric startup and disk JAR evidence';
    await diagnose(ui);
    console.log(`Fabric: Minecraft ${minecraft}, Loader ${fabric}; ${mods.map(mod =>
      `${mod.id} disk=${mod.disk} log=${mod.logged} (line ${mod.line})`).join('; ')}`);
    await ui.screenshot(screenshot);
    savedEvidence = true;

    if (testAgent) {
      const profile = windows ? windowsProfile : join(dataHome, dataDirectoryName);
      stage = 'selecting and approving an inert archive through the native file chooser';
      const artifact = await importAgentFixture(ui, root, profile);
      stage = 'running native Agent streaming, approval, cancellation and archiving';
      model = await startSmokeModel(root, note, artifact);
      await runAgentTurns(ui, model, root, artifact, screenshot);
      stage = 'relaunching the packaged desktop app and replaying archived history';
      if (windows) await ui.close();
      else await quitForRelaunch(ui, dataHome);
      ui = undefined;
      ui = await startSession();
      await reopenAgentHistory(ui, model, root, screenshot);
      if (windows) {
        stage = 'verifying the Windows SQLite journal and Agent history';
        await verifyWindowsState(profile, root);
      }
    } else {
      stage = 'moving and restoring the disposable note through reviewed file plans';
      await moveAndRestore(ui, root);
      console.log('Trash and restore: previews did not mutate the file; move and byte-exact restore committed.');
    }
  } catch (error) {
    if (ui) {
      const failure = savedEvidence ? evidencePath(screenshot, 'failure') : screenshot;
      await ui.screenshot(failure).catch(cause => console.error(`Failure screenshot unavailable: ${cause.message}`));
      try {
        for (const alert of await ui.alerts()) console.error(`UI alert: ${alert}`);
      } catch (cause) {
        console.error(`UI alerts unavailable: ${cause.message}`);
      }
    }
    try {
      model?.assertHealthy();
    } catch (providerError) {
      throw new AggregateError([error, providerError],
        `Failed while ${stage}: ${error.message}; model fixture: ${providerError.message}`);
    }
    throw new Error(`Failed while ${stage}: ${error.message}`, { cause: error });
  } finally {
    try {
      if (ui) await ui.close();
    } catch (error) {
      console.error(`Native WebView session cleanup failed: ${error.message}`);
      process.exitCode = 1;
    } finally {
      await model?.close();
      await rm(scratch, { recursive: true, force: true, maxRetries: 5 });
    }
  }
}

try {
  await run();
  if (!process.exitCode) console.log(`PASS: packaged Tauri ${windows ? 'Windows WebView2' : 'Linux WebKitGTK'} smoke`);
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
