"""Prepare the two text inputs for the existing AMM desktop; never start it or the MCP."""
from __future__ import annotations

import argparse
import base64
import configparser
import hashlib
import io
import json
import stat
import sys
import zipfile
from email.parser import BytesParser
from pathlib import Path

BASE = Path(__file__).resolve().parent
REPO = BASE.parent.parent
WHEEL = Path('tools/gta-rpf-mcp/dist/gta_rpf_mcp-1.0.0-py3-none-any.whl')
ARGS = ['--offline', '--no-index', '--from', './' + WHEEL.as_posix(), 'gta-rpf-mcp']
FILES = ('adapter.json', 'SKILL.md')
GAMES = {'gta5-legacy': ('271590', 'GTA5.exe'), 'gta5-enhanced': ('3240220', 'GTA5_Enhanced.exe')}
TYPES = {'scripthookv', 'asi', 'script', 'game-root', 'rpf-gameconfig', 'rpf-dlc', 'rpf-vehicle', 'rpf-ped'}


def fixed_path(root: Path, relative: Path) -> Path:
    """Reject symlink/reparse escapes (including intermediate Windows junctions)."""
    if relative.is_absolute() or '..' in relative.parts:
        raise ValueError('not a fixed relative path')
    path = root
    for part in (None, *relative.parts):
        path = path if part is None else path / part
        flags = path.lstat()
        if stat.S_ISLNK(flags.st_mode) or getattr(flags, 'st_file_attributes', 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT:
            raise ValueError(f'linked path: {path}')
    return path


def blob(root: Path, relative: Path, limit: int) -> bytes:
    path = fixed_path(root, relative)
    details = path.stat()
    if not stat.S_ISREG(details.st_mode) or details.st_nlink != 1 or details.st_size > limit:
        raise ValueError(f'expected a bounded regular file: {path}')
    with path.open('rb') as stream:
        data = stream.read(limit + 1)
    if len(data) != details.st_size:
        raise ValueError(f'file changed during read: {path}')
    return data


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def local_reference(repo: Path) -> tuple[str, str]:
    data = blob(repo, WHEEL, 8 * 1024 * 1024)
    # Inspect only two inert records. Do not install/import the wheel or extract archive paths.
    info = 'gta_rpf_mcp-1.0.0.dist-info/'
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        names = archive.namelist()
        if len(names) != len(set(names)) or len(names) > 256:
            raise ValueError('unexpected wheel inventory')
        def record(name):
            if archive.getinfo(info + name).file_size > 32768:
                raise ValueError('oversized wheel metadata')
            return archive.read(info + name)
        meta = BytesParser().parsebytes(record('METADATA'))
        entry = configparser.ConfigParser(interpolation=None)
        entry.read_string(record('entry_points.txt').decode('utf-8'))
        if (meta.get_all('Name') != ['gta-rpf-mcp'] or meta.get_all('Version') != ['1.0.0']
                or meta.get_all('Requires-Dist') or 'gta_rpf_mcp/server.py' not in names
                or entry.get('console_scripts', 'gta-rpf-mcp') != 'gta_rpf_mcp.server:main'):
            raise ValueError('wrong wheel identity, dependencies or entry point')
    sha = digest(data)
    return f'gta-rpf-mcp @ {(repo / WHEEL).as_uri()}#sha256={sha}', sha


def source(slug: str, base: Path = BASE):
    if slug not in GAMES:
        raise ValueError('unknown slug')
    if {p.name for p in fixed_path(base, Path(slug)).iterdir()} != set(FILES):
        raise ValueError('a package must contain exactly adapter.json and SKILL.md')
    j = json.loads(blob(base, Path(slug, FILES[0]), 65536)); skill = blob(base, Path(slug, FILES[1]), 65536)
    appid, marker = GAMES[slug]
    if (j['slug'] != slug or j['schemaVersion'] != 1 or j['version'] != '1.0.0' or j['platforms'] != ['windows']
            or j['steamAppId'] != appid or j['markers'] != [dict(path=marker, kind='file', required=True, platforms=['windows'])]
            or not skill.decode('utf-8').startswith(f'---\nname: {slug}\n')):
        raise ValueError('wrong package identity or marker')
    rules = j['archiveRules']; types = j['modTypes']
    if (len(rules) != len(TYPES) or {r['modType'] for r in rules} != TYPES or any(r['layout'] != 'manual' for r in rules)
            or len(types) != len(TYPES) or {t['id'] for t in types} != TYPES):
        raise ValueError('unexpected installation rules')
    server, = j['mcpServers']
    if (set(server) != {'id', 'name', 'transport', 'command', 'args'} or server['id'] != 'rpf'
            or server['transport'] != 'stdio' or server['command'] != 'uvx' or server['args'] != ARGS):
        raise ValueError('unexpected MCP declaration')
    return j, skill


def render(slug: str, repo: Path = REPO, base: Path = BASE):
    j, skill = source(slug, base)
    requirement, sha = local_reference(repo)
    j['mcpServers'][0]['args'][3] = requirement
    result = (json.dumps(j, ensure_ascii=False, indent=2) + '\n').encode('utf-8')
    return {'adapter.json': result, 'SKILL.md': skill}, sha


def verify(slug: str, data_root: Path, files: dict[str, bytes]):
    for name, expected in files.items():
        actual = blob(data_root, Path('plugins', 'games', slug, name), 65536)
        if digest(expected) != digest(actual):
            raise ValueError(f'{slug}/{name}: installed bytes differ (expected {digest(expected)}, observed {digest(actual)})')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subs = parser.add_subparsers(dest='action', required=True)
    subs.add_parser('check', help='check source packages only, including when the wheel is not yet built')
    emit = subs.add_parser('emit', help='write exact UTF-8 field to stdout for the native draft editor')
    emit.add_argument('slug', choices=tuple(GAMES)); emit.add_argument('field', choices=FILES)
    emit.add_argument('--base64', action='store_true', help='single ASCII line for PowerShell/clipboard (no newline conversion)')
    subs.add_parser('hashes', help='read wheel; print expected local draft/install fingerprints')
    readback = subs.add_parser('verify-installed', help='compare installation files; registration/binding require desktop readback')
    readback.add_argument('data_root', type=Path); readback.add_argument('slug', choices=tuple(GAMES))
    args = parser.parse_args()
    if args.action == 'check':
        for slug in GAMES:
            source(slug); print(f'{slug}: source contract OK (not AMM validation)')
    elif args.action == 'emit':
        files, _ = render(args.slug)
        data = files[args.field]
        sys.stdout.buffer.write(base64.b64encode(data) if args.base64 else data)
    elif args.action == 'hashes':
        for slug in GAMES:
            files, sha = render(slug)
            print(f'{slug}: wheel {sha}; ' + '; '.join(f'{k} {digest(v)}' for k, v in files.items()))
    else:
        if not args.data_root.is_absolute():
            raise ValueError('supply the absolute AMM data root shown in the desktop')
        files, _ = render(args.slug); verify(args.slug, args.data_root, files)
        print(f'{args.slug}: both installed files match; this does not prove binding or runtime support')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, KeyError, zipfile.BadZipFile, configparser.Error) as exc:
        sys.exit(f'Not ready: {exc}')
