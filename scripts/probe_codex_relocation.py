#!/usr/bin/env python3
"""Offline, disposable Codex relocation probe. Never uses the user's Codex home."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import sqlite3
import subprocess
import tempfile
import threading
import time


ALLOWED_METHODS = frozenset({
    'initialize', 'thread/start', 'thread/inject_items', 'thread/read',
    'thread/list', 'thread/items/list', 'thread/turns/list', 'thread/resume', 'thread/archive',
    'thread/unarchive', 'thread/metadata/update', 'thread/loaded/list',
    'project/create', 'project/update', 'project/read',
    'thread/settings/update', 'thread/unsubscribe',
})


STRATEGIES = ('resume', 'settings', 'settings-unloaded', 'resume-roots', 'resume-unsubscribe', 'project-resume')
SCENARIOS = ('ordinary', 'missing-old', 'archived', 'archive-cycle', 'project', 'loaded', 'independent-server')
DEFERRED = ['two clients on shared server', 'batch interruption', 'nested group and unrelated controls',
            'guarded reversal conflict', 'multi-root project', 'workspace-write roots and trust']


def relocate(server, tid, destination, strategy):
    """Only schema-backed operations; never start a turn or edit persisted storage."""
    if strategy not in STRATEGIES:
        raise ValueError('unknown relocation strategy: ' + strategy)
    if strategy.startswith('settings'):
        if strategy == 'settings':
            server.call('thread/resume', {'threadId': tid, 'excludeTurns': True})
        return server.call('thread/settings/update', {'threadId': tid, 'cwd': str(destination)})
    params = {'threadId': tid, 'cwd': str(destination), 'excludeTurns': True}
    if strategy == 'resume-roots':
        params['runtimeWorkspaceRoots'] = [str(destination)]
    result = server.call('thread/resume', params)
    if strategy == 'resume-unsubscribe':
        server.call('thread/unsubscribe', {'threadId': tid})
        deadline = time.monotonic() + 10
        while tid in server.call('thread/loaded/list')['data']:
            if time.monotonic() >= deadline:
                raise RPCError('unsubscribe did not unload within 10 seconds')
            time.sleep(.1)
    return result


def strategy_requirements(definitions, strategy):
    required = {'ThreadResumeParams': {'threadId', 'cwd', 'excludeTurns'}}
    if strategy.startswith('settings'):
        required['ThreadSettingsUpdateParams'] = {'threadId', 'cwd'}
    if strategy == 'resume-roots':
        required['ThreadResumeParams'].add('runtimeWorkspaceRoots')
    if strategy == 'resume-unsubscribe':
        required['ThreadUnsubscribeParams'] = {'threadId'}
    if strategy == 'project-resume':
        required.update(ProjectCreateParams={'roots'}, ProjectUpdateParams={'projectId', 'roots'},
                        ThreadMetadataUpdateParams={'threadId', 'projectId'})
    return [f'{name}.{field}' for name, fields in required.items() for field in sorted(fields)
            if field not in definitions.get(name, {}).get('properties', {})]


def basic_verdict(cases, strategy, repeat):
    selected = [c for c in cases if c['strategy'] == strategy and c['scenario'] == 'ordinary']
    expected = {(r, mode) for r in range(repeat) for mode in ('legacy', 'paginated')}
    actual = {(c['repeat'], c['history_mode']) for c in selected}
    if actual != expected or any(c['outcome'] == 'inconclusive' for c in selected):
        return 'inconclusive'
    return 'pass' if all(c['outcome'] == 'pass' for c in selected) else 'fail'


def exit_code(cases):
    if not cases or any(c['outcome'] == 'inconclusive' for c in cases):
        return 2
    return int(any(c['outcome'] == 'fail' for c in cases))


class RPCError(RuntimeError):
    pass


class Server:
    def __init__(self, binary, root, env):
        self.log = (root / f'rpc-{time.time_ns()}.jsonl').open('w')
        self.err = (root / f'stderr-{time.time_ns()}.log').open('w')
        self.p = subprocess.Popen([binary, 'app-server', '--stdio'], cwd=root,
                                  env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=self.err, text=True)
        self.q = queue.Queue()
        self.seq = 0
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        try:
            self.call('initialize', {'clientInfo': {'name': 'tracr_probe', 'version': '0.1'},
                                    'capabilities': {'experimentalApi': True}})
            self.send({'method': 'initialized'})
        except BaseException:
            self.close()
            raise

    def _read(self):
        for line in self.p.stdout:
            self.q.put(line)
        self.q.put(None)

    def record(self, direction, value):
        self.log.write(json.dumps({'direction': direction, 'message': value}) + '\n')
        self.log.flush()

    def send(self, value):
        self.record('send', value)
        self.p.stdin.write(json.dumps(value) + '\n')
        self.p.stdin.flush()

    def call(self, method, params=None):
        if method not in ALLOWED_METHODS:
            raise RPCError(f'Probe forbids method: {method}')
        self.seq += 1
        self.send({'id': self.seq, 'method': method, 'params': params or {}})
        deadline = time.monotonic() + 20
        while True:
            try:
                line = self.q.get(timeout=max(0.001, deadline - time.monotonic()))
            except queue.Empty:
                raise RPCError(f'{method}: timeout') from None
            if line is None:
                raise RPCError(f'{method}: server exited')
            value = json.loads(line)
            self.record('receive', value)
            if 'method' in value and 'id' in value:
                raise RPCError(f'unexpected server request: {value}')
            if value.get('id') == self.seq:
                if 'error' in value:
                    raise RPCError(f'{method}: {value["error"]}')
                return value['result']

    def close(self):
        if self.log.closed:
            return
        if self.p.poll() is None:
            self.p.stdin.close()
            try:
                self.p.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.p.terminate()
                try:
                    self.p.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    self.p.kill()
                    self.p.wait()
        self.reader.join(timeout=2)
        self.p.stdout.close()
        self.err.close()
        self.log.close()


def environment(root):
    # Allowlist rather than attempting to enumerate credential variable names.
    env = {k: os.environ[k] for k in ('PATH', 'LANG', 'LC_ALL', 'TERM') if k in os.environ}
    for key, directory in [('HOME', 'home'), ('CODEX_HOME', 'codex'),
                           ('XDG_CONFIG_HOME', 'config'), ('XDG_DATA_HOME', 'data'),
                           ('XDG_STATE_HOME', 'state'), ('XDG_CACHE_HOME', 'cache')]:
        path = root / directory
        path.mkdir()
        env[key] = str(path)
    (root / 'codex/config.toml').write_text('''model_provider = "probe"
model = "probe"
thread_unload_delay_secs = 1
[model_providers.probe]
name = "Offline probe"
base_url = "http://127.0.0.1:1/v1"
wire_api = "responses"
requires_openai_auth = false
[analytics]
enabled = false
[otel]
exporter = "none"
''')
    return env


def history(s, tid):
    thread = s.call('thread/read', {'threadId': tid, 'includeTurns': True})['thread']
    if thread.get('historyMode') == 'legacy':
        return thread['turns']
    result, cursor = [], None
    while True:
        p = {'threadId': tid, 'limit': 100}
        if cursor:
            p['cursor'] = cursor
        page = s.call('thread/items/list', p)
        result.extend(page['data'])
        cursor = page.get('nextCursor')
        if not cursor:
            return result


def read(s, tid):
    return s.call('thread/read', {'threadId': tid})['thread']


def inventory(s, archived=False):
    result, cursor = [], None
    while True:
        p = {'limit': 100, 'archived': archived, 'sourceKinds': [], 'modelProviders': []}
        if cursor:
            p['cursor'] = cursor
        page = s.call('thread/list', p)
        result.extend(page['data'])
        cursor = page.get('nextCursor')
        if not cursor:
            return result


def prepare_fixture(binary, root, mode):
    """Create and validate a fixture in a NEW home; never relocate a thread."""
    root.mkdir()
    return _populate_fixture(binary, root, mode)


def _populate_fixture(binary, root, mode):
    if any(root.iterdir()):
        raise ValueError('fixture directory must be empty')
    env = environment(root)
    old, new = root / 'old', root / 'new folder'
    old.mkdir(); new.mkdir()
    s = None
    try:
        s = Server(binary, root, env)
        tid = s.call('thread/start', {'cwd': str(old), 'historyMode': mode,
                                     'modelProvider': 'probe', 'model': 'probe',
                                     'approvalPolicy': 'never', 'sandbox': 'read-only'})['thread']['id']
        template = json.loads((Path(__file__).resolve().parents[1] /
                              'tests/fixtures/codex-relocation/messages.json').read_text())
        items = json.loads(json.dumps(template).replace('OLD_PATH', str(old)))
        s.call('thread/inject_items', {'threadId': tid, 'items': items})
        s.close(); s = None
        # Fixture-only synthesis: injection has no real turn, so give persisted
        # synthetic messages a completed turn boundary before Codex rereads them.
        rollout = next((root / 'codex/sessions').rglob('*.jsonl'))
        lines = [json.loads(line) for line in rollout.read_text().splitlines()]
        stamp = lines[-1]['timestamp']
        start = {'timestamp': stamp, 'type': 'event_msg', 'payload':
                 {'type': 'task_started', 'turn_id': 'fixture-turn', 'model_context_window': 10000}}
        finish = {'timestamp': stamp, 'type': 'event_msg', 'payload':
                  {'type': 'task_complete', 'turn_id': 'fixture-turn', 'last_agent_message': items[1]['content'][0]['text']}}
        idx = next(i for i, line in enumerate(lines) if line['type'] == 'response_item')
        lines.insert(idx, start)
        lines.insert(idx + 1, {'timestamp': stamp, 'type': 'event_msg', 'payload':
                             {'type': 'user_message', 'message': items[0]['content'][0]['text'], 'images': [], 'local_images': []}})
        lines.append({'timestamp': stamp, 'type': 'event_msg', 'payload':
                      {'type': 'agent_message', 'message': items[1]['content'][0]['text'], 'phase': 'final_answer'}})
        lines.append(finish)
        if mode == 'paginated':
            for ordinal, line in enumerate(lines):
                line['ordinal'] = ordinal
        rollout.write_text(''.join(json.dumps(line) + '\n' for line in lines))
        # Drop disposable derived indexes so Codex rebuilds from the fixture log.
        for pattern in ('state_*.sqlite*', 'thread_history_*.sqlite*'):
            for database in (root / 'codex').glob(pattern):
                database.unlink()
        s = Server(binary, root, env)
        inventory(s)
        baseline = read(s, tid)
        if mode == 'paginated':
            history(s, tid)  # Initialize the disposable history database.
            # Paginated history is authoritative in SQLite; seed synthetic
            # completed items only in this fresh, stopped, disposable home.
            s.close(); s = None
            db = sqlite3.connect(root / 'codex/thread_history_1.sqlite')
            # The turn-list API requires a complete root-turn record, not just
            # item rows. Use the actual synthetic rollout bounds and identities.
            encoded = rollout.read_bytes().splitlines(keepends=True)
            start_index = next(i for i, line in enumerate(lines)
                               if line.get('payload', {}).get('type') == 'task_started')
            end_index = next(i for i, line in enumerate(lines)
                             if line.get('payload', {}).get('type') == 'task_complete')
            db.execute('''INSERT INTO thread_turns
                (thread_id,turn_id,rollout_ordinal,status,root_turn_id,
                 first_user_item_id,final_agent_item_id,rollout_byte_offset,
                 rollout_end_ordinal,rollout_end_byte_offset,started_at,completed_at,duration_ms)
                VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)''',
                (tid, 'fixture-turn', start_index, 'completed', 'fixture-turn',
                 'fixture-0', 'fixture-1', sum(map(len, encoded[:start_index])),
                 end_index, sum(map(len, encoded[:end_index+1])),
                 baseline['createdAt'], baseline['createdAt'], 0))
            for i, item in enumerate(items):
                projected = ({'type': 'userMessage', 'id': f'fixture-{i}', 'content':
                              [{'type': 'text', 'text': item['content'][0]['text'], 'text_elements': []}]}
                             if i == 0 else {'type': 'agentMessage', 'id': f'fixture-{i}',
                                             'text': item['content'][0]['text'], 'phase': 'final_answer'})
                db.execute('INSERT INTO thread_items (thread_id,turn_id,item_id,rollout_ordinal,item_json,item_type,created_at_ms) VALUES (?,?,?,?,?,?,?)',
                           (tid, 'fixture-turn', f'fixture-{i}', i + 1, json.dumps(projected), projected['type'], 0))
            db.commit(); db.close()
            s = Server(binary, root, env)
        before = history(s, tid)
        assert all(item['content'][0]['text'] in json.dumps(before) for item in items), 'fixture history incomplete'
        turns = s.call('thread/turns/list', {'threadId': tid})['data']
        assert all(item['content'][0]['text'] in json.dumps(turns) for item in items), 'fixture absent from turn summaries'
        assert baseline['cwd'] == str(old), 'fixture read directory mismatch'
        assert baseline['historyMode'] == mode, 'fixture history mode mismatch'
        assert any(t['id'] == tid and t['cwd'] == str(old) for t in inventory(s)), 'fixture absent or wrong directory in listing'
        assert s.call('thread/resume', {'threadId': tid, 'excludeTurns': True})['thread']['cwd'] == str(old)
        s.close(); s = None
        return {'env': env, 'baseline': baseline, 'history_before': before,
                'expected_messages': [item['content'][0]['text'] for item in items]}
    finally:
        if s is not None:
            s.close()


def run_case(binary, root, mode, scenario, strategy="resume", prepare_only=False):
    root.mkdir()
    old, new = root / 'old', root / 'new folder'
    result = {'history_mode': mode, 'scenario': scenario, 'strategy': strategy, 'outcome': 'inconclusive'}
    s = None
    peer = None
    validated = False
    try:
        fixture = _populate_fixture(binary, root, mode)
        env = fixture['env']
        baseline, before = fixture['baseline'], fixture['history_before']
        tid = baseline['id']
        result.update(baseline=baseline, history_before=before)
        s = Server(binary, root, env)
        validated = True
        if prepare_only:
            result['outcome'] = 'pass'
            result['fixture_only'] = True
            return result
        if scenario == 'missing-old':
            old.rmdir()
        if scenario in ('archived', 'archive-cycle'):
            s.call('thread/archive', {'threadId': tid})
        if scenario == 'archive-cycle':
            s.call('thread/unarchive', {'threadId': tid})
        if scenario == 'project' or strategy == 'project-resume':
            created = s.call('project/create', {'idempotencyKey': tid, 'name': 'Fixture',
                                               'roots': [{'path': str(old)}]})
            result['project_created'] = created
            project = created.get('project', created)
            pid = project['id']
            s.call('thread/metadata/update', {'threadId': tid, 'projectId': pid})
            result['project_updated'] = s.call('project/update', {'projectId': pid, 'roots': [{'path': str(new)}]})
            result['cwd_after_project_only'] = read(s, tid)['cwd']
        if scenario == 'independent-server':
            peer = Server(binary, root, env)
            result['peer_before'] = peer.call('thread/resume', {'threadId': tid, 'excludeTurns': True})
            result['primary_loaded'] = s.call('thread/loaded/list')
            result['peer_loaded'] = peer.call('thread/loaded/list')
        if scenario == 'loaded':
            s.call('thread/resume', {'threadId': tid, 'excludeTurns': True})
        result['config_before'] = (root / 'codex/config.toml').read_text()
        result['immediate'] = relocate(s, tid, new, strategy)
        if scenario == 'independent-server':
            result['peer_after'] = peer.call('thread/resume', {'threadId': tid, 'excludeTurns': True})
            peer.close(); peer = None
        if scenario == 'archive-cycle':
            s.call('thread/archive', {'threadId': tid})
        s.close(); s = Server(binary, root, env)
        result['after_restart'] = read(s, tid)
        result['listed'] = inventory(s, scenario in ('archived', 'archive-cycle'))
        if scenario == 'archive-cycle':
            s.call('thread/unarchive', {'threadId': tid})
        result['resumed_without_override'] = s.call('thread/resume', {'threadId': tid, 'excludeTurns': True})
        result['history_after'] = history(s, tid)
        result['config_after'] = (root / 'codex/config.toml').read_text()
        checks = {
            'read_cwd': result['after_restart']['cwd'] == str(new),
            'resume_cwd': result['resumed_without_override']['thread']['cwd'] == str(new),
            'listing_cwd': any(t['id'] == tid and t['cwd'] == str(new) for t in result['listed']),
            'history_preserved': before == result['history_after'],
            'identity_preserved': result['after_restart']['id'] == tid,
            'config_preserved': result['config_before'] == result['config_after'],
            'archive_state_preserved': any(t['id'] == tid for t in result['listed']) and not any(t['id'] == tid for t in inventory(s, True)) if scenario not in ('archived', 'archive-cycle') else any(t['id'] == tid for t in result['listed']),
        }
        checks['metadata_preserved'] = all(
            baseline.get(k) == result['after_restart'].get(k)
            for k in ('id', 'name', 'createdAt', 'model', 'modelProvider', 'gitInfo', 'historyMode'))
        if scenario in ('loaded', 'independent-server'):
            # Accepting a loaded resume, even while ignoring cwd, is not exclusion.
            checks['loaded_session_excluded'] = False
        result['checks'] = checks
        s.close(); s = Server(binary, root, env)
        result['reverse'] = relocate(s, tid, old, strategy)
        s.close(); s = Server(binary, root, env)
        result['reverse_after_restart'] = read(s, tid)['cwd']
        checks['reversal_persisted'] = result['reverse_after_restart'] == str(old)
        if scenario == 'project' or strategy == 'project-resume':
            result['project_after_restart'] = s.call('project/read', {'projectId': pid})
        if scenario == 'archive-cycle':
            s.call('thread/archive', {'threadId': tid})
            checks['archive_restored'] = any(t['id'] == tid for t in inventory(s, True))
        result['outcome'] = 'pass' if all(checks.values()) else 'fail'
    except (RPCError, AssertionError, OSError, ValueError, KeyError, sqlite3.Error) as e:
        result['error'] = str(e)
        result['outcome'] = 'fail' if validated else 'inconclusive'
    finally:
        if peer is not None:
            peer.close()
        if s is not None:
            s.close()
        (root / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--codex', default='codex')
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--repeat', type=int, default=2)
    p.add_argument('--strategy', choices=STRATEGIES, action='append', help='repeat to select strategies; default: resume')
    p.add_argument('--scenario', choices=SCENARIOS, action='append', help='repeat to select scenarios; default: original seven')
    p.add_argument('--investigate', action='store_true', help='run baseline first, then all selected strategies through basic gates')
    args = p.parse_args()
    if args.repeat < 1:
        p.error('--repeat must be positive')
    binary = shutil.which(args.codex)
    if not binary:
        p.error('Codex executable not found')
    binary = str(Path(binary).resolve())
    if Path(binary).read_bytes()[:4] != b'\x7fELF':
        p.error('--codex must point to the installed Linux binary, not a package-manager wrapper')
    args.output.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='tracr-probe-', dir=args.output.resolve()))
    env = environment(root)
    version = subprocess.check_output([binary, '--version'], env=env, cwd=root, text=True).strip()
    schema = root / 'schema'
    subprocess.run([binary, 'app-server', 'generate-json-schema', '--experimental', '--out', str(schema)],
                   env=env, cwd=root, check=True, capture_output=True, timeout=30)
    digest = hashlib.sha256()
    for f in sorted(schema.rglob('*.json')):
        digest.update(str(f.relative_to(schema)).encode()); digest.update(f.read_bytes())
    definitions = json.loads((schema / 'codex_app_server_protocol.v2.schemas.json').read_text())['definitions']
    report = {'report_version': 2, 'production_relocation_enabled': False,
              'untested': DEFERRED, 'version': version, 'executable': binary,
              'executable_sha256': hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
              'schema_sha256': digest.hexdigest(), 'cases': [], 'strategy_verdicts': {},
              'schema_fields': {name: sorted(definitions.get(name, {}).get('properties', {})) for name in
                  ('ThreadResumeParams', 'ThreadSettingsUpdateParams', 'ThreadMetadataUpdateParams',
                   'ThreadUnsubscribeParams', 'ProjectUpdateParams')}}
    strategies = list(dict.fromkeys(args.strategy or (STRATEGIES if args.investigate else ['resume'])))
    if args.investigate:
        strategies = ['resume'] + [s for s in strategies if s != 'resume']
    scenarios = list(dict.fromkeys(args.scenario or (['ordinary'] if args.investigate else SCENARIOS)))
    if args.investigate:
        scenarios = ['ordinary'] + [s for s in scenarios if s != 'ordinary']
    report['requested'] = {'strategies': strategies, 'scenarios': scenarios, 'repeat': args.repeat}
    for strategy in strategies:
        # Complete ordinary cases in both formats before any optional expanded scenarios.
        for scenario in scenarios:
            if args.investigate and scenario != 'ordinary' and basic_verdict(report['cases'], strategy, args.repeat) != 'pass':
                continue
            for repeat in range(args.repeat):
                for mode in ('legacy', 'paginated'):
                    missing = strategy_requirements(definitions, strategy)
                    if missing:
                        case = {'strategy': strategy, 'scenario': scenario, 'history_mode': mode,
                                'outcome': 'inconclusive', 'error': 'Schema lacks required fields: ' + ', '.join(missing)}
                    else:
                        case = run_case(binary, root / f'{strategy}-{repeat}-{mode}-{scenario}', mode, scenario, strategy)
                    case['repeat'] = repeat
                    report['cases'].append(case)
                    report['strategy_verdicts'][strategy] = basic_verdict(report['cases'], strategy, args.repeat)
                    print(strategy, repeat, mode, scenario, case['outcome'], case.get('error', case.get('checks')), flush=True)
                    (root / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
    report['skipped_scenarios'] = [{'strategy': strategy, 'scenario': scenario, 'reason': 'basic persistence gate did not pass'}
        for strategy in strategies for scenario in scenarios if scenario != 'ordinary' and args.investigate
        and basic_verdict(report['cases'], strategy, args.repeat) != 'pass']
    report['coverage_note'] = 'Basic gates are not production approval. Extended coverage remains explicitly untested.'
    (root / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f'Evidence: {root}')
    return exit_code(report['cases'])


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except (OSError, subprocess.SubprocessError, ValueError) as error:
        print(f'Setup failed: {error}', file=__import__('sys').stderr)
        raise SystemExit(2)
