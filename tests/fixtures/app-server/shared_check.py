"""Opt-in native sharing check: two Tracr UIs and a local scripted model.
Uses a disposable Codex home, no credentials, and no external model inference.
"""
import http.server
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import threading
import time

from pty_support import Terminal
from real_check import history_visible, stop_daemons, submit_status, verify_status
from real_status_check import Owner, probe
from diagnostics import run_cli, scratch_dir


def main():
    binary, codex = (str(Path(p).resolve()) for p in sys.argv[1:])
    root = Path(scratch_dir(prefix='tracr-shared-'))
    home = root / 'fixture'
    clients = []
    owner = server = None
    result = {'status': 'failed', 'checks': []}
    env = None
    try:
        fixture = probe.prepare_fixture(codex, home, 'legacy')
        env = {**fixture['env'], 'PATH': '/usr/bin:/bin'}
        project = home / 'old'
        # Confine Git discovery and trust to the disposable project, even when
        # the test scratch directory is itself inside a developer checkout.
        subprocess.run(['git', 'init', '-q', str(project)], env=env, check=True)
        config = home / 'codex/config.toml'
        requests = []
        approval = {"issued": False}
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass
            def do_GET(self):
                self.send_response(200)
                self.end_headers()
                self.wfile.write(b'{"data":[]}')
            def do_POST(self):
                requests.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))))
                markers = re.findall(r"SHARED_INPUT_(\d+)", json.dumps(requests[-1]))
                number = max(map(int, markers), default=0)
                item = {'type': 'message', 'id': f'msg_{number}', 'role': 'assistant',
                        'status': 'completed', 'content': [{'type': 'output_text',
                        'text': f'SHARED_REPLY_{number}', 'annotations': []}]}
                if 'SHARED_APPROVAL' in json.dumps(requests[-1]):
                    if not approval['issued'] and any(t.get('name') == 'exec_command' for t in requests[-1].get('tools', [])):
                        approval['issued'] = True
                        item = {'type': 'function_call', 'id': 'fc_shared_approval', 'call_id': 'call_shared_approval',
                                'name': 'exec_command', 'arguments': json.dumps({
                                    'cmd': 'printf TRACR_APPROVAL_EXECUTED', 'sandbox_permissions': 'require_escalated',
                                    'justification': 'Allow the disposable shared-session approval check?',
                                    'max_output_tokens': 100})}
                    else:
                        item['content'][0]['text'] = 'SHARED_APPROVAL_DONE'
                events = [
                    ('response.created', {'response': {'id': f'resp_{number}', 'status': 'in_progress', 'output': []}}),
                    ('response.output_item.done', {'output_index': 0, 'item': item}),
                    ('response.completed', {'response': {'id': f'resp_{number}', 'status': 'completed', 'output': [item],
                                                       'usage': {'input_tokens': 1, 'output_tokens': 1, 'total_tokens': 2}}}),
                ]
                self.send_response(200)
                self.send_header('Content-Type', 'text/event-stream')
                self.end_headers()
                for name, data in events:
                    data['type'] = name
                    self.wfile.write(f'event: {name}\ndata: {json.dumps(data)}\n\n'.encode())
                    self.wfile.flush()
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        config.write_text(config.read_text().replace('http://127.0.0.1:1/v1', f'http://127.0.0.1:{server.server_port}/v1')
                          + f'\n[projects.{json.dumps(str(project))}]\ntrust_level = "trusted"\n')
        tid = fixture['baseline']['id']
        result['version'] = subprocess.check_output([codex, '--version'], env=env, text=True).strip()
        command = [binary, '--codex-home', env['CODEX_HOME'], '--codex-bin', codex, str(project)]
        for _ in range(2):
            client = Terminal(command, env)
            clients.append(client)
            client.inspect('ID: ' + tid)
            client.send(b'\r')
            client.wait(lambda: history_visible(client.text(), fixture['expected_messages']), timeout=30)
            submit_status(client)
            client.wait(lambda: tid in client.text())
            verify_status(client.text(), tid, str(project))
        result['checks'].append('two ordinary resumes display the same session')
        first, second = clients
        second.resize(40, 120)
        first.resize(60, 180)
        result['checks'].append('independent terminal dimensions')
        # Connect using a relative path to stay below AF_UNIX's pathname limit.
        previous = Path.cwd()
        try:
            os.chdir(home)
            owner = Owner(codex, {**env, 'CODEX_HOME': 'codex'})
        finally:
            os.chdir(previous)
        assert tid in owner.call('thread/loaded/list', {})['data']
        for number, sender in enumerate([first, second], 1):
            prompt = f'SHARED_INPUT_{number}'
            sender.send(prompt.encode())
            sender.expect(prompt)
            # Let Codex's burst/paste detector settle before submitting.
            time.sleep(.3)
            sender.send(b'\r')
            for client in clients:
                client.expect(f'SHARED_REPLY_{number}')
        turns = owner.call('thread/read', {'threadId': tid, 'includeTurns': True})['thread']['turns']
        assert len(turns) == 3, 'expected fixture turn plus two UI turns'
        assert len({turn['id'] for turn in turns}) == 3, 'duplicate turns'
        result['checks'].append('input from either UI produces one response visible in both')
        second.send(b'\x02')
        second.expect('Open here')
        second.send(b'q')
        second.expect('Close all owned terminals and quit?')
        second.send(b'y')
        second.wait(lambda: second.child.poll() is not None)
        assert second.child.returncode == 0
        first.send(b'SHARED_INPUT_3')
        first.expect('SHARED_INPUT_3')
        time.sleep(.3)
        first.send(b'\r')
        first.expect('SHARED_REPLY_3')
        turns = owner.call('thread/read', {'threadId': tid, 'includeTurns': True})['thread']['turns']
        assert len(turns) == 4, 'expected one further UI turn'
        result['checks'].append('first UI remains interactive after second Tracr quits')
        # Raise an approval with one UI attached, then join from another Tracr.
        owner.call('turn/start', {'threadId': tid, 'input': [{'type': 'text', 'text': 'SHARED_APPROVAL', 'text_elements': []}],
                                 'approvalPolicy': 'on-request', 'sandboxPolicy': {'type': 'readOnly'}})
        first.expect('Allow the disposable shared-session approval check?')
        joining = Terminal(command, env)
        clients.append(joining)
        joining.inspect('ID: ' + tid)
        joining.send(b'\r')
        joining.expect('Allow the disposable shared-session approval check?')
        joining.send(b'\r')
        for client in [first, joining]:
            client.expect('SHARED_APPROVAL_DONE')
        turns = owner.call('thread/read', {'threadId': tid, 'includeTurns': True})['thread']['turns']
        assert len(turns) == 5
        result['checks'].append('newly attached UI resolves an existing approval for both views')
        result['status'] = 'passed'
    except Exception as error:
        result['error'] = str(error)
        raise
    finally:
        for index, client in enumerate(clients):
            (root / f'terminal-{index}.bin').write_bytes(client.output)
            client.close()
        if owner:
            owner.close()
        if env:
            stop_daemons(codex, env, home)
        if server:
            server.shutdown()
            server.server_close()
        (root / 'results.json').write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps(result, indent=2), flush=True)
        print('Shared-session evidence:', root, flush=True)


if __name__ == '__main__':
    raise SystemExit(run_cli(main))
