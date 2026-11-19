"""Reproducible Linux idle CPU probe. No timing thresholds: results are evidence.
Example: python3 cpu_probe.py target/debug/tracr --seconds 30 --sessions 1000
"""
import argparse
from collections import Counter
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
from pty_support import Terminal

parser = argparse.ArgumentParser()
parser.add_argument('binary')
parser.add_argument('--seconds', type=float, default=30)
parser.add_argument('--sessions', type=int, default=1000)
parser.add_argument('--cases', nargs='+', default=['plain', 'git', 'hidden', 'old-head'])
args = parser.parse_args()
binary = str(Path(args.binary).resolve())
real_git = shutil.which('git')
base = json.loads(Path(__file__).parents[1].joinpath('sessions/catalog.json').read_text())['sessions'][0]
for case in args.cases:
    with tempfile.TemporaryDirectory(prefix='tracr-cpu-probe-') as directory:
        root = Path(directory)
        project = root / 'project'
        project.mkdir()
        env = {**os.environ, 'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1'}
        def git(*command, **kwargs):
            return subprocess.run([real_git, '-C', str(project), *command], env=env, check=True,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, **kwargs)
        selected = project
        if case != 'plain':
            git('init', '-qb', 'main')
            stream = ''.join(f'commit refs/heads/main\ncommitter Test <test@example.invalid> {1700000000+i} +0000\ndata {len(str(i))+7}\nCommit {i}\n\n' for i in range(301))
            git('fast-import', '--quiet', input=stream.encode())
            if case == 'old-head':
                selected = root / 'linked'
                git('worktree', 'add', '-qb', 'old', str(selected), 'HEAD~200')
        records = []
        for i in range(args.sessions):
            row = json.loads(json.dumps(base))
            row['key']['native_id'] = str(i)
            row['cwd'] = str(selected)
            row['archived'] = False
            records.append(row)
        fixture = root / 'catalog.json'
        fixture.write_text(json.dumps({'page_size': max(1, args.sessions), 'sessions': records}))
        bindir = root / 'bin'
        bindir.mkdir()
        log = root / 'git-calls'
        wrapper = bindir / 'git'
        wrapper.write_text('''#!/usr/bin/env python3
import os,sys
with open(os.environ['PROBE_LOG'],'a') as log:
    log.write(next((arg for arg in sys.argv if arg in ('log','show-ref','rev-parse','symbolic-ref','worktree','for-each-ref')), 'other')+'\\n')
os.execv(os.environ['PROBE_GIT'], [os.environ['PROBE_GIT'], *sys.argv[1:]])
''')
        wrapper.chmod(0o755)
        env.update(PATH=str(bindir)+os.pathsep+env['PATH'], PROBE_LOG=str(log), PROBE_GIT=real_git)
        term = Terminal([binary, 'browse', '--fixture', str(fixture), str(selected)], env)
        def pause(seconds):
            end = time.monotonic()+seconds
            term.wait(lambda: time.monotonic() >= end, timeout=seconds+5)
        def cpu():
            fields = Path(f'/proc/{term.child.pid}/stat').read_text().split()
            return (int(fields[13])+int(fields[14]), int(fields[15])+int(fields[16]))
        def calls():
            return log.read_text().splitlines() if log.exists() else []
        try:
            term.expect('Build tracr')
            if case != 'plain':
                term.send(b'\x1b`')
                term.expect('[HEAD]')
            if case == 'hidden':
                term.send(b'\x1b`')
                term.expect('Enter Open')
            pause(3)
            start_calls = len(calls())
            before = cpu()
            before_bytes = len(term.output)
            start = time.monotonic()
            pause(args.seconds)
            elapsed = time.monotonic()-start
            after = cpu()
            hz = os.sysconf('SC_CLK_TCK')
            output_bytes = len(term.output)-before_bytes
            idle_calls = dict(Counter(calls()[start_calls:]))
            mark = len(term.output)
            response_start = time.monotonic()
            term.send(b'\x1b`')
            term.wait(lambda: len(term.output) > mark)
            response_ms = round(1000*(time.monotonic()-response_start), 2)
            print(json.dumps(dict(binary=binary, case=case, sessions=args.sessions, seconds=round(elapsed,2),
                parent_cpu_percent=round(100*(after[0]-before[0])/hz/elapsed,3),
                child_cpu_percent=round(100*(after[1]-before[1])/hz/elapsed,3),
                idle_git_commands=idle_calls,
                terminal_output_bytes=output_bytes, input_response_ms=response_ms)), flush=True)
            term.finish()
        finally:
            term.close()
