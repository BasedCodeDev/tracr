"""Polling, transition and disconnect acceptance using a scripted WebSocket proxy."""
import json
from pathlib import Path
import shutil
import sys
import tempfile
from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix='tracr-status-') as tmp:
    home = Path(tmp)
    (home/'listed').mkdir()
    (home/'resume').mkdir()
    (home/'config.toml').write_text('')
    fake=home/'fake'
    shutil.copyfile(Path(__file__).with_name('fake.py'),fake)
    fake.chmod(0o755)
    def settings(status, disconnect=False):
        value={'live_enabled':True,'live_threads':{'first':status},'live_disconnect':disconnect}
        staging=home/'fake.tmp'
        staging.write_text(json.dumps(value))
        staging.replace(home/'fake.json')
    settings({'type':'active','activeFlags':[]})
    t=Terminal([binary,'--config',str(home/'config.toml'),'--codex-home',tmp,'--codex-bin',str(fake),str(home/'listed')])
    try:
        t.expect('Running')
        settings({'type':'active','activeFlags':['waitingOnApproval']})
        t.expect('Waiting for approval')
        t.send(b'/first')
        t.expect('Search sessions')
        t.expect('Waiting for approval')
        t.send(b'\x1b')
        t.wait(lambda:'Search sessions' not in t.text())
        settings({'type':'idle'})
        t.wait(lambda:'Waiting for approval' not in t.text() and 'Running' not in t.text())
        settings({'type':'active','activeFlags':[]})
        t.expect('Running')
        settings({'type':'active','activeFlags':[]},True)
        t.expect('live status unavailable')
        t.wait(lambda:'Running' not in t.text())
        settings({'type':'active','activeFlags':['waitingOnUserInput']})
        t.expect('Waiting for input')
        t.finish()
        methods={json.loads(line)['method'] for line in (home/'live_requests.jsonl').read_text().splitlines()}
        assert methods == {'initialize','initialized','thread/loaded/list','thread/read'}, methods
        print('live polling, status transitions, search, disconnect and reconnect: passed')
    finally:t.close()
