#!/usr/bin/env python3
"""Disposable shared-server peer for explicit release; stdio delegates to relink fake."""
import json, os, sys, time, runpy
from pathlib import Path
if sys.argv[1:]==['--version']:
    print('codex-cli 0.169.0');raise SystemExit
home=Path(os.environ['CODEX_HOME'])
settings=json.loads((home/'native.json').read_text())
if sys.argv[1:]==['app-server','proxy']:
    import base64
    import hashlib
    import struct
    if settings.get('unavailable'):
        sys.exit(0)
    (home/'proxy.pid').write_text(str(os.getpid()))
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
            settings = json.loads((home/'native.json').read_text())
            if settings.get('live_disconnect'): break
            method = request['method']
            if method == 'initialized': continue
            params = request.get('params', {})
            if method == 'initialize': result = {}
            elif method == 'thread/loaded/list':
                result = {'data':['session'] if settings.get('loaded') else [], 'nextCursor':None}
            elif method == 'thread/read':
                result = {'thread':dict(id='session', createdAt=100, cwd=settings['cwd'], path=str(home/'sessions/fixture.jsonl'), status=settings.get('status',{'type':'idle'}))}
            elif method == 'thread/backgroundTerminals/list':
                entries=settings.get('terminals', [])
                if entries is True: entries=[dict(processId='p1',itemId='i1',command='sleep 300',cwd=settings['cwd'])]
                if entries is False: entries=[]
                start=int(params.get('cursor') or 0);size=settings.get('terminal_page_size',100)
                result={'data':entries[start:start+size], 'nextCursor':str(start+size) if start+size<len(entries) else None}
                if settings.get('malformed_terminals'): result={'data':None}
                if settings.get('repeated_cursor'): result={'data':[], 'nextCursor':'1'}
            elif method == 'thread/backgroundTerminals/terminate':
                entries=settings.get('terminals', [])
                if entries is True: entries=[dict(processId='p1',itemId='i1',command='sleep 300',cwd=settings['cwd'])]
                mode=settings.get('terminate_mode','success')
                if mode != 'remains': entries=[e for e in entries if e['processId']!=params['processId']]
                if settings.get('new_terminal'): entries.append(settings.pop('new_terminal'))
                if settings.get('active_after_stop'): settings['status']={'type':'active','activeFlags':[]}
                settings['terminals']=entries
                (home/'native.json').write_text(json.dumps(settings))
                result={} if mode=='malformed' else {'terminated':mode!='false_removed'}
            elif method == 'thread/resume':
                assert params == {'threadId':'session','excludeTurns':True,'config':{'thread_unload_delay_secs':0}},params
                if settings.get('stall'): time.sleep(60)
                time.sleep(settings.get('release_delay',0))
                result = {'thread':{'id':'session','createdAt':100},'cwd':settings['cwd'],'runtimeWorkspaceRoots':[settings['cwd']], 'sandbox':{'type':'readOnly','networkAccess':False},'model':'probe','modelProvider':'probe','approvalPolicy':'never','activePermissionProfile':None}
            elif method == 'thread/unsubscribe':
                if not settings.get('attached'):
                    settings['loaded']=False
                    (home/'native.json').write_text(json.dumps(settings))
                result = {}
            else: raise AssertionError(method)
            response = json.dumps({'id':request['id'], 'result':result}).encode()
            prefix = bytes([129, len(response)]) if len(response) < 126 else bytes([129,126])+struct.pack('!H',len(response))
            sys.stdout.buffer.write(prefix+response)
            sys.stdout.buffer.flush()
    sys.exit(0)
runpy.run_path(str(Path(__file__).with_name('relink_fake.py')), run_name='__main__')
