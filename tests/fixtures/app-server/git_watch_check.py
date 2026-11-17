"""Native notifications update the UI without refresh and stay quiet when idle."""
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
real_git = shutil.which('git')
with tempfile.TemporaryDirectory(prefix='tracr-watch-') as directory:
    root = Path(directory)
    project = root/'project'; project.mkdir()
    env = {**os.environ, 'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1'}
    def git(*args):
        return subprocess.run([real_git, '-C', str(project), *args], env=env, check=True, capture_output=True).stdout.decode().strip()
    def commit(message):
        git('-c', 'user.name=Test', '-c', 'user.email=t@example.invalid', 'commit', '--allow-empty', '-qm', message)
    git('init', '-qb', 'main'); commit('Watch initial'); commit('Watch second')
    other = root/'other'; other.mkdir()
    git('-C', str(other), 'init', '-qb', 'main')
    git('-C', str(other), '-c', 'user.name=Test', '-c', 'user.email=t@example.invalid', 'commit', '--allow-empty', '-qm', 'Other initial')
    fixture = root/'catalog.json'
    base = json.loads(Path(__file__).parents[1].joinpath('sessions/catalog.json').read_text())['sessions'][0]
    base.update(cwd=str(project), archived=False)
    other_session = json.loads(json.dumps(base))
    other_session['key']['native_id'] = 'other'
    other_session.update(cwd=str(other), title='Other project session')
    fixture.write_text(json.dumps({'sessions': [base, other_session]}))
    bindir = root/'bin'; bindir.mkdir(); calls = root/'calls'
    wrapper = bindir/'git'
    wrapper.write_text('''#!/usr/bin/env python3
import os,sys
with open(os.environ['WATCH_CALLS'],'a') as f: f.write(' '.join(sys.argv[1:])+'\\n')
os.execv(os.environ['WATCH_GIT'], [os.environ['WATCH_GIT'], *sys.argv[1:]])
''')
    wrapper.chmod(0o755)
    env.update(PATH=str(bindir)+os.pathsep+env['PATH'], WATCH_CALLS=str(calls), WATCH_GIT=real_git)
    t = Terminal([binary, 'browse', '--fixture', str(fixture), str(project)], env)
    def settle(seconds=1):
        end = time.monotonic()+seconds
        t.wait(lambda: time.monotonic() >= end, timeout=seconds+5)
    # Status checks are independent of graph/metadata discovery reads.
    def read_calls(): return [line for line in calls.read_text().splitlines() if ' status ' not in ' '+line+' ']
    try:
        t.expect('Watch second'); t.expect('[HEAD]'); settle()
        # Cross-repository search below intentionally uses the global scope.
        t.send(b'\x1b[1;3D\x1bz\x1b[1;3C')
        t.wait(lambda: 'Folders · Focused' not in t.text())
        commit('Watch external'); t.expect('Watch external'); t.expect('[HEAD]')
        git('tag', 'watch-tag'); t.expect('tag: watch-tag')
        git('branch', 'watch-branch'); t.expect('[watch-branch]')
        git('pack-refs', '--all'); settle()
        assert '[watch-branch]' in t.text()
        # Manual navigation must survive an external commit.
        t.send(b'\t\x1b[B'); t.expect('g Collapse')
        selected_oid = git('rev-parse', '--short=7', 'HEAD~1')
        commit('Watch manual preserved'); t.expect('Watch manual preserved'); t.expect('g Collapse')
        t.expect('> ● ' + selected_oid)
        t.send(b'f'); t.expect('[HEAD]')
        git('checkout', '--detach', '-q'); settle(); t.expect('[HEAD]')
        git('checkout', '-q', 'main'); t.expect('[HEAD]')
        t.send(b'g'); t.expect('g Expand'); settle()
        mark = len(read_calls())
        linked = root/'elsewhere'
        git('worktree', 'add', '-qb', 'watch-linked', str(linked))
        t.expect('─ ◇ watch-linked'); settle()
        assert not any(' log ' in ' '+line+' ' for line in read_calls()[mark:]), read_calls()[mark:]
        git('worktree', 'remove', str(linked)); t.wait(lambda: '─ ◇ watch-linked' not in t.text())
        commit('Watch hidden commit'); settle()
        t.send(b'g'); t.expect('Watch hidden commit'); settle()
        mark = len(read_calls()); before = len(t.output)
        (project/'ordinary.txt').write_text('Source change is not metadata\n')
        t.expect('[main *]'); settle()
        before = len(t.output)
        git('status', '--porcelain'); git('log', '-1')
        git('-C', str(other), 'branch', 'inactive-change'); settle(2)
        assert len(read_calls()) == mark, read_calls()[mark:]
        assert len(t.output) == before, 'Unexpected unchanged-screen output'
        t.send(b'\t/Other project session'); t.expect('Other project session')
        t.send(b'\r'); t.expect('Other initial'); settle()
        mark = len(read_calls()); before = len(t.output)
        commit('Inactive old repository'); settle(1)
        assert len(read_calls()) == mark, read_calls()[mark:]
        assert len(t.output) == before, 'Old repository still wakes UI'
        git('-C', str(other), 'tag', 'active-other-tag'); t.expect('tag: active-other-tag')
        # Quit releases the native inotify descriptor as well as Git children.
        t.finish()
    finally: t.close()
print('Native Git watch: external updates, manual mode, hidden history, registration changes and idle passed')
