#!/usr/bin/env python3
"""Scripted metadata-only app server. Configuration comes from its disposable home."""
import json
import os
import sys
import time
from pathlib import Path
home = Path(os.environ['CODEX_HOME'])
settings = json.loads((home / 'fake.json').read_text())
if len(sys.argv) > 1 and sys.argv[1] == 'resume':
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
            if settings.get('malformed'):
                result['data'].append({'id':'invalid'})
            print(json.dumps({'method':'thread/status/changed','params':{}}), flush=True)
        elif method == 'thread/read':
            assert request['params']['includeTurns'] is False
            if settings.get('read_wait'):
                while not (home / 'release').exists():
                    time.sleep(.02)
            result = {'thread':dict(id=request['params']['threadId'], name='Task', cwd=settings.get('read_cwd', str(home/'resume')), canAcceptDirectInput=settings.get('direct', True), createdAt=100, updatedAt=200, source='exec')}
        else:
            raise AssertionError('Forbidden RPC method: '+method)
        print(json.dumps({'id':request['id'], 'result':result}), flush=True)
