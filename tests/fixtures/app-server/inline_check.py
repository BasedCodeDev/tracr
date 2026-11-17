"""Inline PTYs, background/reattach, multiple owners and explicit shutdown."""
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix='tracr-inline-') as directory:
    home = Path(directory)
    executable = home / 'fake'
    shutil.copyfile(Path(__file__).with_name('fake.py'), executable)
    executable.chmod(0o755)
    (home/'fake.json').write_text('{}')
    (home/'resume').mkdir()
    (home/'listed').mkdir()
    command = [binary, '--codex-home', directory, '--codex-bin', str(executable), str(home/'listed')]
    terminal = Terminal(command)
    other = None
    try:
        terminal.inspect('ID: first')
        terminal.send(b'\r')
        terminal.wait(lambda: (home/'launch.json').exists())
        first = json.loads((home/'launch.json').read_text())
        terminal.expect('Ctrl+B background')
        assert 'Folders' in terminal.text()
        assert os.tcgetpgrp(terminal.master) == terminal.child.pid
        # Pane navigation stays in tracr, with the same PTY still displayed.
        terminal.send(b'\x1b[1;3D')
        terminal.expect('Enter Sessions')
        assert 'Ctrl+B background' in terminal.text()
        terminal.send(b'?')
        terminal.expect('Actions & shortcuts')
        terminal.send(b'\x1b')
        terminal.wait(lambda: 'Actions & shortcuts' not in terminal.text())
        terminal.send(b'\x1b[1;3C')
        terminal.expect('Keyboard input goes to the terminal')
        assert json.loads((home/'launch.json').read_text())['pid'] == first['pid']
        terminal.send(b'\x02')
        terminal.expect('Open here')
        os.kill(first['pid'], 0)
        terminal.send(b'\r')
        terminal.expect('Ctrl+B background')
        assert json.loads((home/'launch.json').read_text())['pid'] == first['pid']
        terminal.send(b'\x02')
        terminal.expect('Open here')
        other = Terminal(command)
        other.inspect('ID: first')
        other.send(b'\r')
        other.expect('open in another tracr')
        other.send(b'q')
        other.wait(lambda: other.child.poll() is not None)
        os.kill(first['pid'], 0)
        # Start another session, leaving the first terminal running.
        terminal.send(b'/second\r')
        terminal.expect('> Task second')
        terminal.inspect('ID: second')
        terminal.send(b'\r')
        terminal.wait(lambda: json.loads((home/'launch.json').read_text())['pid'] != first['pid'])
        second = json.loads((home/'launch.json').read_text())
        terminal.expect('Ctrl+B background')
        terminal.send(b'\x02')
        terminal.expect('Open here')
        terminal.send(b'b')
        terminal.expect('Open terminals')
        terminal.expect('#1 Task')
        terminal.expect('#2 Task')
        terminal.send(b'\x1b')
        terminal.wait(lambda: 'Open terminals' not in terminal.text())
        terminal.send(b'q')
        terminal.expect('Close all owned terminals and quit?')
        terminal.send(b'\x1b')
        terminal.wait(lambda: 'Close all owned terminals and quit?' not in terminal.text())
        for launch in (first, second): os.kill(launch['pid'], 0)
        terminal.send(b'q')
        terminal.expect('Close all owned terminals and quit?')
        terminal.send(b'y')
        terminal.wait(lambda: terminal.child.poll() is not None)
        assert terminal.child.returncode == 0
        for launch in (first, second):
            try: os.kill(launch['pid'], 0)
            except ProcessLookupError: pass
            else: raise AssertionError('owned terminal survived quit')
        print('inline terminals: passed')
    finally:
        terminal.close()
        if other is not None: other.close()
