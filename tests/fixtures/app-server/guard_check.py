"""Run test-only terminal guard failures; no production fault injection."""
import os
import sys
import termios
from pty_support import Terminal
for mode in ['error', 'panic']:
    terminal = Terminal([sys.argv[1], '--ignored', '--exact', 'tui::cleanup_tests::guard_child', '--nocapture'],
                        {**os.environ, 'TRACR_TEST_CLEANUP': mode})
    try:
        terminal.wait(lambda: terminal.child.poll() is not None)
        assert terminal.child.returncode == 0, terminal.text()
        assert termios.tcgetattr(terminal.slave) == terminal.original
    finally:
        terminal.close()
