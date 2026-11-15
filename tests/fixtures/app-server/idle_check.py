"""Deterministic terminal-idle regression checks; CPU percentages are not thresholds."""
import json
from pathlib import Path
import subprocess
import sys
result = subprocess.run([sys.executable, str(Path(__file__).with_name('cpu_probe.py')), sys.argv[1],
                         '--seconds', '2', '--sessions', '50'], capture_output=True, text=True, timeout=60)
assert result.returncode == 0, result.stderr
rows = [json.loads(line) for line in result.stdout.splitlines()]
assert len(rows) == 4, rows
for row in rows:
    assert not row['idle_git_commands'], row
    assert row['terminal_output_bytes'] == 0, row
print('Idle plain/Git/hidden/old-HEAD views: no commands or redraw output; input wakes rendering')
