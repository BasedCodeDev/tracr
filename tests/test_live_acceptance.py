import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

helpers = Path(__file__).parent/'fixtures/app-server'
sys.path.insert(0, str(helpers))
spec = importlib.util.spec_from_file_location('real_check', helpers/'real_check.py')
live = importlib.util.module_from_spec(spec)
spec.loader.exec_module(live)

class AcceptanceTests(unittest.TestCase):
    def test_exit_acknowledges_composer_clear_before_confirmation(self):
        class ExitTerminal:
            def __init__(self, replies):
                self.output = bytearray(b'Ask Codex to do anything')
                self.replies = iter(replies)
                self.sent = 0
            def send(self, data):
                self.assert_interrupt(data)
                self.sent += 1
                self.output.extend(next(self.replies))
            def assert_interrupt(self, data):
                assert data == b'\x03'
            def wait(self, predicate):
                assert predicate(), 'unrecognized interrupt response'
        for replies in ([b'Ask Codex to do anything', b'Press Ctrl-C again', b'\x1b[1;1Htracr'],
                        [b'Press Ctrl-C again', b'\x1b[1;1Htracr'],
                        [b'\x1b[1;1Htracr']):
            terminal = ExitTerminal(replies)
            live.return_to_browser(terminal)
            self.assertEqual(terminal.sent, len(replies))
        for replies in ([b''], [b'Ask Codex to do anything', b'Ask Codex to do anything'],
                        [b'Press Ctrl-C again', b'still in Codex']):
            with self.assertRaises(AssertionError):
                live.return_to_browser(ExitTerminal(replies))

    def test_history_uses_fixture_messages_not_turn_completion_marker(self):
        expected = ['Remember the project.', 'The project is here.']
        self.assertFalse(live.history_visible('Synthetic fixture complete.', expected))
        self.assertFalse(live.history_visible(expected[0], expected))
        self.assertTrue(live.history_visible('Remember the project. The project is here.', expected))
        self.assertFalse(live.history_visible('anything', []))

    def test_cleanup_waits_for_async_shutdown_but_still_rejects_survivors(self):
        import types
        for clock, processes, succeeds in [
                ([0, 6, 6, 7, 7], [[10], [10], []], True),
                ([0, 31, 31], [[10], [10]], False)]:
            with self.subTest(succeeds=succeeds), tempfile.TemporaryDirectory() as temp, \
                    patch.object(live, 'owned_daemons', side_effect=processes), \
                    patch.object(live.subprocess, 'run', return_value=types.SimpleNamespace(returncode=0, stdout='stopped', stderr='')), \
                    patch.object(live.time, 'monotonic', side_effect=clock), \
                    patch.object(live.time, 'sleep'):
                if succeeds:
                    self.assertEqual(live.stop_daemons('/codex', {}, Path(temp))['shutdown_seconds'], 7)
                else:
                    with self.assertRaisesRegex(AssertionError, 'survived 30-second'):
                        live.stop_daemons('/codex', {}, Path(temp))

    def test_status_submission_waits_for_each_composer_edit(self):
        class Composer:
            def __init__(self):
                self.prefix = ''
                self.output = bytearray()
                self.acknowledged = True
                self.submitted = False
            def send(self, data):
                assert self.acknowledged
                if data == b'\r':
                    assert self.prefix == '/status'
                    self.submitted = True
                else:
                    self.prefix += data.decode()
                    self.output.extend(data)
                    self.acknowledged = False
            def text(self):
                return '› ' + self.prefix
            def wait(self, predicate):
                assert predicate()
                self.acknowledged = True
        composer = Composer()
        live.submit_status(composer)
        self.assertTrue(composer.submitted)

    def test_updater_cleanup_checks_identity_and_only_signals_owned_process(self):
        import json
        import os
        import subprocess
        with tempfile.TemporaryDirectory() as temp:
            home = Path(temp)
            folder = home/'codex/app-server-daemon'
            folder.mkdir(parents=True)
            owned = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)', 'app-server'],
                                     env={**os.environ, 'CODEX_HOME':str(home/'codex')})
            unrelated = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)', 'app-server'],
                                         env={**os.environ, 'CODEX_HOME':str(home/'other')})
            try:
                stat = (Path('/proc')/str(owned.pid)/'stat').read_text().rsplit(')',1)[1].split()
                record = {'pid':owned.pid, 'processIdentity':{'startTicks':int(stat[19])+1,
                    'bootId':Path('/proc/sys/kernel/random/boot_id').read_text().strip()}}
                (folder/'daemon-updater.pid').write_text(json.dumps(record))
                with self.assertRaisesRegex(AssertionError, 'identity changed'):
                    live.stop_owned_updater(home)
                self.assertIsNone(owned.poll())
                record['processIdentity']['startTicks'] -= 1
                (folder/'daemon-updater.pid').write_text(json.dumps(record))
                self.assertEqual(live.stop_owned_updater(home), owned.pid)
                owned.wait(timeout=5)
                self.assertIsNone(unrelated.poll())
            finally:
                for process in [owned, unrelated]:
                    if process.poll() is None:
                        process.terminate()
                    process.wait(timeout=5)

    def test_failure_precedence(self):
        self.assertEqual(live.exit_code([{'status':'passed'}]), 0)
        self.assertEqual(live.exit_code([{'status':'blocked'}]), 2)
        self.assertEqual(live.exit_code([{'status':'blocked'}, {'status':'failed'}]), 1)
        self.assertEqual(live.failure(AssertionError('wrong thread')), 'failed')
        self.assertEqual(live.failure(TimeoutError('timeout')), 'failed')
        self.assertEqual(live.failure(live.Blocked('trust')), 'blocked')

    def test_startup_rejection_is_narrow(self):
        self.assertIsNotNone(live.startup_blocker('app-server socket: Operation not permitted'))
        self.assertIsNone(live.startup_blocker('Something else: Operation not permitted'))
        self.assertIsNotNone(live.startup_blocker('Do you trust this folder?'))

    def test_status_checks_identity_and_directory(self):
        live.verify_status('Session: abc Directory: /tmp/project', 'abc', '/tmp/project')
        for text in ['Session: other Directory: /tmp/project', 'Session: abc Directory: /tmp/other', 'welcome']:
            with self.assertRaises(AssertionError):
                live.verify_status(text, 'abc', '/tmp/project')

    def test_inventory_checks_metadata(self):
        row = {'key':{'native_id':'abc'}, 'cwd':'/tmp/project', 'archived':False}
        live.verify_inventory({'complete':True, 'sessions':[row]}, 'abc', '/tmp/project')
        for mutation in [{'cwd':'/other'}, {'archived':True}, {'key':{'native_id':'other'}}]:
            with self.assertRaises(AssertionError):
                live.verify_inventory({'complete':True, 'sessions':[{**row, **mutation}]}, 'abc', '/tmp/project')

    def test_readiness_and_stale_output(self):
        good = 'Loaded ID: abc Folder: /project Resume: Enter'
        self.assertTrue(live.ready(good, 'abc', '/project'))
        self.assertFalse(live.ready(good+' Fetching metadata', 'abc', '/project'))
        terminal = live.Terminal.__new__(live.Terminal)
        terminal.output = bytearray(b'Synthetic fixture complete.')
        mark = len(terminal.output)
        terminal.output.extend(b'new screen')
        self.assertNotIn('Synthetic', live.screen_since(terminal, mark))
        self.assertIn('new screen', live.screen_since(terminal, mark))
        self.assertIn(b'Synthetic', terminal.output)

    def test_cases_reset_state_on_fixture_failure(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(live.probe, 'prepare_fixture', side_effect=AssertionError('invalid fixture')):
            one = live.run_case('/tracr', '/codex', Path(temp)/'a', 'legacy')
            one['version'] = 'should not leak'
            two = live.run_case('/tracr', '/codex', Path(temp)/'b', 'paginated')
            self.assertIsNone(two['version'])
            self.assertIsNone(two['session_id'])
            self.assertEqual(two['status'], 'failed')

    def test_terminal_early_exit_fails_promptly(self):
        terminal = live.Terminal([sys.executable, '-c', 'pass'])
        try:
            with self.assertRaises(RuntimeError):
                terminal.expect('never emitted')
        finally:
            terminal.close()

class SimulatedFlowTests(unittest.TestCase):
    def exercise(self, scenario):
        import json
        import types
        from contextlib import ExitStack
        real_terminal = live.Terminal
        with tempfile.TemporaryDirectory() as temp:
            home = Path(temp)/'case'
            home.mkdir()
            executable = Path(temp)/'codex'
            executable.write_bytes(b'fake executable fingerprint')
            cwd = str(home/'old')
            browser = f'Loaded ID: abc Folder: {cwd} Resume: Enter > ▸ old (1)'
            class FakeTerminal:
                def __init__(self, *args):
                    self.output = bytearray(browser.encode())
                    self.child = types.SimpleNamespace(pid=50, returncode=0)
                    self.master = 3
                    self.state = browser
                    self.interrupts = 0
                    self.refreshing = False
                def text(self):
                    return self.state
                def send(self, data):
                    if data == b'\r':
                        self.output.extend(b'\x1b[?1049l')
                        message = ('app-server socket: Operation not permitted' if scenario == 'blocked' else 'Synthetic fixture complete.')
                        if scenario == 'timeout': message = 'No recognized startup state'
                        self.output.extend(message.encode())
                    elif data == b'/status\r':
                        identity = 'wrong' if scenario == 'wrong-session' else 'abc'
                        directory = '/wrong' if scenario == 'wrong-directory' else cwd
                        self.output.extend(f' Session: {identity} Directory: {directory}'.encode())
                    elif data == b'\x03':
                        self.interrupts += 1
                        self.output.extend(b' Press Ctrl-C again' if self.interrupts == 1 else b'\x1b[1;1Htracr')
                    elif data == b'r':
                        self.output.extend(b'Loading')
                        self.state = 'Loading'
                        self.refreshing = True
                def wait(self, predicate, timeout=15):
                    if predicate(): return
                    if self.refreshing:
                        self.state = browser
                        self.refreshing = False
                    if not predicate(): raise AssertionError('unexplained timeout')
                def finish(self):
                    pass
                def close(self):
                    if scenario == 'cleanup': raise AssertionError('cleanup failed')
            fixture = {'env': {}, 'baseline': {'id':'abc'}, 'expected_messages':['Synthetic fixture complete.']}
            listing = {'complete':True, 'sessions':[{'key':{'native_id':'abc'}, 'cwd':cwd, 'archived':False}]}
            def screen(terminal, mark):
                view = real_terminal.__new__(real_terminal)
                view.output = terminal.output[mark:]
                return view.text()
            with ExitStack() as stack:
                stack.enter_context(patch.object(live.probe, 'prepare_fixture', return_value=fixture))
                stack.enter_context(patch.object(live.subprocess, 'check_output', return_value='codex test'))
                stack.enter_context(patch.object(live.subprocess, 'run', return_value=types.SimpleNamespace(returncode=0, stdout=json.dumps(listing), stderr='')))
                stack.enter_context(patch.object(live, 'Terminal', FakeTerminal))
                stack.enter_context(patch.object(live, 'screen_since', side_effect=screen))
                stack.enter_context(patch.object(live, 'submit_status', side_effect=lambda terminal: terminal.send(b'/status\r')))
                stack.enter_context(patch.object(live.os, 'tcgetpgrp', side_effect=[100,100,50]))
                stack.enter_context(patch.object(live, 'stop_daemons', return_value={'observed_pids':[]}))
                return live.run_case('/tracr', str(executable), home, 'legacy')

    def test_simulated_success_and_failure_phases(self):
        for scenario, expected in [('success','passed'), ('blocked','blocked'), ('timeout','failed'),
                                   ('wrong-session','failed'), ('wrong-directory','failed'), ('cleanup','failed')]:
            with self.subTest(scenario=scenario):
                result = self.exercise(scenario)
                self.assertEqual(result['status'], expected, result)


if __name__ == '__main__':
    unittest.main()
