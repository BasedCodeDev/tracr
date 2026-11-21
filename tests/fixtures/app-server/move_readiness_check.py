"""Live move-dialog release, attached-client closure, and cancellation."""
import json
import os
from pathlib import Path
import sys
import tempfile
from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
fake = str(Path(__file__).with_name('release_fake.py').resolve())
with tempfile.TemporaryDirectory(prefix='tracr-readiness-') as tmp:
    root = Path(tmp)
    home = root/'codex'; home.mkdir()
    old = root/'project'; old.mkdir()
    state = home/'native.json'
    state.write_text(json.dumps({'cwd':str(old), 'loaded':True, 'attached':True}))
    log = home/'live_requests.jsonl'
    def requests():
        return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
    def resumes():
        return sum(r['method']=='thread/resume' for r in requests())
    t = Terminal([binary,'--codex-home',str(home),'--codex-bin',fake,str(old)],
                 {**os.environ,'XDG_STATE_HOME':str(root/'state')})
    try:
        t.expect('Idle')
        assert resumes() == 0
        t.send(b'\x1b`');t.expect_focus('Folders');t.send(b'm');t.expect('Destination folder')
        t.wait(lambda:resumes()>0)
        t.expect('Close sessions still open in Codex.')
        data=json.loads(state.read_text());data['attached']=False;state.write_text(json.dumps(data))
        t.expect('No loaded sessions.')
        assert not json.loads(state.read_text())['loaded']
        assert old.is_dir() and not (root/'state/tracr/folder-moves/current.json').exists()
        data=json.loads(state.read_text());data.update(loaded=True,attached=True,stall=True);state.write_text(json.dumps(data))
        before=resumes();t.wait(lambda:resumes()>before)
        t.send(b'\x1b');t.wait(lambda:'Destination folder' not in t.text())
        assert old.is_dir()
        assert not any(r['method'].startswith('turn/') for r in requests())
        t.finish()
    finally:t.close()
print('move readiness automatically releases after client closure and cancels safely: passed')
