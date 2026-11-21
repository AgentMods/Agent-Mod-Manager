import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import stat
import sys
import tarfile
import tempfile
import time
from urllib.request import Request, urlopen
import zipfile


ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / 'scripts/mcp-runtime.lock.json'
OUTPUT = ROOT / 'src-tauri/resources/mcp-runtime/runtime.zip'
CACHE = ROOT / 'src-tauri/target/mcp-runtime-downloads'
MAX_DOWNLOAD_BYTES = 128 * 1024 * 1024

# Node's names differ from both Rust's target triples and Astral's asset names.
NODE_TARGETS = {
    'aarch64-apple-darwin': 'darwin-arm64',
    'x86_64-apple-darwin': 'darwin-x64',
    'aarch64-unknown-linux-gnu': 'linux-arm64',
    'x86_64-unknown-linux-gnu': 'linux-x64',
    'aarch64-pc-windows-msvc': 'win-arm64',
    'x86_64-pc-windows-msvc': 'win-x64',
}


def host_target():
    machine = platform.machine().lower()
    # An emulated Python should still select the native OS architecture.
    if sys.platform == 'win32':
        machine = os.environ.get('PROCESSOR_ARCHITEW6432', machine).lower()
    arch = {'x86_64': 'x86_64', 'amd64': 'x86_64', 'arm64': 'aarch64', 'aarch64': 'aarch64'}.get(machine)
    system = {'win32': 'pc-windows-msvc', 'linux': 'unknown-linux-gnu', 'darwin': 'apple-darwin'}.get(sys.platform)
    if not arch or not system:
        raise ValueError(f'No MCP runtime for host {sys.platform}/{machine}; specify --target.')
    return f'{arch}-{system}'


