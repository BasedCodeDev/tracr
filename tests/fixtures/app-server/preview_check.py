"""Real read-only Git previews against an independent Codex owner and local model.
No credentials, external model requests, or user sessions. Checks both history formats.
"""
import http.server
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import threading
import time
from pty_support import Terminal
from real_status_check import probe
from diagnostics import run_cli, scratch_dir
from check_codex_release import connect


def check(binary, codex, root, mode):
    fixture = probe.prepare_fixture(codex, root, mode)
    env = {**fixture['env'], 'PATH': '/usr/bin:/bin'}
    project = root / 'old'
    subprocess.run(['git', 'init', '-q', str(project)], env=env, check=True)
    subprocess.run(['git', '-C', str(project), '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
                    'commit', '--allow-empty', '-qm', 'PREVIEW_ANCHOR'], env=env, check=True)
    requests = []
    state = {'approval': False, 'issued': False}
    stream_release = threading.Event()
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass
        def do_POST(self):
            requests.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))))
            n = len(requests)
            item = {'type': 'message', 'id': f'msg{n}', 'role': 'assistant', 'status': 'completed',
                    'content': [{'type': 'output_text', 'text': f'EXTERNAL_REPLY_{n}', 'annotations': []}]}
            if state['approval'] and not state['issued']:
                state['issued'] = True
                item = {'type': 'function_call', 'id': f'fc{n}', 'call_id': f'call{n}', 'name': 'exec_command',
                        'arguments': json.dumps({'cmd': 'printf PREVIEW_APPROVAL_EXECUTED', 'sandbox_permissions': 'require_escalated',
                                                'justification': 'Disposable preview test', 'max_output_tokens': 100})}
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.end_headers()
            def emit(name, data):
                data['type'] = name
                self.wfile.write(f'event: {name}\ndata: {json.dumps(data)}\n\n'.encode())
                self.wfile.flush()
            if n == 1:
                emit('response.created', {'response': {'id': 'resp1', 'status': 'in_progress', 'output': []}})
                emit('response.output_item.added', {'output_index': 0, 'item': {**item, 'status': 'in_progress', 'content': []}})
                emit('response.content_part.added', {'output_index': 0, 'item_id': item['id'], 'content_index': 0,
                                                    'part': {'type': 'output_text', 'text': '', 'annotations': []}})
                emit('response.output_text.delta', {'output_index': 0, 'item_id': item['id'], 'content_index': 0,
                                                   'delta': 'LIVE_STREAM_PREFIX'})
                assert stream_release.wait(20), 'test did not release stream'
                item['content'][0]['text'] = 'LIVE_STREAM_PREFIX EXTERNAL_REPLY_1'
            for name, data in [
                ('response.created', {'response': {'id': f'resp{n}', 'status': 'in_progress', 'output': []}}),
                ('response.output_item.done', {'output_index': 0, 'item': item}),
                ('response.completed', {'response': {'id': f'resp{n}', 'status': 'completed', 'output': [item],
                                                    'usage': {'input_tokens': 1, 'output_tokens': 1, 'total_tokens': 2}}}),
            ]:
                data['type'] = name
                self.wfile.write(f'event: {name}\ndata: {json.dumps(data)}\n\n'.encode())
                self.wfile.flush()
    model = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=model.serve_forever, daemon=True).start()
    config = root / 'codex/config.toml'
    config.write_text(config.read_text().replace('http://127.0.0.1:1/v1', f'http://127.0.0.1:{model.server_port}/v1'))
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    log = (root / 'daemon.log').open('w')
    daemon = subprocess.Popen([codex, 'app-server', '--listen', f'ws://127.0.0.1:{port}'], env=env, cwd=root, stdout=log, stderr=log)
    wrapper = root / 'codex-proxy'
    wrapper.write_text('#!/usr/bin/env python3\nimport os,sys,socket,select\n' +
        f'if sys.argv[1:] != ["app-server","proxy"]: os.execv({codex!r},[{codex!r}]+sys.argv[1:])\n' +
        f's=socket.create_connection(("127.0.0.1",{port}))\n' +
        'while True:\n ready,_,_=select.select([s,sys.stdin.buffer],[],[])\n for stream in ready:\n  data=os.read(stream.fileno(),65536)\n  if not data: sys.exit(0)\n  if stream is s: sys.stdout.buffer.write(data);sys.stdout.buffer.flush()\n  else: s.sendall(data)\n')
    wrapper.chmod(0o755)
    owner = terminal = None
    tid = fixture['baseline']['id']
    checks = []
    try:
        deadline = time.monotonic() + 12
        while owner is None:
            try:
                owner = connect(codex, env, port)
            except OSError:
                assert time.monotonic() < deadline
                time.sleep(.1)
        baseline = owner.call('thread/resume', {'threadId': tid, 'excludeTurns': True})
        terminal = Terminal([binary, '--codex-home', env['CODEX_HOME'], '--codex-bin', str(wrapper), str(project)], env)
        terminal.resize(40, 160)
        terminal.expect('PREVIEW_ANCHOR')
        terminal.send(b'\x1b`')
        terminal.send(b'\x1b[1;3C')
        terminal.expect_action('Select worktree folder')
        terminal.send(b'\x1b[B')
        terminal.expect('read-only')
        terminal.expect('You: Remember the synthetic project')
        terminal.expect('Assistant: The synthetic project')
        assert not requests, 'highlighting started a model turn'
        checks.append('highlight renders external history without starting a turn')
        def turn(text):
            owner.call('turn/start', {'threadId': tid, 'input': [{'type': 'text', 'text': text, 'text_elements': []}],
                                     'approvalPolicy': 'on-request', 'sandboxPolicy': {'type': 'readOnly'}})
        turn('Synthetic external update')
        terminal.expect('LIVE_STREAM_PREFIX')
        terminal.send(b'\x1b[A')
        terminal.wait(lambda: 'LIVE_STREAM_PREFIX' not in terminal.text())
        stream_release.set()
        deadline = time.monotonic() + 10
        while owner.call('thread/read', {'threadId': tid, 'includeTurns': False})['thread']['status']['type'] != 'idle':
            assert time.monotonic() < deadline
            time.sleep(.1)
        terminal.send(b'\x1b[B')
        terminal.expect('EXTERNAL_REPLY_1')
        checks.append('streaming deltas render; detaching mid-turn preserves owner completion')
        terminal.send(b'\x1b[A')
        terminal.wait(lambda: 'EXTERNAL_REPLY_1' not in terminal.text())
        assert tid in owner.call('thread/loaded/list', {})['data']
        terminal.send(b'\x1b[B')
        terminal.expect('EXTERNAL_REPLY_1')
        checks.append('commit restores session list; reselect restores external transcript')
        state['approval'] = True
        turn('Synthetic approval')
        deadline = time.monotonic() + 10
        approval = None
        while approval is None:
            message = owner.q.get(timeout=max(.01, deadline-time.monotonic()))
            if isinstance(message, dict) and 'id' in message and 'requestApproval' in message.get('method', ''):
                approval = message
        terminal.expect('interactive client')
        assert len(requests) == 2, 'preview answered approval'
        owner.send({'id': approval['id'], 'result': {'decision': 'decline'}})
        terminal.expect('EXTERNAL_REPLY_3')
        checks.append('preview leaves approval unanswered; owner resolves it while preview stays attached')
        terminal.send(b'\x1b')
        terminal.wait(lambda: 'EXTERNAL_REPLY_3' not in terminal.text())
        turn('Owner still works after leaving Git')
        deadline = time.monotonic() + 10
        while len(requests) < 4:
            assert time.monotonic() < deadline
            time.sleep(.1)
        while owner.call('thread/read', {'threadId': tid, 'includeTurns': False})['thread']['status']['type'] != 'idle':
            assert time.monotonic() < deadline
            time.sleep(.1)
        current = owner.call('thread/resume', {'threadId': tid, 'excludeTurns': True})
        for field in ['cwd', 'model', 'modelProvider']:
            assert current[field] == baseline[field], field
        assert len(requests) == 4
        checks.append('leaving preview preserves owner and produces no duplicate turns')
        print(json.dumps({'mode': mode, 'status': 'passed', 'checks': checks}), flush=True)
        (root / 'results.json').write_text(json.dumps({'status': 'passed', 'mode': mode, 'checks': checks}, indent=2))
    finally:
        stream_release.set()
        if terminal:
            (root / 'terminal.bin').write_bytes(terminal.output)
            terminal.close()
        if owner:
            owner.close()
        daemon.terminate()
        try:
            daemon.wait(timeout=8)
        except subprocess.TimeoutExpired:
            daemon.kill()
            daemon.wait()
        model.shutdown()
        model.server_close()
        log.close()


def main():
    binary, codex = (str(Path(p).resolve()) for p in sys.argv[1:])
    root = Path(scratch_dir(prefix='tracr-preview-'))
    for mode in ['legacy', 'paginated']:
        check(binary, codex, root / mode, mode)


if __name__ == '__main__':
    raise SystemExit(run_cli(main))
