"""On-demand information, keyboard isolation, live state and copied content."""
import json
from pathlib import Path
import sys,tempfile
from pty_support import Terminal

binary=str(Path(sys.argv[1]).resolve())
fake=str(Path(__file__).with_name('release_fake.py').resolve())
with tempfile.TemporaryDirectory(prefix='tracr-info-') as tmp:
    root=Path(tmp);home=root/'codex';home.mkdir();folder=root/('long folder name '*6);folder.mkdir()
    native={'cwd':str(folder),'loaded':True}
    (home/'native.json').write_text(json.dumps(native))
    capture=root/'copy.txt';copier=root/'wl-copy'
    copier.write_text("#!/usr/bin/python3\nimport os,sys\nopen(os.environ['COPY_CAPTURE'],'w').write(sys.stdin.read())\n");copier.chmod(0o755)
    env={'HOME':str(root),'PATH':str(root)+':/usr/bin:/bin','WAYLAND_DISPLAY':'fixture','COPY_CAPTURE':str(capture)}
    t=Terminal([binary,'--codex-home',str(home),'--codex-bin',fake,str(folder)],env)
    try:
        t.expect('Relink fixture');t.expect('Idle')
        assert 'Details' not in t.text() and 'ID: session' not in t.text()
        t.send(b'i');t.expect('Session information');t.expect('c Copy')
        t.send(b'\x1b[C/l');t.expect('Session information')
        assert 'Search sessions' not in t.text() and 'Use this folder' not in t.text()
        t.send(b'c');t.expect('Message copied')
        report=capture.read_text();assert str(folder) in report and 'Status: Idle' in report
        t.resize(12,36);t.expect('c Copy')
        t.send(b'\x1b[F');t.expect('Git commit')
        t.send(b'c');t.expect('Message copied');t.expect('Git commit')
        native['status']={'type':'active','activeFlags':['waitingOnApproval']}
        (home/'native.json').write_text(json.dumps(native))
        t.resize(40,140);t.send(b'\x1b[H');t.expect('Waiting for approval')
        t.send(b'i');t.wait(lambda:'Session information' not in t.text())
        t.expect('Relink fixture');assert not (home/'launch.json').exists()
        t.send(b'\x1b');t.expect('Enter Sessions')
        t.send(b'i');t.send(b'?');t.expect('Actions & shortcuts')
        t.send(b'\x1b');t.wait(lambda:'Actions & shortcuts' not in t.text())
        assert 'Session information' not in t.text()
        t.finish()
        log=(home/'live_requests.jsonl').read_text();assert 'thread/resume' not in log
    finally:t.close()
print('information modal copy, wrapping, live state, isolation and dismissal: passed')
