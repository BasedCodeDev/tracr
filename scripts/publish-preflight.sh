#!/usr/bin/env bash
# Read-only checks; run from the real writable checkout in a normal terminal.
set -euo pipefail
project_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd -- "$project_root"
command -v git >/dev/null
command -v gh >/dev/null
git_dir="$(git rev-parse --absolute-git-dir)" || {
    echo 'Git history is inaccessible here. Use the original writable checkout; do not replace .git.' >&2
    exit 1
}
[[ -w "$git_dir" ]] || { echo 'Git metadata is not writable in this environment.' >&2; exit 1; }
[[ "$(git config user.name)" == 'Sebastiaan Fehr' ]] || { echo 'Review repository Git user.name.' >&2; exit 1; }
[[ "$(git config user.email)" == 'seb@actuator.digital' ]] || { echo 'Review repository Git user.email.' >&2; exit 1; }
gh auth status --hostname github.com
[[ "$(gh api user --jq .login)" == 'BasedCodeDev' ]] || { echo 'Expected GitHub account BasedCodeDev.' >&2; exit 1; }
origin="$(git remote get-url origin)"
case "$origin" in
    https://github.com/BasedCodeDev/tracr|https://github.com/BasedCodeDev/tracr.git|git@github.com:BasedCodeDev/tracr.git) ;;
    *) echo 'Origin does not point to BasedCodeDev/tracr; inspect it before publishing.' >&2; exit 1 ;;
esac
gh repo view BasedCodeDev/tracr --json nameWithOwner,visibility,viewerPermission,defaultBranchRef
git ls-remote --exit-code origin HEAD
if git ls-files -- .aws .codex .agents target dist release-assets downloaded | grep -q .; then
    echo 'Local-only paths are tracked. Review and remove them from the index before publishing.' >&2
    exit 1
fi
echo 'Git access verified. Review all staged files before committing or pushing.'
