#!/usr/bin/env python3
"""Scripted metadata-only app server. Configuration comes from its disposable home."""
import json
import os
import sys
import time
from pathlib import Path
home = Path(os.environ['CODEX_HOME'])
settings = json.loads((home / 'fake.json').read_text())
# A tiny WebSocket peer for the byte-proxy path; standard-library only.
if sys.argv[1:] == ['app-server', 'proxy']:
    import base64
    import hashlib
    import struct
    if not settings.get('live_threads') and not settings.get('live_enabled'):
        sys.exit(0)
    stream = sys.stdin.buffer
    headers = {}
    stream.readline()
    while True:
        line = stream.readline().strip()
        if not line: break
        name, value = line.split(b':', 1)
        headers[name.lower()] = value.strip()
    accept = base64.b64encode(hashlib.sha1(headers[b'sec-websocket-key'] + b'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest())
    sys.stdout.buffer.write(b'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ' + accept + b'\r\n\r\n')
    sys.stdout.buffer.flush()
    with (home/'live_requests.jsonl').open('a', buffering=1) as log:
        while True:
            header = stream.read(2)
            if not header: break
            opcode = header[0] & 15
            length = header[1] & 127
            if length == 126: length = struct.unpack('!H', stream.read(2))[0]
            if length == 127: length = struct.unpack('!Q', stream.read(8))[0]
            mask = stream.read(4) if header[1] & 128 else None
            payload = stream.read(length)
            if mask: payload = bytes(v ^ mask[i%4] for i,v in enumerate(payload))
            if opcode == 8: break
            if opcode != 1: continue
            request = json.loads(payload)
            log.write(json.dumps(request)+'\n')
            settings = json.loads((home/'fake.json').read_text())
            if settings.get('live_disconnect'): break
            method = request['method']
            assert method in ('initialize', 'initialized', 'thread/loaded/list', 'thread/read'), method
            if method == 'initialized': continue
            threads = settings.get('live_threads', {})
            params = request['params']
            if method == 'initialize': result = {'userAgent':'live-fake'}
            elif method == 'thread/loaded/list':
                start = int(params.get('cursor') or 0)
                ids = list(threads)
                next_cursor = str(start+2) if start+2 < len(ids) else None
                if settings.get('live_repeat_cursor'): next_cursor = '2'
                result = {'data':ids[start:start+2], 'nextCursor':next_cursor}
            else:
                assert params['includeTurns'] is False
                tid = params['threadId']
                result = {'thread':{'id':tid, 'status':threads[tid], 'source':settings.get('live_source', 'cli')}}
            response = json.dumps({'id':request['id'], 'result':result}).encode()
            prefix = bytes([129, len(response)]) if len(response) < 126 else bytes([129,126])+struct.pack('!H',len(response))
            sys.stdout.buffer.write(prefix+response)
            sys.stdout.buffer.flush()
    sys.exit(0)
if len(sys.argv) > 1 and sys.argv[1] in ('resume', '--cd'):
    import signal
    import termios
    import tty
    if settings.get('ignore_term'):
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
    tty.setraw(0)
    (home / 'launch.json').write_text(json.dumps(dict(
        args=sys.argv[1:], cwd=os.getcwd(), home=os.environ['CODEX_HOME'],
        foreground=os.tcgetpgrp(0) == os.getpgrp(), pid=os.getpid())))
    while True:
        key = os.read(0, 1)
        if key == b'\x03':
            os.kill(os.getpid(), signal.SIGINT)
        if key == b'x':
            if sys.argv[1] == '--cd':
                (home / 'created.json').write_text(json.dumps({'cwd': os.getcwd()}))
            sys.exit(settings.get('resume_exit', 0))
with (home / 'requests.jsonl').open('a', buffering=1) as log:
    for line in sys.stdin:
        request = json.loads(line)
        log.write(json.dumps(request) + '\n')
        method = request['method']
        settings = json.loads((home / 'fake.json').read_text())
        if 'id' not in request:
            continue
        if method == 'initialize':
            result = {'userAgent': 'fake'}
        elif method == 'thread/list':
            if settings.get('exit'):
                sys.exit(7)
            if settings.get('timeout'):
                time.sleep(30)
            params = request['params']
            assert 'exec' in params['sourceKinds'] and 'subAgent' in params['sourceKinds']
            assert params['modelProviders'] == []
            assert 'cwd' not in params and 'projectId' not in params and 'sectionId' not in params
            archived = params['archived']
            cursor = params.get('cursor')
            suffix = 'archive' if archived else ('second' if cursor else 'first')
            item = dict(id=suffix, name='Task '+suffix, preview='', cwd=str(home / 'listed'),
                        createdAt=100, updatedAt=200, source='exec', gitInfo={'branch':'main'})
            result = {'data':[item], 'nextCursor':None if archived or cursor else 'page-2'}
            if (home / 'created.json').exists() and not archived and not cursor:
                result['data'].insert(0, dict(item, id='created', name='New session', cwd=json.loads((home / 'created.json').read_text())['cwd'], updatedAt=300))
            if settings.get('guardians'):
                guardian = dict(item, id='guardian-'+suffix, name='Internal review', source={'subAgent':{'other':'guardian'}})
                if not archived and not cursor:
                    result['data'] = [guardian]  # A hidden-only page must not end pagination.
                else:
                    result['data'].extend([guardian, dict(guardian, id='legacy-'+suffix, source={'subagent':{'other':'guardian'}})])
                    result['data'].append(dict(item, id='other-'+suffix, source={'subAgent':{'other':'worker'}}))
                    item['name'] = 'Guardian review'  # Ordinary sessions with this name must remain visible.
            if settings.get('malformed'):
                result['data'].append({'id':'invalid'})
            print(json.dumps({'method':'thread/status/changed','params':{}}), flush=True)
        elif method == 'thread/read':
            assert request['params']['includeTurns'] is False
            if settings.get('read_wait'):
                while not (home / 'release').exists():
                    time.sleep(.02)
            result = {'thread':dict(id=request['params']['threadId'], name='Task', cwd=settings.get('read_cwd', str(home/'resume')), canAcceptDirectInput=settings.get('direct', True), createdAt=100, updatedAt=200, source='exec')}
            if settings.get('guardian_read'):
                result['thread']['source'] = {'subAgent':{'other':'guardian'}}
        else:
            raise AssertionError('Forbidden RPC method: '+method)
        print(json.dumps({'id':request['id'], 'result':result}), flush=True)
