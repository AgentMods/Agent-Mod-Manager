"""Contract tests. Fixtures are disposable and never reference a real GTA/AMM data root."""
import json
import tempfile
import unittest
from pathlib import Path

from local_package import BASE, FILES, GAMES, source, verify


class Packages(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(); self.addCleanup(temp.cleanup)
        self.root = Path(temp.name).resolve()

    def test_different_required_file_not_a_shared_launcher(self):
        markers = {slug: json.loads(source(slug)['adapter.json'])['markers'][0]['path'] for slug in GAMES}
        for slug, (app, exe) in GAMES.items():
            directory = self.root / app; directory.mkdir()
            (directory / 'PlayGTAV.exe').touch()
            def matches():
                return {name for name, marker in markers.items() if (directory / marker).is_file()}
            self.assertEqual(matches(), set())
            (directory / exe).mkdir(); self.assertFalse(matches()); (directory / exe).rmdir()
            (directory / exe).touch(); self.assertEqual(matches(), {slug})

    def test_staged_only_service_and_root_contract(self):
        runtime_ids = set()
        for slug in GAMES:
            files = source(slug)
            doc = json.loads(files['adapter.json'])
            markdown = files['SKILL.md']
            runtime_ids.add(f"plugin_{slug}_{doc['mcpServers'][0]['id']}")
            roots = {r['id']: r['relativePath'] for r in doc['installRoots']}
            for mod in doc['modTypes']:
                self.assertEqual(roots[mod['root']], 'mods' if mod['id'].startswith('rpf-') else 'scripts' if mod['id']=='script' else '.')
            self.assertIn(b'inspectJobId', markdown); self.assertIn(b'rpf_job_id', markdown)
            self.assertEqual({p.name for p in (BASE/slug).iterdir()},set(FILES))
        self.assertEqual(runtime_ids, {'plugin_gta5-legacy_rpf','plugin_gta5-enhanced_rpf'})

    def test_skill_uses_the_returned_namespaced_job_tool(self):
        for slug in GAMES:
            with self.subTest(slug=slug):
                markdown = source(slug)['SKILL.md'].decode('utf-8')
                for name in (
                    'mcpTool', 'gta_rpf_inspect_dlc', 'gta_rpf_register_dlc',
                    'gta_rpf_replace_gameconfig', 'gta_rpf_prepare',
                    'gta_rpf_list_entries', 'gta_rpf_read_entry',
                    'gta_rpf_write_entry', 'gta_rpf_add_entry',
                    'gta_rpf_delete_entry', 'gta_rpf_create_archive',
                ):
                    self.assertIn(f'`{name}`', markdown)
                self.assertIn('prepare_gta_rpf_archive_job', markdown)
                self.assertIn('mods/update/update.rpf/common/data/dlclist.xml', markdown)
                self.assertIn('`gta_rpf_describe_job({jobId})`', markdown)
                for name in ('gta_rpf_build_vehicle_pack', 'gta_rpf_build_ped_pack'):
                    self.assertEqual(f'`{name}`' in markdown, slug == 'gta5-legacy')

    def test_latest_declaration_and_readback_are_byte_exact(self):
        for slug in GAMES:
            with self.subTest(slug=slug):
                files = source(slug)
                adapter = json.loads(files['adapter.json'])
                self.assertEqual(adapter['version'], '1.1.0')
                mcp = adapter['mcpServers'][0]
                self.assertEqual(mcp['transport'], 'stdio')
                self.assertEqual(mcp['command'], 'uvx')
                self.assertEqual(mcp['args'], ['gta-rpf-mcp@latest'])
                folder = self.root / 'plugins' / 'games' / slug
                folder.mkdir(parents=True)
                for name, content in files.items():
                    self.assertEqual(content, (BASE / slug / name).read_bytes())
                    (folder / name).write_bytes(content)
                verify(slug, self.root, files)
                for name, content in files.items():
                    (folder / name).write_bytes(content.replace(b'\n', b'\r\n'))
                    with self.assertRaisesRegex(ValueError, f'{slug}/{name}: installed bytes differ'):
                        verify(slug, self.root, files)
                    (folder / name).write_bytes(content)

    def test_reject_crlf_source_files(self):
        slug = 'gta5-legacy'
        files = source(slug)
        folder = self.root / slug
        folder.mkdir()
        for name, content in files.items():
            (folder / name).write_bytes(content)
        for name, content in files.items():
            with self.subTest(name=name):
                path = folder / name
                path.write_bytes(content.replace(b'\n', b'\r\n'))
                with self.assertRaisesRegex(ValueError, f'{slug}/{name}: expected LF line endings'):
                    source(slug, self.root)
                path.write_bytes(content)

    def test_reject_non_utf8_manifest(self):
        slug = 'gta5-legacy'
        files = source(slug)
        folder = self.root / slug
        folder.mkdir()
        (folder / 'SKILL.md').write_bytes(files['SKILL.md'])
        (folder / 'adapter.json').write_text(files['adapter.json'].decode('utf-8'), encoding='utf-16')
        with self.assertRaises(UnicodeDecodeError):
            source(slug, self.root)

    def test_reject_old_adapter_version_and_altered_mcp_args(self):
        slug = 'gta5-legacy'
        files = source(slug)
        folder = self.root / slug
        folder.mkdir()
        (folder / 'SKILL.md').write_bytes(files['SKILL.md'])
        adapter = json.loads(files['adapter.json'])
        adapter['version'] = '1.0.2'
        manifest = folder / 'adapter.json'
        manifest.write_text(json.dumps(adapter), encoding='utf-8')
        with self.assertRaisesRegex(ValueError, 'wrong package identity'):
            source(slug, self.root)

        adapter['version'] = '1.1.0'
        for args in (['gta-rpf-mcp'], ['gta-rpf-mcp@1.0.0'], ['--from', './local.whl', 'gta-rpf-mcp']):
            with self.subTest(args=args):
                adapter['mcpServers'][0]['args'] = args
                manifest.write_text(json.dumps(adapter), encoding='utf-8')
                with self.assertRaisesRegex(ValueError, 'unexpected MCP declaration'):
                    source(slug, self.root)


if __name__ == '__main__': unittest.main()
