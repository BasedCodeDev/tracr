"""Opt-in live acceptance. No prompt submission, real homes, or launch overrides."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import termios
import time
from pty_support import Terminal

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location('probe', ROOT/'scripts/probe_codex_relocation.py')
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)

class Blocked(RuntimeError):
    pass

def exit_code(results):
    if any(r['status'] == 'failed' for r in results):
        return 1
    return 2 if not results or any(r['status'] == 'blocked' for r in results) else 0

def failure(error):
    return 'blocked' if isinstance(error, Blocked) else 'failed'

def startup_blocker(text):
    lower = text.lower()
    if 'operation not permitted' in lower and ('socket' in lower or 'app-server' in lower):
        return 'Codex background-server socket is prohibited by this environment'
    for marker in ('sign in with chatgpt', 'sign in with your api key', 'do you trust', 'trust this folder'):
        if marker in lower:
            return 'Codex requires interactive authentication/trust: ' + marker
    return None

def screen_since(terminal, offset):
    screen = Terminal.__new__(Terminal)
    screen.output = terminal.output[offset:]
    return screen.text()

def history_visible(text, expected_messages):
    normalize = lambda value: ' '.join(value.split())
    return bool(expected_messages) and all(normalize(message) in normalize(text) for message in expected_messages)


def ready(text, tid, cwd):
    return (tid in text and cwd in text and 'Loaded' in text
            and 'Fetching metadata' not in text and 'Loading' not in text
            and 'Resume: Enter' in text)

def verify_inventory(inventory, tid, cwd):
    assert inventory['complete'], 'inventory incomplete'
    sessions = inventory['sessions']
    assert len(sessions) == 1 and sessions[0]['key']['native_id'] == tid, 'wrong session inventory'
    assert sessions[0]['cwd'] == cwd, 'wrong inventory directory'
    assert sessions[0]['archived'] is False, 'unexpected archive state'

def verify_status(text, tid, cwd):
    session = re.search(r'(?:session(?: id)?|chat(?: id)?)\s*:\s*([\w-]+)', text, re.I)
    directory = re.search(r'(?:directory|cwd)\s*:\s*(\S+)', text, re.I)
    assert session is not None, 'Codex status lacks session identity evidence'
    assert directory is not None, 'Codex status lacks directory evidence'
    assert session[1] == tid, 'Codex resumed the wrong session'
    assert directory[1] == cwd, 'Codex resumed in the wrong directory'

def owned_daemons(home):
    """Only same-user app-server processes with the exact unique CODEX_HOME."""
    found = []
    expected = b'CODEX_HOME=' + os.fsencode(home)
    for process in Path('/proc').glob('[0-9]*'):
        try:
            if process.stat().st_uid != os.getuid():
                continue
            args = (process/'cmdline').read_bytes().split(b'\0')
            if b'app-server' in args and expected in (process/'environ').read_bytes().split(b'\0'):
                found.append(int(process.name))
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            continue
    return found

def submit_status(terminal):
    # Codex treats burst input as paste; acknowledge each composer edit before
    # sending Enter so the submission cannot be swallowed by paste handling.
    prefix = ''
    for character in '/status':
        prefix += character
        mark = len(terminal.output)
        terminal.send(character.encode())
        terminal.wait(lambda: len(terminal.output) > mark and '› ' + prefix in terminal.text())
    terminal.send(b'\r')


def return_to_browser(terminal):
    # Ctrl-C can clear residual composer input before it arms Codex's exit
    # confirmation. Require a fresh, recognized redraw between interrupts.
    start = len(terminal.output)
    returned = lambda: b'\x1b[1;1Htracr' in terminal.output[start:]
    cleared = False
    for _ in range(2):
        mark = len(terminal.output)
        terminal.send(b'\x03')
        def acknowledged():
            text = screen_since(terminal, mark).lower()
            return (returned() or ('again' in text and 'ctrl' in text)
                    or (not cleared and 'ask codex to do anything' in text))
        terminal.wait(acknowledged)
        if returned():
            return
        text = screen_since(terminal, mark).lower()
        if 'again' in text and 'ctrl' in text:
            terminal.send(b'\x03')
            terminal.wait(returned)
            return
        cleared = True
    raise AssertionError('Codex did not return to tracr after acknowledged interrupts')


def stop_owned_updater(home):
    """The daemon stop command leaves its updater behind. Signal only the
    updater whose recorded boot/start identity and isolated home still match."""
    record_path = home/'codex/app-server-daemon/daemon-updater.pid'
    if not record_path.exists():
        return None
    record = json.loads(record_path.read_text())
    pid = record['pid']
    if pid not in owned_daemons(home/'codex'):
        return None
    # pidfd pins the process, preventing a reused PID from being signalled.
    try:
        fd = os.pidfd_open(pid)
    except ProcessLookupError:
        return None
    try:
        process = Path('/proc')/str(pid)
        identity = record['processIdentity']
        fields = (process/'stat').read_text().rsplit(')', 1)[1].split()
        assert int(fields[19]) == identity['startTicks'], 'updater start identity changed'
        assert Path('/proc/sys/kernel/random/boot_id').read_text().strip() == identity['bootId'], 'updater boot identity changed'
        assert pid in owned_daemons(home/'codex'), 'updater ownership changed'
        signal.pidfd_send_signal(fd, signal.SIGTERM)
        import select
        if not select.select([fd], [], [], 5)[0]:
            signal.pidfd_send_signal(fd, signal.SIGKILL)
            assert select.select([fd], [], [], 5)[0], 'owned updater survived termination'
        return pid
    except (FileNotFoundError, ProcessLookupError):
        return None
    finally:
        os.close(fd)


def stop_daemons(codex, env, home):
    pids = owned_daemons(home/'codex')
    if not pids:
        return {'observed_pids': []}
    stop = subprocess.run([codex, 'app-server', 'daemon', 'stop'], env=env, cwd=home,
                          capture_output=True, text=True, timeout=15)
    (home/'daemon-stop.log').write_text(stop.stdout + stop.stderr)
    assert stop.returncode == 0, 'supported daemon shutdown failed'
    updater = stop_owned_updater(home)
    started = time.monotonic()
    deadline = started + 30
    while True:
        remaining = owned_daemons(home/'codex')
        (home/'daemon-cleanup.json').write_text(json.dumps({'observed_pids': pids, 'remaining_pids': remaining, 'elapsed_seconds': time.monotonic()-started}, indent=2))
        if not remaining:
            break
        assert time.monotonic() < deadline, f'test-home daemon survived 30-second shutdown wait: {remaining}'
        time.sleep(.1)
    return {'observed_pids': pids, 'updater_terminated': updater, 'shutdown_seconds': time.monotonic()-started}

def run_case(binary, codex, home, mode):
    result = {'mode': mode, 'status': 'failed', 'phases': [], 'version': None,
              'executable_sha256': None, 'session_id': None, 'expected_directory': str(home/'old')}
    terminal = None
    env = None
    phase = 'fixture'
    def passed(name, **details):
        result['phases'].append(dict(phase=name, status='passed', **details))
    try:
        fixture = probe.prepare_fixture(codex, home, mode)
        env = fixture['env']
        # Use only system tools; never inherit caller credentials or config.
        env['PATH'] = '/usr/bin:/bin'
        tid, cwd = fixture['baseline']['id'], str(home/'old')
        result.update(session_id=tid, executable_sha256=hashlib.sha256(Path(codex).read_bytes()).hexdigest())
        result['version'] = subprocess.check_output([codex, '--version'], env=env, cwd=home, text=True, timeout=10).strip()
        passed(phase)
        phase = 'inventory'
        cmd = [binary, '--codex-home', str(home/'codex'), '--codex-bin', codex]
        listing = subprocess.run(cmd+['list','--json'], env=env, cwd=home, capture_output=True, text=True, timeout=60)
        (home/'inventory.json').write_text(listing.stdout)
        assert listing.returncode == 0, listing.stderr
        verify_inventory(json.loads(listing.stdout), tid, cwd)
        passed(phase)
        phase = 'browser-ready'
        terminal = Terminal(cmd+[cwd], env)
        terminal.wait(lambda: ready(terminal.text(), tid, cwd))
        passed(phase)
        phase = 'startup'
        mark = len(terminal.output)
        terminal.send(b'\r')
        terminal.wait(lambda: b'\x1b[?1049l' in terminal.output[mark:])
        handoff = terminal.output.index(b'\x1b[?1049l', mark) + len(b'\x1b[?1049l')
        def started():
            text = screen_since(terminal, handoff)
            raw = terminal.output[handoff:].decode(errors='replace')
            blocker = startup_blocker(raw)
            if blocker:
                raise Blocked(blocker)
            assert b'\x1b[1;1Htracr' not in terminal.output[handoff:], 'Codex exited before acceptance'
            return history_visible(text, fixture['expected_messages']) and os.tcgetpgrp(terminal.master) != terminal.child.pid
        terminal.wait(started, timeout=30)
        passed(phase, foreground_group=os.tcgetpgrp(terminal.master))
        phase = 'session-and-directory'
        mark = len(terminal.output)
        submit_status(terminal)
        terminal.wait(lambda: re.search(r'(?:session|chat)(?: id)?\s*:', screen_since(terminal, mark), re.I) is not None)
        status = screen_since(terminal, mark)
        (home/'status.txt').write_text(status)
        verify_status(status, tid, cwd)
        passed(phase)
        phase = 'return-to-browser'
        return_to_browser(terminal)
        terminal.wait(lambda: ready(terminal.text(), tid, cwd))
        assert os.tcgetpgrp(terminal.master) == terminal.child.pid, 'foreground not restored'
        assert 'including subfolders' in terminal.text(), 'folder filter not restored'
        # Source metadata must be fetched again, not merely retained in the screen.
        before_refresh = len(terminal.output)
        terminal.send(b'r')
        observed_loading = False
        def refreshed():
            nonlocal observed_loading
            text = terminal.text()
            observed_loading |= len(terminal.output) > before_refresh and 'Loading' in text
            return observed_loading and ready(text, tid, cwd)
        terminal.wait(refreshed)
        terminal.finish()
        passed(phase)
        result['status'] = 'passed'
    except (AssertionError, RuntimeError, OSError, ValueError, KeyError, probe.sqlite3.Error, subprocess.SubprocessError) as error:
        result['status'] = failure(error)
        result['phases'].append(dict(phase=phase, status=result['status'], reason=str(error)))
    finally:
        # Cleanup failure always dominates a blocked startup or a successful run.
        try:
            if terminal:
                terminal.close()
                assert terminal.child.returncode == 0, 'tracr did not shut down cleanly during cleanup'
                (home/'terminal.bin').write_bytes(terminal.output)
                (home/'terminal.txt').write_text(terminal.text())
            if env:
                passed('daemon-cleanup', **stop_daemons(codex, env, home))
        except (AssertionError, OSError, ValueError, subprocess.SubprocessError) as error:
            result['status'] = 'failed'
            result['phases'].append(dict(phase='cleanup', status='failed', reason=str(error)))
    return result

def main():
    binary, codex = map(lambda p: str(Path(p).resolve()), sys.argv[1:])
    evidence = Path(tempfile.mkdtemp(prefix='tracr-real-check-'))
    results = []
    for mode in ['legacy', 'paginated']:
        results.append(run_case(binary, codex, evidence/mode, mode))
        (evidence/'results.json').write_text(json.dumps(results, indent=2)+'\n')
    print(json.dumps(results, indent=2))
    print('Real Codex evidence:', evidence)
    return exit_code(results)

if __name__ == '__main__':
    raise SystemExit(main())
