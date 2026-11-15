"""Git-specific Linux PTY acceptance using isolated repositories and owned processes."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import termios
from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
real_git = shutil.which('git')
fake = Path(__file__).with_name('fake.py').resolve()


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def run_case(operation=None, action='walkthrough'):
    with tempfile.TemporaryDirectory(prefix='tracr-git-pty-') as directory:
        home = Path(directory)
        project = home / 'project'
        linked = home / 'listed'
        project.mkdir()
        env = {**os.environ, 'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1'}
        def git(*args):
            subprocess.run([real_git, '-C', str(project), *args], env=env,
                           check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        git('init', '-q')
        main_branch = subprocess.check_output([real_git, '-C', str(project), 'symbolic-ref', '--short', 'HEAD'], env=env).decode().strip()
        git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
            'commit', '--allow-empty', '-qm', 'Lifecycle root')
        if action in ('progressive', 'expansion-quit', 'follow'):
            for i in range(300):
                git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
                    'commit', '--allow-empty', '-qm', f'History {i:03}')
        if action == 'presentation':
            git('checkout', '-qb', 'side')
            git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '--allow-empty', '-qm', 'Side commit')
            git('checkout', '-q', main_branch)
            git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '--allow-empty', '-qm', 'Main commit')
            git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'merge', '--no-ff', '-qm', 'Presentation merge ' + '界' * 100, 'side')
            git('update-ref', 'refs/remotes/origin/main', 'HEAD')
            git('tag', 'light')
            git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'tag', '-a', 'release', '-m', 'Release')
        git('worktree', 'add', '-qb', 'feature', str(linked), 'HEAD~200' if action == 'follow' else 'HEAD')
        executable = home / 'fake'
        shutil.copyfile(fake, executable)
        executable.chmod(0o755)
        (home / 'fake.json').write_text(json.dumps({'read_cwd': str(linked)}))
        wrapper_dir = home / 'bin'
        wrapper_dir.mkdir()
        wrapper = wrapper_dir / 'git'
        wrapper.write_text('''#!/usr/bin/env python3
import json, os, signal, sys
from pathlib import Path
home = Path(os.environ['GIT_TEST_HOME'])
args = sys.argv[1:]
operation = 'history' if 'log' in args else 'inventory' if 'worktree' in args else 'directory'
if operation == os.environ.get('GIT_TEST_BLOCK') or (os.environ.get('GIT_TEST_BLOCK') == 'expansion' and '--max-count=256' in args):
    (home / ('pending-' + str(os.getpid()))).write_text(operation)
    while True: signal.pause()
os.execv(os.environ['GIT_TEST_REAL'], [os.environ['GIT_TEST_REAL'], *args])
''')
        wrapper.chmod(0o755)
        env.update(PATH=str(wrapper_dir) + os.pathsep + env['PATH'],
                   GIT_TEST_HOME=str(home), GIT_TEST_REAL=real_git,
                   GIT_TEST_BLOCK=operation or '')
        terminal = Terminal([binary, '--codex-home', str(home), '--codex-bin', str(executable), str(linked)], env)
        try:
            terminal.expect('Task first')
            if action == 'presentation':
                terminal.expect('[remote: origin/main]')
                terminal.expect('[tag: light] [tag: release]')
                terminal.expect('[HEAD] [feature]')
                terminal.send(b'\t\x1b[C')
                terminal.expect('f follow HEAD')
                terminal.send(b'r')
                terminal.wait(lambda: 'loading more' not in terminal.text())
                terminal.send(b'f')
                terminal.expect('following HEAD')
                terminal.expect('[HEAD] [feature]')
                terminal.finish()
                return
            if action == 'follow':
                terminal.expect('[HEAD] [feature] History 099')
                terminal.send(b'\t\x1b[A')
                oid = subprocess.check_output([real_git, '-C', str(project), 'rev-parse', 'HEAD~199'], env=env).decode().strip()[:7]
                terminal.expect('> * ' + oid)
                mark = len(terminal.output)
                terminal.send(b'r')
                terminal.wait(lambda: b'loading more' in terminal.output[mark:])
                terminal.wait(lambda: 'loading more' not in terminal.text())
                terminal.expect('> * ' + oid)
                terminal.send(b'f')
                terminal.expect('following HEAD')
                terminal.wait(lambda: '> * ' + subprocess.check_output([real_git, '-C', str(project), 'rev-parse', 'HEAD~200'], env=env).decode().strip()[:7] in terminal.text())
                terminal.send(b'\t\x1b[D\x1b[A')
                terminal.expect(f'[HEAD] [{main_branch}] History 299')
                terminal.send(b'\x1b[B')
                terminal.expect('[HEAD] [feature] History 099')
                terminal.finish()
                return
            if action in ('progressive', 'expansion-quit'):
                terminal.expect('History 299')
                terminal.send(b'\t\x1b[F')
                if action == 'expansion-quit':
                    terminal.wait(lambda: bool(list(home.glob('pending-*'))))
                else:
                    terminal.expect('History 172')
                    terminal.wait(lambda: 'loading more' not in terminal.text())
                    terminal.send(b'\x1b[B')
                    terminal.expect('History 171')
                    # Wait for the refreshed frame, then check selection by oid.
                    oid = subprocess.check_output([real_git, '-C', str(project), 'rev-parse', 'HEAD~128'], env=env).decode().strip()[:7]
                    mark = len(terminal.output)
                    terminal.send(b'r')
                    terminal.wait(lambda: b'loading more' in terminal.output[mark:])
                    terminal.wait(lambda: 'loading more' not in terminal.text())
                    terminal.expect('> * ' + oid)
                terminal.send(b'q')
                terminal.wait(lambda: terminal.child.poll() is not None)
                assert terminal.child.returncode == 0
                assert termios.tcgetattr(terminal.slave) == terminal.original
                terminal.wait(lambda: all(not alive(int(p.name.split('-')[1])) for p in home.glob('pending-*')))
                return
            if operation:
                terminal.wait(lambda: bool(list(home.glob('pending-*'))))
            else:
                terminal.expect('↳ feature')
                terminal.expect('Lifecycle root')
                terminal.send(b'\x1b[D\x1b[A')
                terminal.wait(lambda: '> ▾ project' in terminal.text())
                terminal.send(b'\x1b[B\x1b[C')
                terminal.expect('Task first')
            pids = [int(p.name.split('-')[1]) for p in home.glob('pending-*')]
            if action in ('walkthrough', 'handoff'):
                terminal.send(b'\r')
                terminal.wait(lambda: (home / 'launch.json').exists())
                launch = json.loads((home / 'launch.json').read_text())
                assert launch['cwd'] == str(linked), launch
                assert launch['foreground'], launch
                assert all(not alive(pid) for pid in pids), "Git processes survived session handoff"
                if action == 'walkthrough':
                    # Preserve the selected folder while removing its Git registration.
                    (linked / '.git').unlink()
                    git('worktree', 'prune', '--expire', 'now')
                mark = len(terminal.output)
                terminal.send(b'x')
                terminal.wait(lambda: b'Sessions' in terminal.output[mark:])
                terminal.expect('Task first')
                if action == 'walkthrough':
                    terminal.wait(lambda: '↳ feature' not in terminal.text())
                else:
                    terminal.wait(lambda: len(list(home.glob('pending-*'))) > len(pids))
                terminal.send(b'q')
            elif action == 'sigterm':
                terminal.child.send_signal(signal.SIGTERM)
            else:
                terminal.send(b'q' if action == 'q' else b'\x03')
            terminal.wait(lambda: terminal.child.poll() is not None)
            assert terminal.child.returncode == 0
            assert termios.tcgetattr(terminal.slave) == terminal.original
            terminal.wait(lambda: all(not alive(int(p.name.split('-')[1])) for p in home.glob('pending-*')))
        finally:
            terminal.close()
            # Cleanup is defensive; assertions above must prove application cleanup.
            for record in home.glob('pending-*'):
                pid = int(record.name.split('-')[1])
                if alive(pid):
                    os.kill(pid, signal.SIGKILL)


run_case()
run_case(action='progressive')
run_case(action='follow')
run_case(action='presentation')
run_case(operation='expansion', action='expansion-quit')
for operation in ('directory', 'inventory', 'history'):
    for action in ('q', 'ctrl-c', 'sigterm', 'handoff'):
        run_case(operation, action)
print('Git lifecycle: navigation, return, removal, cancellation and terminal restoration passed')
