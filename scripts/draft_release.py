"""Publish validated assets to a draft only, never replace a published release."""
import json
from pathlib import Path
import re
import subprocess
import sys

from release import COMMON, TARGETS, validate_tag, verify_checksums


def publish(tag, folder):
    validate_tag(tag)
    names = [v[0] for v in TARGETS.values()] + list(COMMON)
    verify_checksums(folder, names)
    # Listing failure is fatal; it must not be mistaken for a missing release.
    releases = json.loads(subprocess.check_output(
        ['gh', 'api', '--paginate', '--slurp', 'repos/{owner}/{repo}/releases?per_page=100'], text=True))
    matches = [r for page in releases for r in page if r['tag_name'] == tag]
    if matches and not matches[0]['draft']:
        raise ValueError('refusing to overwrite a published release')
    if not matches:
        subprocess.run(['gh', 'release', 'create', tag, '--verify-tag', '--draft', '--prerelease',
                        '--title', f'tracr {tag} — browse and resume preview',
                        '--notes-file', str(Path(__file__).resolve().parents[1]/'docs/release-notes.md')], check=True)
    # Recheck immediately before uploading, including on reruns.
    state = json.loads(subprocess.check_output(['gh', 'release', 'view', tag, '--json', 'isDraft'], text=True))
    if not state['isDraft']:
        raise ValueError('release is no longer a draft')
    subprocess.run(['gh', 'release', 'upload', tag, *[str(folder/n) for n in names + ['SHA256SUMS']], '--clobber'], check=True)


def main():
    if len(sys.argv) != 3 or not re.fullmatch(r'v\d+\.\d+\.\d+', sys.argv[1]):
        raise SystemExit('Usage: draft_release.py vMAJOR.MINOR.PATCH ASSET_DIRECTORY')
    try:
        publish(sys.argv[1], Path(sys.argv[2]))
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        raise SystemExit(f'draft release: {error}')


if __name__ == '__main__':
    main()
