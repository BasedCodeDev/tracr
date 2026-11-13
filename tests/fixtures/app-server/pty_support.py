import fcntl
import os
import pty
import re
import select
import signal
import struct
import subprocess
import termios
import time

class Terminal:
    def __init__(self, command, env=None):
        self.master, self.slave = pty.openpty()
        self.resize(80, 240)
        self.original = termios.tcgetattr(self.slave)
        def setup():
            os.setsid()
            fcntl.ioctl(self.slave, termios.TIOCSCTTY, 0)
        self.child = subprocess.Popen(command, stdin=self.slave, stdout=self.slave, stderr=self.slave,
                                      preexec_fn=setup, env={**(env or os.environ), 'TERM':'xterm-256color'})
        self.output = bytearray()
    def resize(self, rows, cols):
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, 0, 0))
        if hasattr(self, 'child'):
            self.child.send_signal(signal.SIGWINCH)
    def text(self):
        # Reconstruct cursor-addressed updates; stripping ANSI loses unchanged
        # characters when Ratatui redraws only differences.
        cells = {}; row = col = 0
        for token in re.findall(r'\x1b\[[0-?]*[ -/]*[@-~]|[^\x1b]', self.output.decode(errors='replace')):
            if token.startswith('\x1b['):
                final = token[-1]
                numbers = [int(n) if n else 0 for n in token[2:-1].lstrip('?').split(';') if n.isdigit() or not n]
                if final in 'Hf':
                    row = max(0, (numbers[0] if numbers else 1)-1)
                    col = max(0, (numbers[1] if len(numbers)>1 else 1)-1)
                elif final == 'J' and numbers and numbers[0] in (2,3):
                    cells.clear()
                elif final == 'K':
                    for key in list(cells):
                        if key[0] == row and key[1] >= col:
                            del cells[key]
                elif final in 'ABCD':
                    amount = (numbers[0] if numbers else 1) or 1
                    if final == 'A': row = max(0,row-amount)
                    if final == 'B': row += amount
                    if final == 'C': col += amount
                    if final == 'D': col = max(0,col-amount)
            elif token == '\r': col = 0
            elif token == '\n': row += 1
            elif token == '\b': col = max(0,col-1)
            elif token.isprintable():
                cells[row,col] = token
                col += 1
        self.cells = cells
        rows = {}
        for (r,c), value in cells.items():
            rows.setdefault(r,{})[c] = value
        return re.sub(r'\s+', ' ', ' '.join(''.join(values.get(c,' ') for c in range(max(values)+1)) for r,values in sorted(rows.items())))
    def line(self, row, start=0, end=240):
        self.text()
        return ''.join(self.cells.get((row, col), ' ') for col in range(start, end))
    def send(self, data):
        os.write(self.master, data)
    def wait(self, predicate, timeout=15):
        end = time.monotonic() + timeout
        while not predicate():
            if time.monotonic() > end:
                raise AssertionError(self.text()[-3000:])
            if select.select([self.master], [], [], .05)[0]:
                try:
                    data = os.read(self.master, 65536)
                except OSError as error:
                    if predicate():
                        return
                    raise RuntimeError('terminal closed before expected state') from error
                if not data:
                    if predicate():
                        return
                    raise RuntimeError('terminal EOF before expected state')
                self.output.extend(data)
            elif self.child.poll() is not None and not predicate():
                raise RuntimeError(f'terminal process exited unexpectedly: {self.child.returncode}')
    def expect(self, text):
        self.wait(lambda: text in self.text())
    def finish(self):
        self.send(b'q')
        self.wait(lambda: self.child.poll() is not None)
        assert self.child.returncode == 0, self.text()[-2000:]
        assert termios.tcgetattr(self.slave) == self.original
    def close(self):
        if self.child.poll() is None:
            self.child.terminate()
            try:
                self.child.wait(timeout=8)
            except subprocess.TimeoutExpired:
                self.child.kill()
        self.child.wait()
        os.close(self.master)
        os.close(self.slave)
