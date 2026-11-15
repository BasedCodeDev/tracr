#!/usr/bin/env python3
"""Opt-in native idle-release acceptance. Uses disposable homes and a local TCP server.
The byte proxy wrapper routes only this fixture to that server; other invocations use
real Codex. Never touches the user's daemon, credentials, or configuration.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import probe_codex_relocation as probe
import probe_codex_profiles as profiles
sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'tests/fixtures/app-server'))
from real_status_check import Owner


def connect(binary, env, port):
    original = socket.socket
    class TcpSocket(original):
        def __init__(self, *args, **kwargs):
            super().__init__(socket.AF_INET, socket.SOCK_STREAM)
        def connect(self, address):
            return super().connect(('127.0.0.1', port))
    try:
        socket.socket = TcpSocket
        return Owner(binary, env)
    finally:
        socket.socket = original


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--codex', required=True)
    parser.add_argument('--test-binary', required=True)
    parser.add_argument('--mode', action='append', choices=['legacy','paginated'])
    parser.add_argument('--scenario', action='append', choices=['detached','attached','folder','workspace','read-only'])
    args=parser.parse_args()
    binary=str(Path(args.codex).resolve(strict=True))
    test=str(Path(args.test_binary).resolve(strict=True))
    root=Path(tempfile.mkdtemp(prefix='tracr-release-'))
    print('Evidence:',root,flush=True)
    version=subprocess.check_output([binary,"--version"],text=True).strip()
    with open(binary,"rb") as stream: digest=hashlib.file_digest(stream,"sha256").hexdigest()
    results=[]
    for mode in args.mode or ['legacy','paginated']:
        for scenario in args.scenario or ['detached','attached','folder','workspace','read-only']:
            home=root/(mode+'-'+scenario)
            daemon=None; owner=None; observer=None
            result={'version':version,'sha256':digest,'mode':mode,'scenario':scenario,'status':'failed'}
            try:
                fixture=probe.prepare_fixture(binary,home,mode)
                env=fixture['env'];tid=fixture['baseline']['id']
                config=home/'codex/config.toml'
                config.write_text(config.read_text().replace('thread_unload_delay_secs = 1\n',''))
                before_config=config.read_bytes()
                with socket.socket() as reservation:
                    reservation.bind(('127.0.0.1',0));port=reservation.getsockname()[1]
                with (home/'daemon.log').open('wb') as log:
                    daemon=subprocess.Popen([binary,'app-server','--listen',f'ws://127.0.0.1:{port}'],env=env,cwd=home,stdout=log,stderr=log)
                deadline=time.monotonic()+15
                while True:
                    try:owner=connect(binary,env,port);break
                    except (OSError,RuntimeError):
                        if time.monotonic()>deadline:raise
                        time.sleep(.1)
                observer=connect(binary,env,port)
                baseline=owner.call('thread/resume',{'threadId':tid,'excludeTurns':True})
                if scenario in ['workspace','read-only']:
                    profile=':'+scenario
                    owner.call('thread/settings/update',{'threadId':tid,'permissions':profile})
                    profiles.settle(owner,tid,lambda r:(r.get('activePermissionProfile') or {}).get('id')==profile)
                    baseline=owner.call('thread/resume',{'threadId':tid,'excludeTurns':True})
                attached=scenario=='attached'
                if not attached:owner.call('thread/unsubscribe',{'threadId':tid})
                assert tid in observer.call('thread/loaded/list',{})['data']
                wrapper=home/'codex-proxy'
                wrapper.write_text('#!/usr/bin/env python3\nimport os,sys,socket,select\n'+
                    f'if sys.argv[1:] != ["app-server","proxy"]: os.execv({binary!r},[{binary!r}]+sys.argv[1:])\n'+
                    f's=socket.create_connection(("127.0.0.1",{port}))\n'+
                    'while True:\n ready,_,_=select.select([s,sys.stdin.buffer],[],[])\n for stream in ready:\n  data=os.read(stream.fileno(),65536)\n  if not data: sys.exit(0)\n  if stream is s: sys.stdout.buffer.write(data);sys.stdout.buffer.flush()\n  else: s.sendall(data)\n')
                wrapper.chmod(0o755)
                (home/'release.json').write_text(json.dumps({'id':tid,'attached':attached,'folder':scenario=='folder'}))
                started=time.monotonic()
                completed=subprocess.run([test,'--ignored','--exact','real_cached_idle_release','--nocapture'],env={**env,'TRACR_RELEASE_FIXTURE':str(home)},capture_output=True,text=True,timeout=120)
                (home/'rust.log').write_text(completed.stdout+completed.stderr)
                assert completed.returncode==0,completed.stdout+completed.stderr
                result['seconds']=time.monotonic()-started
                assert config.read_bytes()==before_config,'persistent config changed'
                current=owner.call('thread/resume',{'threadId':tid,'excludeTurns':True})
                for field in ['model','modelProvider','sandbox','approvalPolicy','approvalsReviewer','activePermissionProfile']:
                    assert current.get(field)==baseline.get(field),(field,current.get(field),baseline.get(field))
                expected=home/'old' if attached else home/'new folder'
                assert current['cwd']==str(expected),current
                assert probe.history(owner,tid)==fixture['history_before'],'history changed'
                owner.call('thread/unsubscribe',{'threadId':tid})
                time.sleep(.3)
                assert tid in observer.call('thread/loaded/list',{})['data'],'normal resume inherited zero unload delay'
                result['status']='passed'
            except Exception as error:
                result['error']=str(error)
            finally:
                for client in [owner,observer]:
                    if client:client.close()
                if daemon:
                    daemon.terminate()
                    try:daemon.wait(timeout=5)
                    except subprocess.TimeoutExpired:daemon.kill();daemon.wait()
            results.append(result)
            print(json.dumps(result),flush=True)
            (root/'results.json').write_text(json.dumps(results,indent=2))
    return 0 if all(r['status']=='passed' for r in results) else 1

if __name__=='__main__':raise SystemExit(main())
