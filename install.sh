#!/bin/sh
# Install the current tracr preview from GitHub Releases. POSIX sh; no Rust needed.
set -eu

main() {
    version=${TRACR_VERSION:-v0.1.0}
    install_dir=${TRACR_INSTALL_DIR:-${HOME:?HOME must be set}/.local/bin}
    data_dir=${XDG_DATA_HOME:-${HOME:?HOME must be set}/.local/share}/tracr
    case "$install_dir" in /*) ;; *) fail 'TRACR_INSTALL_DIR must be an absolute path' ;; esac
    case "$data_dir" in /*) ;; *) fail 'XDG_DATA_HOME must be an absolute path' ;; esac
    printf '%s\n' "$version" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || fail 'TRACR_VERSION must look like v0.1.0'
    [ "$#" -eq 0 ] || fail 'Use TRACR_VERSION and TRACR_INSTALL_DIR to configure installation; no arguments are accepted'
    [ "$(uname -s)" = Linux ] || fail 'tracr currently supports Linux only; macOS and Windows are planned'
    case "$(uname -m)" in
        x86_64|amd64) asset=tracr-linux-x86_64 ;;
        aarch64|arm64) asset=tracr-linux-aarch64 ;;
        *) fail 'Unsupported architecture; tracr supports x86_64 and ARM64' ;;
    esac
    for command in curl sha256sum awk mktemp chmod mkdir mv cp; do
        command -v "$command" >/dev/null 2>&1 || fail "Required command is missing: $command"
    done
    [ ! -d "$install_dir/tracr" ] || fail "$install_dir/tracr is a directory"
    umask 077
    temp=$(mktemp -d)
    staged=''
    trap 'rm -rf "$temp"; if [ -n "$staged" ]; then rm -f "$staged"; fi' 0
    trap 'exit 1' 1 2 15
    base="https://github.com/BasedCodeDev/tracr/releases/download/$version"
    printf 'Installing tracr %s (%s)\n' "$version" "$asset"
    download "$base/SHA256SUMS" "$temp/SHA256SUMS" || fail "Release $version is unavailable or could not be downloaded. Check https://github.com/BasedCodeDev/tracr/releases; drafts are not publicly downloadable."
    for file in "$asset" LICENSE THIRD_PARTY_NOTICES.txt; do
        download "$base/$file" "$temp/$file" || fail "Could not download $file; existing installation was not changed"
        expected=$(awk -v name="$file" '$2 == name { count++; hash=$1 } END { if (count != 1) exit 1; print hash }' "$temp/SHA256SUMS") || fail "Missing or duplicate checksum for $file"
        [ "${#expected}" -eq 64 ] || fail "Malformed checksum for $file"
        case "$expected" in *[!0-9a-f]*) fail "Malformed checksum for $file" ;; esac
        actual=$(sha256sum < "$temp/$file")
        actual=${actual%% *}
        [ "$actual" = "$expected" ] || fail "Checksum mismatch for $file; existing installation was not changed"
    done
    chmod 755 "$temp/$asset"
    actual_version=$("$temp/$asset" --version) || fail 'Downloaded binary cannot run on this machine; existing installation was not changed'
    [ "$actual_version" = "tracr ${version#v}" ] || fail 'Downloaded binary version does not match the release'
    mkdir -p "$install_dir" "$data_dir"
    cp "$temp/LICENSE" "$temp/THIRD_PARTY_NOTICES.txt" "$data_dir/"
    chmod 644 "$data_dir/LICENSE" "$data_dir/THIRD_PARTY_NOTICES.txt"
    # Stage beside the destination, then rename: upgrades also work while tracr runs.
    staged=$(mktemp "$install_dir/.tracr-install.XXXXXX")
    cp "$temp/$asset" "$staged"
    chmod 755 "$staged"
    mv -f "$staged" "$install_dir/tracr"
    staged=''
    printf 'Installed %s to %s/tracr\n' "$actual_version" "$install_dir"
    case ":$PATH:" in
        *":$install_dir:"*) printf 'Run: tracr .\n' ;;
        *) printf '%s\n' "Add $install_dir to your PATH, then run: tracr ." ;;
    esac
    printf 'License notices: %s\n' "$data_dir"
}

download() {
    curl --proto '=https' --proto-redir '=https' -fsSL --retry 3 \
        --connect-timeout 10 --max-time 120 "$1" -o "$2"
}
fail() { printf 'tracr installer: %s\n' "$*" >&2; exit 1; }

# Keep execution at the end so piping a partial download cannot start installation.
main "$@"
