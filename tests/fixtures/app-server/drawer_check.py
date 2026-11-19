"""Sessions-first drawer, dialog ownership, and live PTY sizing/input acceptance."""
import json
import os
from pathlib import Path
import shutil
import signal
import sys
import tempfile
from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix='tracr-drawer-') as directory:
    root = Path(directory)
    project = root / 'project'
    project.mkdir()
    config = root / 'config.toml'
    config.write_text('')
    fake = root / 'fake'
    shutil.copyfile(Path(__file__).with_name('fake.py'), fake)
    fake.chmod(0o755)
    (root / 'fake.json').write_text(json.dumps({
        'report_size': True, 'scroll_fixture': True, 'read_cwd': str(project),
    }))
    t = Terminal([binary, '--config', str(config), '--codex-bin', str(fake),
                  '--codex-home', str(root), str(project)])
    toggle = b'\x1b`'
    try:
        t.expect('Create new session')
        t.expect('key below Esc; no Shift needed')
        assert 'Close drawer' not in t.text()
        # An unfinished main search and an unfinished drawer search are independent.
        t.send(b'/main draft')
        t.expect('main draft')
        t.send(toggle)
        t.expect('Alt+` / Esc Close')
        assert 'Search within folder' not in t.text()
        t.send(b'/drawer draft')
        t.expect('drawer draft')
        t.send(toggle)
        t.expect('main draft')
        assert 'drawer draft' not in t.text()
        t.send(b'\x1b')
        t.wait(lambda: 'draft' not in t.text())
        t.send(toggle)
        t.expect('drawer draft')
        t.send(b'\x1b')
        t.wait(lambda: 'draft' not in t.text())
        t.expect('Alt+` / Esc Close')
        t.send(b'\x1b')
        t.wait(lambda: 'Alt+` / Esc Close' not in t.text())
        # Launch a real child PTY using the test harness, without a real Codex account.
        t.send(b'n')
        t.wait(lambda: (root / 'launch.json').exists())
        t.expect('COMPOSER')
        assert [t.line(row, 233, 240) for row in range(3)] == ['┌─────┐', '│Alt+~│', '└─────┘']
        launch = json.loads((root / 'launch.json').read_text())
        def size():
            return json.loads((root / 'terminal-size.json').read_text())
        t.wait(lambda: size()['columns'] == 240)
        t.send(b'\t')
        inputs = root / 'terminal-input.bin'
        assert size()['rows'] == 80
        t.wait(lambda: inputs.exists() and inputs.read_bytes() == b'\t')
        t.send(toggle)
        t.expect('Alt+` / Esc Close')
        t.wait(lambda: size()['columns'] == 178)
        t.send(b'\t')
        t.send(toggle)
        t.wait(lambda: size()['columns'] == 240)
        t.send(b'a')
        t.wait(lambda: inputs.read_bytes().endswith(b'a'))
        assert inputs.read_bytes() == b'\ta', inputs.read_bytes()
        assert size()['pid'] == launch['pid']
        # Alt+F is ordinary child input, not a layout control.
        t.send(b'\x1bf')
        t.wait(lambda: inputs.read_bytes() == b'\ta\x1bf')
        assert size()['columns'] == 240 and size()['rows'] == 80
        assert 'Ctrl+B background' not in t.text() and 'Alt+`' not in t.text()
        t.send(toggle)
        t.expect('Ctrl+B Background')
        assert '│Alt+~│' not in t.text()
        t.wait(lambda: size()['columns'] == 178)
        t.send(b'\x1b~')
        t.wait(lambda: size()['columns'] == 240)
        # Resize across the breakpoint while the drawer owns input.
        t.send(toggle)
        t.expect('Alt+` / Esc Close')
        t.resize(30, 119)
        t.expect('Alt+` / Esc Close')
        assert 'COMPOSER' not in t.text()
        t.resize(30, 120)
        t.wait(lambda: size()['columns'] == 70)
        t.send(toggle)
        t.wait(lambda: size()['columns'] == 120)
        assert size()['pid'] == launch['pid']
        # A background child failure belongs to main, without replacing a drawer form.
        t.send(toggle + b'/retained draft')
        t.expect('retained draft')
        os.kill(launch['pid'], signal.SIGTERM)
        t.wait(lambda: not Path(f"/proc/{launch['pid']}").exists())
        assert 'Terminal exited' not in t.text()
        t.send(toggle)
        t.expect('Terminal exited')
        assert '│Alt+~│' not in t.text()
        t.send(b'\x1b')
        t.wait(lambda: 'Terminal exited' not in t.text())
        t.send(toggle)
        t.expect('retained draft')
        t.send(b'\x1b')
        t.wait(lambda: 'draft' not in t.text())
        t.expect('Alt+` / Esc Close')
        t.send(b'\x1b')
        t.wait(lambda: 'Alt+` / Esc Close' not in t.text())
        t.finish()
        print('drawer startup, dual dialogs, input isolation, PTY resize and default full screen and child input: passed')
    finally:
        t.close()
