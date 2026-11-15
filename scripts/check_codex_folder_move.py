#!/usr/bin/env python3
"""Folder move acceptance in disposable Codex homes; never uses real sessions."""
import argparse,json,subprocess,tempfile,sqlite3
from pathlib import Path
import probe_codex_relocation as probe

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--codex',required=True)
    parser.add_argument('--test-binary',required=True)
    parser.add_argument('--scenario',choices=('normal','locked','interrupted'),default='normal')
    args=parser.parse_args()
    binary=str(Path(args.codex).resolve(strict=True))
    test=str(Path(args.test_binary).resolve(strict=True))
    root=Path(tempfile.mkdtemp(prefix='tracr-folder-native-'))
    print('Evidence:',root,flush=True)
    results=[]
    for mode in ('legacy','paginated'):
        home=root/mode
        owner=None
        try:
            fixture=probe.prepare_fixture(binary,home,mode)
            tid=fixture['baseline']['id']
            sub=home/'old/sub';sub.mkdir()
            (home/'old/.sentinel').write_text('preserved')
            server=probe.Server(binary,home,fixture['env'])
            try:
                server.call('thread/resume',{'threadId':tid})
                server.call('thread/settings/update',{'threadId':tid,'permissions':':workspace'})
                second=server.call('thread/start',{'cwd':str(sub),'historyMode':mode,'modelProvider':'probe',
                    'model':'probe','approvalPolicy':'never','sandbox':'read-only'})['thread']['id']
                server.call('thread/inject_items',{'threadId':second,'items':[{'type':'message','role':'user','content':[{'type':'input_text','text':'Nested archived session'}]}]})
                server.call('thread/archive',{'threadId':second})
                other=server.call('thread/start',{'cwd':str(home/'new folder'),'historyMode':mode,'modelProvider':'probe',
                    'model':'probe','approvalPolicy':'never','sandbox':'read-only'})['thread']['id']
                server.call('thread/inject_items',{'threadId':other,'items':[{'type':'message','role':'user','content':[{'type':'input_text','text':'Unrelated session'}]}]})
            finally:server.close()
            # Injection does not create a user-message event. Give the disposable
            # nested fixture a listing preview that survives native resume.
            for rollout in (home/'codex').rglob('*.jsonl'):
                if second in rollout.name:
                    lines=[json.loads(line) for line in rollout.read_text().splitlines()]
                    event={'timestamp':lines[-1]['timestamp'],'type':'event_msg','payload':{'type':'user_message','message':'Nested archived session','images':[],'local_images':[]}}
                    if mode=='paginated':
                        event['ordinal']=len(lines)
                        with rollout.open('a') as out:out.write(json.dumps(event)+'\n')
                    else:
                        lines.insert(1,event)
                        rollout.write_text(''.join(json.dumps(line)+'\n' for line in lines))
            # This fresh synthetic thread has no real model turn; seed its derived
            # first-message field so unarchive/resume does not filter it as empty.
            db=sqlite3.connect(home/'codex/state_5.sqlite')
            db.execute("UPDATE threads SET first_user_message = ? WHERE id = ?",('Nested archived session',second))
            db.commit();db.close()
            if args.scenario=='locked':
                owner=probe.Server(binary,home,fixture['env'])
                owner.call('thread/resume',{'threadId':tid})
            result=subprocess.run([test,'--ignored','--exact','native_folder_move','--nocapture'],
                env={**fixture['env'],'TRACR_FOLDER_FIXTURE':str(home),'TRACR_FOLDER_CODEX':binary,'TRACR_FOLDER_SCENARIO':args.scenario},
                capture_output=True,text=True,timeout=180)
            (home/'rust.log').write_text(result.stdout+result.stderr)
            assert result.returncode==0,(result.stdout+result.stderr)[-3500:]
            if owner:owner.close();owner=None
            final=home/('old' if args.scenario=='locked' else 'moved')
            server=probe.Server(binary,home,fixture['env'])
            try:
                assert probe.read(server,tid)['cwd']==str(final)
                assert probe.read(server,second)['cwd']==str(final/'sub')
                assert probe.read(server,other)['cwd']==str(home/'new folder')
                assert any(t['id']==second for t in probe.inventory(server,True))
                assert probe.history(server,tid)==fixture['history_before']
                assert (final/'.sentinel').read_text()=='preserved'
                assert (home/'old').exists()==(args.scenario=='locked')
            finally:server.close()
            case={'mode':mode,'status':'passed'}
        except Exception as e:case={'mode':mode,'status':'failed','error':str(e)}
        finally:
            if owner:owner.close()
        case['scenario']=args.scenario
        results.append(case);print(json.dumps(case),flush=True)
    (root/'results.json').write_text(json.dumps(results,indent=2))
    return int(any(r['status']!='passed' for r in results))
if __name__=='__main__':raise SystemExit(main())
