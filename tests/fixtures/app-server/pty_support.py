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
import tempfile

class Terminal:
    def __init__(self, command, env=None, *, start_in_sessions=True):
        self.state = tempfile.TemporaryDirectory(prefix='tracr-pty-state-')
        self.master, self.slave = pty.openpty()
        self.resize(80, 240)
        self.original = termios.tcgetattr(self.slave)
        def setup():
            os.setsid()
            fcntl.ioctl(self.slave, termios.TIOCSCTTY, 0)
        self.child = subprocess.Popen(command, stdin=self.slave, stdout=self.slave, stderr=self.slave,
                                      preexec_fn=setup, env={**(env if env is not None else os.environ), 'XDG_STATE_HOME': ((env or {}).get('XDG_STATE_HOME') if (env or {}).get('XDG_STATE_HOME') != os.environ.get('XDG_STATE_HOME') else None) or self.state.name, 'TERM':'xterm-256color'})
        self.output = bytearray()
        # Most interaction fixtures exercise Sessions first. Startup coverage opts
        # out so it can verify the real initial drawer and its focus explicitly.
        if start_in_sessions:
            self.wait(lambda: self.drawer_focused())
            self.send(b'\x1b')
            self.wait(lambda: not self.drawer_focused())
    def resize(self, rows, cols):
        changed = getattr(self, 'dimensions', None) != (rows, cols)
        self.dimensions = (rows, cols)
        offset = len(self.output) if hasattr(self, 'output') else 0
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, 0, 0))
        if hasattr(self, 'child') and changed:
            self.child.send_signal(signal.SIGWINCH)
            # Ratatui clears the screen when adopting new dimensions. Wait for
            # the completed redraw (cursor visibility follows the frame), not just
            # its initial clear, before sending keys. Old screen text can otherwise
            # satisfy expect() while SIGWINCH is still being processed.
            self.wait(lambda: re.search(rb'\x1b\[2J.*\x1b\[\?25[lh]', bytes(self.output[offset:]), re.S) is not None)
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
    def expect_selected_commit(self, title):
        def selected():
            self.text()
            for row in range(self.dimensions[0]):
                line = ''.join(self.cells.get((row, col), ' ') for col in range(self.dimensions[1]))
                if '> ● ' in line and title in line:
                    return True
            return False
        self.wait(selected)

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
    def drawer_focused(self):
        text = self.text()
        return "┏Folders" in text or "┏Git Worktrees" in text or "┏Git━" in text
    def expect_focus(self, pane):
        self.wait(lambda: '┏' + pane in self.text() and self.output.endswith((b'\x1b[?25l', b'\x1b[?25h')))
    def expect_action(self, label, present=True):
        self.send(b'?/')
        self.expect('Search:')
        self.send(label.encode())
        self.expect(label if present else 'No matching actions')
        if present:
            self.wait(lambda: self.text().count(label) >= 2)
        self.send(b'\x1b')
        self.wait(lambda: 'Search:' not in self.text())
        self.send(b'\x1b')
        self.wait(lambda: 'Actions & shortcuts' not in self.text())
    def action(self, label):
        self.send(b'?/')
        self.expect('Search:')
        self.send(label.encode())
        self.wait(lambda: self.text().count(label) >= 2)
        self.send(b'\r')

    def inspect(self, expected):
        self.expect('Sessions')
        self.wait(lambda: all(marker not in self.text() for marker in ('Loading', 'refreshing', 'Fetching metadata')) and '? Actions' in self.text())
        self.send(b'i')
        self.expect('Session information')
        if expected.startswith('ID: '):
            self.send(b'\x1b[F')
            expected='Session ID '+expected[4:]
        elif expected == 'DIRECTORY MISMATCH':
            expected="Folder differs from Codex's current resume destination."
        self.expect(expected)
        self.send(b'i')
        self.wait(lambda: 'Session information' not in self.text())
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
        self.state.cleanup()
