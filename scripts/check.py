"""Stage runner: failures never conceal independent checks or reuse stale builds."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument('--offline', action='store_true')
parser.add_argument('--codex')
parser.add_argument('--cargo', nargs=argparse.REMAINDER, required=True)
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
os.chdir(root)
logs = Path(tempfile.mkdtemp(prefix='tracr-check-'))
results = []

def stage(name, command, prerequisite=True, timeout=300):
    if not prerequisite:
        results.append(dict(stage=name, status='blocked', reason='build prerequisite failed'))
        return False
    try:
        result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                text=True, timeout=timeout)
        output = result.stdout
        status = 'passed' if result.returncode == 0 else 'failed'
        if (name == 'real-codex' and result.returncode == 2) or (name != 'real-codex' and any(s in output for s in ['failed to download', 'Could not resolve', 'attempting to make an HTTP request', 'no release found', 'is not installed'])):
            status = 'blocked'
    except (OSError, subprocess.TimeoutExpired) as error:
        output = str(error)
        status = 'blocked' if isinstance(error, OSError) else 'failed'
    (logs / (name + '.log')).write_text(output)
    results.append(dict(stage=name, status=status))
    print(f'{name}: {status}', flush=True)
    if status != 'passed':
        print(output[-3000:], flush=True)
    return status == 'passed'

cargo = args.cargo
locked = ['--locked'] + (['--offline'] if args.offline else [])
if not args.offline:
    stage('dependencies', cargo + ['fetch', '--locked'])
stage('format', cargo + ['fmt', '--check'])
built = stage('build', cargo + ['build', *locked, '--target-dir', str(root / 'target')])
stage('clippy', cargo + ['clippy', '--all-targets', *locked, '--', '-D', 'warnings'])
stage('rust-tests', cargo + ['test', *locked])
stage('python-tests', ['python3', '-m', 'unittest', 'discover', '-s', 'tests', '-v'])
for helper in ['terminal_check', 'folder_check', 'resume_check', 'edge_check', 'browse_check', 'create_check']:
    command = ['python3', f'tests/fixtures/app-server/{helper}.py', str(root / 'target/debug/tracr')]
    if helper == 'terminal_check':
        command.append(str(root / 'tests/fixtures/sessions/catalog.json'))
    stage(helper, command, built)
if args.codex:
    stage('real-codex', ['python3', 'tests/fixtures/app-server/real_check.py', str(root / 'target/debug/tracr'), args.codex], built)
(logs / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
print(f'Logs and results: {logs}')
raise SystemExit(int(any(r['status'] != 'passed' for r in results)))
