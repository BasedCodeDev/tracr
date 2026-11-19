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
    (home/'fake.json').write_text(json.dumps({'report_size': True, 'read_cwd': str(home/'listed')}))
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
        assert 'Alt+`' in terminal.text()
        assert os.tcgetpgrp(terminal.master) == terminal.child.pid
        def size():
            return json.loads((home/'terminal-size.json').read_text())
        terminal.wait(lambda: (home/'terminal-size.json').exists())
        initial_columns = size()['columns']
        assert initial_columns > 240 // 2, size()
        terminal.resize(40, 160)
        terminal.wait(lambda: size()['columns'] < initial_columns)
        assert size()['columns'] == 158, size()
        assert size()['pid'] == first['pid']
        terminal.send(b'\x1b`')
        terminal.expect('Enter Sessions')
        terminal.wait(lambda: size()['columns'] == 98)
        terminal.send(b'\x1b`')
        terminal.resize(60, 100)
        terminal.wait(lambda: size()['columns'] == 98)
        terminal.resize(40, 99)
        terminal.wait(lambda: size()['columns'] == 97)
        terminal.resize(80, 240)
        terminal.wait(lambda: size()['columns'] == initial_columns)
        terminal.send(b'\x1b`')
        terminal.expect('Enter Sessions')
        # Moving to the parent hides this terminal, then returning restores it
        # without moving keyboard focus out of Folders or restarting Codex.
        terminal.send(b'\x1bz\x1b[A')
        terminal.wait(lambda: 'Ctrl+B background' not in terminal.text())
        terminal.expect('No sessions currently shown')
        os.kill(first['pid'], 0)
        terminal.send(b'\x1b[B')
        terminal.expect('Ctrl+B background')
        terminal.expect('Enter Sessions')
        assert json.loads((home/'launch.json').read_text())['pid'] == first['pid']
        terminal.send(b'\x1bz')
        terminal.send(b'?')
        terminal.expect('Actions & shortcuts')
        terminal.send(b'\x1b')
        terminal.wait(lambda: 'Actions & shortcuts' not in terminal.text())
        terminal.send(b'\x1b`')
        terminal.expect('Keyboard input goes to the terminal')
        assert json.loads((home/'launch.json').read_text())['pid'] == first['pid']
        terminal.send(b'\x02')
        terminal.expect('Open here')
        # Explicit backgrounding keeps the list when returning to this folder.
        terminal.send(b'\x1b`\x1bz\x1b[A')
        terminal.expect('No sessions currently shown')
        terminal.send(b'\x1b[B')
        terminal.expect('Task first')
        assert 'Ctrl+B background' not in terminal.text()
        terminal.send(b'\x1bz\x1b`')
        terminal.expect('Open here')
        os.kill(first['pid'], 0)
        terminal.send(b'\r')
        terminal.wait(lambda: size()['columns'] == initial_columns and 'Ctrl+B background' in terminal.text())
        assert size()['pid'] == first['pid']
        terminal.send(b'\x02')
        terminal.expect('Open here')
        terminal.send(b'\r')
        terminal.expect('Ctrl+B background')
        assert json.loads((home/'launch.json').read_text())['pid'] == first['pid']
        terminal.send(b'\x1bf')
        terminal.wait(lambda: size()['columns'] == 240 and size()['rows'] == 80)
        terminal.wait(lambda: 'Folders' not in terminal.text() and 'Ctrl+B background' not in terminal.text())
        assert size()['pid'] == first['pid']
        terminal.send(b'\x1bf\x1b`')
        terminal.expect('Ctrl+B background')
        terminal.expect('Enter Sessions')
        terminal.wait(lambda: size()['columns'] == initial_columns - 60)
        terminal.send(b'\x1b`\x1bf')
        terminal.wait(lambda: size()['columns'] == 240 and size()['rows'] == 80)
        terminal.resize(45, 150)
        terminal.wait(lambda: size()['columns'] == 150 and size()['rows'] == 45)
        # Return embedded before Shift+Tab moves focus from Sessions to Folders.
        terminal.send(b'\x1bf\x1b`')
        terminal.expect('Ctrl+B background')
        terminal.expect('Enter Sessions')
        terminal.expect('Ctrl+B background')
        terminal.send(b'\x1b`')
        terminal.send(b'\x02')
        terminal.expect('Open here')
        terminal.resize(80, 240)
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
        terminal.wait(lambda: size()['pid'] == second['pid'] and size()['columns'] == initial_columns)
        terminal.expect('Ctrl+B background')
        terminal.send(b'\x02')
        terminal.expect('Open here')
        terminal.send(b'g')
        terminal.expect('Open terminals')
        terminal.expect('#1 Task')
        terminal.expect('#2 Task')
        terminal.send(b'\r')
        terminal.wait(lambda: 'Open terminals' not in terminal.text() and 'Ctrl+B background' in terminal.text())
        terminal.send(b'\x02g')
        terminal.expect('Open terminals')
        terminal.send(b'\x1bf')
        terminal.wait(lambda: 'Folders' not in terminal.text())
        terminal.send(b'\x02')
        terminal.expect('Open here')
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
