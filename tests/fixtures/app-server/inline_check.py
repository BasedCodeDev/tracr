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
        terminal.expect('FAKE CODING SESSION')
        assert 'Alt+F' not in terminal.text()
        assert 'Ctrl+B background' not in terminal.text()
        assert os.tcgetpgrp(terminal.master) == terminal.child.pid
        def size():
            return json.loads((home/'terminal-size.json').read_text())
        terminal.wait(lambda: (home/'terminal-size.json').exists() and size()['columns'] == 240)
        initial_columns = 240
        terminal.resize(40, 160)
        terminal.wait(lambda: size()['columns'] == 160 and size()['rows'] == 40)
        terminal.send(b'\x1b`')
        terminal.expect('Ctrl+B Background')
        terminal.wait(lambda: size()['columns'] == 98)
        # Ctrl+B works while drawer navigation owns input and keeps it open.
        terminal.send(b'\x02')
        terminal.expect('Open here')
        terminal.expect('Enter Sessions')
        assert 'FAKE CODING SESSION' not in terminal.text()
        os.kill(first['pid'], 0)
        terminal.send(b'\x1b`\r')
        terminal.expect('FAKE CODING SESSION')
        terminal.wait(lambda: size()['columns'] == 160)
        assert size()['pid'] == first['pid']
        terminal.send(b'\x1b`')
        terminal.resize(30, 100)
        terminal.expect('Ctrl+B Background')
        assert 'FAKE CODING SESSION' not in terminal.text()
        terminal.send(b'\x02')
        terminal.expect('Enter Sessions')
        terminal.send(b'\x1b`')
        terminal.expect('Open here')
        terminal.send(b'\r')
        terminal.expect('FAKE CODING SESSION')
        terminal.wait(lambda: size()['columns'] == 100 and size()['rows'] == 30)
        assert size()['pid'] == first['pid']
        terminal.resize(80, 240)
        terminal.wait(lambda: size()['columns'] == 240)
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
        terminal.expect('FAKE CODING SESSION')
        terminal.send(b'\x02')
        terminal.expect('Open here')
        terminal.send(b'g')
        terminal.expect('Open terminals')
        terminal.expect('#1 Task')
        terminal.expect('#2 Task')
        terminal.send(b'\r')
        terminal.wait(lambda: 'Open terminals' not in terminal.text() and 'FAKE CODING SESSION' in terminal.text())
        terminal.send(b'\x02g')
        terminal.expect('Open terminals')
        terminal.send(b'\r')
        terminal.expect('FAKE CODING SESSION')
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
