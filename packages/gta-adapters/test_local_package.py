"""Contract tests. Fixtures are disposable and never reference a real GTA/AMM data root."""
import json
import tempfile
import unittest
import zipfile
from pathlib import Path

from local_package import BASE, FILES, GAMES, WHEEL, digest, local_reference, render, source, verify


class Packages(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(); self.addCleanup(temp.cleanup)
        self.root = Path(temp.name).resolve()

    def wheel(self, version='1.0.0', depends=''):
        path = self.root / WHEEL
        path.parent.mkdir(parents=True, exist_ok=True)
        metadata = 'gta_rpf_mcp-1.0.0.dist-info/'
        with zipfile.ZipFile(path, 'w') as z:
            z.writestr(metadata + 'METADATA', f'Metadata-Version: 2.1\nName: gta-rpf-mcp\nVersion: {version}\n{depends}')
            z.writestr(metadata + 'entry_points.txt', '[console_scripts]\ngta-rpf-mcp = gta_rpf_mcp.server:main\n')
            z.writestr('gta_rpf_mcp/server.py', b'# never executed\n')
        return path

    def test_different_required_file_not_a_shared_launcher(self):
        for slug, (app, exe) in GAMES.items():
            directory = self.root / app; directory.mkdir()
            (directory / 'PlayGTAV.exe').touch()
            def matches():
                return {s for s in GAMES if (directory / source(s)[0]['markers'][0]['path']).is_file()}
            self.assertEqual(matches(), set())
            (directory / exe).mkdir(); self.assertFalse(matches()); (directory / exe).rmdir()
            (directory / exe).touch(); self.assertEqual(matches(), {slug})

    def test_staged_only_service_and_root_contract(self):
        runtime_ids = set()
        for slug in GAMES:
            doc, markdown = source(slug)
            runtime_ids.add(f"plugin_{slug}_{doc['mcpServers'][0]['id']}")
            roots = {r['id']: r['relativePath'] for r in doc['installRoots']}
            for mod in doc['modTypes']:
                self.assertEqual(roots[mod['root']], 'mods' if mod['id'].startswith('rpf-') else 'scripts' if mod['id']=='script' else '.')
            self.assertIn(b'inspectJobId', markdown); self.assertIn(b'rpf_job_id', markdown)
            self.assertEqual({p.name for p in (BASE/slug).iterdir()},set(FILES))
        self.assertEqual(runtime_ids, {'plugin_gta5-legacy_rpf','plugin_gta5-enhanced_rpf'})

    def test_no_wheel_no_network_fallback(self):
        with self.assertRaises(FileNotFoundError): render('gta5-legacy', self.root)

    def test_pin_and_readback_are_byte_exact(self):
        wheel = self.wheel(); expected = digest(wheel.read_bytes())
        declarations = []
        for slug in GAMES:
            files, sha = render(slug, self.root)
            self.assertEqual(expected, sha)
            mcp = json.loads(files['adapter.json'])['mcpServers'][0]; declarations.append(mcp['args'])
            self.assertEqual(mcp['args'], ['--offline', '--no-index', '--from', f'gta-rpf-mcp @ {wheel.as_uri()}#sha256={expected}', 'gta-rpf-mcp'])
            folder = self.root / 'plugins' / 'games' / slug; folder.mkdir(parents=True)
            for name, content in files.items(): (folder/name).write_bytes(content)
            verify(slug, self.root, files)
            (folder/'SKILL.md').write_bytes(files['SKILL.md']+b'\n')
            with self.assertRaisesRegex(ValueError, 'installed bytes differ'): verify(slug, self.root, files)
        self.assertEqual(*declarations)

    def test_reject_mislabeled_or_dependency_bearing_wheel(self):
        for version, dep in [('1.0.1',''), ('1.0.0','Requires-Dist: unexpected\n')]:
            with self.subTest(version=version,dep=dep):
                self.wheel(version,dep)
                with self.assertRaisesRegex(ValueError, 'wrong wheel identity'): local_reference(self.root)


if __name__ == '__main__': unittest.main()
