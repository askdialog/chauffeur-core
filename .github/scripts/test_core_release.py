import hashlib
import importlib.util
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('core_release', ROOT / 'scripts/core_release.py')
core = importlib.util.module_from_spec(spec)
spec.loader.exec_module(core)


class CoreReleaseTests(unittest.TestCase):
    def test_release_requires_one_version_and_no_private_source(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            files = {'Cargo.toml': '[workspace.package]\nversion="0.37.0"',
                     'sidecar/src/version.ts': 'export const SIDECAR_VERSION = "0.37.0";',
                     'app/src/state/app.ts': 'clientName: "chauffeur-app 0.37.0"'}
            for name in ('app/package.json', 'sidecar/package.json', 'app/src-tauri/tauri.conf.json'):
                files[name] = '{"version":"0.37.0"}'
            for name, content in files.items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content)
            self.assertEqual(core.validate(root, '0.37.0'), '0.37.0')
            with self.assertRaisesRegex(ValueError, 'not requested'):
                core.validate(root, '0.36.0')
            (root / 'distribution').mkdir()
            with self.assertRaisesRegex(ValueError, 'public source'):
                core.validate(root)
            (root / 'distribution').rmdir()
            (root / 'app/package.json').write_text('{"version":"0.30.0"}')
            with self.assertRaisesRegex(ValueError, 'Synchronize'):
                core.validate(root)

    def github(self, *, draft=None):
        """A fake `gh`: the releases of the repository, answering 404 to a draft looked up by tag."""
        releases = [draft] if draft else []
        calls = []

        def run(*args):
            calls.append(args)
            if args[1:3] == ('api', '--paginate'):
                return json.dumps([releases])
            if args[1:3] == ('api', '--method'):
                return '{}'
            if args[1] == 'api' and '/matching-refs/' in args[2]:
                return '[]'
            if args[1] == 'api' and '/contents/' in args[2]:
                return '[workspace.package]\nversion="0.37.0"'
            if args[1] == 'api' and '/commits/' in args[2]:
                return json.dumps({'sha': 'a' * 40})
            if args[1] == 'api' and '/releases/tags/' in args[2]:
                # GitHub does not return a draft by its tag.
                found = [r for r in releases if not r['draft'] and args[2].endswith('/' + r['tag_name'])]
                if not found:
                    raise subprocess.CalledProcessError(1, args)
                return json.dumps(found[0])
            if args[1:3] == ('release', 'create'):
                releases.append(self.release(draft=True))
            if args[1:3] == ('release', 'edit'):
                releases[0]['draft'] = False
            return ''
        return run, calls, releases

    def release(self, *, draft):
        return {'tag_name': 'core-v0.37.0', 'draft': draft, 'assets': [
            {'name': p.name, 'size': p.stat().st_size,
             'digest': 'sha256:' + hashlib.sha256(p.read_bytes()).hexdigest()} for p in core.assets('0.37.0')]}

    def packages(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        before = Path.cwd()
        os.chdir(folder.name)
        self.addCleanup(os.chdir, before)
        paths = core.assets('0.37.0')
        for archive, checksum in zip(paths[::2], paths[1::2]):
            archive.parent.mkdir(parents=True, exist_ok=True)
            archive.write_bytes(b'package')
            checksum.write_text(f'{hashlib.sha256(b"package").hexdigest()}  {archive.name}\n')
        return paths

    def test_core_publishes_its_own_tag_without_replacing_the_legacy_latest(self):
        paths = self.packages()
        run, calls, releases = self.github()
        with patch.object(core, 'run', side_effect=run):
            core.publish('0.37.0', 'a' * 40, Path('notes.md'))
        mutations = [c for c in calls if c[1] == 'release']
        self.assertEqual(len(mutations), 2)
        for call in mutations:
            self.assertIn('core-v0.37.0', call)
            self.assertIn('--latest=false', call)
        self.assertFalse(releases[0]['draft'])
        paths[0].write_bytes(b'tampered')
        with self.assertRaisesRegex(ValueError, 'Invalid Core checksum'):
            core.verify_assets('0.37.0')

    def test_a_draft_left_by_an_earlier_run_is_published_without_creating_another(self):
        self.packages()
        run, calls, releases = self.github(draft=self.release(draft=True))
        with patch.object(core, 'run', side_effect=run):
            core.publish('0.37.0', 'a' * 40, Path('notes.md'))
        self.assertEqual([c[2] for c in calls if c[1] == 'release'], ['edit'])
        self.assertEqual(len(releases), 1)
        self.assertFalse(releases[0]['draft'])

    def test_a_published_core_release_is_left_as_it_is(self):
        self.packages()
        run, calls, releases = self.github(draft=self.release(draft=False))
        with patch.object(core, 'run', side_effect=run):
            core.publish('0.37.0', 'a' * 40, Path('notes.md'))
        self.assertEqual([c for c in calls if c[1] == 'release'], [])

    def test_a_core_draft_with_other_files_is_not_published(self):
        self.packages()
        draft = self.release(draft=True)
        draft['assets'].pop()
        run, calls, _ = self.github(draft=draft)
        with patch.object(core, 'run', side_effect=run), self.assertRaisesRegex(ValueError, 'missing or unexpected'):
            core.publish('0.37.0', 'a' * 40, Path('notes.md'))
        self.assertEqual([c for c in calls if c[1] == 'release'], [])

    def test_core_can_never_publish_a_different_source_version(self):
        with patch.object(core, 'verify_assets', return_value=[]), patch.object(core, 'run', return_value='[workspace.package]\nversion="0.30.0"'):
            with self.assertRaisesRegex(ValueError, 'source'):
                core.publish('0.37.0', 'a' * 40, Path('notes.md'))
