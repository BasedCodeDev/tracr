"""Wheel scrollback above a CLI composer, without sending child arrow keys."""
import json
import shutil
import sys
import tempfile
from pathlib import Path
from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix='tracr-scroll-') as directory:
    home = Path(directory)
    listed = home/'listed'
    listed.mkdir()
    fake = home/'fake'
    shutil.copyfile(Path(__file__).with_name('fake.py'), fake)
    fake.chmod(0o755)
    (home/'fake.json').write_text(json.dumps({'read_cwd': str(listed), 'scroll_fixture': True}))
    t = Terminal([binary, '--codex-home', str(home), '--codex-bin', str(fake), str(listed)])
    try:
        t.inspect('ID: first')
        t.send(b'\r')
        t.expect('COMPOSER')
        t.expect('HISTORY-0199')
        t.wait(lambda: b'\x1b[?1006h' in t.output)
        # With the drawer closed, the wheel belongs to the coding terminal.
        t.send(b'\x1b[<64;200;10M' * 20)
        t.expect('60 lines')
        t.expect('HISTORY-0100')
        assert 'COMPOSER' not in t.text()
        assert 'HISTORY-0199' not in t.text()
        assert not (home/'terminal-input.bin').exists()
        t.send(b'\x1b`?')
        t.expect('Actions & shortcuts')
        t.send(b'\x1b[<64;200;10M' * 5)
        t.send(b'\x1b')
        t.wait(lambda: 'Actions & shortcuts' not in t.text())
        t.send(b'\x1b`')
        t.expect('60 lines')
        t.resize(80, 180)
        t.expect('60 lines')
        t.send(b'a')
        # The static fixture does not redraw its composer after a height change.
        t.wait(lambda: 'type to return' not in t.text())
        t.wait(lambda: (home/'terminal-input.bin').exists())
        assert (home/'terminal-input.bin').read_bytes() == b'a'
        t.send(b'\x1b[<64;160;10M')
        t.expect('3 lines')
        t.send(b'\x1b[<65;160;10M')
        t.wait(lambda: '3 lines' not in t.text())
        t.send(b'x')
        t.expect_focus('Sessions')
        t.finish()
        assert b'\x1b[?1006l' in t.output
    finally:
        t.close()
print('wheel scrollback, input isolation, modal blocking and resize: passed')
