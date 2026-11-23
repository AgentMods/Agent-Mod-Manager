import argparse
import base64
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
from urllib.request import Request, urlopen
import zipfile


ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / 'scripts/mcp-runtime.lock.json'
OUTPUT = ROOT / 'src-tauri/resources/mcp-runtime/runtime.zip'
SEVEN_ZIP_LOCK = ROOT / 'scripts/seven-zip.lock.json'
SEVEN_ZIP_OUTPUT = ROOT / 'src-tauri/resources/seven-zip'
CACHE = ROOT / 'src-tauri/target/mcp-runtime-downloads'
MAX_DOWNLOAD_BYTES = 128 * 1024 * 1024
BING_HELPER = 'bin/bing-search.cjs'
PLAYWRIGHT_DIR = 'bin/node_modules/playwright-core'
PLAYWRIGHT_CLI = f'{PLAYWRIGHT_DIR}/cli.js'

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


def hash_file(path, algorithm='sha256'):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, algorithm).hexdigest()


def download(url, digest, algorithm='sha256'):
    if len(digest) != 2 * hashlib.new(algorithm).digest_size or not re.fullmatch(r'[0-9a-f]+', digest):
        raise ValueError(f'Invalid locked {algorithm} for {url}')
    destination = CACHE / f'{digest}-{url.rsplit("/", 1)[-1]}'
    if destination.is_file() and hash_file(destination, algorithm) == digest:
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
                actual = hash_file(path, algorithm)
                if actual != digest:
                    raise ValueError(f'{algorithm} mismatch for {url}: expected {digest}, got {actual}')
                path.replace(destination)
            return destination
        except (OSError, ValueError) as error:
            if attempt == 2:
                raise RuntimeError(f'Could not obtain verified asset {url}: {error}') from error
            time.sleep(2 ** attempt)
    raise AssertionError('unreachable')


