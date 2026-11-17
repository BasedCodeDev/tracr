"""Worktree creation through both UI contexts; all repositories and sessions are disposable."""
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
def open_worktree(terminal, expected='Starting commit:', menu=False):
    # Discovery can invalidate ownership between a rendered shortcut and a key.
    # Retry only opening this non-mutating form; never retry creation.
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if menu:
            terminal.send(b'?')
            terminal.expect('Actions & shortcuts')
            terminal.send(b'\x1b[F')
            try:
                terminal.wait(lambda: '> w Create worktree' in terminal.text(), timeout=.5)
            except AssertionError:
                terminal.send(b'\x1b')
                terminal.wait(lambda: 'Actions & shortcuts' not in terminal.text())
                continue
            terminal.send(b'\r')
        else:
            terminal.send(b'w')
        try:
            terminal.wait(lambda: expected in terminal.text(), timeout=.5)
            return
        except AssertionError:
            screen = terminal.text()
            if 'Actions & shortcuts' in screen:
                terminal.send(b'\x1b')
                terminal.wait(lambda: 'Actions & shortcuts' not in terminal.text())
            elif 'Create worktree' in screen:
                terminal.expect(expected)
                return
            elif 'Worktree information' in screen:
                raise AssertionError('Worktree action unexpectedly opened information')
    raise AssertionError(terminal.text()[-3000:])


with tempfile.TemporaryDirectory(prefix='tracr-create-worktree-') as directory:
    home = Path(directory)
    project = home / 'project'
    project.mkdir()
    env = {**os.environ, 'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1'}
    def git(path, *args):
        return subprocess.check_output(['git', '-C', str(path), *args], env=env, stderr=subprocess.PIPE).decode().strip()
    git(project, 'init', '-q')
    git(project, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '--allow-empty', '-qm', 'First creation commit')
    first = git(project, 'rev-parse', 'HEAD')
    git(project, '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '--allow-empty', '-qm', 'Second creation commit')
    head = git(project, 'rev-parse', 'HEAD')
    fake = home / 'fake'
    shutil.copyfile(Path(__file__).with_name('fake.py'), fake)
    fake.chmod(0o755)
    (home / 'fake.json').write_text(json.dumps({'read_cwd': str(project)}))
    t = Terminal([binary, '--codex-home', str(home), '--codex-bin', str(fake), str(project)], env)
    try:
        t.expect('Sessions')
        t.resize(80, 139)  # Full-width graph keeps commit subjects visible.
        t.expect('Second creation commit')
        # Folders menu and keyboard use the same action; escape before execution is inert.
        t.send(b'\x1b');t.expect('Enter Sessions');t.expect('w Worktree');open_worktree(t, menu=True)
        t.send(b'cancelled')
        t.send(b'\r\r')
        t.expect('>[ Create worktree ]')
        assert not (project / '.worktrees').exists()
        t.send(b'\x1b')
        t.wait(lambda: 'Create worktree' not in t.text())
        assert not (project / '.worktrees').exists()
        open_worktree(t)
        t.send(b'feature/from-folder\r\r')
        t.expect('>[ Create worktree ]')
        assert 'this repository will require Git' not in t.text()
        t.resize(22, 70)
        t.expect('>[ Create worktree ]')
        t.send(b'\r')
        t.expect('Worktree created.')
        created = project / '.worktrees/feature/from-folder'
        assert git(created, 'rev-parse', 'HEAD') == head
        assert git(project, 'status', '--porcelain') == ''
        assert not (project / '.gitignore').exists()
        # New worktree is selected in Folders, and its HEAD is followed.
        t.resize(60, 100)
        t.expect('─ ◇ feature/from-folder')
        t.expect('Second creation commit')
        t.send(b'\x1b[Z\x1b[B')
        t.expect('> ● ' + first[:7])
        open_worktree(t)
        t.expect(first[:8])
        t.send(b'from-history\r\r\r')
        t.expect('Worktree created.')
        history = project / '.worktrees/from-history'
        t.wait(lambda: history.exists())
        assert git(history, 'rev-parse', 'HEAD') == first
        assert not (created / '.worktrees').exists()
        # Wait for refreshed ownership before opening the next operation.
        t.expect('[HEAD] [◇ from-history]')
        # Existing branch rejection retains the editable input.
        t.expect('w Worktree');open_worktree(t, menu=True)
        t.send(b'from-history\r\r\r')
        t.expect('That branch already exists')
        t.expect('from-history')
        t.send(b'\x1b')
        t.wait(lambda: 'Create worktree' not in t.text())
        t.finish()
    finally:
        t.close()
    # Feature gating is checked against the invoked Git, independently of browsing.
    real_git = shutil.which('git')
    bindir = home / 'bin'
    bindir.mkdir()
    wrapper = bindir / 'git'
    wrapper.write_text('#!/usr/bin/env python3\nimport os, sys\nif "--version" in sys.argv:\n print("git version 2.47.0")\nelse:\n os.execv(' + repr(real_git) + ', [' + repr(real_git) + '] + sys.argv[1:])\n')
    wrapper.chmod(0o755)
    old_env = {**env, 'PATH': str(bindir) + os.pathsep + env['PATH']}
    t = Terminal([binary, '--codex-home', str(home), '--codex-bin', str(fake), str(project)], old_env)
    try:
        t.expect('Sessions')
        t.resize(80, 139)  # Full-width graph keeps commit subjects visible.
        t.expect('Second creation commit')
        t.send(b'\x1b');t.expect('Enter Sessions');t.expect('w Worktree');open_worktree(t, expected='requires Git 2.48 or newer')
        t.send(b'\x1b')
        t.wait(lambda: 'Create worktree' not in t.text())
        t.finish()
    finally:
        t.close()
print('Worktree creation: Folders/menu, selected commit, local excludes, cancellation, narrow layout and collision passed')
