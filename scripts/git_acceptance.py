#!/usr/bin/env python3
"""Explicit large-repository acceptance; writes JSON evidence, never publishes.
Usage: python3 scripts/git_acceptance.py target/release/tracr --output /tmp/git-acceptance.json
Timing and memory are measurements, not pass/fail thresholds.
"""
import argparse
from collections import Counter
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT/'tests/fixtures/app-server'))
from pty_support import Terminal


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--seconds', type=float, default=30)
    args = parser.parse_args()
    if args.seconds <= 0: parser.error('--seconds must be positive')
    binary = str(args.binary.resolve())
    real_git = shutil.which('git')
    report = {'binary': binary, 'git': subprocess.check_output([real_git, '--version'], text=True).strip(),
              'terminal': {'rows': 80, 'columns': 240}, 'cases': [], 'passed': False}
    env = {**os.environ, 'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1'}
    try:
        with tempfile.TemporaryDirectory(prefix='tracr-acceptance-') as tmp:
            root = Path(tmp)
            def git(path, *command, data=None):
                return subprocess.run([real_git, '-C', str(path), *command], input=data,
                                      env=env, check=True, capture_output=True).stdout.decode().strip()
            def generate(path, wide=False):
                path.mkdir(); git(path, 'init', '-qb', 'main')
                stream = []
                def commit(branch, mark, subject, parent=None, merge=None):
                    stream.extend([f'commit refs/heads/{branch}\nmark :{mark}\n',
                                   f'committer Acceptance <test@example.invalid> {1700000000+mark} +0000\n',
                                   f'data {len(subject)}\n{subject}\n'])
                    if parent: stream.append(f'from :{parent}\n')
                    if merge: stream.append(f'merge :{merge}\n')
                    stream.append('\n')
                if not wide:
                    for i in range(30000): commit('main', i+1, f'Commit {i:05}', i if i else None)
                    count = 30000
                else:
                    commit('main', 1, 'Wide root'); mark = 1; heads = []
                    for branch in range(64):
                        parent = 1
                        for i in range(16):
                            mark += 1; commit(f'branch-{branch:02}', mark, f'Branch {branch:02} commit {i:02}', parent); parent = mark
                        heads.append(mark)
                    parent = 1
                    for branch, head in enumerate(heads):
                        mark += 1; commit('main', mark, f'Merge {branch:02}', parent, head); parent = mark
                    count = mark
                git(path, 'fast-import', '--quiet', data=''.join(stream).encode())
                assert int(git(path, 'rev-list', '--all', '--count')) == count
                short = git(path, 'log', '--format=%H', '--all', '--topo-order', '--max-count=128').splitlines()
                longer = git(path, 'log', '--format=%H', '--all', '--topo-order', '--max-count=256').splitlines()
                commit_line = re.compile(r'[0-9a-f]{40}$')
                last = max(i for i, line in enumerate(short) if commit_line.search(line))
                assert short[:last+1] == longer[:last+1], 'Commit ordering changed across expanding prefix'
                return count
            linear = root/'linear'; wide = root/'wide'
            counts = {linear: generate(linear), wide: generate(wide, True)}
            old = root/'old'
            git(linear, 'worktree', 'add', '-qb', 'old', str(old), 'HEAD~25000')
            for case, project, selected, marker in [
                ('linear', linear, linear, 'Commit 29999'), ('wide', wide, wide, 'Merge 63'),
                ('old-head', linear, old, 'Commit 04999'), ('cancel-expansion', linear, linear, 'Commit 29999')]:
                case_dir = root/case; case_dir.mkdir(exist_ok=True)
                fixture = case_dir/'catalog.json'
                session = json.loads((ROOT/'tests/fixtures/sessions/catalog.json').read_text())['sessions'][0]
                session.update(cwd=str(selected), archived=False)
                fixture.write_text(json.dumps({'sessions': [session]}))
                bindir = case_dir/'bin'; bindir.mkdir(); calls = case_dir/'calls.jsonl'; blocked = case_dir/'blocked'
                wrapper = bindir/'git'
                wrapper.write_text('''#!/usr/bin/env python3
import json,os,signal,sys
with open(os.environ['ACCEPT_CALLS'],'a') as f: f.write(json.dumps(sys.argv[1:])+'\\n')
if os.environ.get('ACCEPT_BLOCK') == '1' and 'log' in sys.argv and '--max-count=256' in sys.argv:
    with open(os.environ['ACCEPT_PENDING'],'w') as f: f.write(str(os.getpid()))
    while True: signal.pause()
os.execv(os.environ['ACCEPT_GIT'], [os.environ['ACCEPT_GIT'], *sys.argv[1:]])
''')
                wrapper.chmod(0o755)
                test_env = {**env, 'PATH': str(bindir)+os.pathsep+env['PATH'], 'ACCEPT_GIT': real_git,
                            'ACCEPT_CALLS': str(calls), 'ACCEPT_PENDING': str(blocked),
                            'ACCEPT_BLOCK': '1' if case == 'cancel-expansion' else '0'}
                start = time.monotonic()
                term = Terminal([binary, 'browse', '--fixture', str(fixture), str(selected)], test_env)
                def pause(seconds):
                    end = time.monotonic()+seconds
                    term.wait(lambda: time.monotonic() >= end, timeout=seconds+5)
                def selected_commit(oid):
                    term.text()
                    for row in range(80):
                        line = ''.join(term.cells.get((row, col), ' ') for col in range(240))
                        if '> ' in line and oid in line: return True
                    return False
                def commands(): return [json.loads(line) for line in calls.read_text().splitlines()] if calls.exists() else []
                def limits(): return [int(arg.split('=')[1]) for command in commands() if 'log' in command for arg in command if arg.startswith('--max-count=')]
                result = {'case': case, 'commits': counts[project]}
                try:
                    term.expect(marker); term.expect('following HEAD')
                    result['initial_display_ms'] = round(1000*(time.monotonic()-start), 2)
                    pause(1)
                    assert limits() and limits()[0] == 128, limits()
                    if case != 'old-head': assert max(limits()) == 128, limits()
                    else:
                        assert max(limits()) >= 25000, limits()
                        term.expect('[HEAD] [old]')
                    if case != 'old-head':
                        term.send(b'\t'); start = time.monotonic(); term.send(b'\x1b[F')
                        if case == 'cancel-expansion':
                            term.wait(blocked.exists)
                            pid = int(blocked.read_text()); term.finish()
                            try: os.kill(pid, 0)
                            except ProcessLookupError: pass
                            else: raise AssertionError('Expansion Git process survived exit')
                            result['cancelled_and_reaped'] = True
                        else:
                            term.wait(lambda: 256 in limits()); term.wait(lambda: 'loading more' not in term.text())
                            result['expansion_ms'] = round(1000*(time.monotonic()-start), 2)
                            # Current native traversal selects commit 128 on End and preserves it on replacement.
                            commits = git(project, 'log', '--format=%H', '--all', '--topo-order', '--max-count=256').splitlines()
                            term.wait(lambda: selected_commit(commits[127][:7]))
                            term.send(b'\x1b[B'); term.wait(lambda: selected_commit(commits[128][:7]))
                            term.send(b'\x1b[H\x1b[C'); term.expect('f follow HEAD')
                            term.send(b'f'); term.expect('following HEAD'); term.expect(marker)
                    if case != 'cancel-expansion':
                        pause(1)
                        def cpu():
                            fields = Path(f'/proc/{term.child.pid}/stat').read_text().split()
                            return int(fields[13])+int(fields[14]), int(fields[15])+int(fields[16])
                        first_cpu = cpu(); first_calls = len(commands()); first_bytes = len(term.output); start = time.monotonic()
                        pause(args.seconds); elapsed = time.monotonic()-start; last_cpu = cpu()
                        result.update(idle_seconds=round(elapsed, 2), idle_git_commands=len(commands())-first_calls,
                                      idle_output_bytes=len(term.output)-first_bytes,
                                      parent_cpu_percent=round(100*(last_cpu[0]-first_cpu[0])/os.sysconf('SC_CLK_TCK')/elapsed, 3),
                                      child_cpu_percent=round(100*(last_cpu[1]-first_cpu[1])/os.sysconf('SC_CLK_TCK')/elapsed, 3))
                        assert result['idle_git_commands'] == 0, result
                        assert result['idle_output_bytes'] == 0, result
                        status = Path(f'/proc/{term.child.pid}/status').read_text()
                        result['peak_rss_kib'] = int(re.search(r'VmHWM:\s+(\d+)', status)[1])
                        term.finish()
                    result['history_limits'] = limits()
                    result['git_commands'] = dict(Counter(next((arg for arg in command if arg in ('log', 'worktree', 'rev-parse', 'show-ref', 'symbolic-ref')), 'other') for command in commands()))
                    result['passed'] = True
                    report['cases'].append(result)
                    print(json.dumps(result), flush=True)
                finally:
                    term.close()
                    # Defensive cleanup if an assertion failed before the normal reaping check.
                    if blocked.exists():
                        try: os.kill(int(blocked.read_text()), signal.SIGKILL)
                        except ProcessLookupError: pass
            report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
        raise
    finally:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2)+'\n')

if __name__ == '__main__': main()
