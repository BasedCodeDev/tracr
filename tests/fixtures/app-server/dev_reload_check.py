"""A development reload must wait for a folder move to commit and clean up."""
import json
import os
from pathlib import Path
import sys
import tempfile
import termios
import time
from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix='tracr-reload-move-') as directory:
    root = Path(directory)
    home = root / 'codex'
    home.mkdir()
    old = root / 'old'
    old.mkdir()
    (old / 'file').write_text('preserved')
    destination = root / 'moved'
    (home / 'native.json').write_text(json.dumps({'cwd': str(old), 'profile': ':workspace'}))
    # Gate the native settings update after the directory has moved. Reloading
    # here must wait for the write and the operation's final verification.
    fake = root / 'fake'
    source = Path(__file__).with_name('relink_fake.py').read_text()
    source = source.replace("            assert lock is not None", """            assert lock is not None
            import time
            (home / 'updating').touch()
            while not (home / 'continue').exists(): time.sleep(.02)""")
    fake.write_text(source)
    fake.chmod(0o755)
    control = root / 'control'
    control.mkdir()
    terminal = Terminal([binary, '--codex-home', str(home), '--codex-bin', str(fake), str(old)],
                        {**os.environ, 'TRACR_DEV_CONTROL': str(control), 'XDG_STATE_HOME': str(root / 'state')})
    try:
        terminal.inspect('ID: session')
        terminal.send(b'\x1b`')
        terminal.expect_focus('Folders')
        terminal.send(b'm')
        terminal.expect('Destination folder')
        terminal.send(b'\x1b[Z\x15' + str(destination).encode() + b'\r')
        terminal.wait(lambda: 'Enter confirms' in terminal.text() or '>[ Use this folder ]' in terminal.text())
        if 'Enter confirms' not in terminal.text():
            terminal.send(b'\r')
        terminal.expect('Enter confirms')
        terminal.send(b'\r')
        terminal.wait(lambda: (home / 'updating').exists())
        terminal.send(b'\x1b`')  # Hidden operations still finish before reload.
        (control / 'request').touch()
        time.sleep(1)
        assert terminal.child.poll() is None
        assert not (control / 'ack').exists()
        (home / 'continue').touch()
        terminal.wait(lambda: terminal.child.poll() is not None)
        assert terminal.child.returncode == 75, terminal.text()[-2000:]
        assert (control / 'ack').exists()
        assert termios.tcgetattr(terminal.slave) == terminal.original
        assert (destination / 'file').read_text() == 'preserved'
        assert json.loads((home / 'native.json').read_text())['cwd'] == str(destination)
        assert json.loads((root / 'state/tracr/folder-moves/current.json').read_text())['phase'] == 'complete'
    finally:
        (home / 'continue').touch()
        terminal.close()
print('reload waits for folder move completion: passed')