def locked_playwright():
    # Use Yarn's exact registry tarball and integrity, not the mutable node_modules tree
    # or a second version pin. Cross-target builds do not need Yarn's install tree.
    package = json.loads((ROOT / 'package.json').read_text(encoding='utf-8'))
    version = package['devDependencies'].get('playwright-core', '')
    if not re.fullmatch(r'\d+\.\d+\.\d+', version):
        raise ValueError('Pin playwright-core to an exact version in package.json')
    selector = f'playwright-core@{version}'
    yarn_lock = (ROOT / 'yarn.lock').read_text(encoding='utf-8')
    stanzas = []
    for stanza in re.split(r'\n(?=\S)', yarn_lock):
        header, _, body = stanza.partition('\n')
        if header.endswith(':') and selector in [key.strip().strip('"') for key in header[:-1].split(',')]:
            stanzas.append(body)
    url = f'https://registry.npmjs.org/playwright-core/-/playwright-core-{version}.tgz'
    if len(stanzas) != 1:
        raise ValueError(f'Expected exactly one {selector} entry in yarn.lock')
    block = stanzas[0]
    locked_version = re.search(r'^  version "([^"]+)"$', block, re.MULTILINE)
    resolved = re.search(r'^  resolved "([^"]+)"$', block, re.MULTILINE)
    integrity = re.search(r'^  integrity sha512-([A-Za-z0-9+/=]+)$', block, re.MULTILINE)
    if (not locked_version or locked_version[1] != version or not resolved
            or not re.fullmatch(re.escape(url) + r'#[0-9a-f]{40}', resolved[1]) or not integrity):
        raise ValueError(f'Invalid version, registry URL or integrity for {selector} in yarn.lock')
    return version, url, base64.b64decode(integrity[1], validate=True).hex()


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
        def executable(dest):
            return dest.startswith('bin/') and dest.count('/') == 1

        if asset.suffix == '.zip':
            with zipfile.ZipFile(asset) as source:
                for member in source.infolist():
                    dest = select(member.filename)
                    if dest is None or member.is_dir():
                        continue
                    if stat.S_IFMT(member.external_attr >> 16) not in (0, stat.S_IFREG):
                        raise ValueError(f'Unexpected non-file in {asset.name}: {member.filename}')
                    with source.open(member) as stream:
                        self.add(dest, stream, member.file_size, executable(dest))
        else:
            with tarfile.open(asset, 'r|*') as source:
                for member in source:
                    dest = select(member.name)
                    if dest is None or member.isdir():
                        continue
                    if not member.isfile():
                        raise ValueError(f'Unexpected non-file in {asset.name}: {member.name}')
                    with source.extractfile(member) as stream:
                        self.add(dest, stream, member.size, executable(dest))


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
    playwright_version, playwright_url, playwright_hash = locked_playwright()
    helper = ROOT / 'scripts/bing-search.cjs'
    if not helper.is_file():
        raise ValueError(f'Missing Bing search helper: {helper.relative_to(ROOT)}')

    CACHE.mkdir(parents=True, exist_ok=True)
    print(f'Preparing MCP runtime for {target}: Node {node["version"]}, npm {node["npmVersion"]}, '
          f'uv {uv["version"]}, playwright-core {playwright_version}', flush=True)
    node_archive = download(f'https://nodejs.org/dist/v{node["version"]}/{node_asset}', node['sha256'][node_target])
    uv_archive = download(f'https://github.com/astral-sh/uv/releases/download/{uv["version"]}/{uv_asset}', uv['sha256'][target])
    playwright_archive = download(playwright_url, playwright_hash, 'sha512')
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

    def select_playwright(name):
        if name.startswith('package/') and name != 'package/':
            return f'{PLAYWRIGHT_DIR}/{name[len("package/"):]}'
        return None

    # Stage outside Tauri's watched resource directory. A failed run cannot publish a partial ZIP.
    with tempfile.TemporaryDirectory(prefix='assemble-', dir=CACHE) as work:
        path = Path(work) / 'runtime.zip'
        with zipfile.ZipFile(path, 'w') as archive:
            runtime = RuntimeZip(archive)
            runtime.copy_distribution(node_archive, select_node)
            runtime.copy_distribution(uv_archive, select_uv)
            with helper.open('rb') as stream:
                runtime.add(BING_HELPER, stream, os.fstat(stream.fileno()).st_size)
            runtime.copy_distribution(playwright_archive, select_playwright)
            for name, sha256 in uv['licenses'].items():
                license_path = download(f'https://raw.githubusercontent.com/astral-sh/uv/{uv["commit"]}/{name}', sha256)
                with license_path.open('rb') as stream:
                    runtime.add(f'licenses/uv/{PurePosixPath(name)}', stream, license_path.stat().st_size)
            required = {
                f'bin/node{exe}', f'bin/uv{exe}', f'bin/uvx{exe}',
                'npm/bin/npm-cli.js', 'npm/bin/npx-cli.js', 'npm/package.json', 'npm/LICENSE',
                'licenses/node/LICENSE', 'licenses/uv/LICENSE-APACHE', 'licenses/uv/LICENSE-MIT',
                BING_HELPER, f'{PLAYWRIGHT_DIR}/index.js', f'{PLAYWRIGHT_DIR}/LICENSE',
                f'{PLAYWRIGHT_DIR}/package.json', PLAYWRIGHT_CLI,
            }
            if missing := required - runtime.names:
                raise ValueError(f'Missing entries in assembled MCP runtime: {", ".join(sorted(missing))}')
        with zipfile.ZipFile(path) as archive:
            npm_package = json.loads(archive.read('npm/package.json'))
            if npm_package['name'] != 'npm' or npm_package['version'] != node['npmVersion']:
                raise ValueError('Node distribution does not contain the locked npm version')
            playwright_package = json.loads(archive.read(f'{PLAYWRIGHT_DIR}/package.json'))
            if (playwright_package['name'] != 'playwright-core'
                    or playwright_package['version'] != playwright_version):
                raise ValueError('Playwright distribution does not match the Yarn lock')
            if playwright_package.get('bin', {}).get('playwright-core') != 'cli.js':
                raise ValueError('Bundled playwright-core does not expose its CLI at cli.js')
            if any(playwright_package.get(key) for key in ('dependencies', 'optionalDependencies', 'peerDependencies')):
                raise ValueError('Bundled playwright-core must not need external runtime packages')
        digest = hash_file(path)
        # Preserve the timestamp for repeat builds of identical bytes (Cargo watches this directory).
        if not OUTPUT.is_file() or hash_file(OUTPUT) != digest:
            path.replace(OUTPUT)
    print(f'{OUTPUT.relative_to(ROOT)}: SHA-256 {digest}')


def seven_zip_files(lock, target):
    if target not in lock['targets']:
        raise ValueError(f'Unsupported 7-Zip target {target!r}. Supported: {", ".join(lock["targets"])}')
    source = lock['source']
    return {
        **lock['targets'][target]['files'],
        source['asset']: source['sha256'],
        'COPYING.txt': source['copyingSha256'],
    }


def check_seven_zip(directory, files):
    for name, digest in files.items():
        path = directory / name
        if path.is_symlink() or not path.is_file() or hash_file(path) != digest:
            raise ValueError(f'Missing or unofficial 7-Zip resource: {path}')
        if name == '7zz' and os.name != 'nt' and not os.access(path, os.X_OK):
            raise ValueError(f'7-Zip resource is not executable: {path}')


def copy_seven_zip_tar_files(archive, directory, members):
    with tarfile.open(archive, 'r:xz') as source:
        for entry, name in members.items():
            member = source.getmember(entry)
            if not member.isfile():
                raise ValueError(f'Expected regular file {entry!r} in {archive.name}')
            path = directory / name
            with source.extractfile(member) as stream, path.open('wb') as output:
                shutil.copyfileobj(stream, output)
            if name == '7zz':
                path.chmod(0o755)


def seven_zip_asset(lock, asset):
    # These hashes correspond to the versioned ip7z/7zip release assets, not /latest.
    return download(f'https://github.com/ip7z/7zip/releases/download/{lock["version"]}/{asset["asset"]}',
                    asset['sha256'])


def unix_seven_zip_cli(target):
    return '7zzs' if '-linux-' in target else '7zz'


