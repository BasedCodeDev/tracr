"""Relinking modal, cancellation, completion, startup recovery, and rediscovery."""
import json
from pathlib import Path
import sys
import tempfile
from pty_support import Terminal

binary=str(Path(sys.argv[1]).resolve())
fake=str(Path(__file__).with_name('relink_fake.py').resolve())
with tempfile.TemporaryDirectory(prefix='tracr-relink-ui-') as tmp:
    root=Path(tmp);home=root/'codex';home.mkdir();new=root/'new folder';new.mkdir()
    old=root/'missing';state=root/'state'
    (home/'native.json').write_text(json.dumps({'cwd':str(old),'archived':False,'profile':':workspace'}))
    env={'PATH':'/usr/bin:/bin','HOME':str(root),'XDG_STATE_HOME':str(state)}
    command=[binary,'--codex-home',str(home),'--codex-bin',fake,str(old)]
    t=Terminal(command,env)
    try:
        t.inspect('ID: session')
        t.expect('Unlinked · codex')
        t.inspect('Folder missing — Enter or R to relink.')
        t.expect('R Relink');assert 'u Recover' not in t.text()
        t.send(b'u?');t.expect('Actions & shortcuts')
        assert 'Inspect/recover relocation' not in t.text()
        t.send(b'\x1b');t.wait(lambda:'Actions & shortcuts' not in t.text())
        assert 'No unfinished relocations' not in t.text()
        t.resize(24, 36)
        t.expect('n New');t.expect('? Help · q Quit')
        t.resize(80, 240)
        t.send(b'R');t.expect('New folder');t.expect('Use this folder')
        t.wait(lambda:'Loading folders' not in t.text())
        t.send(b'\x1b[B')
        t.wait(lambda:'Loading folders' not in t.text())
        assert json.loads((home/'native.json').read_text())['cwd']==str(old)
        t.send(b'\r')  # Tree Enter expands/collapses, never confirms.
        assert json.loads((home/'native.json').read_text())['cwd']==str(old)
        t.send(b'\x15'+str(new).encode()+b'\r');t.expect('Enter confirms');t.expect('Permissions: Workspace')
        t.send(b'\x1b');t.wait(lambda:'Enter confirms' not in t.text())
        assert json.loads((home/'native.json').read_text())['cwd']==str(old)
        t.send(b'\r');t.expect('New folder');t.expect('Use this folder')
        t.resize(24, 36);t.expect('New folder');t.expect('Use this folder')
        t.resize(10, 24);t.expect('Enlarge terminal')
        t.resize(80, 240);t.expect('Use this folder')
        t.send(b'\x15\x1b[200~'+str(new).encode()+b'\x1b[201~\r');t.expect('Enter confirms')
        t.send(b'\r');t.expect('Session relinked and verified.')
        t.inspect('ID: session')
        t.wait(lambda:'Unlinked · codex' not in t.text())
        assert json.loads((home/'native.json').read_text())['cwd']==str(new)
        t.finish()
    finally:t.close()
    reopened=Terminal([binary,'--codex-home',str(home),'--codex-bin',fake,str(new)],env)
    try:
        reopened.inspect('ID: session')
        assert 'Unlinked · codex' not in reopened.text()
        assert 'u Recover' not in reopened.text()
        journal=next((state/'tracr/relocations').glob('*.json'))
        data=json.loads(journal.read_text());data['phase']='verifying';journal.write_text(json.dumps(data))
        reopened.send(b'r');reopened.expect('u Recover')
        reopened.send(b'?');reopened.expect('Inspect/recover relocation')
        reopened.send(b'\x1b');reopened.wait(lambda:'Actions & shortcuts' not in reopened.text())
        data['phase']='complete';journal.write_text(json.dumps(data))
        reopened.send(b'u');reopened.wait(lambda:'u Recover' not in reopened.text())
        assert 'Inspect/recover relocation' not in reopened.text()
        assert 'No unfinished relocations' not in reopened.text()
        data['phase']='rejected';journal.write_text(json.dumps(data))
        reopened.send(b'r?');reopened.expect('Actions & shortcuts')
        assert 'Inspect/recover relocation' not in reopened.text()
        reopened.send(b'\x1b');reopened.wait(lambda:'Actions & shortcuts' not in reopened.text())
        reopened.finish()
    finally:reopened.close()
    journals=list((state/'tracr/relocations').glob('*.json'));assert len(journals)==1
    data=json.loads(journals[0].read_text());data['phase']='verifying';journals[0].write_text(json.dumps(data))
    command[-1]=str(new)
    t=Terminal(command,env)
    try:
        t.expect('unfinished relocation')
        t.send(b'u');t.expect('Inspect/recover relocation')
        t.send(b'\r');t.expect('Session relinked and verified.')
        t.inspect('ID: session');assert 'u Recover' not in t.text();t.finish()
    finally:t.close()
    requests=(home/'requests.jsonl').read_text()
    assert 'turn/start' not in requests
    assert json.loads(journals[0].read_text())['phase']=='complete'
    child=new/'child';child.mkdir()
    (new/'sentinel').write_text('unchanged')
    t=Terminal([binary,'--codex-home',str(home),'--codex-bin',fake,str(new)],env)
    try:
        t.inspect('ID: session');t.send(b'R');t.expect('New folder')
        t.send(b'\x15'+str(child).encode()+b'\r');t.expect('Enter confirms')
        t.expect('Project files stay where they are.')
        t.send(b'\r');t.expect('Session relinked and verified.')
        t.inspect('ID: session')
        assert json.loads((home/'native.json').read_text())['cwd']==str(child)
        assert (new/'sentinel').read_text()=='unchanged'
        t.finish()
    finally:t.close()
    print('relink input, preview, cancel, confirmation, rediscovery and recovery: passed')

