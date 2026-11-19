import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { access, mkdir, mkdtemp, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const [application, evidenceDir] = process.argv.slice(2);
const timeoutMs = 15_000;

function usage() {
  throw new Error('Usage: node scripts/run-macos-package-smoke.mjs <absolute .app> <absolute evidence dir>');
}

function isAbsolutePath(value) {
  return typeof value === 'string' && resolve(value) === value;
}

async function waitForDatabase(path, child, output, getLaunchError) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const launchError = getLaunchError();
    if (launchError) throw launchError;
    if (child.exitCode !== null || child.signalCode !== null) {
      throw new Error(`packaged app exited before initializing SQLite: ${output.join('').slice(-2000)}`);
    }
    try {
      await stat(path);
      return;
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
    await delay(250);
  }
  throw new Error(`timed out waiting for ${path}`);
}

async function stop(child, exit) {
  if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
  await Promise.race([exit, delay(5000)]);
  if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
  await exit;
}

async function run() {
  if (process.platform !== 'darwin') throw new Error('This smoke test must run on macOS');
  if (!isAbsolutePath(application) || !application.endsWith('.app') || !isAbsolutePath(evidenceDir)) usage();

  const binary = join(application, 'Contents', 'MacOS', 'agent-mod-manager');
  await access(binary);
  await mkdir(evidenceDir, { recursive: true });

  const profile = await mkdtemp(join(tmpdir(), 'amm-macos-package-smoke-'));
  await mkdir(join(profile, 'tmp'), { recursive: true });
  const output = [];
  const child = spawn(binary, [], {
    cwd: profile,
    env: {
      ...process.env,
      HOME: profile,
      TMPDIR: join(profile, 'tmp'),
      XDG_CONFIG_HOME: join(profile, 'config'),
      XDG_DATA_HOME: join(profile, 'data'),
      XDG_STATE_HOME: join(profile, 'state'),
      XDG_CACHE_HOME: join(profile, 'cache'),
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  child.stdout.on('data', chunk => output.push(chunk.toString()));
  child.stderr.on('data', chunk => output.push(chunk.toString()));
  let launchError;
  child.once('error', error => { launchError = error; });
  const exit = new Promise(resolveExit => {
    child.once('exit', (code, signal) => resolveExit({ code, signal }));
  });

  const database = join(profile, 'Library', 'Application Support', 'app.agentmodmanager.desktop', 'manager.db');
  try {
    await waitForDatabase(database, child, output, () => launchError);
    const { DatabaseSync } = await import('node:sqlite');
    const db = new DatabaseSync(database, { readOnly: true });
    let quickCheck;
    let pluginCount;
    try {
      quickCheck = db.prepare('PRAGMA quick_check').get().quick_check;
      pluginCount = db.prepare('SELECT count(*) AS count FROM plugins').get().count;
    } finally {
      db.close();
    }
    assert.equal(quickCheck, 'ok');
    assert.equal(pluginCount, 6);
    const result = await stop(child, exit);
    await writeFile(join(evidenceDir, 'launch.json'), JSON.stringify({
      application: basename(application),
      database: 'Library/Application Support/app.agentmodmanager.desktop/manager.db',
      quickCheck,
      pluginCount,
      exit: result,
    }, null, 2) + '\n');
    await writeFile(join(evidenceDir, 'launch.log'), output.join(''));
    console.log(`PASS: macOS packaged app initialized SQLite and seeded ${pluginCount} plugins`);
  } finally {
    if (child.exitCode === null && child.signalCode === null) await stop(child, exit);
    await writeFile(join(evidenceDir, 'launch.log'), output.join('')).catch(() => {});
    await rm(profile, { recursive: true, force: true });
  }
}

run().catch(error => {
  console.error(error.message);
  process.exitCode = 1;
});
