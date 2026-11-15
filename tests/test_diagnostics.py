import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import diagnostics as d


class DiagnosticsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='tracr-retention-test-')
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)

    def run_case(self, status=0, keep=False, explicit=False):
        run = d.DiagnosticRun('fixture', keep, self.base / 'diagnostics')
        token = d._CURRENT.set(run)
        try:
            path = Path(d.scratch_dir(prefix='tracr-retention-', dir=self.base if explicit else None))
        finally:
            d._CURRENT.reset(token)
        self.addCleanup(lambda: __import__('shutil').rmtree(path, ignore_errors=True))
        (path / 'results.json').write_text('{"status":"example"}')
        (path / 'failure.log').write_text('diagnostic')
        (path / 'rpc.jsonl').write_text('raw protocol')
        (path / 'packages').mkdir()
        (path / 'packages/runtime').write_text('bulky runtime')
        run.finish(status)
        return path

    def test_success_removes_environment(self):
        self.assertFalse(self.run_case().exists())
        self.assertFalse((self.base / 'diagnostics').exists())

    def test_failure_and_blocked_keep_only_reports(self):
        for status in (1, 2):
            path = self.run_case(status)
            self.assertFalse(path.exists())
        reports = list((self.base / 'diagnostics/fixture').glob('*/run.json'))
        self.assertEqual({json.loads(p.read_text())['exit_code'] for p in reports}, {1, 2})
        self.assertEqual(len(list((self.base / 'diagnostics').rglob('results.json'))), 2)
        self.assertFalse(list((self.base / 'diagnostics').rglob('rpc.jsonl')))
        self.assertFalse(list((self.base / 'diagnostics').rglob('runtime')))

    def test_keep_temp_preserves_full_environment(self):
        self.assertTrue((self.run_case(keep=True) / 'packages/runtime').exists())

    def test_explicit_output_keeps_reports_on_success(self):
        path = self.run_case(explicit=True)
        self.assertTrue((path / 'results.json').exists())
        self.assertFalse((path / 'packages').exists())

    def test_pruning_keeps_three_completed_and_leaves_pending(self):
        parent = self.base / 'diagnostics/fixture'
        pending = parent / '.pending-active'
        pending.mkdir(parents=True)
        for _ in range(5):
            self.run_case(1)
        self.assertEqual(len(list(parent.glob('*/run.json'))), 3)
        self.assertTrue(pending.exists())

    def test_child_reaped_before_cleanup(self):
        path = self.base / 'child'; path.mkdir()
        child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'], cwd=path)
        self.addCleanup(lambda: child.poll() is None and child.kill())
        run = d.DiagnosticRun('child', diagnostics=self.base / 'diagnostics')
        run.paths.append((path, False))
        self.assertTrue(d.busy_paths(path))
        run.finish(0)
        self.assertTrue(path.exists())
        def close():
            child.terminate(); child.wait(timeout=5)
        run.closers.append(close)
        run.finish(0)
        self.assertIsNotNone(child.poll())
        self.assertFalse(path.exists())

    def test_sigterm_unwinds_and_retains_report(self):
        ready = self.base / 'ready'
        code = '''import os, sys, time
from pathlib import Path
import diagnostics as d
d.DIAGNOSTICS = Path(sys.argv[2])
# Default constructor argument is bound at import, so supply the destination.
original = d.DiagnosticRun
d.DiagnosticRun = lambda name, keep: original(name, keep, Path(sys.argv[2]))
def main():
    root = Path(d.scratch_dir())
    (root / 'results.json').write_text('{}')
    Path(sys.argv[1]).write_text(str(root))
    time.sleep(30)
d.run_cli(main)
'''
        env = {**os.environ, 'PYTHONPATH': str(Path(d.__file__).parent)}
        child = subprocess.Popen([sys.executable, '-c', code, str(ready), str(self.base / 'diagnostics')],
                                 env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            import time
            deadline = time.monotonic() + 5
            while not ready.exists() and child.poll() is None and time.monotonic() < deadline:
                time.sleep(.01)
            self.assertTrue(ready.exists())
            child.send_signal(signal.SIGTERM)
            child.communicate(timeout=5)
            self.assertFalse(Path(ready.read_text()).exists())
            reports = list((self.base / 'diagnostics').rglob('run.json'))
            self.assertEqual(json.loads(reports[0].read_text())['exit_code'], 130)
        finally:
            if child.poll() is None:child.kill()
            child.communicate()

    def test_cli_retains_exit_codes_and_forwards_keep(self):
        for code in (0, 1, 2):
            with patch.object(sys, 'argv', ['fixture', '--keep-temp']), patch.dict(os.environ, {}, clear=True):
                def main():
                    self.assertEqual(os.environ['TRACR_KEEP_TEMP'], '1')
                    self.assertNotIn('--keep-temp', sys.argv)
                    raise SystemExit(code)
                with self.assertRaises(SystemExit) as caught:
                    d.run_cli(main)
                self.assertEqual(caught.exception.code, code)
                self.assertNotIn('TRACR_KEEP_TEMP', os.environ)

    def test_cleanup_failure_preserves_scratch(self):
        path = self.base / 'failed-cleanup'; path.mkdir()
        run = d.DiagnosticRun('failed-cleanup', diagnostics=self.base / 'diagnostics')
        run.paths.append((path, False))
        def failed_close():
            raise RuntimeError('child did not stop')
        run.closers.append(failed_close)
        run.finish(1)
        self.assertTrue(path.exists())

    def test_evidence_does_not_follow_symlinks_and_bounds_logs(self):
        source = self.base / 'source'; source.mkdir()
        external = self.base / 'external'; external.mkdir()
        (external / 'private.log').write_text('do not copy')
        (source / 'link').symlink_to(external, target_is_directory=True)
        (source / 'alias.log').symlink_to(external / 'private.log')
        (source / 'large.log').write_bytes(b'x' * (3 * 1024 * 1024) + b'end')
        destination = self.base / 'evidence'
        d.copy_reports(source, destination)
        self.assertFalse((destination / 'link').exists())
        self.assertFalse((destination / 'alias.log').exists())
        data = (destination / 'large.log').read_bytes()
        self.assertEqual(len(data), 2 * 1024 * 1024)
        self.assertTrue(data.endswith(b'end'))

    def test_concurrent_completed_runs_do_not_delete_active_scratch(self):
        code = '''from pathlib import Path
import sys
import diagnostics as d
run = d.DiagnosticRun('parallel', diagnostics=Path(sys.argv[1]))
token = d._CURRENT.set(run)
try:
    p = Path(d.scratch_dir())
    (p / 'results.json').write_text('{}')
    run.finish(1)
finally:
    d._CURRENT.reset(token)
'''
        env = {**os.environ, 'PYTHONPATH': str(Path(d.__file__).parent)}
        children = [subprocess.Popen([sys.executable, '-c', code, str(self.base / 'diagnostics')], env=env,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE) for _ in range(6)]
        for child in children:
            out, err = child.communicate(timeout=15)
            self.assertEqual(child.returncode, 0, err.decode())
        self.assertEqual(len(list((self.base / 'diagnostics/parallel').glob('*/run.json'))), 3)


if __name__ == '__main__':
    unittest.main()