def run_seven_zip(program, args, cwd):
    result = subprocess.run([str(program), *args], cwd=cwd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=60,
                            creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
    if result.returncode:
        raise ValueError(f'{program.name} exited {result.returncode}: {result.stdout.decode(errors="replace")}')
    return result.stdout


def prepare_seven_zip(lock, target):
    files = seven_zip_files(lock, target)
    try:
        check_seven_zip(SEVEN_ZIP_OUTPUT, files)
    except ValueError:
        CACHE.mkdir(parents=True, exist_ok=True)
        # Build outside the resources directory. Publish only locally verified output.
        with tempfile.TemporaryDirectory(prefix='seven-zip-', dir=CACHE) as work:
            stage = Path(work)
            asset = seven_zip_asset(lock, lock['targets'][target])
            if '7z.exe' in files:
                # The upstream Windows installer is a self-extracting 7z archive.
                # Extract selected files without running the installer or installing anything.
                if os.name == 'nt':
                    extractor = seven_zip_asset(lock, lock['bootstrap'])
                else:
                    host = host_target()
                    host_asset = seven_zip_asset(lock, lock['targets'][host])
                    copy_seven_zip_tar_files(host_asset, stage, {unix_seven_zip_cli(host): '7zz'})
                    extractor = stage / '7zz'
                run_seven_zip(extractor, ['e', '-y', '-spd', f'-o{stage}', '--', str(asset),
                                          '7z.exe', '7z.dll', 'License.txt'], cwd=stage)
            else:
                copy_seven_zip_tar_files(asset, stage, {unix_seven_zip_cli(target): '7zz', 'License.txt': 'License.txt'})
            source = lock['source']
            source_archive = seven_zip_asset(lock, source)
            shutil.copyfile(source_archive, stage / source['asset'])
            copy_seven_zip_tar_files(source_archive, stage, {'DOC/copying.txt': 'COPYING.txt'})
            check_seven_zip(stage, files)
            SEVEN_ZIP_OUTPUT.mkdir(parents=True, exist_ok=True)
            for name in files:
                (stage / name).replace(SEVEN_ZIP_OUTPUT / name)
    # A switched target must not include the other platform's binaries in the bundle.
    for name in {'7z.exe', '7z.dll', '7zz'} - files.keys():
        (SEVEN_ZIP_OUTPUT / name).unlink(missing_ok=True)
    print(f'{SEVEN_ZIP_OUTPUT.relative_to(ROOT)}: verified 7-Zip {lock["version"]} for {target}')


def verify_seven_zip(lock, target, resource_dir):
    directory = resource_dir.resolve(strict=True) / 'seven-zip'
    files = seven_zip_files(lock, target)
    check_seven_zip(directory, files)
    cli = '7z.exe' if '7z.exe' in files else '7zz'
    info = run_seven_zip(directory / cli, ['i'], cwd=directory).decode(errors='replace')
    # Unix builds identify themselves as "7-Zip (z)" rather than "7-Zip".
    banner = rf'^7-Zip(?: \(z\))? {re.escape(lock["version"])} '
    if not re.search(banner, info, re.MULTILINE) or 'Formats:' not in info:
        raise ValueError(f'Unexpected 7-Zip CLI output from {directory / cli}')
    # Some 7-Zip distributions omit the RAR unpackers. Check the packaged binary.
    for handler in ('Rar', 'Rar5'):
        if not re.search(rf'^\s*\d+\s+\S+\s+{handler}\s+rar\s', info, re.MULTILINE):
            raise ValueError(f'Missing {handler} unpacker in {directory / cli}')
    print(f'{directory}: ran verified 7-Zip {lock["version"]} for {target}')


def main():
    parser = argparse.ArgumentParser(description='Prepare pinned desktop CLI runtime resources.')
    parser.add_argument('--target', help='Rust target triple; defaults to TAURI_ENV_TARGET_TRIPLE or native host')
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--seven-zip-only', action='store_true', help='Prepare 7-Zip instead of the default MCP runtime')
    mode.add_argument('--verify-seven-zip', type=Path, metavar='RESOURCE_DIR',
                      help='Check installed 7-Zip files and run the native CLI offline')
    args = parser.parse_args()
    use_seven_zip = args.seven_zip_only or args.verify_seven_zip is not None
    runtime = '7-Zip' if use_seven_zip else 'MCP runtime'
    try:
        target = args.target or os.environ.get('TAURI_ENV_TARGET_TRIPLE') or host_target()
        if use_seven_zip:
            lock = json.loads(SEVEN_ZIP_LOCK.read_text(encoding='utf-8'))
            if args.verify_seven_zip is not None:
                verify_seven_zip(lock, target, args.verify_seven_zip)
            else:
                prepare_seven_zip(lock, target)
        else:
            prepare(target)
    except (OSError, ValueError, RuntimeError, tarfile.TarError, zipfile.BadZipFile,
            subprocess.TimeoutExpired) as error:
        parser.exit(1, f'{runtime} setup failed: {error}\n')


if __name__ == '__main__':
    main()
