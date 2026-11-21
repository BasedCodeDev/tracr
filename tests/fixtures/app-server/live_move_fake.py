#!/usr/bin/env python3
"""Live relocation fault injection, only for disposable CLI tests."""
import base64
import hashlib
import json
import os
from pathlib import Path
import runpy
import struct
import sys

if sys.argv[1:] == ['--version']:
    print('codex-cli 0.170.0')
    raise SystemExit
if sys.argv[1:] != ['app-server', 'proxy']:
    runpy.run_path(str(Path(__file__).with_name('relink_fake.py')), run_name='__main__')
    raise SystemExit
home = Path(os.environ['CODEX_HOME'])
stream = sys.stdin.buffer
stream.readline()
headers = {}
while True:
    line = stream.readline().strip()
    if not line:
        break
    name, value = line.split(b':', 1)
    headers[name.lower()] = value.strip()
accept = base64.b64encode(hashlib.sha1(headers[b'sec-websocket-key'] + b'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest())
sys.stdout.buffer.write(b'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ' + accept + b'\r\n\r\n')
sys.stdout.buffer.flush()
while True:
    header = stream.read(2)
    if not header or header[0] & 15 == 8:
        break
    size = header[1] & 127
    if size == 126:
        size = struct.unpack('!H', stream.read(2))[0]
    if size == 127:
        size = struct.unpack('!Q', stream.read(8))[0]
    mask = stream.read(4)
    request = json.loads(bytes(v ^ mask[i % 4] for i, v in enumerate(stream.read(size))))
    if 'id' not in request:
        continue
    with (home / 'live_requests.jsonl').open('a') as log:
        log.write(json.dumps(request) + '\n')
    state = json.loads((home / 'native.json').read_text())
    scenario = state.get('scenario', '')
    cwd = state['cwd']
    thread = dict(id='session', cwd=cwd, createdAt=100, historyMode='legacy',
                  canAcceptDirectInput=True, status={'type': 'active', 'activeFlags': []},
                  path=str(home / 'sessions/fixture.jsonl'),
                  environments=[{'environmentId': 'local', 'cwd': cwd, 'runtimeWorkspaceRoots': [cwd]}])
    method = request['method']
    if method == 'initialize':
        version = '0.169.0' if scenario == 'old-daemon' else '0.170.0'
        result = {'userAgent': 'tracr/' + version, 'codexHome': str(home)}
    elif method == 'thread/loaded/list':
        result = {'data': [] if scenario == 'unloaded' else ['session'], 'nextCursor': None}
    elif method == 'thread/read':
        result = {'thread': thread}
    elif method == 'thread/resume':
        assert set(request['params']) == {'threadId', 'excludeTurns'}
        result = dict(thread=thread, cwd=cwd, runtimeWorkspaceRoots=[cwd], model='probe', modelProvider='probe',
                      sandbox={'type': 'readOnly', 'networkAccess': False}, approvalPolicy='never', activePermissionProfile=None)
        if scenario == 'extra-root':
            result['runtimeWorkspaceRoots'].append('/extra')
        if state.get('changed_settings'):
            result['model'] = 'different'
    elif method == 'thread/settings/update':
        assert set(request['params']) == {'threadId', 'cwd'}
        state['cwd'] = request['params']['cwd']
        if scenario == 'settings-race':
            state['changed_settings'] = True
        (home / 'native.json').write_text(json.dumps(state))
        if scenario == 'lost-ack':
            break
        result = {}
    else:
        raise AssertionError('Forbidden live operation: ' + method)
    data = json.dumps({'id': request['id'], 'result': result}).encode()
    prefix = bytes([129, len(data)]) if len(data) < 126 else bytes([129, 126]) + struct.pack('!H', len(data))
    sys.stdout.buffer.write(prefix + data)
    sys.stdout.buffer.flush()
