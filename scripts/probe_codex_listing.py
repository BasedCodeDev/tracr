#!/usr/bin/env python3
"""Disposable native relinking/list-mode experiment; never touches a user's home.

Uses the original probe's no-turn RPC allowlist and synthetic history fixtures.
Success proves these cases only, not production-safe relocation/exclusion.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

import probe_codex_relocation as probe


def listing(server, *, database=False, archived=False, cwd=None):
    params = {'limit': 1, 'archived': archived, 'modelProviders': [], 'sourceKinds': [],
              'useStateDbOnly': database}
    if cwd is not None:
        params['cwd'] = str(cwd)
    rows, seen = [], set()
    while True:
        page = server.call('thread/list', params)
        rows.extend(page['data'])
        cursor = page.get('nextCursor')
        if not cursor:
            return rows
        if cursor in seen:
            raise probe.RPCError('repeated listing cursor')
        seen.add(cursor)
        params['cursor'] = cursor


def inspect(server, tid, destination, archived):
    # DB listing MUST be first: read/resume must not repair it before observation.
    db = listing(server, database=True, archived=archived)
    default = listing(server, archived=archived)
    again = listing(server, database=True, archived=archived)
    found = lambda rows: next((t['cwd'] for t in rows if t['id'] == tid), None)
    filtered = listing(server, database=True, archived=archived, cwd=destination)
    return {'database_first': found(db), 'default': found(default),
            'database_after_default': found(again),
            'read': probe.read(server, tid)['cwd'],
            'destination_filter_contains_id': any(t['id'] == tid for t in filtered)}


def run(binary, root, mode, strategy, scenario):
    result = dict(mode=mode, strategy=strategy, scenario=scenario, outcome='inconclusive')
    server = None
    validated = False
    try:
        fixture = probe.prepare_fixture(binary, root, mode)
        validated = True
        tid = fixture['baseline']['id']
        env = fixture['env']
        old, new = root / 'old', root / 'new folder'
        result['id'] = tid
        server = probe.Server(binary, root, env)
        control_dir = root / 'unrelated'
        control_dir.mkdir()
        control_id = server.call('thread/start', {'cwd': str(control_dir), 'historyMode': mode,
            'modelProvider': 'probe', 'model': 'probe', 'approvalPolicy': 'never',
            'sandbox': 'read-only'})['thread']['id']
        server.call('thread/inject_items', {'threadId': control_id, 'items': [
            {'type': 'message', 'role': 'user', 'content': [{'type': 'input_text', 'text': 'Unrelated control.'}]}]})
        server.close()
        server = probe.Server(binary, root, env)
        if scenario == 'missing-old':
            old.rmdir()
        if scenario == 'archived':
            server.call('thread/archive', {'threadId': tid})
            server.call('thread/unarchive', {'threadId': tid})
        result['mutation'] = probe.relocate(server, tid, new, strategy)
        if scenario == 'archived':
            server.call('thread/archive', {'threadId': tid})
        server.close()
        server = probe.Server(binary, root, env)
        archived = scenario == 'archived'
        result['observations'] = inspect(server, tid, new, archived)
        if archived:
            server.call('thread/unarchive', {'threadId': tid})
        result['resume'] = server.call('thread/resume', {'threadId': tid, 'excludeTurns': True})['thread']['cwd']
        checks = {
            'unrelated_unchanged': probe.read(server, control_id)['cwd'] == str(control_dir),
            'db_first': result['observations']['database_first'] == str(new),
            'db_after_default': result['observations']['database_after_default'] == str(new),
            'read': result['observations']['read'] == str(new),
            'filter': result['observations']['destination_filter_contains_id'],
            'resume': result['resume'] == str(new),
            'history': probe.history(server, tid) == fixture['history_before'],
            'old_filter_excludes_id': not any(t['id'] == tid for t in listing(server, database=True, cwd=old)),
        }
        if archived:
            server.call('thread/archive', {'threadId': tid})
        server.close()
        server = probe.Server(binary, root, env)
        checks['second_restart'] = inspect(server, tid, new, archived)['database_first'] == str(new)
        if not old.exists():
            old.mkdir()  # Only restore the empty disposable fixture directory for reversal.
        if archived:
            server.call('thread/unarchive', {'threadId': tid})
        probe.relocate(server, tid, old, strategy)
        if archived:
            server.call('thread/archive', {'threadId': tid})
        server.close()
        server = probe.Server(binary, root, env)
        reverse = inspect(server, tid, old, archived)
        checks['reverse_db'] = reverse['database_first'] == str(old)
        checks['reverse_read'] = reverse['read'] == str(old)
        if archived:
            server.call('thread/unarchive', {'threadId': tid})
        checks['reverse_resume'] = server.call('thread/resume', {'threadId': tid, 'excludeTurns': True})['thread']['cwd'] == str(old)
        checks['reverse_history'] = probe.history(server, tid) == fixture['history_before']
        if archived:
            server.call('thread/archive', {'threadId': tid})
            checks['archive_restored'] = any(t['id'] == tid for t in listing(server, database=True, archived=True))
        result.update(checks=checks, outcome='pass' if all(checks.values()) else 'fail')
    except (probe.RPCError, OSError, ValueError, AssertionError, KeyError) as error:
        result['error'] = str(error)
        result['outcome'] = 'fail' if validated else 'inconclusive'
    finally:
        if server:
            server.close()
        (root / 'listing-result.json').write_text(json.dumps(result, indent=2) + '\n')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--codex', required=True)
    parser.add_argument('--repeat', type=int, default=2)
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error('--repeat must be positive')
    binary = str(Path(args.codex).resolve(strict=True))
    root = Path(tempfile.mkdtemp(prefix='tracr-listing-relink-'))
    report = {'version': subprocess.check_output([binary, '--version'], text=True).strip(),
              'sha256': hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
              'evidence_directory': str(root), 'cases': [], 'production_approved': False}
    print('Evidence:', root, flush=True)
    for repeat in range(args.repeat):
        for mode in ('legacy', 'paginated'):
            for strategy in ('resume', 'settings'):
                for scenario in ('ordinary', 'missing-old', 'archived'):
                    case = run(binary, root / f'{repeat}-{mode}-{strategy}-{scenario}', mode, strategy, scenario)
                    case['repeat'] = repeat
                    report['cases'].append(case)
                    (root / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
                    print(repeat, mode, strategy, scenario, case['outcome'], case.get('error', case.get('checks')), flush=True)
    return probe.exit_code(report['cases'])


if __name__ == '__main__':
    raise SystemExit(main())
