"""Stash nodes and older-entry deletion update the real terminal graph."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix='tracr-stash-history-') as directory:
    root = Path(directory)
    project = root/'project'
    project.mkdir()
    env = {**os.environ, 'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1'}
    def git(*args):
        result = subprocess.run(['git', '-C', str(project), '-c', 'user.name=Test',
                                 '-c', 'user.email=test@example.invalid', *args],
                                env=env, check=True, capture_output=True, text=True)
        return result.stdout.strip()
    git('init', '-qb', 'main')
    (project/'tracked').write_text('base')
    git('add', 'tracked')
    git('commit', '-qm', 'Stash fixture base')
    fixture = root/'catalog.json'
    session = json.loads(Path(__file__).parents[1].joinpath('sessions/catalog.json').read_text())['sessions'][0]
    session.update(cwd=str(project), archived=False)
    fixture.write_text(json.dumps({'sessions': [session]}))
    terminal = Terminal([binary, 'browse', '--fixture', str(fixture), str(project)], env)
    try:
        terminal.expect('Sessions')
        terminal.resize(80, 139)  # Keep complete stash labels visible.
        terminal.expect('Stash fixture base')
        (project/'tracked').write_text('first')
        git('stash', 'push', '-m', 'Older saved work')
        terminal.expect('[stash@{0}]')
        terminal.expect('Older saved work')
        (project/'tracked').write_text('second')
        (project/'untracked').write_text('extra')
        git('stash', 'push', '-u', '-m', 'Newer saved work')
        terminal.expect('[stash@{1}]')
        terminal.expect('Newer saved work')
        assert '▣' in terminal.text()
        assert 'index on main' not in terminal.text()
        assert 'untracked files on main' not in terminal.text()
        terminal.send(b'\x1b[1;3B\x1b[H')
        terminal.expect('Enter Stash')
        terminal.send(b'\r')
        terminal.expect('Saved stash stash@{0}')
        latest = git('rev-parse', 'refs/stash')
        git('stash', 'drop', 'stash@{1}')
        assert git('rev-parse', 'refs/stash') == latest
        terminal.wait(lambda: '[stash@{1}]' not in terminal.text() and 'Older saved work' not in terminal.text())
        terminal.expect('Newer saved work')
        terminal.resize(24, 60)
        terminal.expect('[stash@{0}]')
        terminal.resize(80, 139)
        git('stash', 'clear')
        terminal.wait(lambda: '[stash@{' not in terminal.text())
        terminal.expect('Stash fixture base')
        terminal.finish()
    finally:
        terminal.close()
print('stash history: markers, navigation, resize, live creation/drop/clear passed')
