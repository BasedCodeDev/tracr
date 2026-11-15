import importlib.util
from pathlib import Path
import sys
import unittest
from unittest.mock import Mock, call

scripts = Path(__file__).resolve().parents[1] / 'scripts'
sys.path.insert(0, str(scripts))
spec = importlib.util.spec_from_file_location('listing_probe', scripts / 'probe_codex_listing.py')
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class ListingProbeTests(unittest.TestCase):
    def test_database_flag_and_filters_survive_pagination(self):
        server = Mock()
        server.call.side_effect = [
            {'data': [{'id': 'a'}], 'nextCursor': 'next'},
            {'data': [{'id': 'b'}], 'nextCursor': None},
        ]
        params = []
        replies = iter(server.call.side_effect)
        def reply(method, values):
            params.append(dict(values))
            return next(replies)
        server.call.side_effect = reply
        self.assertEqual(probe.listing(server, database=True, archived=True, cwd='/new'),
                         [{'id': 'a'}, {'id': 'b'}])
        self.assertNotIn('cursor', params[0])
        self.assertEqual(params[1]['cursor'], 'next')
        for values in params:
            self.assertTrue(values['useStateDbOnly'])
            self.assertTrue(values['archived'])
            self.assertEqual(values['cwd'], '/new')

    def test_repeated_cursor_fails_instead_of_partial_success(self):
        server = Mock()
        server.call.return_value = {'data': [], 'nextCursor': 'loop'}
        with self.assertRaisesRegex(probe.probe.RPCError, 'repeated'):
            probe.listing(server, database=True)

    def test_read_cannot_repair_before_first_listing(self):
        server = Mock()
        server.call.side_effect = [
            {'data': [{'id': 'id', 'cwd': '/new'}]},
            {'data': [{'id': 'id', 'cwd': '/old'}]},
            {'data': [{'id': 'id', 'cwd': '/new'}]},
            {'data': [{'id': 'id', 'cwd': '/new'}]},
            {'thread': {'cwd': '/new'}},
        ]
        result = probe.inspect(server, 'id', Path('/new'), False)
        self.assertEqual(result['database_first'], '/new')
        self.assertEqual(result['default'], '/old')
        calls = server.call.call_args_list
        self.assertTrue(calls[0].args[1]['useStateDbOnly'])
        self.assertEqual(calls[-1], call('thread/read', {'threadId': 'id'}))


if __name__ == '__main__':
    unittest.main()
