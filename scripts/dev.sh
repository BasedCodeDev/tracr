#!/usr/bin/env bash
# Build tracr and launch live browsing, or forward explicit tracr arguments.
set -euo pipefail

caller_dir="$PWD"
project_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    cat <<'USAGE'
Usage: ./scripts/dev.sh [tracr arguments...]

No arguments: build and browse your live Codex sessions.
  ./scripts/dev.sh                    Browse your live Codex sessions
  ./scripts/dev.sh .                  Browse this folder and its subfolders
  ./scripts/dev.sh browse --fixture tests/fixtures/sessions/catalog.json
  ./scripts/dev.sh list --json        Print your live inventory as JSON
  ./scripts/dev.sh list --fixture tests/fixtures/sessions/catalog.json --json

Run from any directory; application paths are relative to your calling directory.
Set TRACR_TOOLCHAIN to select an installed rustup toolchain explicitly.
Use ./scripts/dev.sh -- --help for tracr's own help.
USAGE
    exit 0
fi

cd -- "$project_root"
source "$project_root/scripts/toolchain.sh"

if [[ "${1:-}" == -- ]]; then
    shift
fi
if [[ $# -eq 0 ]]; then
    set -- browse
fi

if [[ ! -f Cargo.lock ]]; then
    echo 'Generating Cargo.lock (first run requires crates.io access)…' >&2
    if ! "${cargo_command[@]}" generate-lockfile; then
        echo 'Dependency resolution failed. Check crates.io connectivity and retry.' >&2
        exit 1
    fi
fi

echo 'Building and running tracr…' >&2
# Build in the project, then preserve the caller's directory for all app paths.
"${cargo_command[@]}" build --locked --target-dir "$project_root/target"
cd -- "$caller_dir"
exec "$project_root/target/debug/tracr" "$@"
