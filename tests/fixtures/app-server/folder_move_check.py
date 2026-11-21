"""Folder picker, naming, confirmation, cancellation, and successful move."""
import json
import os
from pathlib import Path
import sys,tempfile
from pty_support import Terminal

binary=str(Path(sys.argv[1]).resolve())
fake=str(Path(__file__).with_name('relink_fake.py').resolve())
with tempfile.TemporaryDirectory(prefix='tracr-folder-ui-') as tmp:
    root=Path(tmp);home=root/'codex';home.mkdir()
    old=root/'old';old.mkdir();(old/'file').write_text('unchanged')
    (home/'native.json').write_text(json.dumps({'cwd':str(old),'archived':False,'profile':':workspace'}))
    env={**os.environ,'XDG_STATE_HOME':str(root/'state')}
    t=Terminal([binary,'--codex-home',str(home),'--codex-bin',fake,str(old)],env)
    try:
        t.inspect('ID: session')
        t.send(b'\x1b`');t.expect_focus('Folders');t.send(b'm');t.expect('Destination folder')
        # Edit the exact destination directly; no separate naming screen.
        t.send(b'\x1b[Z\x15'+str(root/'moved').encode()+b'\r');t.expect('>[ Use this folder ]');t.send(b'\r');t.expect('Enter confirms')
        t.expect('1 sessions')
        t.send(b'\x1b')
        t.wait(lambda: 'Enter confirms' not in t.text())
        assert old.exists() and not (root/'moved').exists()
        t.expect('Destination folder')
        t.expect(str(root/'moved'))
        t.send(b'\r');t.expect('Enter confirms')
        t.send(b'\r');t.expect('Folder and sessions moved and verified.')
        t.expect_action('Inspect/recover relocation', False)
        assert not old.exists()
        assert (root/'moved/file').read_text()=='unchanged'
        assert json.loads((home/'native.json').read_text())['cwd']==str(root/'moved')
        t.finish()
    finally:t.close()
    # Simulate a lost final acknowledgement and inspect recovery on restart.
    journal=root/'state/tracr/folder-moves/current.json'
    data=json.loads(journal.read_text());data['phase']='verifying';journal.write_text(json.dumps(data))
    t=Terminal([binary,'--codex-home',str(home),'--codex-bin',fake,str(root/'moved')],env,start_in_sessions=False)
    try:
        t.expect('Unfinished folder move');t.expect_action('Inspect/recover relocation')
        t.send(b'u');t.expect('Inspect/recover folder move')
        t.send(b'\r');t.expect('Folder and sessions moved and verified.')
        t.expect_action('Inspect/recover relocation', False)
        t.finish();t.close()
        archive=root/'archive';archive.mkdir()
        t=Terminal([binary,'--codex-home',str(home),'--codex-bin',fake,str(root/'moved')],env)
        t.inspect('ID: session')
        t.send(b'\x1b`');t.expect_focus('Folders');t.send(b'm');t.expect('Destination folder')
        t.send(b'\x1b[Z\x15'+(str(archive)+'/').encode()+b'\r');t.expect('>[ Use this folder ]');t.send(b'\r');t.expect('Enter confirms')
        t.expect('Final path:');t.expect('moved')
        t.send(b'\r');t.expect('Folder and sessions moved and verified.')
        t.expect_action('Inspect/recover relocation', False)
        assert (archive/'moved/file').read_text()=='unchanged'
        assert json.loads((home/'native.json').read_text())['cwd']==str(archive/'moved')
        t.finish()
    finally:t.close()
    print('folder move picker, name, cancel, confirmation, selection and recovery: passed')

# Both preflight rejection and partial execution use the same copyable dialog.
for recovery in (False, True):
    with tempfile.TemporaryDirectory(prefix='tracr-folder-error-') as tmp:
        root=Path(tmp);home=root/'codex';home.mkdir()
        old=root/'old';old.mkdir()
        native={'cwd':str(old),'archived':False,'profile':':workspace' if recovery else 'unsupported'}
        if recovery:native['reject_update']=True
        (home/'native.json').write_text(json.dumps(native))
        copier=root/'wl-copy'
        copier.write_text("#!/usr/bin/python3\nimport os,sys\nfrom pathlib import Path\nPath(os.environ['COPY_CAPTURE']).write_text(sys.stdin.read())\n")
        copier.chmod(0o755)
        capture=root/'report'
        env={**os.environ,'XDG_STATE_HOME':str(root/'state'),'WAYLAND_DISPLAY':'tracr-test',
             'PATH':str(root)+':/usr/bin:/bin','COPY_CAPTURE':str(capture)}
        t=Terminal([binary,'--codex-home',str(home),'--codex-bin',fake,str(old)],env)
        try:
            t.inspect('ID: session');t.send(b'\x1b`');t.expect_focus('Folders');t.send(b'm');t.expect('Destination folder')
            t.send(b'\x1b[Z\x15'+str(root/'moved').encode()+b'\r');t.expect('>[ Use this folder ]');t.send(b'\r')
            if recovery:
                t.expect('Enter confirms');t.send(b'\r')
            t.expect('! Folder move failed');t.expect('c Copy message')
            t.expect('Move needs recovery' if recovery else 'Folder was not moved')
            t.resize(12,36);t.expect('c Copy message')
            t.send(b'c');t.expect('Message copied')
            report=capture.read_text()
            assert str(old) in report and str(root/'moved') in report,report
            assert 'Technical details:' in report and '\n\n' in report
            assert 'injected update failure' in report if recovery else 'unsupported' in report,report
            assert ('press u' in report)==recovery,report
            assert 'Affected sessions: '+('1' if recovery else 'Not yet determined') in report
            t.send(b'\x1b');t.wait(lambda:'! Folder move failed' not in t.text())
            t.finish()
        finally:t.close()
print('folder failure reports, copy, resize and dismissal: passed')
