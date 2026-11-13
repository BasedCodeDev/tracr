"""Check folder shortcuts and caller-directory behavior against a compiled binary.
Run: python tests/fixtures/app-server/folder_check.py target/debug/tracr
"""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time

binary = str(Path(sys.argv[1]).resolve())
project = Path(__file__).resolve().parents[3]
for args in [[''], ['one', 'two'], ['list', '.'], [str(project / 'Cargo.toml')]]:
    result = subprocess.run([binary, *args], capture_output=True)
    assert result.returncode == 2, (args, result.stderr)
for args in [['.'], ['../missing'], ['/missing'], ['a b'], ['./list'], ['--', '-folder'], ['browse', 'list']]:
    result = subprocess.run([binary, *args], capture_output=True)
    assert result.returncode == 1 and b'requires a terminal' in result.stderr, (args, result.stderr)

with tempfile.TemporaryDirectory(prefix='tracr-folder-') as tmp:
    root = Path(tmp)
    (root / 'config.toml').write_text('')
    sessions = []
    for i, (title, cwd) in enumerate([('DIRECTMATCH', root), ('CHILDMATCH', root / 'child'), ('SIBLINGEXCLUDED', Path(tmp + '-other'))]):
        sessions.append(dict(key=dict(provider='codex', source_id='fixture', native_id=str(i)),
                             title=title, cwd=str(cwd), created_at=1, updated_at=3-i,
                             archived=False, source_kind=None, git=None))
    fixture = root / 'catalog.json'
    fixture.write_text(json.dumps(dict(page_size=1, sessions=sessions)))
    for runner, path in [(binary, '.'), (binary, 'missing'), (str(project / 'scripts/dev.sh'), '.')]:
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 35, 150, 0, 0))
        original = termios.tcgetattr(slave)
        def setup():
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
        child = subprocess.Popen([runner, '--config', 'config.toml', 'browse', '--fixture', 'catalog.json', path],
                                 cwd=root, stdin=slave, stdout=slave, stderr=slave,
                                 preexec_fn=setup, env={**os.environ, 'TERM':'xterm-256color', 'CARGO_NET_OFFLINE':'true'})
        output = bytearray()
        try:
            deadline = time.monotonic() + 20
            marker = b'DIRECTMATCH' if path == '.' else b'matching'
            while marker not in output:
                assert child.poll() is None, bytes(output[-1500:])
                assert time.monotonic() < deadline, bytes(output[-1500:])
                if select.select([master], [], [], .1)[0]:
                    output.extend(os.read(master, 65536))
            assert b'SIBLINGEXCLUDED' not in output
            assert b'CHILDMATCH' not in output
            if path == '.':
                assert b'DIRECTMATCH' in output
            # Enter on fixtures must display the browse-only reason, proving session focus.
            output.clear()
            os.write(master, b'\r')
            if path == '.':
                deadline = time.monotonic() + 10
                while b'cannot' not in output:
                    assert time.monotonic() < deadline, bytes(output[-2000:])
                    if select.select([master], [], [], .05)[0]:
                        output.extend(os.read(master, 65536))
            os.write(master, b'q')
            child.wait(timeout=10)
            assert child.returncode == 0
            assert termios.tcgetattr(slave) == original
            print(f'{Path(runner).name} {path}: passed')
        finally:
            if child.poll() is None:
                child.kill()
            child.wait()
            os.close(master)
            os.close(slave)
