"""Linux PTY acceptance checks against a compiled tracr, without inference or real homes.
Run: python tests/fixtures/app-server/resume_check.py target/debug/tracr
"""
import fcntl
import json
import os
from pathlib import Path
from pty_support import Terminal
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

binary = str(Path(sys.argv[1]).resolve())
fake = str(Path(__file__).with_name('fake.py').resolve())
for action in ('normal', 'nonzero', 'ctrl-c', 'sigterm', 'kill-timeout', 'reload'):
    with tempfile.TemporaryDirectory(prefix='tracr resume $ ') as directory:
        home = Path(directory)
        import shutil
        executable = home / 'fake'
        shutil.copyfile(fake, executable)
        executable.chmod(0o755)
        (home / 'resume').mkdir()
        (home / 'listed').mkdir()
        (home / 'fake.json').write_text(json.dumps({'resume_exit': 7 if action == 'nonzero' else 0, 'ignore_term': action == 'kill-timeout'}))
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 80, 240, 0, 0))
        original = termios.tcgetattr(slave)
        def setup():
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
        child = subprocess.Popen([binary, '--codex-home', directory, '--codex-bin', str(executable), str(home / 'listed')],
                                 stdin=slave, stdout=slave, stderr=slave, preexec_fn=setup,
                                 env={**os.environ, 'XDG_STATE_HOME': str(home/'state'), 'TERM': 'xterm-256color', **({'TRACR_DEV_CONTROL': directory} if action == 'reload' else {})})
        output = bytearray()
        screen = Terminal.__new__(Terminal)
        screen.output = output
        def wait(predicate):
            deadline = time.monotonic() + 12
            while not predicate():
                if time.monotonic() > deadline:
                    raise AssertionError(f'{action} timeout: {screen.text()[-2500:]}')
                if select.select([master], [], [], .05)[0]:
                    output.extend(os.read(master, 65536))
        try:
            wait(lambda: b'Task first' in output)
            # Wait for discovery and detail readiness, then resume from the initially focused sessions pane.
            wait(lambda: (home / 'requests.jsonl').exists() and 'thread/read' in (home / 'requests.jsonl').read_text())
            os.write(master,b'i')
            wait(lambda: "Folder differs from Codex's current resume destination." in screen.text())
            os.write(master,b'i')
            wait(lambda: 'Session information' not in screen.text())
            session_id = 'first'
            if action == 'normal':
                os.write(master, b'/second\r')
                wait(lambda: '> Task second' in screen.text() and 'Search sessions' not in screen.text())
                os.write(master,b'i')
                wait(lambda: "Folder differs from Codex's current resume destination." in screen.text())
                os.write(master,b'\x1b[F')
                wait(lambda: 'Session ID second' in screen.text())
                os.write(master,b'i')
                wait(lambda: 'Session information' not in screen.text())
                session_id = 'second'
            os.write(master, b'\r')
            wait(lambda: (home / 'launch.json').exists())
            launch = json.loads((home / 'launch.json').read_text())
            assert launch['foreground'], launch
            assert launch['cwd'] == str(home / 'resume'), launch
            assert launch['args'] == ['resume', '--cd', str(home / 'resume'), '--', session_id], launch
            assert launch['home'] == directory
            if action == 'reload':
                (home / 'request').touch()
                time.sleep(1)
                assert child.poll() is None
                assert not (home / 'ack').exists()
                os.kill(launch['pid'], 0)  # Active Codex must survive the request.
                os.write(master, b'x')
            elif action in ('sigterm', 'kill-timeout'):
                child.send_signal(signal.SIGTERM)
            else:
                return_mark=len(output)
                os.write(master, b'\x03' if action == 'ctrl-c' else b'x')
                if action in ('nonzero', 'ctrl-c'):
                    wait(lambda: 'Terminal exited' in screen.text())
                    os.write(master, b'\x1b')
                wait(lambda: os.tcgetpgrp(master) == child.pid)
                # Wait for refreshed browser before quitting.
                wait(lambda: b'Sessions' in output[return_mark:])
                if action == 'normal':
                    wait(lambda: '> Task second' in screen.text())
                    assert 'Search sessions' not in screen.text()
                    os.write(master,b'i')
                    wait(lambda: 'Session information' in screen.text())
                    os.write(master,b'\x1b[F')
                    wait(lambda: 'Session ID second' in screen.text())
                    os.write(master,b'i')
                    wait(lambda: 'Session information' not in screen.text())
                os.write(master, b'q')
            wait(lambda: child.poll() is not None)
            assert child.returncode == (75 if action == 'reload' else 0), bytes(output)
            if action == 'reload':
                assert (home / 'ack').exists()
            assert termios.tcgetattr(slave) == original, 'terminal attributes not restored'
            try:
                os.kill(launch['pid'], 0)
                raise AssertionError('resume child survived')
            except ProcessLookupError:
                pass
            print(action + ': passed')
        finally:
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    child.kill()
            child.wait()
            os.close(master)
            os.close(slave)
