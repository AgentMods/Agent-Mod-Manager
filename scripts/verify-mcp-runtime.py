"""Exercise the packaged MCP launchers and Bing helper offline."""
import argparse
import json
import os
import shutil
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import zipfile

LOCK = Path(__file__).with_name('mcp-runtime.lock.json')
PACKAGE = LOCK.parent.parent / 'package.json'


def python_fixture(directory):
    wheel = directory / 'amm_fixture-1.0-py3-none-any.whl'
    info = 'amm_fixture-1.0.dist-info'
    files = {
        'amm_fixture.py': 'def main():\n    print("python-fixture-ok")\n',
        f'{info}/METADATA': 'Metadata-Version: 2.1\nName: amm-fixture\nVersion: 1.0\n',
        f'{info}/WHEEL': 'Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n',
        f'{info}/entry_points.txt': '[console_scripts]\namm-fixture = amm_fixture:main\n',
    }
    record = f'{info}/RECORD'
    files[record] = ''.join(f'{name},,\n' for name in [*files, record])
    with zipfile.ZipFile(wheel, 'w') as archive:
        for name, text in files.items():
            archive.writestr(name, text)
    return wheel


def verify(archive_path, work):
    runtime = work / 'installed runtime'
    # No links or traversal: extract files exactly as the production runtime does.
    with zipfile.ZipFile(archive_path) as archive:
        for entry in archive.infolist():
            dest = (runtime / entry.filename).resolve()
            if not dest.is_relative_to(runtime) or stat.S_ISLNK(entry.external_attr >> 16):
                raise ValueError(f'Unsafe runtime entry {entry.filename!r}')
            archive.extract(entry, runtime)
            if not entry.is_dir() and (entry.external_attr >> 16) & 0o111:
                dest.chmod(0o700)

    bin_dir = runtime / 'bin'
    exe = '.exe' if os.name == 'nt' else ''
    node = bin_dir / f'node{exe}'
    uvx = bin_dir / f'uvx{exe}'
    npx = runtime / 'npm/bin/npx-cli.js'
    home = work / 'home'
    home.mkdir()
    env = {k: os.environ[k] for k in ('SYSTEMROOT', 'WINDIR') if k in os.environ}
    if os.name == 'nt':
        os_bins = [str(Path(env['SYSTEMROOT']) / 'System32')]
    else:
        # Expose shell tools used by package wrappers, excluding system node/npm/uv.
        shell_dir = work / 'shell'
        shell_dir.mkdir()
        for name in ('sh', 'realpath', 'dirname'):
            source = shutil.which(name)
            if source is None:
                raise RuntimeError(f'Missing system utility: {name}')
            (shell_dir / name).symlink_to(source)
        os_bins = [str(shell_dir)]
    env.update(PATH=os.pathsep.join([str(bin_dir), *os_bins]), HOME=str(home), USERPROFILE=str(home),
               TEMP=str(work), TMP=str(work), TMPDIR=str(work), npm_config_cache=str(work / 'npm-cache'),
               UV_CACHE_DIR=str(work / 'uv-cache'), UV_PYTHON_DOWNLOADS='never',
               UV_OFFLINE='true', npm_config_offline='true')
    def run(args, expected):
        result = subprocess.run(args, cwd=work, env=env, text=True, capture_output=True, timeout=90)
        if result.returncode or expected not in result.stdout.splitlines():
            raise RuntimeError(f'{args[0]} {args[1:]} failed: {result.returncode}\n{result.stdout}\n{result.stderr}')
        print(result.stdout.strip())

    lock = json.loads(LOCK.read_text(encoding='utf-8'))
    run([node, '--version'], 'v' + lock['node']['version'])
    run([node, npx, '--version'], lock['node']['npmVersion'])
    helper = bin_dir / 'bing-search.cjs'
    if helper.read_bytes() != LOCK.with_name('bing-search.cjs').read_bytes():
        raise RuntimeError('Packaged Bing helper does not match scripts/bing-search.cjs')
    node_flags = [node, '--no-global-search-paths']
    subprocess.run([*node_flags, '--check', helper], cwd=work, env=env, check=True, timeout=15)
    playwright_version = json.loads(PACKAGE.read_text(encoding='utf-8'))['devDependencies']['playwright-core']
    # Use Node's resolution from the actual helper location and require the packaged
    # API. Do not launch a browser or perform an HTTP request in installer smoke tests.
    run([*node_flags, '--eval', '''
const assert = require('node:assert/strict');
const { realpathSync } = require('node:fs');
const { createRequire } = require('node:module');
const { dirname, join } = require('node:path');
const helper = process.argv[1];
const load = createRequire(helper);
const local = realpathSync(join(dirname(helper), 'node_modules', 'playwright-core', 'index.js'));
assert.equal(realpathSync(load.resolve('playwright-core')), local);
assert.equal(load('playwright-core/package.json').version, process.argv[2]);
assert.equal(typeof load('playwright-core').chromium.launch, 'function');
console.log('bing-helper-playwright-ok');
''', '--', helper, playwright_version], 'bing-helper-playwright-ok')
    # Check the shipped CLI, including the browser registry, without downloading a browser.
    cli = bin_dir / 'node_modules/playwright-core/cli.js'
    run([*node_flags, cli, 'mcp', '--help'], 'Usage: npx playwright mcp [options]')
    plan = subprocess.check_output([*node_flags, cli, 'install', '--dry-run', '--no-shell', 'chromium'],
                                   cwd=work, env=env, text=True, timeout=15)
    if 'chromium' not in plan or 'Install location:' not in plan:
        raise RuntimeError('Packaged Playwright CLI could not resolve its Chromium build')
    # uvx's version may also display its upstream build commit and date.
    version = subprocess.check_output([uvx, '--version'], cwd=work, env=env, text=True, timeout=15).strip()
    if version.split()[:2] != ['uvx', lock['uv']['version']]:
        raise RuntimeError(f'Unexpected uvx version: {version}')
    print(version)
    package = work / 'npm-fixture'
    package.mkdir()
    (package / 'package.json').write_text(json.dumps({'name': 'amm-fixture', 'version': '1.0.0',
                                                      'bin': {'amm-fixture': 'cli.js'}}), encoding='utf-8')
    cli = package / 'cli.js'
    cli.write_text('#!/usr/bin/env node\nconsole.log(require("node:fs").realpathSync(process.execPath));\n', encoding='utf-8', newline='\n')
    cli.chmod(0o755)
    run([node, npx, '--yes', '--offline', '--package=file:./npm-fixture', '--', 'amm-fixture'], str(node.resolve()))
    # Resolve the stock Nexus package from the installed runtime, without fetching dependencies.
    requirement = work / 'nexus-requirements.in'
    requirement.write_text('glossmod-nexusmods-mcp\n', encoding='utf-8')
    run([bin_dir / f'uv{exe}', 'pip', 'compile', '--no-index', '--find-links', runtime / 'nexus',
         '--no-deps', '--no-header', '--python', sys.executable, requirement],
        'glossmod-nexusmods-mcp==1.0.0+amm.1')
    wheel = python_fixture(work)
    run([uvx, '--offline', '--python', sys.executable, '--from', str(wheel), 'amm-fixture'], 'python-fixture-ok')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('resource', type=Path, help='mcp-runtime/runtime.zip or the installed resource directory')
    resource = parser.parse_args().resource.resolve()
    if resource.is_dir():
        resource = resource / 'mcp-runtime/runtime.zip'
    with tempfile.TemporaryDirectory(prefix='amm runtime smoke ') as temp:
        verify(resource, Path(temp).resolve())


if __name__ == '__main__':
    main()
