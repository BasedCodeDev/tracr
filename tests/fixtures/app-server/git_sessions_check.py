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
        t.expect('└─ Running Task first · codex')
        t.send(b'\x1b[1;3C')  # Alt+Right focuses Git from Sessions.
        t.expect_action('Select worktree folder')
        t.send(b'\x1b[B')  # First session beneath the commit.
        t.expect_action('Resume session')
        settled = time.monotonic() + 1
        t.wait(lambda: time.monotonic() >= settled)
        count = (home/'git-reads').read_text()
        settings({'type': 'active', 'activeFlags': ['waitingOnUserInput']})
        t.expect('└─ Input Task first · codex')
        t.expect_action('Resume session')
        assert (home/'git-reads').read_text() == count, (count, (home/'git-reads').read_text())
        t.send(b'\r')
        t.wait(lambda: (home/'launch.json').exists())
        launch = json.loads((home/'launch.json').read_text())
        assert launch['args'] == ['resume', '--cd', str(linked), '--', 'first'], launch
        assert launch['cwd'] == str(linked) and launch['foreground'], launch
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
        t.expect_action('Select worktree folder')
        settings({'type': 'idle'})
        t.expect('└─ Idle Task first · codex')
        t.finish()
        print('Git live sessions, direct resume, disappearance, and cached graph: passed')
    finally:
        t.close()
