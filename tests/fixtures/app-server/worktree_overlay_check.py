"""A full-screen coding terminal keeps its branch and dirty marker live."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix='tracr-worktree-overlay-') as directory:
    home = Path(directory)
    main = home / 'project'
    main.mkdir()
    checkout = home / 'listed'
    env = {**os.environ, 'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1'}

    def git(path, *args):
        subprocess.run(['git', '-C', str(path), *args], env=env, check=True, capture_output=True)

    git(main, 'init', '-qb', 'main')
    git(main, '-c', 'user.name=Test', '-c', 'user.email=t@example.invalid',
        'commit', '--allow-empty', '-qm', 'Initial')
    git(main, 'worktree', 'add', '-b', 'feature/overlay', str(checkout))
    executable = home / 'fake'
    shutil.copyfile(Path(__file__).with_name('fake.py'), executable)
    executable.chmod(0o755)
    (home / 'fake.json').write_text(json.dumps({'report_size': True, 'read_cwd': str(checkout)}))
    terminal = Terminal([binary, '--codex-home', directory, '--codex-bin', str(executable), str(checkout)], env)
    try:
        terminal.inspect('ID: first')
        terminal.send(b'\r')
        terminal.expect('FAKE CODING SESSION')

        def label(value, framed=False):
            edge = terminal.dimensions[1] - int(framed)
            terminal.wait(lambda: terminal.line(0, 0, edge).endswith(value))

        label('feature/overlay')
        terminal.wait(lambda: (home / 'terminal-size.json').exists())
        size = json.loads((home / 'terminal-size.json').read_text())
        assert (size['rows'], size['columns']) == terminal.dimensions
        (checkout / 'new.txt').write_text('untracked\n')
        label('feature/overlay *')
        git(checkout, 'add', 'new.txt')
        git(checkout, '-c', 'user.name=Test', '-c', 'user.email=t@example.invalid',
            'commit', '-qm', 'Clean')
        label('feature/overlay')
        git(checkout, 'switch', '-c', 'feature/renamed')
        label('feature/renamed')
        git(checkout, 'switch', '--detach')
        label('listed')
        git(checkout, 'switch', 'feature/overlay')
        label('feature/overlay')
        terminal.send(b'\x1b`')
        label('feature/overlay', framed=True)
        terminal.send(b'\x1b`')
        label('feature/overlay')
        terminal.resize(30, 45)
        label('feature/overlay')
        terminal.send(b'x')
        terminal.wait(lambda: 'FAKE CODING SESSION' not in terminal.text())
        terminal.finish()
    finally:
        terminal.close()
print('worktree session overlay: passed')
