"""Idle relocation confirmation, progress and attached-session copyable failure."""
import json
from pathlib import Path
import sys
import tempfile
from pty_support import Terminal

binary=str(Path(sys.argv[1]).resolve())
fake=str(Path(__file__).with_name('release_fake.py').resolve())
for attached in [False,True]:
    with tempfile.TemporaryDirectory(prefix='tracr-release-ui-') as tmp:
        root=Path(tmp);home=root/'codex';home.mkdir()
        old=root/'old';old.mkdir();new=root/'new';new.mkdir()
        (home/'native.json').write_text(json.dumps({'cwd':str(old),'loaded':True,'attached':attached,'release_delay':.4}))
        copier=root/'wl-copy';capture=root/'copied.txt'
        copier.write_text("#!/usr/bin/python3\nimport os,sys\nopen(os.environ['COPY_CAPTURE'],'w').write(sys.stdin.read())\n");copier.chmod(0o755)
        env={'HOME':str(root),'XDG_STATE_HOME':str(root/'state'),'PATH':str(root)+':/usr/bin:/bin','WAYLAND_DISPLAY':'tracr-test','COPY_CAPTURE':str(capture)}
        t=Terminal([binary,'--codex-home',str(home),'--codex-bin',fake,str(old)],env)
        try:
            t.inspect('ID: session');t.expect('Idle')
            t.send(b'R');t.expect('Use this folder')
            t.send(b'\x15'+str(new).encode()+b'\r');t.expect('Enter confirms')
            assert 'thread/resume' not in (home/'live_requests.jsonl').read_text()
            t.send(b'\r');t.expect('Releasing idle session')
            if attached:
                t.expect("Couldn't relink session");t.expect('Another client may still be attached')
                t.expect('c Copy message');t.send(b'c');t.expect('Message copied')
                report=capture.read_text();assert str(old) in report and str(new) in report and 'Close it and retry' in report
                t.send(b'\x1b')
                t.wait(lambda: "Couldn't relink session" not in t.text())
            else:t.expect('Session relinked and verified.')
            assert json.loads((home/'native.json').read_text())['cwd']==str(old if attached else new)
            t.finish()
        finally:t.close()
print('idle release confirmation, progress and copyable attached-session failure: passed')
