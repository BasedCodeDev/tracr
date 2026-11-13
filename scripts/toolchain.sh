# Shared by dev.sh and check.sh; caller sets project_root and enters it.
if [[ -d "${CARGO_HOME:-${HOME:-}/.cargo}/bin" ]]; then
    export PATH="${CARGO_HOME:-${HOME:-}/.cargo}/bin:$PATH"
fi

cargo_command=(cargo)
if command -v rustup >/dev/null 2>&1; then
    pinned="$(sed -n 's/^channel = "\([^"]*\)"$/\1/p' rust-toolchain.toml)"
    chosen="${TRACR_TOOLCHAIN:-}"
    if [[ -z "$chosen" ]]; then
        # Check installed toolchains without downloading a pinned alias.
        installed="$(cd / && rustup toolchain list)"
        while read -r name _; do
            if [[ "$name" == "$pinned" || "$name" == "$pinned-"* ]]; then
                chosen="$name"
                break
            fi
        done <<< "$installed"
        if [[ -z "$chosen" ]]; then
            while read -r name _; do
                if [[ "$name" == stable || "$name" == stable-* ]]; then
                    chosen="$name"
                    break
                fi
            done <<< "$installed"
        fi
    fi
    if [[ -z "$chosen" ]]; then
        echo "No suitable Rust toolchain is installed. Install $pinned or set TRACR_TOOLCHAIN." >&2
        exit 1
    fi
    cargo_command=(rustup run "$chosen" cargo)
    echo "Using Rust toolchain: $chosen" >&2
elif ! command -v cargo >/dev/null 2>&1; then
    echo 'Cargo was not found. Install Rust, then run this script again.' >&2
    exit 1
elif [[ -n "${TRACR_TOOLCHAIN:-}" ]]; then
    echo 'TRACR_TOOLCHAIN requires rustup, which was not found.' >&2
    exit 1
fi
