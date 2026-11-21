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
        assert 'Ctrl+B Browser' not in terminal.text()
        assert os.tcgetpgrp(terminal.master) == terminal.child.pid
        def size(pid=first['pid']):
            return json.loads((home/f'terminal-size-{pid}.json').read_text())
        terminal.wait(lambda: (home/'terminal-size.json').exists() and size()['columns'] == 240)
        initial_columns = 240
        terminal.resize(40, 160)
        terminal.wait(lambda: size()['columns'] == 160 and size()['rows'] == 40)
        terminal.send(b'\x1b`')
        terminal.expect_action('Background session')
        terminal.wait(lambda: size()['columns'] == 98)
        # Ctrl+B works while drawer navigation owns input and keeps it open.
        terminal.send(b'\x02')
        terminal.expect('Open here')
        # Mouse-mode updates can follow the frame's cursor-visibility escape.
        terminal.expect('┏Folders')
        assert terminal.line(0).startswith('┏Folders')
        assert 'FAKE CODING SESSION' not in terminal.text()
        os.kill(first['pid'], 0)
        terminal.send(b'\x1b`\r')
        terminal.expect('FAKE CODING SESSION')
        terminal.wait(lambda: size()['columns'] == 160)
        assert size()['pid'] == first['pid']
        terminal.send(b'\x1b`')
        terminal.resize(30, 100)
        terminal.expect_action('Background session')
        assert 'FAKE CODING SESSION' not in terminal.text()
        terminal.send(b'\x02')
        # Mouse-mode updates can follow the frame's cursor-visibility escape.
        terminal.expect('┏Folders')
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
        terminal.expect('? Actions')
        assert terminal.line(0).startswith('┌Folders')
        other = Terminal(command)
        other.inspect('ID: first')
        other.send(b'\r')
        other.expect('FAKE CODING SESSION')
        other.wait(lambda: json.loads((home/'launch.json').read_text())['pid'] != first['pid'])
        shared = json.loads((home/'launch.json').read_text())
        assert shared['args'] == first['args']
        assert shared['home'] == first['home']
        assert len(list(home.glob('launch-*.json'))) == 2
        other.resize(35, 120)
        other.wait(lambda: size(shared['pid'])['columns'] == 120)
        assert size()['columns'] == initial_columns
        other.send(b'\x02')
        other.expect('Open here')
        other.send(b'\r')
        other.expect('FAKE CODING SESSION')
        assert len(list(home.glob('launch-*.json'))) == 2
        other.send(b'\x02')
        other.expect('Open here')
        other.send(b'q')
        other.expect('Close all owned terminals and quit?')
        other.send(b'y')
        other.wait(lambda: other.child.poll() is not None)
        try: os.kill(shared['pid'], 0)
        except ProcessLookupError: pass
        else: raise AssertionError('second instance terminal survived quit')
        os.kill(first['pid'], 0)
        terminal.send(b'\r')
        terminal.expect('FAKE CODING SESSION')
        terminal.resize(70, 220)
        terminal.wait(lambda: size()['columns'] == 220)
        terminal.resize(80, 240)
        terminal.wait(lambda: size()['columns'] == initial_columns)
        terminal.send(b'\x02')
        terminal.expect('Open here')
        # Start another session, leaving the first terminal running.
        terminal.send(b'/second\r')
        terminal.expect('> Task second')
        terminal.inspect('ID: second')
        terminal.send(b'\r')
        terminal.wait(lambda: json.loads((home/'launch.json').read_text())['pid'] not in (first['pid'], shared['pid']))
        second = json.loads((home/'launch.json').read_text())
        terminal.wait(lambda: size(second['pid'])['pid'] == second['pid'] and size(second['pid'])['columns'] == initial_columns)
        terminal.expect('FAKE CODING SESSION')
        terminal.send(b'\x02')
        terminal.expect('Open here')
        terminal.send(b'\x1b`g')
        terminal.expect('Open terminals')
        terminal.expect('#1 Task')
        terminal.expect('#2 Task')
        terminal.send(b'\r')
        terminal.wait(lambda: 'Open terminals' not in terminal.text() and 'FAKE CODING SESSION' in terminal.text())
        terminal.wait(lambda: not terminal.drawer_focused())
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
