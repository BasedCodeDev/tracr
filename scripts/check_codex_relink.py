#!/usr/bin/env python3
"""Opt-in acceptance of the Rust relinking implementation in disposable Codex homes."""
from diagnostics import run_cli, scratch_dir
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import http.server
import threading
import queue
import time
import shutil
import probe_codex_profiles as profiles
import probe_codex_relocation as probe


def connect_owner(owner_type,binary,env,daemon):
    deadline=time.monotonic()+15
    while True:
        try:return owner_type(binary,env)
        except (OSError,RuntimeError,queue.Empty):
            if time.monotonic()>=deadline or daemon.poll() is not None:raise
            time.sleep(.1)


SCENARIOS = ('existing-child','existing-parent','existing-other','normal','archived','workspace','stale','locked','shared','shared-working','shared-waiting','custom','builtin-workspace','builtin-read-only','builtin-parent','builtin-archived','builtin-locked','builtin-stale')

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--keep-temp', action='store_true', help='Retain full diagnostic scratch instead of cleaning it')
    parser.add_argument('--codex',required=True)
    parser.add_argument('--test-binary',required=True,help='Compiled tests/relocation.rs executable')
    parser.add_argument('--mode',choices=('legacy','paginated'),action='append')
    parser.add_argument('--scenario',choices=SCENARIOS,action='append')
    args=parser.parse_args()
    binary=str(Path(args.codex).resolve(strict=True))
    test_binary=str(Path(args.test_binary).resolve(strict=True))
    root=Path(scratch_dir(prefix='tracr-rl-'))
    results=[]
    print('Scratch:',root,flush=True)
    for mode in args.mode or ('legacy','paginated'):
        for scenario in args.scenario or SCENARIOS:
            home=root/(mode+'-'+scenario)
            owner=None
            daemon=None
            daemon_log=None
            model=None
            release=threading.Event()
            turn_id=None
            try:
                fixture=probe.prepare_fixture(binary,home,mode)
                env=fixture['env'];tid=fixture['baseline']['id']
                builtin=scenario.startswith('builtin-')
                target=home if scenario=='builtin-parent' else home/'new folder'
                archived=scenario in ('archived','builtin-archived')
                profile=':read-only' if scenario=='builtin-read-only' else ':workspace'
                if builtin:
                    config=home/'codex/config.toml'
                    config.write_text(f'default_permissions = "{profile}"\n'+config.read_text())
                    for folder in (home/'old', target):
                        for name in ('.git','.agents','.codex','.aws'):(folder/name).mkdir(exist_ok=True)
                    seed=probe.Server(binary,home,env)
                    try:
                        seed.call('thread/resume',{'threadId':tid,'excludeTurns':True})
                        seed.call('thread/settings/update',{'threadId':tid,'permissions':profile})
                        profiles.settle(seed,tid,lambda r:(r.get('activePermissionProfile') or {}).get('id')==profile)
                    finally:seed.close()
                if scenario in ('workspace','custom'):
                    config=home/'codex/config.toml'
                    (home/'extra').mkdir()
                    config.write_text('sandbox_mode = "workspace-write"\n'+config.read_text()+
                        f'\n[projects."{home / "old"}"]\ntrust_level = "trusted"\n'+
                        f'\n[projects."{home / "new folder"}"]\ntrust_level = "trusted"\n'+
                        (f'\n[sandbox_workspace_write]\nexclude_slash_tmp = true\nexclude_tmpdir_env_var = true\nwritable_roots = ["{home / "extra"}"]\n' if scenario=='custom' else ''))
                if scenario in ('workspace','custom'):
                    seed=probe.Server(binary,home,env)
                    seed.call('thread/resume',{'threadId':tid,'excludeTurns':True})
                    seed.call('thread/settings/update',{'threadId':tid,'sandboxPolicy':{'type':'workspaceWrite','writableRoots':[str(home/'extra')] if scenario=='custom' else [],'excludeSlashTmp':scenario=='custom','excludeTmpdirEnvVar':scenario=='custom'}})
                    time.sleep(.2)
                    seeded=seed.call('thread/resume',{'threadId':tid,'excludeTurns':True})
                    seed.close()
                    verify=probe.Server(binary,home,env)
                    persisted=verify.call('thread/resume',{'threadId':tid,'excludeTurns':True})
                    verify.close()
                    (home/'permissions.json').write_text(json.dumps({'seeded':seeded['sandbox'],'persisted':persisted['sandbox']},indent=2))
                    assert persisted['sandbox']['type']=='workspaceWrite',persisted['sandbox']
                    if scenario=='custom':assert persisted['sandbox']['writableRoots'],persisted['sandbox']
                if archived or scenario in ('locked','builtin-locked'):
                    owner=probe.Server(binary,home,env)
                    if archived:
                        owner.call('thread/archive',{'threadId':tid});owner.close();owner=None
                    else:
                        owner.call('thread/resume',{'threadId':tid,'excludeTurns':True})
                if scenario.startswith('shared'):
                    sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'tests/fixtures/app-server'))
                    from real_status_check import Owner
                    daemon_log=(home/'daemon.log').open('w')
                    daemon=subprocess.Popen([binary,'app-server','--listen','unix://'],env=env,cwd=home,stdout=daemon_log,stderr=daemon_log)
                    deadline=time.monotonic()+10
                    while not (home/'codex/app-server-control/app-server-control.sock').exists():
                        assert time.monotonic()<deadline,'shared daemon did not start'
                        time.sleep(.05)
                    owner=connect_owner(Owner,binary,env,daemon)
                    owner.call('thread/resume',{'threadId':tid,'excludeTurns':True})
                    if scenario!='shared':
                        received=threading.Event()
                        class Handler(http.server.BaseHTTPRequestHandler):
                            def log_message(self,*args):pass
                            def do_POST(self):
                                self.rfile.read(int(self.headers.get('Content-Length','0')))
                                received.set();release.wait(30)
                                item={'type':'function_call','id':'fc_wait','call_id':'call_wait','name':'request_user_input','arguments':json.dumps({'questions':[{'id':'choice','header':'Test','question':'Synthetic test?', 'options':[{'label':'One','description':'First'},{'label':'Two','description':'Second'}]}]})}
                                events=[('response.created',{'response':{'id':'resp','status':'in_progress','output':[]}}),('response.output_item.done',{'output_index':0,'item':item}),('response.completed',{'response':{'id':'resp','status':'completed','output':[item],'usage':{'input_tokens':1,'output_tokens':1,'total_tokens':2}}})]
                                self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
                                try:
                                    for event,data in events:
                                        data['type']=event;self.wfile.write(f'event: {event}\ndata: {json.dumps(data)}\n\n'.encode());self.wfile.flush()
                                except (BrokenPipeError,ConnectionResetError):pass
                        model=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
                        threading.Thread(target=model.serve_forever,daemon=True).start()
                        # Reload the fixture daemon after installing the local provider endpoint.
                        owner.close();owner=None;daemon.terminate();daemon.wait(timeout=8)
                        config=home/'codex/config.toml'
                        config.write_text(config.read_text().replace('http://127.0.0.1:1/v1',f'http://127.0.0.1:{model.server_port}/v1'))
                        daemon=subprocess.Popen([binary,'app-server','--listen','unix://'],env=env,cwd=home,stdout=daemon_log,stderr=daemon_log)
                        time.sleep(.3)
                        owner=connect_owner(Owner,binary,env,daemon)
                        owner.call('thread/resume',{'threadId':tid,'excludeTurns':True})
                        turn_id=owner.call('turn/start',{'threadId':tid,'input':[{'type':'text','text':'Synthetic exclusion test.','text_elements':[]}], 'collaborationMode':{'mode':'plan','settings':{'model':'probe','reasoning_effort':None,'developer_instructions':None}}})['turn']['id']
                        assert received.wait(15),'local scripted provider not reached'
                        if scenario=='shared-waiting':release.set()
                        deadline=time.monotonic()+15
                        while True:
                            status=owner.call('thread/read',{'threadId':tid,'includeTurns':False})['thread']['status']
                            if status['type']=='active' and (scenario=='shared-working' or 'waitingOnUserInput' in status.get('activeFlags',[])):break
                            assert time.monotonic()<deadline,status
                            time.sleep(.05)
                        (home/'observed-status.json').write_text(json.dumps(status))
                control_id=None
                if scenario.startswith('existing-'):
                    control=probe.Server(binary,home,env)
                    try:
                        control_id=control.call('thread/start',{'cwd':str(home/'old'),'historyMode':mode,
                            'modelProvider':'probe','model':'probe','approvalPolicy':'never','sandbox':'read-only'})['thread']['id']
                        control.call('thread/inject_items',{'threadId':control_id,'items':[{'type':'message','role':'user','content':[{'type':'input_text','text':'Unrelated control session'}]}]})
                    finally:control.close()
                    (home/'old/sentinel').write_text('unchanged')
                    if scenario=='existing-child':
                        target=home/'old/child';target.mkdir()
                    elif scenario=='existing-parent':target=home
                else:
                    shutil.rmtree(home/'old')
                (home/'fixture.json').write_text(json.dumps({'id':tid,'archived':archived,'scenario':scenario,'target':str(target)}))
                result=subprocess.run([test_binary,'--ignored','--exact','real_native_relink','--nocapture'],
                    env={**env,'TRACR_RELINK_FIXTURE':str(home),'TRACR_RELINK_CODEX':binary},
                    cwd=home,capture_output=True,text=True,timeout=120)
                (home/'rust.log').write_text(result.stdout+result.stderr)
                assert result.returncode==0,(result.stdout+result.stderr)[-2500:]
                if turn_id:
                    owner.call('turn/interrupt',{'threadId':tid,'turnId':turn_id})
                    release.set();turn_id=None
                if owner:owner.close();owner=None
                check=probe.Server(binary,home,env)
                try:
                    after=probe.history(check,tid)
                    if control_id:
                        assert probe.read(check,control_id)['cwd']==str(home/'old'),'other session moved'
                        assert (home/'old/sentinel').read_text()=='unchanged','project file changed'
                    if scenario in ('shared-working','shared-waiting'):
                        assert all(message in json.dumps(after) for message in fixture['expected_messages']),'fixture history lost'
                    else:assert after==fixture['history_before'],'history changed'
                    if scenario in ('stale','locked','shared','shared-working','shared-waiting','custom','builtin-locked','builtin-stale'):
                        assert probe.read(check,tid)['cwd']==str(home/'old'),'rejection changed cwd'
                    assert any(t['id']==tid for t in probe.inventory(check,archived)), 'archive state changed'
                finally:check.close()
                if builtin and scenario not in ('builtin-locked','builtin-stale'):
                    stored=profiles.durable_snapshot(home,tid)
                    assert stored['active_permission_profile']['id']==profile,stored
                    assert stored['runtime_workspace_roots']==[str(target)],stored
                    if profile==':workspace':
                        entries=stored['permission_profile']['file_system']['entries']
                        for name in ('.git','.agents','.codex','.aws'):
                            assert any(e['path'].get('path')==str(target/name) and e['access']=='read' for e in entries),(name,entries)
                    assert not (home/'old').exists()
                case={'mode':mode,'scenario':scenario,'status':'passed'}
            except Exception as error:
                case={'mode':mode,'scenario':scenario,'status':'failed','error':str(error)}
            finally:
                release.set()
                if owner:owner.close()
                if daemon:
                    daemon.terminate()
                    try:daemon.wait(timeout=8)
                    except subprocess.TimeoutExpired:daemon.kill();daemon.wait()
                if daemon_log:daemon_log.close()
                if model:model.shutdown();model.server_close()
            results.append(case);print(json.dumps(case),flush=True)
            (root/'results.json').write_text(json.dumps(results,indent=2)+'\n')
    return int(any(r['status']!='passed' for r in results))


if __name__=='__main__':raise SystemExit(run_cli(main))
