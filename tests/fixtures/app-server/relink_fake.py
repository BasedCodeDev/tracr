#!/usr/bin/env python3
"""Stateful protocol double exclusively for disposable relinking terminal tests."""
import fcntl
import json
import os
from pathlib import Path
import sys

if sys.argv[1:]==['--version']:
    print('codex-cli 0.169.0');raise SystemExit
home=Path(os.environ['CODEX_HOME'])
if sys.argv[1:]==['app-server','proxy']:raise SystemExit(1)
assert sys.argv[1:]==['app-server','--stdio'],sys.argv
lock=None
for line in sys.stdin:
    request=json.loads(line)
    if 'id' not in request:continue
    with (home/'requests.jsonl').open('a') as log:log.write(line)
    state=json.loads((home/'native.json').read_text())
    metadata=home/'sessions'/'fixture.jsonl'
    metadata.parent.mkdir(exist_ok=True)
    recorded=state.get('recorded_profile', state.get('profile'))
    if isinstance(recorded,str):recorded={'id':recorded}
    metadata.write_text(json.dumps({'type':'session_meta','payload':{'id':'session'}})+'\n'+json.dumps({'type':'turn_context','payload':{'active_permission_profile':recorded}})+'\n')
    thread=dict(path=str(metadata),id='session',name='Relink fixture',preview='',cwd=state['cwd'],createdAt=100,updatedAt=200,source='cli',historyMode='legacy',model='probe',modelProvider='probe',canAcceptDirectInput=True)
    method=request['method'];params=request.get('params',{})
    try:
        if method=='initialize':result={}
        elif method=='thread/list':result={'data':[thread] if params.get('archived',False)==state.get('archived',False) else [],'nextCursor':None}
        elif method=='thread/read':result={'thread':thread}
        elif method=='thread/resume':
            if state.get('loaded') or state.get('reattach'):raise RuntimeError('already has an active writer')
            if state.get('archived'):raise RuntimeError('session is archived')
            if lock is None:
                lock=(home/'writer.lock').open('w')
                try:fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
                except BlockingIOError:lock.close();lock=None;raise RuntimeError('already has an active writer')
            result={'thread':thread,'cwd':state['cwd'],'runtimeWorkspaceRoots':[state['cwd']], 'sandbox':({'type':'workspaceWrite','writableRoots':[],'networkAccess':False,'excludeSlashTmp':False,'excludeTmpdirEnvVar':False} if state.get('profile')==':workspace' else {'type':'readOnly','networkAccess':False}),'model':'probe','modelProvider':'probe','approvalPolicy':'never','activePermissionProfile':({'id':state['profile']} if state.get('profile') else None)}
        elif method=='thread/settings/update':
            assert lock is not None
            if state.get('reject_update'):raise RuntimeError('injected update failure')
            state['cwd']=params['cwd']
            if state.get('fail_after_update'):state['profile']='test-profile'
            (home/'native.json').write_text(json.dumps(state));result={}
        elif method in ('thread/archive','thread/unarchive'):
            state['archived']=method=='thread/archive';(home/'native.json').write_text(json.dumps(state));result={}
        else:raise AssertionError('Forbidden operation '+method)
        print(json.dumps({'id':request['id'],'result':result}),flush=True)
    except RuntimeError as error:
        print(json.dumps({'id':request['id'],'error':{'code':-32600,'message':str(error)}}),flush=True)
