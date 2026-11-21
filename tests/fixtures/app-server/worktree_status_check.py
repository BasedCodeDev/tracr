"""Dirty checkout labels, branch annotations, and folder details update live."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix='tracr-dirty-') as directory:
    root = Path(directory)
    project = root / 'project'
    project.mkdir()
    env = {**os.environ, 'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1'}
    def git(*args):
        subprocess.run(['git', '-C', str(project), *args], env=env, check=True, capture_output=True)
    git('init', '-qb', 'main')
    git('-c', 'user.name=Test', '-c', 'user.email=t@example.invalid', 'commit', '--allow-empty', '-qm', 'Initial clean commit')
    fixture = root / 'catalog.json'
    session = json.loads(Path(__file__).parents[1].joinpath('sessions/catalog.json').read_text())['sessions'][0]
    session.update(cwd=str(project), archived=False)
    fixture.write_text(json.dumps({'sessions': [session]}))
    terminal = Terminal([binary, 'browse', '--fixture', str(fixture), str(project)], env)
    try:
        terminal.send(b'\x1b`')
        terminal.expect('[main]')
        (project / 'new.txt').write_text('untracked\n')
        terminal.expect('[main *]')
        terminal.expect('project *')
        terminal.expect_focus('Folders')
        terminal.send(b'i')
        terminal.expect('Worktree information')
        terminal.expect('0 staged, 0 modified, 1 untracked')
        terminal.send(b'\x1b')
        git('add', 'new.txt')
        # Read count through a fresh detail view after the status notification.
        terminal.wait(lambda: '[main *]' in terminal.text())
        git('-c', 'user.name=Test', '-c', 'user.email=t@example.invalid', 'commit', '-qm', 'Clean again')
        terminal.wait(lambda: '[main]' in terminal.text() and '[main *]' not in terminal.text())
        terminal.send(b'i')
        terminal.expect('Clean — no uncommitted changes')
        terminal.send(b'\x1b')
        terminal.wait(lambda: 'Worktree information' not in terminal.text())
        terminal.finish()
    finally:
        terminal.close()
print('dirty checkout markers and folder information: passed')
