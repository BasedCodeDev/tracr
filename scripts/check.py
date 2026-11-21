"""Stage runner: failures never conceal independent checks or reuse stale builds."""
from diagnostics import run_cli, scratch_dir
import argparse
import json
import os
from pathlib import Path
import subprocess

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--keep-temp', action='store_true', help='Retain full diagnostic scratch instead of cleaning it')
    parser.add_argument('--offline', action='store_true')
    parser.add_argument('--codex')
    parser.add_argument('--cargo', nargs=argparse.REMAINDER, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    os.chdir(root)
    logs = Path(scratch_dir(prefix='tracr-check-'))
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
    tested = stage('rust-tests', cargo + ['test', *locked])
    stage('python-tests', ['python3', '-m', 'unittest', 'discover', '-s', 'tests', '-v'])
    stage('startup_trace_check', ['python3', 'tests/fixtures/app-server/startup_trace_check.py', str(root / 'target/debug/tracr')], built)
    for helper in ['reminder_check', 'drawer_check', 'terminal_check', 'dev_reload_check', 'git_lifecycle_check', 'git_watch_check', 'stash_history_check', 'git_sessions_check', 'idle_check', 'folder_check', 'resume_check', 'inline_check', 'scroll_check', 'focus_check', 'edge_check', 'browse_check', 'create_check', 'menu_check', 'status_check', 'relink_check', 'folder_move_check', 'release_check', 'terminal_recovery_check', 'info_check', 'move_readiness_check', 'worktree_check', 'worktree_create_check', 'worktree_status_check', 'worktree_overlay_check', 'worktree_discard_check']:
        command = ['python3', f'tests/fixtures/app-server/{helper}.py', str(root / 'target/debug/tracr')]
        if helper == 'terminal_check':
            command.append(str(root / 'tests/fixtures/sessions/catalog.json'))
        stage(helper, command, built)
    if args.codex:
        stage('real-discard', ['python3', 'tests/fixtures/app-server/worktree_discard_check.py', str(root / 'target/debug/tracr'), args.codex], built)
        stage('real-codex', ['python3', 'tests/fixtures/app-server/real_check.py', str(root / 'target/debug/tracr'), args.codex], built)
        stage('real-status', ['python3', 'tests/fixtures/app-server/real_status_check.py', str(root / 'target/debug/tracr'), args.codex], built)
        stage('real-preview', ['python3', 'tests/fixtures/app-server/preview_check.py', str(root / 'target/debug/tracr'), args.codex], built)
        stage('real-shared', ['python3', 'tests/fixtures/app-server/shared_check.py', str(root / 'target/debug/tracr'), args.codex], built)
        # Ask Cargo for the exact acceptance executable instead of guessing a hash or
        # accidentally selecting an old test artifact from another toolchain.
        relink_test = None
        if built and tested:
            artifact = subprocess.run(cargo + ['test', *locked, '--test', 'relocation', '--no-run', '--message-format=json'],
                                      capture_output=True, text=True, timeout=120)
            (logs / 'real-relink-build.log').write_text(artifact.stdout + artifact.stderr)
            if artifact.returncode == 0:
                for line in artifact.stdout.splitlines():
                    try: message = json.loads(line)
                    except ValueError: continue
                    if message.get('reason') == 'compiler-artifact' and message.get('target', {}).get('name') == 'relocation':
                        relink_test = message.get('executable')
        stage('real-relink', ['python3', 'scripts/check_codex_relink.py', '--codex', args.codex,
                             '--test-binary', relink_test or ''], bool(relink_test))
    if args.codex and built and tested:
        artifact = subprocess.run(cargo + ['test', *locked, '--test', 'folder_move', '--no-run', '--message-format=json'],
                                  capture_output=True, text=True, timeout=120)
        (logs / 'real-folder-build.log').write_text(artifact.stdout + artifact.stderr)
        folder_test = None
        if artifact.returncode == 0:
            for line in artifact.stdout.splitlines():
                try: message = json.loads(line)
                except ValueError: continue
                if message.get('reason') == 'compiler-artifact' and message.get('target', {}).get('name') == 'folder_move':
                    folder_test = message.get('executable')
        for scenario in ['normal', 'locked', 'interrupted']:
            stage('real-folder-' + scenario, ['python3', 'scripts/check_codex_folder_move.py', '--codex', args.codex,
                                            '--test-binary', folder_test or '', '--scenario', scenario], bool(folder_test))
    if args.codex and built and tested:
        artifact = subprocess.run(cargo + ['test', *locked, '--test', 'release', '--no-run', '--message-format=json'],
                                  capture_output=True, text=True, timeout=120)
        (logs / 'real-release-build.log').write_text(artifact.stdout + artifact.stderr)
        release_test = None
        if artifact.returncode == 0:
            for line in artifact.stdout.splitlines():
                try: message = json.loads(line)
                except ValueError: continue
                if message.get('reason') == 'compiler-artifact' and message.get('target', {}).get('name') == 'release':
                    release_test = message.get('executable')
        stage('real-release', ['python3', 'scripts/check_codex_release.py', '--codex', args.codex,
                               '--test-binary', release_test or ''], bool(release_test))
    if args.codex and built and tested:
        artifact = subprocess.run(cargo + ['test', *locked, '--test', 'search', '--no-run', '--message-format=json'],
                                  capture_output=True, text=True, timeout=120)
        (logs / 'real-search-build.log').write_text(artifact.stdout + artifact.stderr)
        search_test = None
        if artifact.returncode == 0:
            for line in artifact.stdout.splitlines():
                try: message = json.loads(line)
                except ValueError: continue
                if message.get('reason') == 'compiler-artifact' and message.get('target', {}).get('name') == 'search':
                    search_test = message.get('executable')
        stage('real-search', ['python3', 'scripts/check_codex_search.py', '--codex', args.codex,
                              '--test-binary', search_test or ''], bool(search_test))
    (logs / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
    print(f'Scratch logs and results: {logs}')
    raise SystemExit(int(any(r['status'] != 'passed' for r in results)))


if __name__ == '__main__':
    raise SystemExit(run_cli(main))
