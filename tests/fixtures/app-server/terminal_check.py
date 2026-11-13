"""Verify terminal attributes are restored when the compiled TUI exits."""
import fcntl
import os
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time

binary, fixture = sys.argv[1:]
for action in ('q', 'ctrl-c', 'sigterm'):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 120, 0, 0))
    original = termios.tcgetattr(slave)
    def child_setup():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    child = subprocess.Popen([binary, 'browse', '--fixture', fixture],
                             stdin=slave, stdout=slave, stderr=slave,
                             preexec_fn=child_setup, env={**os.environ, 'TERM':'xterm-256color'})
    try:
        output = b''
        deadline = time.monotonic() + 10
        while b'Search' not in output:
            if child.poll() is not None or time.monotonic() > deadline:
                raise AssertionError(f'TUI did not start: {output!r}')
            if select.select([master], [], [], 0.1)[0]:
                output += os.read(master, 65536)
        if action == 'sigterm':
            # Wait until the signal handler is installed (the first frame follows setup).
            child.send_signal(signal.SIGTERM)
        else:
            os.write(master, b'q' if action == 'q' else b'\x03')
        deadline = time.monotonic() + 10
        while child.poll() is None:
            if time.monotonic() > deadline:
                raise AssertionError(f'TUI failed to exit ({action}); tail: {output[-1500:]!r}')
            if select.select([master], [], [], 0.1)[0]:
                output += os.read(master, 65536)
        assert child.returncode == 0, output
        assert termios.tcgetattr(slave) == original, 'terminal attributes not restored'
    finally:
        if child.poll() is None:
            child.kill()
        child.wait()
        os.close(master)
        os.close(slave)
