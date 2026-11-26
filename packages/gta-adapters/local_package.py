"""Prepare the two text inputs for the existing AMM desktop; never start it or the MCP."""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import stat
import sys
from pathlib import Path

BASE = Path(__file__).resolve().parent
ADAPTER_VERSION = '1.1.0'
ARGS = ['gta-rpf-mcp@latest']
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


def source(slug: str, base: Path = BASE) -> dict[str, bytes]:
    if slug not in GAMES:
        raise ValueError('unknown slug')
    if {p.name for p in fixed_path(base, Path(slug)).iterdir()} != set(FILES):
        raise ValueError('a package must contain exactly adapter.json and SKILL.md')
    files = {name: blob(base, Path(slug, name), 65536) for name in FILES}
    text = {name: content.decode('utf-8') for name, content in files.items()}
    for name, content in text.items():
        if '\r' in content:
            raise ValueError(f'{slug}/{name}: expected LF line endings for byte-exact draft input')
    adapter = json.loads(text['adapter.json'])
    skill = text['SKILL.md']
    appid, marker = GAMES[slug]
    expected_marker = dict(path=marker, kind='file', required=True, platforms=['windows'])
    if (
        adapter['slug'] != slug
        or adapter['schemaVersion'] != 1
        or adapter['version'] != ADAPTER_VERSION
        or adapter['platforms'] != ['windows']
        or adapter['steamAppId'] != appid
        or adapter['markers'] != [expected_marker]
        or not skill.startswith(f'---\nname: {slug}\n')
    ):
        raise ValueError('wrong package identity or marker')
    rules = adapter['archiveRules']
    types = adapter['modTypes']
    if (len(rules) != len(TYPES) or {r['modType'] for r in rules} != TYPES or any(r['layout'] != 'manual' for r in rules)
            or len(types) != len(TYPES) or {t['id'] for t in types} != TYPES):
        raise ValueError('unexpected installation rules')
    server, = adapter['mcpServers']
    if (set(server) != {'id', 'name', 'transport', 'command', 'args'} or server['id'] != 'rpf'
            or server['transport'] != 'stdio' or server['command'] != 'uvx' or server['args'] != ARGS):
        raise ValueError('unexpected MCP declaration')
    return files


def verify(slug: str, data_root: Path, files: dict[str, bytes]):
    for name, expected in files.items():
        actual = blob(data_root, Path('plugins', 'games', slug, name), 65536)
        if digest(expected) != digest(actual):
            raise ValueError(f'{slug}/{name}: installed bytes differ (expected {digest(expected)}, observed {digest(actual)})')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subs = parser.add_subparsers(dest='action', required=True)
    subs.add_parser('check', help='validate both source package contracts')
    emit = subs.add_parser('emit', help='write exact UTF-8 field to stdout for the native draft editor')
    emit.add_argument('slug', choices=tuple(GAMES)); emit.add_argument('field', choices=FILES)
    emit.add_argument('--base64', action='store_true', help='single ASCII line for PowerShell/clipboard (no newline conversion)')
    subs.add_parser('hashes', help='print expected draft/install fingerprints')
    readback = subs.add_parser('verify-installed', help='compare installation files; registration/binding require desktop readback')
    readback.add_argument('data_root', type=Path); readback.add_argument('slug', choices=tuple(GAMES))
    args = parser.parse_args()
    if args.action == 'check':
        for slug in GAMES:
            source(slug)
            print(f'{slug}: source contract OK (not AMM validation)')
    elif args.action == 'emit':
        data = source(args.slug)[args.field]
        sys.stdout.buffer.write(base64.b64encode(data) if args.base64 else data)
    elif args.action == 'hashes':
        for slug in GAMES:
            files = source(slug)
            print(f'{slug}: ' + '; '.join(f'{name} {digest(data)}' for name, data in files.items()))
    else:
        if not args.data_root.is_absolute():
            raise ValueError('supply the absolute AMM data root shown in the desktop')
        verify(args.slug, args.data_root, source(args.slug))
        print(f'{args.slug}: both installed files match; this does not prove binding or runtime support')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, KeyError) as exc:
        sys.exit(f'Not ready: {exc}')
