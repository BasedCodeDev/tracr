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
        t.expect('DIRECTORY MISMATCH')
        t.send(b'?')
        t.expect('Actions & shortcuts')
        t.expect('Enter activate')
        t.send(b'jjjjjj')  # Creation is unavailable in a nonempty folder.
        t.expect('Creation is available when no sessions are shown.')
        t.send(b'\r')
        assert not (home/'launch.json').exists()
        t.send(b'\x1b')
        t.wait(lambda: 'Actions & shortcuts' not in t.text())
        t.expect('ID: first')
        t.send(b'?')
        t.expect('Actions & shortcuts')
        t.resize(15, 36)
        t.send(b'\x1b[F')
        t.expect('Quit tracr')
        t.send(b'\x1b[H')
        t.expect('Previous pane')
        t.resize(40, 140)
        t.send(b'jjjjjjjj\r\r')  # Search: queued Enter must not select a result.
        t.expect('Search sessions')
        t.send(b'second')
        t.expect('Task second')
        t.send(b'\x1b')
        t.wait(lambda: 'Search sessions' not in t.text())
        t.send(b'?jjjjj\r')  # Resume explicitly through menu.
        t.wait(lambda: (home/'launch.json').exists())
        assert json.loads((home/'launch.json').read_text())['args'][-1] == 'first'
        t.send(b'x')
        t.expect('Resume: Enter')
        t.send(b'?')
        t.expect('Actions & shortcuts')
        t.send(b'\x1b[F\r')
        t.wait(lambda: t.child.poll() is not None)
        assert t.child.returncode == 0
        print('menu navigation, disabled action, resize, search, resume and quit: passed')
    finally:
        t.close()
