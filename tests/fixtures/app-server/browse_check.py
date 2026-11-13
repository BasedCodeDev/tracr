"""Observable browser interaction checks using synthetic metadata."""
import json
from pathlib import Path
import shutil
import sys
import tempfile
from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix='tracr-browse-') as tmp:
    home = Path(tmp)
    (home/'listed').mkdir()
    (home/'resume').mkdir()
    (home/'fake.json').write_text('{}')
    (home/'config.toml').write_text('')
    fake = home/'fake'
    shutil.copyfile(Path(__file__).with_name('fake.py'), fake)
    fake.chmod(0o755)
    terminal = Terminal([binary, '--config', str(home/'config.toml'), '--codex-home', tmp, '--codex-bin', str(fake), str(home/'listed')])
    try:
        terminal.expect('DIRECTORY MISMATCH')
        terminal.send(b'/second\r')
        terminal.expect('Global search')
        terminal.expect('ID: second')
        terminal.send(b'\x1b')
        terminal.expect('this folder only')
        terminal.send(b'a/archive\r')
        terminal.expect('ID: archive')
        terminal.send(b'\r')
        terminal.wait(lambda: terminal.text().count('Unarchive this session') >= 2)
        assert not (home/'launch.json').exists()
        terminal.send(b'\x1b')
        terminal.wait(lambda: terminal.text().count('Unarchive this session') == 1)
        terminal.send(b'\x1b')
        terminal.expect('this folder only')
        terminal.send(b'rrr')
        terminal.resize(25, 65)
        terminal.expect('this folder only')
        terminal.resize(80, 240)
        terminal.expect('Details')
        (home/'fake.json').write_text(json.dumps({'malformed': True}))
        terminal.send(b'r')
        terminal.expect('incomplete')
        terminal.expect('Task first')
        terminal.finish()
        print('search, archives, refresh, resize, partial inventory: passed')
    finally:
        terminal.close()
