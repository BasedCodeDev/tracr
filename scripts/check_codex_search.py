#!/usr/bin/env python3
"""Read-only search acceptance against disposable synthetic Codex histories."""
from diagnostics import run_cli, scratch_dir
import argparse
from pathlib import Path
import subprocess
import probe_codex_relocation as probe


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--keep-temp', action='store_true')
    parser.add_argument('--codex', required=True)
    parser.add_argument('--test-binary', required=True)
    args = parser.parse_args()
    binary = str(Path(args.codex).resolve(strict=True))
    test_binary = str(Path(args.test_binary).resolve(strict=True))
    root = Path(scratch_dir(prefix='tracr-search-'))
    for mode in ('legacy', 'paginated'):
        fixture_root = root / mode
        fixture = probe.prepare_fixture(binary, fixture_root, mode)
        (fixture_root / 'search-id').write_text(fixture['baseline']['id'])
        env = fixture['env'] | {
            'TRACR_SEARCH_FIXTURE': str(fixture_root),
            'TRACR_SEARCH_CODEX': binary,
        }
        subprocess.run([test_binary, 'real_codex_search', '--ignored', '--exact'],
                       env=env, check=True, timeout=60)
        print(f'{mode}: passed', flush=True)


if __name__ == '__main__':
    raise SystemExit(run_cli(main))
