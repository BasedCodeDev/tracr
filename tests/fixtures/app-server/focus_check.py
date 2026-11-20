"""Folder focus, scoped search, startup defaults and live terminal resizing."""
import json
from pathlib import Path
import shutil
import sys
import tempfile
from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix='tracr-folder-focus-') as directory:
    home = Path(directory)
    listed = home/'listed'
    listed.mkdir()
    outside = home/'outside'
    outside.mkdir()
    fake = home/'fake'
    shutil.copyfile(Path(__file__).with_name('fake.py'), fake)
    fake.chmod(0o755)
    (home/'fake.json').write_text(json.dumps({'read_cwd': str(listed), 'report_size': True}))
    config = home/'config.toml'
    config.write_text('')
    args = [binary, '--config', str(config), '--codex-home', str(home), '--codex-bin', str(fake)]
    import os
    env = {**os.environ, 'XDG_STATE_HOME': str(home/'state')}
    terminal = Terminal(args+[str(listed)], env)
    try:
        terminal.inspect('ID: first')
        terminal.send(b'\x1b`')
        terminal.expect('Folders · Focused')
        terminal.send(b'\x1b`/')
        terminal.expect('Search within folder')
        terminal.expect('Within:')
        terminal.expect('Task first')
        terminal.send(b'\x1b')
        terminal.wait(lambda: 'Search within folder' not in terminal.text())
        terminal.send(b'\r')
        terminal.expect('FAKE CODING SESSION')
        size = lambda: json.loads((home/'terminal-size.json').read_text())
        terminal.wait(lambda: (home/'terminal-size.json').exists())
        original = size()
        terminal.send(b'\x1b`\x1bz')
        terminal.wait(lambda: 'Folders · Focused' not in terminal.text() and size()['columns'] == original['columns'] - 62)
        assert size()['pid'] == original['pid']
        terminal.send(b'\x1bz')
        terminal.expect('Folders · Focused')
        terminal.wait(lambda: size()['columns'] == original['columns'] - 62)
        terminal.send(b'\x1b`x')
        terminal.wait(lambda: 'FAKE CODING SESSION' not in terminal.text())
        terminal.finish()
    finally:
        terminal.close()
    # A new plain-launch context restores the global last folder unfocused.
    terminal = Terminal(args, env)
    try:
        terminal.inspect('ID: first')
        assert 'Folders · Focused' not in terminal.text()
        terminal.send(b'/')
        terminal.expect('Search sessions')
        terminal.send(b'\x1b')
        terminal.wait(lambda: 'Search sessions' not in terminal.text())
        terminal.finish()
    finally:
        terminal.close()
    # An explicit different path replaces the remembered startup context.
    terminal = Terminal(args+[str(outside)], env)
    try:
        terminal.send(b'\x1b`')
        terminal.expect('Folders · Focused')
        terminal.send(b'/')
        terminal.expect('Search within folder')
        terminal.expect('Within:')
        terminal.wait(lambda: 'refreshing' not in terminal.text())
        assert 'Task first' not in terminal.text()
        terminal.send(b'\x1b')
        terminal.wait(lambda: 'Search within folder' not in terminal.text())
        terminal.send(b'\x1b[1;3D\x1bz/')
        terminal.expect('Search sessions')
        terminal.expect('Task first')
        terminal.send(b'\x1b')
        terminal.wait(lambda: 'Search sessions' not in terminal.text())
        terminal.finish()
    finally:
        terminal.close()
    # Each explicit launch path keeps its own toggle state across other launches.
    for folder, focused in [(listed, True), (outside, False), (listed, True)]:
        terminal = Terminal(args+[str(folder)], env)
        try:
            terminal.send(b'/')
            terminal.expect('Search within folder' if focused else 'Search sessions')
            if focused:
                terminal.expect('Within:')
            terminal.send(b'\x1b')
            terminal.wait(lambda: 'Search within folder' not in terminal.text() and 'Search sessions' not in terminal.text())
            terminal.finish()
        finally:
            terminal.close()
    # Plain launches use their working directory as the same context key.
    original_cwd = Path.cwd()
    try:
        for folder, focused in [(listed, True), (outside, False), (listed, True)]:
            os.chdir(folder)
            terminal = Terminal(args, env)
            try:
                terminal.send(b'/')
                terminal.expect('Search within folder' if focused else 'Search sessions')
                if focused:
                    terminal.expect('Within:')
                terminal.send(b'\x1b')
                terminal.wait(lambda: 'Search within folder' not in terminal.text() and 'Search sessions' not in terminal.text())
                terminal.finish()
            finally:
                terminal.close()
    finally:
        os.chdir(original_cwd)
print('folder focus and per-launch-folder persistence: passed')
