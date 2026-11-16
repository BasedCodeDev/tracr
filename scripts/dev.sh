#!/usr/bin/env bash
# Build tracr and launch live browsing, or forward explicit tracr arguments.
set -euo pipefail

caller_dir="$PWD"
project_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    cat <<'USAGE'
Usage: ./scripts/dev.sh [--once] [--] [tracr arguments...]

No arguments: build and browse your live Codex sessions, rebuilding on source edits.
Use --once to build and run without watching. Watch mode requires Python 3.
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

once=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --once) once=true; shift ;;
        --) shift; break ;;
        *) break ;;
    esac
done

cd -- "$project_root"
source "$project_root/scripts/toolchain.sh"

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

if [[ "$once" == false && -t 0 && -t 1 ]]; then
    command -v python3 >/dev/null 2>&1 || { echo 'Watch mode requires Python 3; use --once to disable watching.' >&2; exit 1; }
    exec python3 "$project_root/scripts/dev_watch.py" "$project_root" "$caller_dir" "${cargo_command[@]}" -- "$@"
fi

echo 'Building and running tracr…' >&2
# Build in the project, then preserve the caller's directory for all app paths.
"${cargo_command[@]}" build --locked --target-dir "$project_root/target"
cd -- "$caller_dir"
exec "$project_root/target/debug/tracr" "$@"