def sha256_file(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def download(url, sha256):
    if not re.fullmatch(r'[0-9a-f]{64}', sha256):
        raise ValueError(f'Invalid locked SHA-256 for {url}')
    destination = CACHE / f'{sha256}-{url.rsplit("/", 1)[-1]}'
    if destination.is_file() and sha256_file(destination) == sha256:
        return destination
    for attempt in range(3):
        try:
            with tempfile.TemporaryDirectory(prefix='download-', dir=CACHE) as work:
                path = Path(work) / 'asset'
                request = Request(url, headers={'User-Agent': 'Agent-Mod-Manager-MCP-Runtime'})
                with urlopen(request, timeout=60) as response, path.open('wb') as stream:
                    size = 0
                    for chunk in iter(lambda: response.read(1024 * 1024), b''):
                        size += len(chunk)
                        if size > MAX_DOWNLOAD_BYTES:
                            raise ValueError(f'Download exceeds size limit: {url}')
                        stream.write(chunk)
                actual = sha256_file(path)
                if actual != sha256:
                    raise ValueError(f'SHA-256 mismatch for {url}: expected {sha256}, got {actual}')
                path.replace(destination)
            return destination
        except (OSError, ValueError) as error:
            if attempt == 2:
                raise RuntimeError(f'Could not obtain verified asset {url}: {error}') from error
            time.sleep(2 ** attempt)
    raise AssertionError('unreachable')


class RuntimeZip:
    def __init__(self, archive):
        self.archive = archive
        self.names = set()

    def add(self, dest, source, size, executable=False):
        parts = dest.split('/')
        if (any(part in ('', '.', '..') for part in parts) or '\\' in dest or ':' in dest
                or '\x00' in dest or dest in self.names):
            raise ValueError(f'Unexpected archive member: {dest!r}')
        self.names.add(dest)
        info = zipfile.ZipInfo(dest)  # Fixed timestamp, no per-machine paths, and Unix mode metadata.
        info.create_system = 3
        info.external_attr = (stat.S_IFREG | (0o755 if executable else 0o644)) << 16
        info.compress_type = zipfile.ZIP_DEFLATED
        info.file_size = size
        with self.archive.open(info, 'w') as entry:
            shutil.copyfileobj(source, entry)

    def copy_distribution(self, asset, select):
        # Read only the selected regular files and never extract upstream archives to disk.
        if asset.suffix == '.zip':
            with zipfile.ZipFile(asset) as source:
                for member in source.infolist():
                    dest = select(member.filename)
                    if dest is None or member.is_dir():
                        continue
                    if stat.S_IFMT(member.external_attr >> 16) not in (0, stat.S_IFREG):
                        raise ValueError(f'Unexpected non-file in {asset.name}: {member.filename}')
                    with source.open(member) as stream:
                        self.add(dest, stream, member.file_size, dest.startswith('bin/'))
        else:
            with tarfile.open(asset, 'r|*') as source:
                for member in source:
                    dest = select(member.name)
                    if dest is None or member.isdir():
                        continue
                    if not member.isfile():
                        raise ValueError(f'Unexpected non-file in {asset.name}: {member.name}')
                    with source.extractfile(member) as stream:
                        self.add(dest, stream, member.size, dest.startswith('bin/'))


def prepare(target):
    if target not in NODE_TARGETS:
        raise ValueError(f'Unsupported MCP runtime target {target!r}. Supported: {", ".join(NODE_TARGETS)}')
    node_target = NODE_TARGETS[target]
    windows = node_target.startswith('win-')
    lock = json.loads(LOCK.read_text(encoding='utf-8'))
    node, uv = lock['node'], lock['uv']
    node_root = f'node-v{node["version"]}-{node_target}'
    node_asset = f'{node_root}.zip' if windows else f'{node_root}.tar.xz'
    uv_root = f'uv-{target}'
    uv_asset = f'{uv_root}.zip' if windows else f'{uv_root}.tar.gz'
    exe = '.exe' if windows else ''

    CACHE.mkdir(parents=True, exist_ok=True)
    print(f'Preparing MCP runtime for {target}: Node {node["version"]}, npm {node["npmVersion"]}, uv {uv["version"]}', flush=True)
    node_archive = download(f'https://nodejs.org/dist/v{node["version"]}/{node_asset}', node['sha256'][node_target])
    uv_archive = download(f'https://github.com/astral-sh/uv/releases/download/{uv["version"]}/{uv_asset}', uv['sha256'][target])
    npm_prefix = f'{node_root}/{"" if windows else "lib/"}node_modules/npm/'
    node_binary = f'{node_root}/{"node.exe" if windows else "bin/node"}'
    uv_names = {f'uv{exe}', f'uvx{exe}'}

    def select_node(name):
        if name == node_binary:
            return f'bin/node{exe}'
        if name == f'{node_root}/LICENSE':
            return 'licenses/node/LICENSE'
        if name.startswith(npm_prefix) and name != npm_prefix:
            return 'npm/' + name[len(npm_prefix):]
        return None

    def select_uv(name):
        prefix = '' if windows else uv_root + '/'
        if name.startswith(prefix) and name[len(prefix):] in uv_names:
            return 'bin/' + name[len(prefix):]
        return None

    # Stage outside Tauri's watched resource directory. A failed run cannot publish a partial ZIP.
    with tempfile.TemporaryDirectory(prefix='assemble-', dir=CACHE) as work:
        path = Path(work) / 'runtime.zip'
        with zipfile.ZipFile(path, 'w') as archive:
            runtime = RuntimeZip(archive)
            runtime.copy_distribution(node_archive, select_node)
            runtime.copy_distribution(uv_archive, select_uv)
            for name, sha256 in uv['licenses'].items():
                license_path = download(f'https://raw.githubusercontent.com/astral-sh/uv/{uv["commit"]}/{name}', sha256)
                with license_path.open('rb') as stream:
                    runtime.add(f'licenses/uv/{PurePosixPath(name)}', stream, license_path.stat().st_size)
            required = {
                f'bin/node{exe}', f'bin/uv{exe}', f'bin/uvx{exe}',
                'npm/bin/npm-cli.js', 'npm/bin/npx-cli.js', 'npm/package.json', 'npm/LICENSE',
                'licenses/node/LICENSE', 'licenses/uv/LICENSE-APACHE', 'licenses/uv/LICENSE-MIT',
            }
            if missing := required - runtime.names:
                raise ValueError(f'Missing entries in assembled MCP runtime: {", ".join(sorted(missing))}')
        with zipfile.ZipFile(path) as archive:
            npm_package = json.loads(archive.read('npm/package.json'))
            if npm_package['name'] != 'npm' or npm_package['version'] != node['npmVersion']:
                raise ValueError('Node distribution does not contain the locked npm version')
        digest = sha256_file(path)
        # Preserve the timestamp for repeat builds of identical bytes (Cargo watches this directory).
        if not OUTPUT.is_file() or sha256_file(OUTPUT) != digest:
            path.replace(OUTPUT)
    print(f'{OUTPUT.relative_to(ROOT)}: SHA-256 {digest}')


def main():
    parser = argparse.ArgumentParser(description='Prepare the pinned Node/npm/uv MCP runtime ZIP.')
    parser.add_argument('--target', help='Rust target triple; defaults to TAURI_ENV_TARGET_TRIPLE or native host')
    args = parser.parse_args()
    try:
        prepare(args.target or os.environ.get('TAURI_ENV_TARGET_TRIPLE') or host_target())
    except (OSError, ValueError, RuntimeError, tarfile.TarError, zipfile.BadZipFile) as error:
        parser.exit(1, f'MCP runtime preparation failed: {error}\n')


if __name__ == '__main__':
    main()
