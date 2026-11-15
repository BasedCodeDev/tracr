"""Lifecycle and bounded evidence retention for command-line diagnostic runs."""
from contextvars import ContextVar
import fcntl
import json
import os
from pathlib import Path
import shutil
import signal
import sys
import tempfile
import time

_CURRENT = ContextVar('diagnostic_run', default=None)
DIAGNOSTICS = Path(__file__).resolve().parents[1] / 'target/diagnostics'


def scratch_dir(*, prefix='tracr-', dir=None):
    """Allocate scratch owned by the enclosing run; dir requests durable reports."""
    run = _CURRENT.get()
    if run is None:
        raise RuntimeError('scratch_dir requires run_cli')
    path = Path(tempfile.mkdtemp(prefix=prefix, dir=dir))
    run.paths.append((path, dir is not None))
    return str(path)


def on_cleanup(callback):
    """Register an idempotent child-process closer before fixture cleanup."""
    run = _CURRENT.get()
    if run is not None:
        run.closers.append(callback)


def busy_paths(root):
    """Refuse to remove scratch still referenced by a visible owned process."""
    for proc in Path('/proc').iterdir():
        if not proc.name.isdigit() or int(proc.name) == os.getpid():
            continue
        try:
            if proc.stat().st_uid != os.getuid():
                continue
            # Login-session managers predate and do not run fixture commands.
            if (proc / 'comm').read_text().strip() in {'systemd', '(sd-pam)'}:
                continue
            links = [proc / 'cwd', proc / 'exe', *list((proc / 'fd').iterdir())]
            for link in links:
                try:
                    target = Path(os.readlink(link))
                    if target.is_relative_to(root):
                        return True
                except (OSError, ValueError):
                    pass
        except FileNotFoundError:
            continue
        except PermissionError:
            # Do not claim safety when another owned process is opaque.
            return True
    return False


def copy_reports(source, destination):
    """Retain bounded reports/log tails, never fixture homes or RPC transcripts."""
    budget = 16 * 1024 * 1024
    for base, dirs, files in os.walk(source, followlinks=False):
        dirs[:] = sorted(d for d in dirs if d not in {'packages', 'sessions', 'archived_sessions', 'schema', '.git'}
                         and not (Path(base) / d).is_symlink())
        for name in sorted(files):
            path = Path(base) / name
            if path.is_symlink() or not (name in {'results.json', 'report.json', 'daemon-cleanup.json'} or path.suffix == '.log'):
                continue
            if budget <= 0:
                return
            limit = min(budget, 2 * 1024 * 1024)
            size = path.stat().st_size
            if path.suffix == '.json' and size > limit:
                continue  # Do not create invalid truncated JSON.
            with path.open('rb') as stream:
                if size > limit:
                    stream.seek(-limit, os.SEEK_END)
                data = stream.read(limit)
            output = destination / path.relative_to(source)
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(data)
            budget -= len(data)


class DiagnosticRun:
    def __init__(self, name, keep=False, diagnostics=DIAGNOSTICS):
        self.name = name
        self.keep = keep
        self.diagnostics = Path(diagnostics)
        self.paths = []
        self.closers = []

    def finish(self, status):
        cleanup_failed = False
        for closer in reversed(self.closers):
            try:
                closer()
            except BaseException as error:
                cleanup_failed = True
                print(f'Child cleanup failed: {error}', file=sys.stderr)
        for path, explicit in self.paths:
            if not path.exists():
                continue
            if self.keep or cleanup_failed or busy_paths(path):
                print(f'Retained full scratch (requested or still in use): {path}', flush=True)
                continue
            report_dir = None
            if explicit or status:
                parent = self.diagnostics / self.name
                parent.mkdir(parents=True, exist_ok=True)
                with (parent / '.lock').open('a') as lock:
                    fcntl.flock(lock, fcntl.LOCK_EX)
                    stage = Path(tempfile.mkdtemp(prefix='.pending-', dir=parent))
                    try:
                        copy_reports(path, stage)
                        (stage / 'run.json').write_text(json.dumps({'exit_code': status, 'scratch_removed': str(path),
                            'retention': 'reports and log tails only; 2 MiB/file, 16 MiB/run'}, indent=2) + '\n')
                        if explicit:
                            # The caller chose this output directory: retain its reports,
                            # but discard all generated homes and downloaded packages.
                            shutil.rmtree(path)
                            path.mkdir()
                            for item in stage.iterdir():
                                shutil.move(str(item), path / item.name)
                            report_dir = path
                        else:
                            report_dir = parent / f'{time.time_ns()}-{stage.name[9:]}'
                            stage.rename(report_dir)
                            completed = sorted((p for p in parent.iterdir() if p.is_dir() and not p.name.startswith('.')),
                                               key=lambda p: p.name, reverse=True)
                            for old in completed[3:]:
                                shutil.rmtree(old)
                    finally:
                        if stage.exists():
                            shutil.rmtree(stage)
            if not explicit:
                shutil.rmtree(path)
            print(f'Retained reports: {report_dir}' if report_dir else f'Removed successful scratch: {path}', flush=True)


def run_cli(main):
    """Wrap an entrypoint without changing its parser or exit-code contract."""
    keep = '--keep-temp' in sys.argv[1:] or os.environ.get('TRACR_KEEP_TEMP') == '1'
    sys.argv[:] = [sys.argv[0], *(arg for arg in sys.argv[1:] if arg != '--keep-temp')]
    previous_keep = os.environ.get('TRACR_KEEP_TEMP')
    if keep:
        os.environ['TRACR_KEEP_TEMP'] = '1'
    run = DiagnosticRun(Path(sys.argv[0]).stem, keep)
    token = _CURRENT.set(run)
    status = 1
    previous_term = signal.getsignal(signal.SIGTERM)

    def interrupted(signum, frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupted)
    try:
        result = main()
        status = result if isinstance(result, int) else 0
        return result
    except SystemExit as error:
        status = error.code if isinstance(error.code, int) else int(error.code is not None)
        raise
    except KeyboardInterrupt:
        status = 130
        raise
    finally:
        try:
            run.finish(status)
        finally:
            signal.signal(signal.SIGTERM, previous_term)
            _CURRENT.reset(token)
            if previous_keep is None:
                os.environ.pop('TRACR_KEEP_TEMP', None)
            else:
                os.environ['TRACR_KEEP_TEMP'] = previous_keep
