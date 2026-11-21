"""Check startup trace attribution and progress against a gated app server."""
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
import time

from pty_support import Terminal

binary = str(Path(sys.argv[1]).resolve())
fake = Path(__file__).with_name('fake.py').resolve()
with tempfile.TemporaryDirectory(prefix='tracr-startup-trace-') as tmp:
    home = Path(tmp)
    folder = home / 'listed'
    folder.mkdir()
    (home / 'config.toml').write_text('')
    (home / 'fake.json').write_text(json.dumps({'startup_gates': True}))
    cmd = [binary, '--config', str(home / 'config.toml'), '--codex-home', tmp,
           '--codex-bin', str(fake), str(folder)]
    env = {key: value for key, value in os.environ.items() if key != 'TRACR_DEV_CONTROL'}
    terminal = Terminal(cmd, {**env, 'TRACR_STARTUP_TRACE_DIR': tmp}, start_in_sessions=False)

    def events():
        files = list(home.glob('tracr-startup-*.jsonl'))
        if not files:
            return []
        assert len(files) == 1
        assert stat.S_IMODE(files[0].stat().st_mode) == 0o600
        # A concurrent write may have started the last line.
        return [json.loads(line) for line in files[0].read_bytes().splitlines(keepends=True)
                if line.endswith(b'\n')]

    def find(stage, event):
        return next((item for item in events()
                     if item['stage'] == stage and item['event'] == event), None)

    try:
        terminal.expect('Loading sessions')
        terminal.wait(lambda: find('codex_initialize', 'start') is not None)
        assert find('codex_initialize', 'ok') is None
        assert find('ui_first_session_frame', 'mark') is None
        time.sleep(.15)
        (home / 'release-initialize').touch()
        terminal.expect('Task first')
        terminal.wait(lambda: find('codex_reconciled_list', 'start') is not None)
        assert find('codex_reconciled_list', 'ok') is None
        assert find('ui_discovery_complete_frame', 'mark') is None
        time.sleep(.20)
        (home / 'release-reconciliation').touch()
        terminal.wait(lambda: find('startup_settled', 'mark') is not None)
        initialize = find('codex_initialize', 'ok')
        reconciliation = find('codex_reconciled_list', 'ok')
        assert initialize['duration_us'] >= 150_000
        assert reconciliation['duration_us'] >= 200_000
        first_rows = find('ui_first_session_frame', 'mark')
        complete = find('ui_discovery_complete_frame', 'mark')
        assert initialize['elapsed_us'] < first_rows['elapsed_us'] < reconciliation['elapsed_us']
        assert find('source_discovery', 'ok')['elapsed_us'] <= complete['elapsed_us']
        assert complete['count'] == 0
        assert find('codex_reconcile_read', 'ok')['source'] == 0
        assert str(home) not in json.dumps(events())
        assert 'reconciled-only' not in json.dumps(events())
        terminal.finish()
    finally:
        terminal.close()

print('startup trace captures initialization, first rows, reconciliation and completion: passed')
