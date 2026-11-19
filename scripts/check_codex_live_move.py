#!/usr/bin/env python3
"""Opt-in live-move acceptance against real Codex, disposable homes and a local model.
No credentials, external inference, or user sessions. Scratch stays in .tmp/.
"""
import argparse
import http.server
import json
import os
from pathlib import Path
import socket
import subprocess
import threading
import time
from diagnostics import run_cli, scratch_dir
import probe_codex_relocation as probe
import probe_codex_profiles as profiles
from check_codex_release import connect


def check(binary, tracr, root, mode, profile, active):
    fixture = probe.prepare_fixture(binary, root, mode)
    env = fixture['env']
    tid = fixture['baseline']['id']
    received, release = threading.Event(), threading.Event()

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            self.rfile.read(int(self.headers.get('Content-Length', '0')))
            received.set()
            release.wait(90)
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.end_headers()
            payload = {'type': 'response.completed', 'response': {
                'id': 'local', 'status': 'completed', 'output': []}}
            try:
                self.wfile.write(('event: response.completed\ndata: ' + json.dumps(payload) + '\n\n').encode())
            except OSError:
                pass

    model = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=model.serve_forever, daemon=True).start()
    config = root / 'codex/config.toml'
    config.write_text(config.read_text().replace('http://127.0.0.1:1/v1', f'http://127.0.0.1:{model.server_port}/v1'))
    original_config = config.read_bytes()
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    log = (root / 'daemon.log').open('w')
    daemon = subprocess.Popen([binary, 'app-server', '--listen', f'ws://127.0.0.1:{port}'], env=env, cwd=root, stdout=log, stderr=log)
    owner = None
    wrapper = root / 'codex-proxy'
    wrapper.write_text('#!/usr/bin/env python3\nimport os,sys,socket,select\n' +
                      f'if sys.argv[1:] != ["app-server","proxy"]: os.execv({binary!r},[{binary!r}]+sys.argv[1:])\n' +
                      f's=socket.create_connection(("127.0.0.1",{port}))\n' +
                      'while True:\n ready,_,_=select.select([s,sys.stdin.buffer],[],[])\n for stream in ready:\n  data=os.read(stream.fileno(),65536)\n  if not data: sys.exit(0)\n  if stream is s: sys.stdout.buffer.write(data);sys.stdout.buffer.flush()\n  else: s.sendall(data)\n')
    wrapper.chmod(0o755)
    target = root / 'new folder'

    def move(*extra, code=0):
        result = subprocess.run([tracr, '--codex-bin', str(wrapper), 'move-session', tid, str(target), '--live', *extra], env=env, cwd=root, capture_output=True, text=True, timeout=160)
        assert result.returncode == code, result.stdout + result.stderr
        report = json.loads(result.stdout)
        assert report['schema_version'] == 1
        return report

    try:
        deadline = time.monotonic() + 15
        while owner is None:
            try:
                owner = connect(binary, env, port)
            except (OSError, RuntimeError):
                if time.monotonic() >= deadline or daemon.poll() is not None:
                    raise
                time.sleep(.1)
        owner.call('thread/resume', {'threadId': tid, 'excludeTurns': True})
        if profile != 'legacy':
            owner.call('thread/settings/update', {'threadId': tid, 'permissions': ':' + profile})
            profiles.settle(owner, tid, lambda r: (r.get('activePermissionProfile') or {}).get('id') == ':' + profile)
        before = owner.call('thread/resume', {'threadId': tid, 'excludeTurns': True})
        if active:
            owner.call('turn/start', {'threadId': tid, 'input': [{'type': 'text', 'text': 'Local relocation test', 'text_elements': []}]})
            assert received.wait(15), 'Local provider not reached'
        history_before = probe.history(owner, tid)
        assert move('--from', str(root / 'wrong'), code=1)['status'] == 'rejected'
        assert move('--dry-run')['status'] == 'preview'
        assert owner.call('thread/read', {'threadId': tid})['thread']['cwd'] == str(root / 'old')
        report = move('--from', str(root / 'old'))
        assert report['status'] == 'moved', report
        assert json.loads(Path(report['journal']).read_text())['phase'] == 'complete'
        after = owner.call('thread/resume', {'threadId': tid, 'excludeTurns': True})
        assert after['cwd'] == str(target)
        assert after['runtimeWorkspaceRoots'] == [str(target)]
        for field in ['sandbox', 'approvalPolicy', 'approvalsReviewer', 'activePermissionProfile', 'model', 'modelProvider']:
            assert before.get(field) == after.get(field), field
        assert probe.history(owner, tid) == history_before, 'History changed during relocation'
        if active:
            assert after['thread']['status']['type'] == 'active', 'Move interrupted the active turn'
        assert move('--from', str(root / 'old'))['status'] == 'already_at_destination'
        release.set()
        deadline = time.monotonic() + 10
        while owner.call('thread/read', {'threadId': tid})['thread']['status']['type'] != 'idle':
            assert time.monotonic() < deadline
            time.sleep(.1)
        assert config.read_bytes() == original_config
    finally:
        release.set()
        if owner:
            owner.close()
        daemon.terminate()
        try:
            daemon.wait(timeout=8)
        except subprocess.TimeoutExpired:
            daemon.kill()
            daemon.wait()
        log.close()
        model.shutdown()
        model.server_close()
    # A fresh independent writer must also resolve the persisted destination.
    fresh = probe.Server(binary, root, env)
    try:
        persisted = fresh.call('thread/resume', {'threadId': tid, 'excludeTurns': True})
        assert persisted['cwd'] == str(target)
        assert persisted['runtimeWorkspaceRoots'] == [str(target)]
        for field in ['sandbox', 'approvalPolicy', 'approvalsReviewer', 'activePermissionProfile']:
            assert persisted.get(field) == before.get(field), field
    finally:
        fresh.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--codex', required=True)
    parser.add_argument('--tracr', required=True)
    parser.add_argument('--keep-temp', action='store_true')
    args = parser.parse_args()
    scratch = Path(__file__).resolve().parents[1] / '.tmp'
    scratch.mkdir(exist_ok=True)
    root = Path(scratch_dir(prefix='live-move-', dir=scratch))
    binary, tracr = (str(Path(p).resolve(strict=True)) for p in (args.codex, args.tracr))
    results = []
    for mode in ['legacy', 'paginated']:
        for profile in ['legacy', 'workspace', 'read-only']:
            for active in [False, True]:
                name = f'{mode}-{profile}-{"active" if active else "idle"}'
                try:
                    check(binary, tracr, root / name, mode, profile, active)
                    result = {'case': name, 'status': 'passed'}
                except Exception as error:
                    result = {'case': name, 'status': 'failed', 'error': str(error)}
                results.append(result)
                print(json.dumps(result), flush=True)
                (root / 'results.json').write_text(json.dumps(results, indent=2))
    return int(any(r['status'] != 'passed' for r in results))


if __name__ == '__main__':
    raise SystemExit(run_cli(main))
