"""Transient inset shortcut reminder and learning across Tracr launches."""
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix='tracr-reminder-') as directory:
    home = Path(directory)
    project = home/'listed'
    project.mkdir()
    fake = home/'fake'
    shutil.copyfile(Path(__file__).with_name('fake.py'), fake)
    fake.chmod(0o755)
    (home/'fake.json').write_text(json.dumps({'read_cwd': str(project), 'report_size': True}))
    state = home/'state'
    env = {**os.environ, 'XDG_STATE_HOME': str(state)}
    command = [binary, '--codex-home', str(home), '--codex-bin', str(fake), str(project)]
    for launch in range(3):
        t = Terminal(command, env)
        try:
            t.inspect('ID: first')
            t.send(b'\r')
            t.expect('FAKE CODING SESSION')
            if launch < 2:
                t.wait(lambda: t.line(2, 4, 9) == 'Alt+~')
                assert t.line(0, 0, 19) == 'FAKE CODING SESSION'
                assert t.line(1, 0, 20).strip() == ''
                size = lambda: json.loads((home/'terminal-size.json').read_text())
                t.wait(lambda: size()['columns'] == 240 and size()['rows'] == 80)
            else:
                assert 'Alt+~' not in t.text()
            if launch == 0:
                # No input or child output is needed to remove the reminder.
                t.wait(lambda: 'Alt+~' not in t.text(), timeout=4)
                t.send(b'\x02')
                t.expect('Open here')
                t.send(b'\r')
                t.expect('FAKE CODING SESSION')
                assert 'Alt+~' not in t.text()
            if launch == 1:
                t.send(b'\x1b~')
                t.expect_action('Background session')
                assert 'Alt+~' not in t.text()
                marker = state/'tracr/drawer-shortcut-learned'
                t.wait(marker.exists)
                t.send(b'\x1b`')
                t.expect('FAKE CODING SESSION')
                assert 'Alt+~' not in t.text()
            t.send(b'\x02')
            t.expect('Open here')
            t.send(b'q')
            t.expect('Close all owned terminals and quit?')
            t.send(b'y')
            t.wait(lambda: t.child.poll() is not None)
            assert t.child.returncode == 0
        finally:
            t.close()
print('reminder placement, expiry, once per launch and persistent learning: passed')