# Failures remain explicit dialogs, with safe rejection distinct from uncertain mutation.
for uncertain in (False, True):
    with tempfile.TemporaryDirectory(prefix='tracr-relink-error-') as tmp:
        root=Path(tmp);home=root/'codex';home.mkdir();new=root/'new';new.mkdir()
        old=root/'missing';state=root/'state'
        native={'cwd':str(old),'archived':False}
        native.update({'fail_after_update':True} if uncertain else {'profile':'test-profile'})
        (home/'native.json').write_text(json.dumps(native))
        env={'PATH':'/usr/bin:/bin','HOME':str(root),'XDG_STATE_HOME':str(state)}
        copier=root/'wl-copy'
        copier.write_text("#!/usr/bin/python3\nimport json,os,sys,time\ntext=sys.stdin.read()\ntime.sleep(.2)\nwith open(os.environ['COPY_CAPTURE'],'a') as f:f.write(json.dumps(text)+'\\n')\n")
        copier.chmod(0o755)
        capture=root/'copies.jsonl'
        env.update({'PATH':str(root)+':/usr/bin:/bin','WAYLAND_DISPLAY':'tracr-test','COPY_CAPTURE':str(capture)})
        command=[binary,'--codex-home',str(home),'--codex-bin',fake,str(old)]
        t=Terminal(command,env)
        try:
            t.inspect('ID: session');t.send(b'R');t.expect('New folder')
            t.send(b'\x15'+str(new).encode()+b'\r')
            if uncertain:
                t.expect('Enter confirms');t.send(b'\r')
            t.expect('! Relink failed')
            t.expect('Enter/Esc close')
            if uncertain:
                t.expect('Relink needs recovery');t.expect('folder may already have changed')
                assert 'folder was not changed' not in t.text()
            else:
                t.expect("Couldn't relink session");t.expect('did not change')
                t.expect('not supported for relinking')
            t.expect('c Copy message');t.send(b'cc');t.expect('Message copied')
            reports=[json.loads(line) for line in capture.read_text().splitlines()]
            assert len(reports)==1
            report=reports[0]
            assert str(old) in report and str(new) in report and 'Technical details:' in report
            assert '\n\n' in report and 'Enter/Esc' not in report and '┃' not in report
            t.resize(12,36);t.expect('! Relink failed');t.expect('Enter/Esc close')
            t.expect('c Copy message')
            t.send(b'\x1b[6~');t.expect('Enter/Esc close')
            t.send(b'c');t.wait(lambda:len(capture.read_text().splitlines())==2)
            assert json.loads(capture.read_text().splitlines()[1])==report
            t.expect('Message copied')
            t.send(b'\x1b[H');t.resize(80,240);t.expect('! Relink failed')
            t.send(b'\x1b' if uncertain else b'\r')
            if uncertain:t.expect('u Recover')
            else:assert 'u Recover' not in t.text()
            t.finish()
        finally:t.close()
        if uncertain:
            journal=json.loads(next((state/'tracr/relocations').glob('*.json')).read_text())
            assert journal['phase']=='changing-directory',journal
            assert journal['failure']['kind']=='PermissionProfile'
        else:assert not list((state/'tracr/relocations').glob('*.json'))
        assert json.loads((home/'native.json').read_text())['cwd']==str(new if uncertain else old)
        requests=(home/'requests.jsonl').read_text()
        if not uncertain:assert 'thread/settings/update' not in requests
        # A rejected attempt is terminal, not unfinished recovery on restart.
        t=Terminal(command,env)
        try:
            t.inspect('ID: session') if not uncertain else t.expect('unfinished relocation')
            if not uncertain:assert 'unfinished relocation' not in t.text()
            t.finish()
        finally:t.close()
print('persistent rejection and recovery dialogs: passed')
