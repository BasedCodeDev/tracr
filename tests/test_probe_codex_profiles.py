import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import probe_codex_profiles as probe


class ProfileProbeTests(unittest.TestCase):
    def test_reports_only_selected_metadata_and_normalizes_nested_paths(self):
        response = {'cwd': '/tmp/example/old', 'activePermissionProfile': {'id': ':workspace'},
                    'thread': {'turns': ['private text']}, 'secret': 'not evidence'}
        view = probe.clean(probe.snapshot(response), Path('/tmp/example'))
        self.assertEqual(view['cwd'], '$CASE/old')
        self.assertNotIn('thread', view)
        self.assertNotIn('secret', view)

    def test_durable_snapshot_reads_archived_settings_and_ignores_foreign_owners(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive = root/'codex/archived_sessions'
            archive.mkdir(parents=True)
            events = [
                {'type': 'response_item', 'payload': {'text': 'not evidence'}},
                {'type': 'event_msg', 'payload': {'type': 'thread_settings_applied',
                 'thread_id': 'session', 'thread_settings': {'cwd': '/new', 'permission_profile': {'type': 'managed'}}}},
                {'type': 'event_msg', 'payload': {'type': 'thread_settings_applied',
                 'thread_id': 'foreign', 'thread_settings': {'cwd': '/wrong'}}},
            ]
            (archive/'rollout-session.jsonl').write_text(''.join(json.dumps(e)+'\n' for e in events))
            self.assertEqual(probe.durable_snapshot(root, 'session')['cwd'], '/new')
            self.assertIsNone(probe.durable_snapshot(root, 'missing'))

    def test_profiles_separate_missing_directory_and_existing_controls(self):
        root = Path('/synthetic')
        profile, rules = probe.profile_config(root, 'custom-existing')
        self.assertEqual(profile, 'project')
        self.assertIn('"**/*.env" = "deny"', rules)
        self.assertEqual((profile, rules), probe.profile_config(root, 'custom'))
        self.assertIn('/synthetic/extra', probe.profile_config(root, 'extra-roots-existing')[1])
        self.assertIn('enabled = true', probe.profile_config(root, 'network-existing')[1])

    def test_rule_comparison_ignores_duplicates_but_preserves_access_differences(self):
        rule = {'path': {'type':'path','path':'/workspace'}, 'access':'read'}
        a = {'permission_profile': {'file_system': {'entries': [rule, rule]}}}
        b = {'permission_profile': {'file_system': {'entries': [rule]}}}
        self.assertEqual(probe.normalized_snapshot(a), probe.normalized_snapshot(b))
        self.assertEqual(len(a['permission_profile']['file_system']['entries']), 2)
        b['permission_profile']['file_system']['entries'] = [dict(rule, access='write')]
        self.assertNotEqual(probe.normalized_snapshot(a), probe.normalized_snapshot(b))

    def test_probe_never_authorizes_model_turns(self):
        self.assertNotIn('turn/start', probe.base.ALLOWED_METHODS)
        self.assertIn('config/read', probe.base.ALLOWED_METHODS)


if __name__ == '__main__':
    unittest.main()
