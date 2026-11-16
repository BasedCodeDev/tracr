#!/usr/bin/env python3
"""Development supervisor. The application opts into watching via a ready file."""
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

RELOAD_EXIT = 75


def snapshot(root):
    paths = list((root / 'src').rglob('*'))
    paths += [root / name for name in (
        'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'build.rs',
        '.cargo/config.toml', 'scripts/dev.sh', 'scripts/dev_watch.py',
        'scripts/toolchain.sh')]
    result = {}
    for path in paths:
        try:
            if path.is_file():
                stat = path.stat()
                result[str(path)] = (stat.st_mtime_ns, stat.st_size, stat.st_ino)
        except FileNotFoundError:
            pass  # An editor may replace a file during the scan.
    return result


def supervise(root, cwd, cargo, args):
    log_fd, log_name = tempfile.mkstemp(prefix='tracr-dev-', suffix='.log')
    os.close(log_fd)
    log_path = Path(log_name)
    print(f'Development build log: {log_path}', file=sys.stderr, flush=True)
    stopping = False

    def stop(_signal, _frame):
        nonlocal stopping
        stopping = True

    signal.signal(signal.SIGINT, stop)
    signal.signal(signal.SIGTERM, stop)
    # The supervisor remains in the terminal group, but only tracr reads input.
    child = build = log = None
    def resize(_signal, _frame):
        if child is not None and child.poll() is None:
            child.send_signal(signal.SIGWINCH)

    signal.signal(signal.SIGWINCH, resize)
    previous = snapshot(root)
    dirty_at = time.monotonic() - 1
    build_snapshot = None
    reload_pending = False
    with tempfile.TemporaryDirectory(prefix='tracr-dev-control-') as directory:
        control = Path(directory)
        env = {**os.environ, 'TRACR_DEV_CONTROL': directory}
        try:
            while not stopping:
                now = time.monotonic()
                current = snapshot(root)
                if current != previous:
                    previous = current
                    dirty_at = now
                if child is not None and child.poll() is not None:
                    code = child.returncode
                    child = None
                    if code != RELOAD_EXIT or not (control / 'ack').exists():
                        return code if code >= 0 else 128 - code
                    reload_pending = False
                    for name in ('ready', 'request', 'ack'):
                        (control / name).unlink(missing_ok=True)
                    # Acknowledgement means terminal cleanup has completed.
                    child = subprocess.Popen([str(root / 'target/debug/tracr'), *args], cwd=cwd, env=env)
                if build is not None and build.poll() is not None:
                    code = build.returncode
                    build = None
                    log.close()
                    log = None
                    if code == 0 and current == build_snapshot:
                        if child is None:
                            child = subprocess.Popen([str(root / 'target/debug/tracr'), *args], cwd=cwd, env=env)
                        else:
                            reload_pending = True
                    elif code != 0 and child is None:
                        print(log_path.read_text(), file=sys.stderr, end='')
                        print('Build failed; waiting for edits. Ctrl-C exits.', file=sys.stderr, flush=True)
                if reload_pending and (control / 'ready').exists():
                    (control / 'request').touch()
                if build is None and dirty_at is not None and now - dirty_at >= 0.3:
                    build_snapshot = current
                    dirty_at = None
                    log = log_path.open('w')
                    build = subprocess.Popen([*cargo, 'build', '--locked', '--target-dir', str(root / 'target')],
                                             cwd=root, stdin=subprocess.DEVNULL, stdout=log,
                                             stderr=subprocess.STDOUT, start_new_session=True)
                time.sleep(0.5)
            return 130
        finally:
            if build is not None and build.poll() is None:
                os.killpg(build.pid, signal.SIGTERM)
                try:
                    build.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(build.pid, signal.SIGKILL)
                    build.wait()
            if log is not None:
                log.close()
            if child is not None and child.poll() is None:
                child.terminate()
                child.wait()  # Let application operations finish their guarded cleanup.


def main():
    root, cwd = map(Path, sys.argv[1:3])
    separator = sys.argv.index('--', 3)
    return supervise(root, cwd, sys.argv[3:separator], sys.argv[separator + 1:])


if __name__ == '__main__':
    sys.exit(main())
