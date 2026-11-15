#!/usr/bin/env python3
"""Native permission-profile relocation evidence; disposable homes, no model turns.

No user configuration or session is read. Raw RPC captures stay in the printed
scratch directory; the JSON report contains selected metadata and normalized paths.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import probe_codex_relocation as base

base.ALLOWED_METHODS = base.ALLOWED_METHODS | {'config/read', 'permissionProfile/list'}
CASES = ('workspace', 'read-only', 'custom', 'extra-roots', 'multi-runtime',
         'destination-layer', 'missing-profile', 'config-race', 'archived', 'locked',
         'workspace-parent', 'custom-simple', 'custom-existing', 'extra-roots-existing',
         'destination-layer-existing', 'network-existing', 'explicit-old-existing', 'config-race-existing')
FIELDS = ('cwd', 'activePermissionProfile', 'runtimeWorkspaceRoots', 'sandbox',
          'approvalPolicy', 'approvalsReviewer', 'model', 'modelProvider')


def snapshot(response):
    return {key: response.get(key) for key in FIELDS}


def clean(value, root):
    if isinstance(value, str):
        return value.replace(str(root), '$CASE')
    if isinstance(value, list):
        return [clean(v, root) for v in value]
    if isinstance(value, dict):
        return {k: clean(v, root) for k, v in value.items()}
    return value


def normalized_snapshot(value):
    """Ignore only duplicate identical rules and rule ordering, not access changes."""
    value = copy.deepcopy(value)
    if value:
        fs = (value.get('permission_profile') or {}).get('file_system', {})
        if 'entries' in fs:
            entries = {json.dumps(e, sort_keys=True): e for e in fs['entries']}
            fs['entries'] = [entries[k] for k in sorted(entries)]
    return value


def settle(server, tid, predicate):
    for _ in range(40):
        value = server.call('thread/resume', {'threadId': tid, 'excludeTurns': True})
        if predicate(value):
            return value
        time.sleep(.05)
    raise RuntimeError('settings did not settle')


def profile_config(root, case):
    case = case.removesuffix('-existing')
    profile = ':read-only' if case == 'read-only' else ':workspace'
    rules = ''
    if case in ('custom', 'custom-simple', 'extra-roots', 'destination-layer', 'missing-profile', 'config-race', 'network', 'explicit-old'):
        profile = 'project'
        rules = '''
[permissions.parent]
extends = ":workspace"
[permissions.project]
extends = "parent"
[permissions.project.filesystem.":workspace_roots"]
"**/*.env" = "deny"
"docs" = "read"
[permissions.project.network]
enabled = false
[permissions.project.network.domains]
"api.openai.com" = "allow"
"tracking.example.com" = "deny"
'''
        if case == 'extra-roots':
            rules += f'\n[permissions.project.workspace_roots]\n"{root / "extra"}" = true\n'
            rules += f'\n[permissions.project.filesystem]\n"{root / "explicit"}" = "read"\n'
        if case == 'custom-simple':
            rules = '[permissions.project]\nextends = ":workspace"\n'
        if case == 'network':
            rules = rules.replace('enabled = false', 'enabled = true')
        if case == 'explicit-old':
            rules += f'\n[permissions.project.filesystem]\n"{root / "old" / "explicit"}" = "read"\n'
    return profile, rules


def durable_snapshot(root, tid):
    """Inspect ONLY synthetic rollout snapshots, never user histories or messages."""
    latest = None
    for path in sorted([*(root/'codex/sessions').rglob('*.jsonl'), *(root/'codex/archived_sessions').rglob('*.jsonl')]):
        if tid not in path.name:
            continue
        for line in path.read_text().splitlines():
            event = json.loads(line)
            payload = event.get('payload', {})
            if event.get('type') == 'event_msg' and payload.get('type') == 'thread_settings_applied':
                if payload.get('thread_id', tid) != tid:
                    continue
                settings = payload['thread_settings']
                latest = {k: settings.get(k) for k in ('permission_profile', 'active_permission_profile',
                    'cwd', 'runtime_workspace_roots', 'approval_policy', 'approvals_reviewer')}
    return latest


def config_view(server, cwd):
    value = server.call('config/read', {'cwd': str(cwd), 'includeLayers': True})
    config = value['config']
    return {k: config[k] for k in ('default_permissions', 'permissions', 'approval_policy',
                                  'approvals_reviewer', 'sandbox_mode') if k in config}


def run_case(binary, root, mode, case, apply_profile=False):
    record = {'history_mode': mode, 'case': case, 'strategy': 'cwd-and-profile' if apply_profile else 'cwd-only', 'status': 'inconclusive'}
    server = peer = None
    phase = 'fixture'
    try:
        fixture = base.prepare_fixture(binary, root, mode)
        phase = 'configure-fixture'
        env = fixture['env']; tid = fixture['baseline']['id']
        old, new = root/'old', root/'new folder'
        if case == 'workspace-parent': new = root
        kind = case.removesuffix('-existing')
        for folder in (old, new):
            for name in ('docs', 'explicit', '.git', '.agents', '.codex', '.aws'):
                (folder/name).mkdir(exist_ok=True)
            (folder/'secret.env').write_text('SYNTHETIC=fixture\n')
        for name in ('extra', 'explicit'):
            (root/name).mkdir(exist_ok=True)
        profile, rules = profile_config(root, case)
        config = root/'codex/config.toml'
        original_config = config.read_text()
        trusted = ''.join(f'\n[projects."{p}"]\ntrust_level = "trusted"\n' for p in (old, new))
        config.write_text(f'default_permissions = "{profile}"\n' + original_config + rules + trusted)
        if kind == 'destination-layer':
            (new/'.git').mkdir(exist_ok=True); (new/'.codex').mkdir(exist_ok=True)
            (new/'.codex/config.toml').write_text('[permissions.project.filesystem.":workspace_roots"]\n"generated" = "deny"\n')
        phase = 'seed-profile'
        server = base.Server(binary, root, env)
        server.call('thread/resume', {'threadId': tid, 'excludeTurns': True})
        server.call('thread/settings/update', {'threadId': tid, 'permissions': profile})
        seeded = settle(server, tid, lambda r: (r.get('activePermissionProfile') or {}).get('id') == profile)
        server.close(); server = None
        server = base.Server(binary, root, env)
        before = server.call('thread/resume', {'threadId': tid, 'excludeTurns': True})
        if case == 'multi-runtime':
            before = server.call('thread/resume', {'threadId': tid, 'excludeTurns': True,
                                                  'runtimeWorkspaceRoots': [str(old), str(root/'extra')]})
        assert before['activePermissionProfile']['id'] == profile
        record['before'] = snapshot(before)
        record['source_config'] = config_view(server, old)
        record['destination_config'] = config_view(server, new)
        record['available_profiles'] = server.call('permissionProfile/list', {'cwd': str(new)})
        # An ephemeral destination thread checks what native resolution exposes
        # without altering the original thread. It never starts a turn.
        phase = 'destination-preview'
        preview_roots = list(dict.fromkeys(str(new) if p == str(old) else p for p in before['runtimeWorkspaceRoots']))
        preview = server.call('thread/start', {'cwd': str(new), 'permissions': profile,
                              'modelProvider': 'probe', 'model': 'probe', 'approvalPolicy': 'never', 'ephemeral': True, 'runtimeWorkspaceRoots': preview_roots})
        record['destination_preview'] = snapshot(preview)
        # Research-only durable twin supplies complete materialized rules for comparison;
        # these fields are not present in the public resume/start response.
        twin = server.call('thread/start', {'cwd': str(new), 'permissions': profile,
                           'modelProvider':'probe', 'model':'probe', 'approvalPolicy':'never', 'runtimeWorkspaceRoots': preview_roots})['thread']['id']
        server.call('thread/inject_items', {'threadId': twin, 'items': [{'type':'message','role':'user','content':[{'type':'input_text','text':'Synthetic permission preview; do not run a turn.'}]}]})
        server.call('thread/settings/update', {'threadId': twin, 'permissions': profile})
        server.close(); server = None
        record['before_durable'] = durable_snapshot(root, tid)
        record['destination_twin_durable'] = durable_snapshot(root, twin)
        # Missing-old behavior matches the real user report, including child->parent moves
        # as a separate built-in workspace case selected below.
        if not case.endswith('-existing'): shutil.rmtree(old)
        if kind == 'config-race':
            config.write_text(config.read_text().replace('"docs" = "read"', '"docs" = "deny"'))
        elif case == 'missing-profile':
            config.write_text(original_config + trusted)
        phase = 'acquire-writer'
        server = base.Server(binary, root, env)
        if case == 'archived':
            server.call('thread/archive', {'threadId': tid})
            server.call('thread/unarchive', {'threadId': tid})
        loaded = server.call('thread/resume', {'threadId': tid, 'excludeTurns': True})
        record['pre_update'] = snapshot(loaded)
        record['pre_update_config'] = config_view(server, new)
        if case == 'locked':
            peer = base.Server(binary, root, env)
            try:
                peer.call('thread/resume', {'threadId': tid, 'excludeTurns': True})
            except base.RPCError as error:
                record.update(status='observed', writer_rejected=True, error=str(error))
                return clean(record, root)
            raise RuntimeError('competing writer unexpectedly acquired session')
        phase = 'update-cwd'
        update = {'threadId': tid, 'cwd': str(new)}
        if apply_profile: update['permissions'] = profile
        server.call('thread/settings/update', update)
        after = settle(server, tid, lambda r: r['cwd'] == str(new))
        record['after'] = snapshot(after)
        server.close(); server = None
        record['after_durable'] = durable_snapshot(root, tid)
        phase = 'restart-verification'
        server = base.Server(binary, root, env)
        record['restart'] = snapshot(server.call('thread/resume', {'threadId': tid, 'excludeTurns': True}))
        record['history_unchanged'] = base.history(server, tid) == fixture['history_before']
        assert record['history_unchanged'], 'synthetic history changed'
        rows = server.call('thread/list', {'useStateDbOnly':True,'limit':100,'archived':False,'sourceKinds':[],'modelProviders':[]})['data']
        record['database_cwd'] = next(t['cwd'] for t in rows if t['id'] == tid)
        record['read_cwd'] = base.read(server, tid)['cwd']
        assert record['database_cwd'] == record['read_cwd'] == str(new), 'native directory views disagree'
        if case == 'archived':
            server.call('thread/archive', {'threadId': tid})
            record['archive_restored'] = any(t['id'] == tid for t in base.inventory(server, True))
        server.close(); server = None
        record['restart_durable'] = durable_snapshot(root, tid)
        record['materialized_rules_stable_after_restart'] = normalized_snapshot(record['after_durable']) == normalized_snapshot(record['restart_durable'])
        record['destination_matches_restart'] = normalized_snapshot(record['destination_twin_durable']) == normalized_snapshot(record['restart_durable'])
        expected_roots = [str(new) if p == str(old) else p for p in before['runtimeWorkspaceRoots']]
        record['runtime_roots_preserved'] = record['restart']['runtimeWorkspaceRoots'] == list(dict.fromkeys(expected_roots))
        record['status'] = 'observed'
    except Exception as error:
        record.update(status='blocked' if phase != 'fixture' else 'inconclusive', phase=phase, error=str(error))
    finally:
        if peer: peer.close()
        if server: server.close()
    return clean(record, root)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--codex', required=True)
    parser.add_argument('--apply-profile', action='store_true', help='Explicitly resolve the same profile at the destination in the native update')
    parser.add_argument('--case', choices=CASES, action='append')
    parser.add_argument('--mode', choices=('legacy', 'paginated'), action='append')
    args = parser.parse_args()
    binary = str(Path(args.codex).resolve(strict=True))
    root = Path(tempfile.mkdtemp(prefix='tracr-profile-probe-'))
    version_root = root/'version'; version_root.mkdir()
    version_env = base.environment(version_root)
    report = {'version': subprocess.check_output([binary, '--version'], env=version_env, text=True).strip(),
              'executable_sha256': hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
              'probe_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(), 'cases': []}
    print('Evidence:', root, flush=True)
    for mode in args.mode or ('legacy', 'paginated'):
        for case in args.case or CASES:
            result = run_case(binary, root/(mode+'-'+case), mode, case, args.apply_profile)
            report['cases'].append(result)
            (root/'report.json').write_text(json.dumps(report, indent=2)+'\n')
            print(mode, case, result['status'], result.get('phase',''), result.get('error','')[:160], flush=True)
    print('Report:', root/'report.json', flush=True)
    return int(any(c['status'] == 'inconclusive' for c in report['cases']))


if __name__ == '__main__':
    raise SystemExit(main())
