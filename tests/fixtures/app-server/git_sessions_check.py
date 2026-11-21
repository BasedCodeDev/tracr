"""Live session annotations and navigation in a real Git history terminal."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix='tracr-git-sessions-') as tmp:
    home = Path(tmp)
    project = home / 'project'
    linked = home / 'listed'
    project.mkdir()
    real_git = shutil.which('git')
    env = {**os.environ, 'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1'}
    def git(*args):
        subprocess.run([real_git, '-C', str(project), *args], env=env, check=True, capture_output=True)
    git('init', '-q')
    git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '--allow-empty', '-qm', 'Earlier commit', '-m', 'Earlier details marker')
    git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '--allow-empty', '-qm', 'Session anchor')
    git('worktree', 'add', '-qb', 'feature', str(linked))
    fake = home / 'fake'
    shutil.copyfile(Path(__file__).with_name('fake.py'), fake)
    fake.chmod(0o755)
    def settings(status):
        staging = home / 'fake.tmp'
        staging.write_text(json.dumps({'live_enabled': True, 'live_threads': {'first': status}, 'read_cwd': str(linked)}))
        staging.replace(home / 'fake.json')
    settings({'type': 'active', 'activeFlags': []})
    # Count graph reads to ensure provider changes do not reload Git geometry.
    wrapper_dir = home / 'bin'
    wrapper_dir.mkdir()
    wrapper = wrapper_dir / 'git'
    wrapper.write_text("#!/usr/bin/env python3\nimport os, sys\nfrom pathlib import Path\nif 'log' in sys.argv[1:]:\n    with Path(os.environ['GIT_READ_LOG']).open('a') as f: f.write('log\\n')\nos.execv(os.environ['REAL_GIT'], [os.environ['REAL_GIT'], *sys.argv[1:]])\n")
    wrapper.chmod(0o755)
    env.update(PATH=str(wrapper_dir) + os.pathsep + env['PATH'], REAL_GIT=real_git, GIT_READ_LOG=str(home/'git-reads'))
    t = Terminal([binary, '--codex-home', tmp, '--codex-bin', str(fake), str(linked)], env)
    try:
        t.expect('Sessions')
        t.resize(40, 119)
        t.send(b'\x1b`')
        t.expect('Running Task first · codex')
        t.send(b'\x1b[1;3C')  # Alt+Right focuses Git from Sessions.
        t.expect_action('Select worktree folder')
        # Zoom uses the same graph, dialogs, and navigation in both layouts.
        t.send(b'\x1bz')
        t.expect_action('Restore Git panel')
        assert t.line(0).startswith('┏Git Worktrees')
        assert '┏Folders' not in t.text() and '┌Folders' not in t.text()
        t.expect('Commit information')
        t.expect('Session anchor')
        t.send(b'\x1b[F')
        t.expect('Earlier details marker')
        t.send(b'\x1b[H')
        t.wait(lambda: 'Earlier details marker' not in t.text())
        t.expect('No extended commit message.')
        t.send(b'i')
        t.wait(lambda: '┏Commit information' in t.line(0))
        t.send(b'2')
        t.expect('No file changes in this commit.')
        t.send(b'\t\x1b[F')
        t.wait(lambda: 'Earlier commit' in t.line(2, 60, 119))
        t.expect('No file changes in this commit.')
        t.send(b'\t3')
        t.expect('COMMITTER')
        t.send(b'\t\x1b[H')
        t.wait(lambda: 'Session anchor' in t.line(2, 60, 119))
        t.expect('COMMITTER')
        t.wait(lambda: t.line(0).startswith('┏Git Worktrees'))
        assert 'Commit information' in t.text()
        t.expect_action('Restore Git panel')
        for rows, cols in [(50, 180), (30, 90), (40, 119)]:
            t.resize(rows, cols)
            t.expect_action('Restore Git panel')
            assert '┏Folders' not in t.text() and '┌Folders' not in t.text()
        t.send(b'\x1b[D')  # Left remains graph scrolling.
        t.expect_action('Restore Git panel')
        for exit_key in [b'\x1bz']:
            t.send(exit_key)
            t.wait(lambda: not t.line(0).startswith('┏Git Worktrees'))
            t.expect_action('Select worktree folder')
            t.send(b'\x1bz')
            t.expect_action('Restore Git panel')
        t.expect('Alt+Z Exit zoom')
        t.send(b'\x1b\x7f')
        t.expect('Alt+Z Exit zoom')
        for tab in [b'\t', b'\x1b[Z']:
            t.send(tab)
            t.expect_focus('Commit information')
            t.expect('Alt+Z Exit zoom')
            t.send(tab)
            t.expect_focus('Git Worktrees')
        t.expect_action('Restore Git panel')
        t.send(b'\x1b[B')  # First session beneath the commit.
        t.expect_action('Resume session')
        t.expect('No extended commit message.')
        assert 'Select a commit to view details' not in t.text()
        settled = time.monotonic() + 1
        t.wait(lambda: time.monotonic() >= settled)
        count = (home/'git-reads').read_text()
        settings({'type': 'active', 'activeFlags': ['waitingOnUserInput']})
        t.expect('Input Task first · codex')
        t.expect_action('Resume session')
        assert (home/'git-reads').read_text() == count, (count, (home/'git-reads').read_text())
        t.send(b'\r')
        t.wait(lambda: (home/'launch.json').exists())
        launch = json.loads((home/'launch.json').read_text())
        assert launch['args'] == ['resume', '--cd', str(linked), '--', 'first'], launch
        assert launch['cwd'] == str(linked) and launch['foreground'], launch
        t.expect('FAKE CODING SESSION')
        assert json.loads((home/'launch.json').read_text())['pid'] == launch['pid']
        # The running terminal stays alive behind full-screen Git.
        t.send(b'\x1b`')
        t.expect('Git Worktrees')
        t.send(b'\x1bz')
        t.expect_action('Restore Git panel')
        assert 'FAKE CODING SESSION' not in t.text()
        t.send(b'\x1bz\x1b[C')
        t.expect('FAKE CODING SESSION')
        assert json.loads((home/'launch.json').read_text())['pid'] == launch['pid']
        t.send(b'x')
        t.expect('? Actions')
        t.inspect('ID: first')
        t.send(b'\x1b`')
        t.expect('Git Worktrees')
        t.send(b'\x1b[H')
        t.expect_action('Select worktree folder')
        t.send(b'\x1b[B')
        t.expect_action('Resume session')
        settings({'type': 'notLoaded'})
        # This fixture has one annotated session; wait for its connector regardless of status.
        t.wait(lambda: '└─ ' not in t.text())
        t.send(b'\x1b[H')  # Select the commit after the live session disappears.
        t.expect_action('Select worktree folder')
        settings({'type': 'idle'})
        t.expect('Idle Task first · codex')
        t.finish()
        print('Git live sessions, direct resume, disappearance, and cached graph: passed')
    finally:
        t.close()
