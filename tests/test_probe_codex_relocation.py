import importlib.util
import io
import json
import os
from pathlib import Path
import queue
import tempfile
import types
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('probe', Path(__file__).parents[1] / 'scripts/probe_codex_relocation.py')
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class ProbeTests(unittest.TestCase):
    def test_environment_never_inherits_credentials_or_home(self):
        with tempfile.TemporaryDirectory() as tmp:
            with patch.dict(os.environ, {'OPENAI_API_KEY': 'secret', 'CODEX_HOME': '/real/home',
                                         'UNEXPECTED_PROVIDER_TOKEN': 'secret'}):
                env = probe.environment(Path(tmp))
            self.assertNotIn('OPENAI_API_KEY', env)
            self.assertNotIn('UNEXPECTED_PROVIDER_TOKEN', env)
            for key in ('HOME', 'CODEX_HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME'):
                self.assertTrue(Path(env[key]).is_relative_to(tmp))
            self.assertIn('127.0.0.1:1', (Path(env['CODEX_HOME']) / 'config.toml').read_text())

    def server(self, replies):
        s = probe.Server.__new__(probe.Server)
        s.seq = 0
        s.log = io.StringIO()
        s.p = types.SimpleNamespace(stdin=io.StringIO())
        s.q = queue.Queue()
        for reply in replies:
            s.q.put(json.dumps(reply) if reply is not None else None)
        return s

    def test_notifications_do_not_consume_response(self):
        s = self.server([{'method': 'thread/started'}, {'id': 1, 'result': {'ok': True}}])
        self.assertEqual(s.call('thread/read'), {'ok': True})

    def test_unexpected_server_request_fails(self):
        s = self.server([{'id': 999, 'method': 'item/commandExecution/requestApproval'}])
        with self.assertRaisesRegex(probe.RPCError, 'unexpected server request'):
            s.call('thread/read')

    def test_forbids_inference_and_command_execution(self):
        for method in ('turn/start', 'thread/shellCommand', 'command/exec'):
            s = self.server([])
            with self.assertRaisesRegex(probe.RPCError, 'forbids'):
                s.call(method)
            self.assertEqual(s.p.stdin.getvalue(), '')

    def test_rpc_error_and_eof_are_not_success(self):
        for reply in ({'id': 1, 'error': {'code': -32600}}, None):
            with self.assertRaises(probe.RPCError):
                self.server([reply]).call('thread/read')


class StrategyTests(unittest.TestCase):
    def test_unknown_strategy_never_calls_server(self):
        from unittest.mock import Mock
        server = Mock()
        with self.assertRaises(ValueError):
            probe.relocate(server, 'id', Path('/new'), 'invented')
        server.call.assert_not_called()

    def test_settings_loads_without_starting_turn(self):
        from unittest.mock import Mock, call
        server = Mock()
        probe.relocate(server, 'id', Path('/new folder'), 'settings')
        self.assertEqual(server.call.call_args_list, [
            call('thread/resume', {'threadId': 'id', 'excludeTurns': True}),
            call('thread/settings/update', {'threadId': 'id', 'cwd': '/new folder'})])

    def test_unloaded_settings_does_not_resume(self):
        from unittest.mock import Mock
        server = Mock()
        probe.relocate(server, 'id', Path('/new'), 'settings-unloaded')
        server.call.assert_called_once_with('thread/settings/update', {'threadId': 'id', 'cwd': '/new'})

    def test_roots_and_unsubscribe(self):
        from unittest.mock import Mock
        server = Mock()
        probe.relocate(server, 'id', Path('/new'), 'resume-roots')
        self.assertEqual(server.call.call_args.args[1]['runtimeWorkspaceRoots'], ['/new'])
        server.reset_mock()
        server.call.side_effect = [{}, {'status': 'unsubscribed'}, {'data': []}]
        probe.relocate(server, 'id', Path('/new'), 'resume-unsubscribe')
        self.assertEqual([c.args[0] for c in server.call.call_args_list],
                         ['thread/resume', 'thread/unsubscribe', 'thread/loaded/list'])

    def test_incomplete_coverage_never_passes(self):
        cases = [dict(strategy='resume', scenario='ordinary', repeat=r, history_mode=m, outcome='pass')
                 for r in range(2) for m in ('legacy', 'paginated')]
        self.assertEqual(probe.basic_verdict(cases, 'resume', 2), 'pass')
        self.assertEqual(probe.basic_verdict(cases[:-1], 'resume', 2), 'inconclusive')
        cases[0]['outcome'] = 'fail'
        self.assertEqual(probe.basic_verdict(cases, 'resume', 2), 'fail')
        self.assertEqual(probe.exit_code(cases), 1)
        cases[0]['outcome'] = 'inconclusive'
        self.assertEqual(probe.basic_verdict(cases, 'resume', 2), 'inconclusive')
        self.assertEqual(probe.exit_code(cases), 2)
        self.assertEqual(probe.exit_code([]), 2)

    def test_rpc_error_propagates_from_strategy(self):
        from unittest.mock import Mock
        server = Mock()
        server.call.side_effect = probe.RPCError('rejected')
        with self.assertRaises(probe.RPCError):
            probe.relocate(server, 'id', Path('/new'), 'settings')


    def test_schema_guard_reports_missing_fields(self):
        missing = probe.strategy_requirements({}, 'settings')
        self.assertIn('ThreadSettingsUpdateParams.cwd', missing)
        self.assertIn('ThreadResumeParams.threadId', missing)


if __name__ == '__main__':
    unittest.main()
