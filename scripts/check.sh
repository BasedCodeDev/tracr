#!/usr/bin/env bash
set -euo pipefail
project_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
args=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --offline) args+=(--offline); shift ;;
        --codex) [[ $# -ge 2 && "$2" = /* ]] || { echo '--codex requires an absolute executable path' >&2; exit 2; }; args+=(--codex "$2"); shift 2 ;;
        -h|--help) echo 'Usage: scripts/check.sh [--offline] [--codex /absolute/path/to/codex]'; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
done
cd -- "$project_root"
source "$project_root/scripts/toolchain.sh"
exec python3 "$project_root/scripts/check.py" "${args[@]}" --cargo "${cargo_command[@]}"
