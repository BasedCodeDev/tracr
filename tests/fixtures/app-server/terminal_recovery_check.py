"""Explicit terminal recovery confirmation, refresh, cancellation and guarded retry."""
import json
from pathlib import Path
import sys,tempfile
from pty_support import Terminal
binary=str(Path(sys.argv[1]).resolve());fake=str(Path(__file__).with_name('release_fake.py').resolve())
for mode in ['stop','refresh','unknown','attached','cancel']:
    with tempfile.TemporaryDirectory(prefix='tracr-terminals-ui-') as tmp:
        root=Path(tmp);home=root/'codex';home.mkdir();old=root/'old';old.mkdir();new=root/'new';new.mkdir()
        state={'cwd':str(old),'loaded':True,'terminals':True,'attached':mode=='attached'}
        if mode=='unknown':state['malformed_terminals']=True
        native=home/'native.json';native.write_text(json.dumps(state))
        env={'HOME':str(root),'XDG_STATE_HOME':str(root/'state'),'PATH':'/usr/bin:/bin','GIT_CEILING_DIRECTORIES':str(root),'TMPDIR':str(root)}
        t=Terminal([binary,'--codex-home',str(home),'--codex-bin',fake,str(old)],env)
        def calls():return (home/'live_requests.jsonl').read_text()
        try:
            t.inspect('ID: session');t.expect('Idle');t.send(b'm');t.expect('Use this folder')
            t.send(b'\x15'+str(new).encode()+b'\r');t.expect('>[ Use this folder ]');t.send(b'\r');t.expect('Enter confirms');t.send(b'\r')
            t.expect('Background commands block relocation');t.expect('[Cancel]')
            assert '/terminate' not in calls()
            if mode=='cancel':
                t.send(b'\r');t.wait(lambda:'Background commands block relocation' not in t.text());assert '/terminate' not in calls()
            elif mode=='unknown':
                t.expect('state is unavailable');assert 'Stop listed commands' not in t.text()
                t.send(b'\x1b');t.wait(lambda:'Background commands block relocation' not in t.text())
                assert '/terminate' not in calls()
            else:
                t.expect('sleep 300');t.expect('Stop listed commands and retry move')
                t.resize(24,50);t.expect('[Cancel]')
                before_scroll=t.text();t.send(b'\x1b[6~');t.wait(lambda:t.text()!=before_scroll)
                t.resize(80,240);t.expect('[Cancel]')
                if mode=='refresh':
                    state['terminals']=[];native.write_text(json.dumps(state));t.send(b'\t');t.expect('[Refresh]');t.send(b'\r')
                    t.expect('No background commands remain');t.expect('[Cancel]')
                    t.send(b'\t');t.expect('[Refresh]');t.send(b'\t');t.expect('[Retry move]');t.send(b'\r')
                else:
                    t.send(b'\t');t.expect('[Refresh]');t.send(b'\t');t.expect('[Stop listed commands and retry move]');t.send(b'\r')
                if mode=='attached':
                    t.expect('Session is still loaded');t.send(b'\x1b');t.wait(lambda: 'Session is still loaded' not in t.text())
                else:t.expect('Session relinked and verified.')
                assert ('/terminate' in calls()) == (mode!='refresh')
            assert json.loads(native.read_text())['cwd']==str(old if mode in ['unknown','attached','cancel'] else new)
            t.finish()
        finally:t.close()
print('terminal recovery: explicit confirmation, refresh, cancel, resize, retry and attached-client checks passed')
