# tracr

tracr is a Linux terminal application for browsing Codex sessions by folder,
searching metadata globally, and resuming work in the same terminal. Browsing and
resume pass automated checks and isolated real-Codex acceptance with Codex 0.167.2.
Session relocation is not yet supported.

## Install

On Linux x86_64 or ARM64:

```sh
curl -fsSL https://raw.githubusercontent.com/BasedCodeDev/tracr/main/install.sh | sh
```

The current preview is [v0.1.0](https://github.com/BasedCodeDev/tracr/releases/tag/v0.1.0).
You can also [install from source](#install-from-source).

The installer downloads the preview binary, verifies SHA-256 checksums, and
installs to `~/.local/bin`. No Rust or `sudo` is needed. Install Codex separately,
then start in your project:

```sh
tracr .
```

If `tracr` is not found, add `~/.local/bin` to your PATH:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

Add that line to your shell configuration to keep it across terminals. Run the
install command again to update to the preview selected by the current installer.
macOS and Windows support are planned.

<details>
<summary>Custom installation, specific versions, and removal</summary>

```sh
curl -fsSL https://raw.githubusercontent.com/BasedCodeDev/tracr/main/install.sh | \
  TRACR_INSTALL_DIR="$HOME/bin" TRACR_VERSION=v0.1.0 sh
```

Installation directories must be absolute. License notices are saved in
`${XDG_DATA_HOME:-$HOME/.local/share}/tracr`. The installer leaves an existing
binary untouched if downloading, verification, or the version check fails.
It does not modify your shell configuration.

To uninstall the default installation, remove `~/.local/bin/tracr`. You may also
remove its license-notice directory above. Your Codex sessions are preserved.
Manual binaries and checksums are available from GitHub Releases.

</details>

### Install from source

With Rust and Cargo installed:

```sh
cargo install --git https://github.com/BasedCodeDev/tracr.git --locked
```

This installs to `~/.cargo/bin`; ensure that directory is on PATH. Developers can
also use `cargo install --path . --locked` from a checkout.

## Build and run

For the shortest build-and-run workflow:

```sh
./scripts/dev.sh                  # Build and browse live Codex sessions
./scripts/dev.sh .                # Build and browse the current folder
./scripts/dev.sh list --json      # Build and print the live inventory
```

The script works from any directory, finds rustup in the Cargo bin directory,
uses the pinned toolchain when installed (otherwise installed stable), generates
`Cargo.lock` on the first run, and builds with `--locked` before launching.
Application paths resolve relative to the directory you launch the script from. Set `TRACR_TOOLCHAIN` to
override toolchain selection. The first build needs crates.io connectivity.
Use `./scripts/dev.sh --help` for script options or `./scripts/dev.sh -- --help`
for the application's help.

Install Rust using your preferred method, including the toolchain pinned in
`rust-toolchain.toml` (1.99.1, with rustfmt and Clippy). Then run:

```sh
cargo build --locked
cargo run --locked -- browse --fixture tests/fixtures/sessions/catalog.json
cargo run --locked -- list --fixture tests/fixtures/sessions/catalog.json --include-archived --json
cargo run --locked -- list --json
cargo run --locked
```

Rust 1.99.1, Cargo, rustfmt, and Clippy are installed in the development
machine's `~/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin` directory.
The shell does not currently include `~/.cargo/bin` on PATH. Load
`~/.cargo/env` in your shell to expose rustup and Cargo. If rustup requests the
pinned toolchain despite the identical version being installed as `stable`,
`cargo +stable` selects that installed toolchain explicitly.

The application builds with the checked-in `Cargo.lock`. Formatting, all-target
Clippy, and the full Rust suite pass after fetching the test dependencies.
The suite caught a fixture/live-source argument conflict, now fixed for either
option ordering. Real Codex interactive acceptance passes in a normal terminal for both history formats; see [validation status](docs/validation.md).

## Configuration

Optional configuration is read from `$XDG_CONFIG_HOME/tracr/config.toml`, falling
back to `~/.config/tracr/config.toml`. Override with `--config PATH`.

```toml
codex_homes = ["/home/me/.codex", "../other-codex-home"]
```

Repeated `--codex-home PATH` options replace configured homes. Otherwise configured
homes take precedence over `CODEX_HOME`, which takes precedence over `~/.codex`.
Relative CLI and environment paths use the invocation directory; relative paths
in the TOML file use its parent directory. Paths are normalized lexically and do
not need to exist. Symlink aliases remain distinct sources. A missing default
configuration is allowed; invalid TOML and missing explicitly named files fail.
XDG directory variables must be absolute; relative values use standard fallbacks.

Use `--codex-bin /absolute/path/to/codex` to override the executable found on PATH.
Relative executable paths containing directory components resolve against the
invocation directory. Each configured home has its own app-server process.

Fixture mode rejects explicit `--codex-home` options. Configuration is still
validated, but no live provider is instantiated. tracr resolves state/cache paths
without creating directories. tracr issues metadata-only requests. Codex may maintain its own indexes during
discovery; this is not a byte-for-byte immutable filesystem operation.

## Fixture format

A fixture is a JSON object with `sessions`, optional `page_size` (default 100),
and optional zero-based `fail_at_page` to simulate partial discovery failure.
See `tests/fixtures/sessions/catalog.json` for the session schema. Relative `cwd`
paths resolve against the fixture directory. These metadata fixtures are separate
from the milestone 1 history-storage fixtures.

Output defaults to non-archived sessions, newest activity first with session
identity as the tie-breaker. JSON contains `sessions`, `complete`, and `error`;
each session includes a `folder_status`. Live and fixture listings also include
per-source diagnostics in `sources`. Text output escapes terminal controls.
On a later-page error the command retains accumulated results and exits with 1.
Invalid arguments exit with 2; configuration/provider errors exit with 1.

## Terminal browsing

`tracr` and `tracr browse` launch the terminal interface. Use
`tracr browse --fixture tests/fixtures/sessions/catalog.json` for synthetic data.
A terminal is required on stdin and stdout; use `tracr list` in pipelines.

Use `tracr .`, `tracr ~/Work/Seb.SysAdmin`, or `tracr browse PATH` to select a
folder immediately, showing only sessions recorded directly there. The session
pane starts focused: choose a session and press Enter to resume. No path still opens All sessions. You can
navigate to other folders or search globally; clearing search restores the folder.

Relative paths use the caller's current directory. Missing directories can still
be browsed; existing files are rejected. Paths are matched lexically, without
resolving symlink aliases. Use `tracr ./list` for a folder named `list`, and
`tracr -- -folder` for a path beginning with a dash. Archives remain hidden until
you toggle them. The `list` command does not accept a folder argument.

The dev script builds inside the repository and launches from your caller directory,
so relative fixture and configuration arguments also resolve there.


The folder, session, and details panes show partial results while loading.
Terminals narrower than 100 columns show the focused pane full-width.

- Up / Down or `j/k`: navigate; Left / Right or Tab / Shift-Tab: switch panes.
- Enter: expand or collapse the selected folder, or resume a session.
- `/`: edit global search; arrow keys still navigate and switch panes. Enter in
  the folder pane toggles expansion; elsewhere Enter finishes editing without
  resuming. Escape moves to the previous pane, like Left Arrow, including while
  editing. Ctrl-U clears the query. Escape still closes help or cancels resume
  preparation when those are active.
- `a`: include archived sessions; `r`: refresh; `?`: help.
- `q` or Ctrl-C: quit. While editing search, `q` is text.

Search requires every whitespace-separated term to match some metadata field:
title, native ID, path, provider, or Git branch. It ignores the selected folder
while a query is active. Clearing the query restores folder filtering.

Missing directories appear under Unlinked folders. Permission errors remain
inaccessible rather than unlinked. Folder matching uses exact paths; select a child
folder to see its sessions. Folder-tree counts include sessions in that folder and
all its descendants, while the session pane shows only direct matches. Archive
visibility affects folder counts and search.

Refresh retains old results while new pages load; failed sources remain visibly
incomplete. A successful refresh removes records no longer listed by that source.
Details are fetched separately after that source finishes discovery. When Codex
lists one folder but reads another, the details pane shows both without silently
regrouping the session. Timestamps currently display as Unix seconds.

Live discovery explicitly includes non-interactive and subagent source kinds
exposed by the tested Codex protocol, and pages active/archived inventories.
Internal Guardian approval-review sessions are excluded by their source metadata
from discovery, search, and folder counts. Ordinary sessions with the same title
and other subagents remain visible.
Backend-hidden attached threads are not separately traversed. Requests time out
after 20 seconds; quitting shuts down only app servers owned by tracr.
Press Enter in the session or details pane to resume an active Codex session in
this terminal. tracr checks fresh metadata and the working directory before launch.
If the destination changed since it was displayed, review it and press Enter again.
Escape cancels preparation. Archived sessions and fixtures remain browse-only.

Codex receives the selected source's `CODEX_HOME` and an explicit working directory.
Exiting Codex returns to tracr, preserving filters and selection and refreshing metadata.
Ctrl-C belongs to Codex while it is running; SIGTERM to tracr terminates the owned
child process group, with a five-second grace period. Relinking remains unavailable.

Resume PTY checks (no real sessions or inference):

```sh
python tests/fixtures/app-server/resume_check.py target/debug/tracr
```


## Checks

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
python -m unittest discover -s tests -v
```

Rust integration tests include a Python fake app server, a 20-second RPC timeout
case, and pseudo-terminal checks for quit, Ctrl-C, and SIGTERM restoration. They
require Python 3 on PATH. Layout tests cover normal, narrow, and tiny terminals.
The pseudo-terminal helper has passed against the compiled binary. The full Rust suite and normal-terminal real-Codex acceptance pass. Release musl
artifacts require their own verification; broader terminal/version coverage remains
release work.

## Relocation investigation

Codex relocation remains unavailable. Codex CLI 0.167.2 can persist a new working
directory for reads and resume while still listing the old directory. See
[the compatibility report](docs/codex-relocation-compatibility.md),
[the evidence](docs/codex-relocation-evidence.json), and [the project plan](PLAN.md).

The follow-up [relinking investigation](docs/codex-relinking-investigation.md) tests six strategies against installed Codex; production relocation remains unavailable.

## One-command verification

Run `./scripts/check.sh --offline` for cached-dependency checks, or
`./scripts/check.sh` to allow locked dependency fetching. Add
`--codex /absolute/path/to/codex` for isolated real-Codex acceptance without inference.
The verifier reports passed, failed, and blocked stages and retains logs. See
[validation status](docs/validation.md) for current blockers and coverage.
