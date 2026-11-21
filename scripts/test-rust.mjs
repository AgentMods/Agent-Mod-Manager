import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const project = fileURLToPath(new URL('../', import.meta.url));
// Unix test fixtures bind filesystem sockets with a short path-length limit.
const windows = process.platform === 'win32';
const scratchRoot = windows ? join(project, 'src-tauri', 'target', 'test-tmp') : '/tmp';
if (windows) mkdirSync(scratchRoot, { recursive: true });
const parent = realpathSync(scratchRoot);
const runDir = mkdtempSync(join(parent, 'amm-'));

try {
  const cargo = spawnSync('cargo', [
    'test', '--manifest-path', 'src-tauri/Cargo.toml', ...process.argv.slice(2),
  ], {
    cwd: project,
    env: { ...process.env, TEMP: runDir, TMP: runDir, TMPDIR: runDir },
    stdio: 'inherit',
  });
  if (cargo.error) throw cargo.error;
  process.exitCode = cargo.status ?? 1;
} finally {
  // SQLite handles held by test fixtures are closed once the test executable exits.
  if (dirname(realpathSync(runDir)) !== parent) throw new Error(`Unexpected test directory: ${runDir}`);
  rmSync(runDir, { recursive: true, maxRetries: 5, retryDelay: 200 });
}
