import { spawnSync } from 'node:child_process';
import { isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';

const script = fileURLToPath(new URL('./prepare-mcp-runtime.py', import.meta.url));
const override = process.env.AMM_BUILD_PYTHON;
if (override && !isAbsolute(override)) {
  console.error('AMM_BUILD_PYTHON must be an absolute path to a Python 3.11+ interpreter.');
  process.exit(1);
}
const candidates = override
  ? [[override]]
  : process.platform === 'win32'
    ? [['py', '-3'], ['python']]
    : [['python3'], ['python']];

for (const [command, ...prefix] of candidates) {
  const probe = spawnSync(command, [...prefix, '-c', 'import sys; sys.exit(sys.version_info < (3, 11))'], {
    windowsHide: true,
  });
  if (probe.status !== 0) continue;
  const result = spawnSync(command, [...prefix, script, ...process.argv.slice(2)], {
    stdio: 'inherit', windowsHide: true,
  });
  if (result.error) console.error(result.error.message);
  process.exit(result.status ?? 1);
}

console.error('Preparing desktop runtime resources requires Python 3.11+. Install it or set AMM_BUILD_PYTHON to its absolute path.');
process.exit(1);
