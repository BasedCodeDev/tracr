"""Action menu keyboard routing and terminal rendering with an isolated provider."""
import json
from pathlib import Path
import shutil
import sys
import tempfile
from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix='tracr-menu-') as tmp:
    home = Path(tmp)
    (home/'listed').mkdir()
    (home/'resume').mkdir()
    (home/'fake.json').write_text('{}')
    (home/'config.toml').write_text('')
    fake = home/'fake'
    shutil.copyfile(Path(__file__).with_name('fake.py'), fake)
    fake.chmod(0o755)
    t = Terminal([binary, '--config', str(home/'config.toml'), '--codex-home', tmp, '--codex-bin', str(fake), str(home/'listed')])
    try:
        t.inspect('DIRECTORY MISMATCH')
        t.send(b'?')
        t.expect('Actions & shortcuts')
        t.expect('Enter activate')
        t.send(b'\x1b[F')  # The last entry is a session action, with no folder actions.
        t.expect('Open session full-screen')
        assert not (home/'launch.json').exists()
        t.send(b'\x1b')
        t.wait(lambda: 'Actions & shortcuts' not in t.text())
        t.inspect('ID: first')
        t.send(b'?')
        t.expect('Actions & shortcuts')
        t.resize(15, 36)
        t.send(b'\x1b[F')
        t.expect('Open session full-screen')
        t.send(b'\x1b[H')
        t.expect('Help / actions')
        t.resize(40, 140)
        t.send(b'jj\r\r')  # Search: queued Enter must not select a result.
        t.expect('Search within folder')
        t.send(b'second')
        t.expect('Task second')
        t.send(b'\x1b')
        t.wait(lambda: 'Search within folder' not in t.text())
        t.send(b'?jjjjjjjjjjj\r')  # Resume explicitly through menu.
        t.wait(lambda: (home/'launch.json').exists())
        assert json.loads((home/'launch.json').read_text())['args'][-1] == 'first'
        t.send(b'x')
        t.wait(lambda: 'Ctrl+B background' not in t.text())
        t.expect('No sessions currently shown')
        t.send(b'?')
        t.expect('Actions & shortcuts')
        t.send(b'\x1b[Hj\r')
        t.wait(lambda: t.child.poll() is not None)
        assert t.child.returncode == 0
        print('menu navigation, disabled action, resize, search, resume and quit: passed')
    finally:
        t.close()
