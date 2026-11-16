"""Exercise the real supervisor with disposable sources and a fake compiler/app."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('dev_watch', ROOT / 'scripts/dev_watch.py')
watch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(watch)

APP = '''#!/usr/bin/env python3
import os, pathlib, sys, time
root = pathlib.Path(__file__).resolve().parents[2]
with (root / 'launches').open('a') as out:
    out.write(str(pathlib.Path.cwd()) + '|' + repr(sys.argv[1:]) + '\\n')
if '--json' in sys.argv:
    print('{}')
    sys.exit(0)
control = pathlib.Path(os.environ['TRACR_DEV_CONTROL'])
(control / 'ready').touch()
while not (root / 'quit').exists():
    if (control / 'request').exists() and not (root / 'busy').exists():
        (control / 'ack').touch()
        sys.exit(75)
    time.sleep(.03)
'''


class WatchTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='tracr watcher test ')
        self.root = Path(self.temp.name)
        (self.root / 'src').mkdir()
        (self.root / 'target/debug').mkdir(parents=True)
        (self.root / 'src/main.rs').write_text('initial')
        self.cargo = self.root / 'cargo.py'
        self.cargo.write_text('''import pathlib, sys, time
root = pathlib.Path.cwd()
with (root / 'builds').open('a') as out: out.write('build\\n')
time.sleep(.7)
if (root / 'fail').exists():
    print('intentional compiler failure')
    sys.exit(1)
(root / 'target/debug/tracr').write_text(%r)
(root / 'target/debug/tracr').chmod(0o755)
''' % APP)
        self.caller = self.root / 'caller directory'
        self.caller.mkdir()
        self.process = None
        self.output = tempfile.TemporaryFile()

    def start(self, *args):
        self.process = subprocess.Popen([sys.executable, str(ROOT / 'scripts/dev_watch.py'),
            str(self.root), str(self.caller), sys.executable, str(self.cargo), '--', *args],
            stdout=self.output, stderr=self.output)

    def count(self, name):
        path = self.root / name
        return len(path.read_text().splitlines()) if path.exists() else 0

    def wait(self, predicate):
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            if predicate():
                return
            time.sleep(.05)
        self.fail('Timed out waiting for supervisor')

    def tearDown(self):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            self.process.wait(timeout=5)
        self.output.close()
        self.temp.cleanup()

    def test_snapshot_tracks_create_delete_ignores_output(self):
        initial = watch.snapshot(self.root)
        (self.root / 'target/build.log').write_text('ignored')
        self.assertEqual(initial, watch.snapshot(self.root))
        (self.root / 'src/new.rs').write_text('new')
        self.assertNotEqual(initial, watch.snapshot(self.root))
        (self.root / 'src/new.rs').unlink()
        self.assertEqual(initial, watch.snapshot(self.root))

    def test_failure_recovery_deferred_reload_and_quit(self):
        self.start('browse', 'folder with spaces')
        self.wait(lambda: self.count('launches') == 1)
        (self.root / 'fail').touch()
        (self.root / 'src/main.rs').write_text('broken')
        self.wait(lambda: self.count('builds') == 2)
        time.sleep(1.2)
        self.assertEqual(self.count('launches'), 1)
        (self.root / 'busy').touch()
        (self.root / 'fail').unlink()
        (self.root / 'src/new.rs').write_text('fixed')
        self.wait(lambda: self.count('builds') == 3)
        time.sleep(1.2)
        self.assertEqual(self.count('launches'), 1)
        (self.root / 'busy').unlink()
        self.wait(lambda: self.count('launches') == 2)
        self.assertIn(str(self.caller), (self.root / 'launches').read_text())
        self.assertIn("['browse', 'folder with spaces']", (self.root / 'launches').read_text())
        (self.root / 'quit').touch()
        self.assertEqual(self.process.wait(timeout=5), 0)

    def test_edit_during_build_is_queued(self):
        self.start()
        self.wait(lambda: self.count('builds') == 1)
        (self.root / 'src/main.rs').write_text('edited during compilation')
        self.wait(lambda: self.count('builds') == 2)
        self.wait(lambda: self.count('launches') == 1)
        (self.root / 'quit').touch()
        self.assertEqual(self.process.wait(timeout=5), 0)

    def test_one_shot_exits(self):
        self.start('list', '--json')
        self.assertEqual(self.process.wait(timeout=8), 0)
        self.assertEqual(self.count('launches'), 1)
        self.output.seek(0)
        self.assertIn(b'{}\n', self.output.read())


if __name__ == '__main__':
    unittest.main()
