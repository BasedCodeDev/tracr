"""Idle relocation confirmation, progress and still-loaded retry/cancel."""
import json
from pathlib import Path
import sys
import time
import tempfile
from pty_support import Terminal

binary=str(Path(sys.argv[1]).resolve())
fake=str(Path(__file__).with_name('release_fake.py').resolve())
for attached in [False,True]:
    with tempfile.TemporaryDirectory(prefix='tracr-release-ui-') as tmp:
        root=Path(tmp);home=root/'codex';home.mkdir()
        old=root/'old';old.mkdir();new=root/'new';new.mkdir()
        (home/'native.json').write_text(json.dumps({'cwd':str(old),'loaded':True,'attached':attached,'release_delay':.4}))
        env={'HOME':str(root),'XDG_STATE_HOME':str(root/'state'),'PATH':'/usr/bin:/bin','GIT_CEILING_DIRECTORIES':str(root),'TMPDIR':str(root)}
        t=Terminal([binary,'--codex-home',str(home),'--codex-bin',fake,str(old)],env)
        try:
            t.inspect('ID: session');t.expect('Idle')
            t.send(b'm');t.expect('Use this folder')
            t.send(b'\x15'+str(new).encode()+b'\r');t.expect('>[ Use this folder ]');t.send(b'\r');t.expect('Enter confirms')
            assert 'thread/resume' not in (home/'live_requests.jsonl').read_text()
            t.send(b'\r');t.expect('Releasing idle session')
            if attached:
                t.expect('Session is still loaded');t.expect('If already closed, you can retry now.')
                t.expect('[Cancel]');t.expect('Retry')
                before=(home/'live_requests.jsonl').read_text().count('thread/resume')
                deadline=time.monotonic()+.3;t.wait(lambda:time.monotonic()>=deadline)
                assert (home/'live_requests.jsonl').read_text().count('thread/resume')==before
                assert str(old) in t.text() and str(new) in t.text()
                t.send(b'\r')
                t.wait(lambda: 'Session is still loaded' not in t.text())
            else:t.expect('Session relinked and verified.')
            assert json.loads((home/'native.json').read_text())['cwd']==str(old if attached else new)
            t.finish()
        finally:t.close()
print('idle release confirmation, progress and still-loaded cancellation: passed')


for scenario in ['retry','stale','terminals']:
    print('retry scenario:', scenario, flush=True)
    with tempfile.TemporaryDirectory(prefix='tracr-loaded-retry-') as tmp:
        root=Path(tmp);home=root/'codex';home.mkdir();old=root/'old';old.mkdir();new=root/'new';new.mkdir()
        native=home/'native.json';native.write_text(json.dumps({'cwd':str(old),'loaded':True,'attached':True,'release_delay':.2}))
        env={'HOME':str(root),'XDG_STATE_HOME':str(root/'state'),'PATH':'/usr/bin:/bin','GIT_CEILING_DIRECTORIES':str(root),'TMPDIR':str(root)}
        t=Terminal([binary,'--codex-home',str(home),'--codex-bin',fake,str(old)],env)
        try:
            t.inspect('ID: session');t.expect('Idle');t.send(b'm');t.expect('Use this folder')
            t.send(b'\x15'+str(new).encode()+b'\r');t.expect('>[ Use this folder ]');t.send(b'\r');t.expect('Enter confirms');t.send(b'\r\r')
            t.expect('Session is still loaded');t.expect('[Cancel]')
            assert json.loads(native.read_text())['cwd']==str(old)
            # A retry while still loaded returns here and preserves the destination.
            before=(home/'live_requests.jsonl').read_text().count('thread/resume')
            t.send(b'\t\r');t.wait(lambda:(home/'live_requests.jsonl').read_text().count('thread/resume')>before)
            t.expect('Session is still loaded');t.expect('[Cancel]')
            assert str(new) in t.text()
            t.resize(24,50);t.expect('[Cancel]');t.resize(80,240)
            settings=json.loads(native.read_text());settings['attached']=False
            if scenario=='stale':
                other=root/'other';other.mkdir();settings['cwd']=str(other)
            elif scenario=='terminals':settings['terminals']=True
            native.write_text(json.dumps(settings));t.send(b'\t\r')
            if scenario=='stale':
                t.expect("Couldn't relink session");assert json.loads(native.read_text())['cwd']==str(other)
                t.send(b'\x1b');t.wait(lambda:"Couldn't relink session" not in t.text())
            else:
                if scenario=='terminals':
                    t.expect('Background commands block relocation');t.expect('[Cancel]')
                    assert '/terminate' not in (home/'live_requests.jsonl').read_text()
                    t.send(b'\t\t\r')
                t.expect('Session relinked and verified.');assert json.loads(native.read_text())['cwd']==str(new)
            t.finish()
        finally:t.close()
print('still-loaded retry: repeat failure, retained destination, success, stale preview and terminal recovery passed')
