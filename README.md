# tracr

tracr is a Linux terminal application for browsing Codex sessions by folder,
searching metadata globally, and resuming work in the same terminal. Browsing and
resume pass automated checks and isolated real-Codex acceptance with Codex 0.167.2.
Relink individual sessions or move a folder and its sessions together, with
recovery support verified on Codex 0.167.2 and 0.169.0.

## Install

On Linux x86_64 or ARM64:

```sh
curl -fsSL https://raw.githubusercontent.com/BasedCodeDev/tracr/main/install.sh | sh
```

The current preview is [v0.3.0](https://github.com/BasedCodeDev/tracr/releases/tag/v0.3.0).
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
  TRACR_INSTALL_DIR="$HOME/bin" TRACR_VERSION=v0.3.0 sh
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
without creating directories. Browsing issues metadata-only requests; explicitly confirmed relinking uses native session settings operations. Codex may maintain its own indexes during
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
pane starts focused: choose a session and press Enter to resume. Without a path, tracr restores the last visited folder; the first launch starts in your current directory. You can
navigate to other folders or search globally; selecting a result opens its folder.

In Sessions, press **n** or choose **Create new session** from `?` to start a
new session in the selected folder, including folders with existing sessions.
When no sessions are shown, Enter also creates a session; otherwise Enter resumes
the selected session.
This opens Codex interactively in that folder and returns to tracr when you exit.
A single configured creation source launches directly. With multiple sources,
**Choose harness** lets you select the configuration to use before launch.
The folder must already exist. Hidden archives can be shown with `a`; fixture data
and the Unlinked/Unknown aggregate views cannot create sessions.

The last concrete folder is saved in `$XDG_STATE_HOME/tracr/browser-state.json`
(default `~/.local/state/tracr/browser-state.json`). Explicit paths override this
preference. Fixture browsing and `tracr list` never read or update it.

Relative paths use the caller's current directory. Missing directories can still
be browsed; existing files are rejected. Paths are matched lexically, without
resolving symlink aliases. Use `tracr ./list` for a folder named `list`, and
`tracr -- -folder` for a path beginning with a dash. Archives remain hidden until
you toggle them. The `list` command does not accept a folder argument.

The dev script builds inside the repository and launches from your caller directory,
so relative fixture and configuration arguments also resolve there.


The folder and session panes show partial results while loading.
Terminals narrower than 100 columns show the focused pane full-width.

- Up / Down or `j/k`: navigate; Left selects Folders, Right selects Sessions
  without wrapping. Tab / Shift-Tab cycle between panes.
- Enter: expand or collapse the selected folder, resume a session, or relink a session whose folder is missing.
- `n` in Sessions: create a new session in the selected folder.
- `l` in Sessions: relink the selected session. Choose an existing destination,
  press Enter to preview, then Enter again to confirm. Escape cancels before execution.
- `R`: inspect/recover an unfinished relocation. Recovery never recreates the old folder.
- `/`: open the search modal. Type to filter results; each result shows its title
  and path. Up/Down selects a result; Enter opens its folder and highlights the
  session without resuming it. Escape cancels without changing your location.
  Ctrl-U clears the modal input. Empty input lists all sessions allowed by the
  archive toggle.
- Escape outside the modal selects Folders, closes help, or cancels
  pending resume preparation.
- `i` in Sessions: open session information; `c` copies it, and Enter/Esc/`i` closes it.
- `a`: include archived sessions; `r`: refresh.
- `?`: open the grouped action menu. Up/Down or `j/k` selects a command; Enter
  closes the menu and runs it. Unavailable commands show a reason and remain
  disabled. Esc or `?` closes without changing the browser. PageUp/PageDown and
  Home/End navigate longer menus; the selection scrolls into view automatically.
- `q` or Ctrl-C: quit. While editing search, `q` is text.

Search requires every whitespace-separated term to match some metadata field:
title, native ID, path, provider, or Git branch. It ignores the selected folder
inside the modal. Choosing a result returns to exact-folder browsing; sessions
without a recorded folder are selected under Unknown folders.

Live activity appears to the right of session titles: **Running** is green;
**Waiting for approval/input** is orange. Folder colors include activity in
subfolders, with orange taking priority when work is waiting. Selection markers
and focus borders remain visible independently of those colors.

Status is polled every two seconds from each configured Codex home's existing
daemon, without resuming sessions or answering approvals. **Idle** means Codex
still has the session loaded; **Unloaded** means the latest successful check found
it released. Idle uses pale yellow, including folders containing idle sessions;
Unloaded stays neutral. Waiting and running take priority over idle folder colors.
Closing a Codex window may leave a session idle briefly before Codex releases it;
tracr tries to release detached idle sessions while the folder-move dialog is
open and after you confirm an individual relink. Sessions still attached to another client remain protected;
close that client and retry. Running or waiting sessions cannot be moved. Session details explain availability.
Before the first successful check, or if monitoring fails, sessions show **Status
unknown**. On failure, activity highlights are cleared and the footer reports that
live status is unavailable; retries occur after five seconds.
Independently hosted sessions and older Codex versions without the daemon
interface may have unknown status. Metadata browsing and `tracr list` still work.

Missing directories appear under Unlinked folders. Permission errors remain
inaccessible rather than unlinked. Folder matching uses exact paths; select a child
folder to see its sessions. Folder-tree counts include sessions in that folder and
all its descendants, while the session pane shows only direct matches. Archive
visibility affects folder counts and search.

Refresh retains old results while new pages load; failed sources remain visibly
incomplete. A successful refresh removes records no longer listed by that source.
The browser has two panes: Folders and Sessions. The footer and `?` action menu
follow the selected pane: folder expansion and moving belong to Folders; resume,
Info, and relinking belong to Sessions. Shared controls stay in the same order,
with Help and Quit first. Both action rows are right-aligned. Feedback shares
the upper row on the left when it fits; otherwise it gets a third row.
Narrow terminals omit trailing footer hints; the full
set remains available in `?`. Recovery is shared by both panes.
 Press **i** in Sessions, or
choose **Session information** in the action menu, to open a centered information
modal. Use arrows, PageUp/PageDown, Home/End to scroll, **c** to copy the information,
and **i**, Enter, or Escape to close it. The selected session and pane are preserved.

Information is grouped into Session, Location, and Technical details, with aligned
labels and wrapped values. Narrow terminals stack labels above values. The title
and copy controls stay visible while scrolling, and copying keeps your position.
Missing metadata reads “Not recorded”; uncertain states read “Unknown”.

Browsing never opens Info automatically. If a fresh resume check discovers a
changed destination, tracr stops the launch and opens **Session folder changed**
with both paths. Close that warning and press Enter separately in Sessions to
request resume again; the destination is checked again before launch.

Details are fetched separately after that source finishes discovery. When Codex
lists one folder but reads another, Session information shows both without silently
regrouping the session. The Sessions pane shows how long ago each session was
last active, ordered from newest to oldest. Exact timestamps remain available
as local dates with a timezone in Session information, and as Unix seconds in CLI output.

Live discovery explicitly includes non-interactive and subagent source kinds
exposed by the tested Codex protocol, and pages active/archived inventories.
Internal Guardian approval-review sessions are excluded by their source metadata
from discovery, search, and folder counts. Ordinary sessions with the same title
and other subagents remain visible.
Backend-hidden attached threads are not separately traversed. Requests time out
after 20 seconds; quitting shuts down only app servers owned by tracr.
Press Enter in the Sessions pane to resume an active Codex session in
this terminal. tracr checks fresh metadata and the working directory before launch.
If the destination changed since it was displayed, review it and press Enter again.
Escape cancels preparation. Archived sessions and fixtures remain browse-only.

Codex receives the selected source's `CODEX_HOME` and an explicit working directory.
Exiting Codex returns to tracr, preserving filters and selection and refreshing metadata.
Ctrl-C belongs to Codex while it is running; SIGTERM to tracr terminates the owned
child process group, with a five-second grace period. See the relinking section below
for supported versions and permission configurations.

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

## Relink or relocate a session

Select the session and press `l`, or choose **Relink session** from `?`. Supply an
existing directory using the folder tree or the editable **New folder** field
(absolute, `~/…`, or relative to tracr's launch directory).

- Up/Down selects a folder and fills the path field, including while typing.
- Enter in the tree expands or collapses the folder, just like the Folders tab.
- Tab switches between the path field, tree, and **Use this folder** button.
- Type or paste a path to reveal it in the tree. Ctrl+U clears the field.
- Enter in the field or on **Use this folder** opens the preview; Esc cancels.

The tree starts at the closest existing parent of the original folder. It includes
hidden directories and loads expanded folders in the background.

Review the resolved destination and confirm. On success, tracr opens that folder
and highlights the same session. It does not start a model turn or launch a chat.

Supported versions are initially Codex 0.167.2 and 0.169.0, with legacy and paginated
histories. Close the session in other Codex clients first, even if it is idle.
Native writer exclusion prevents changing a session owned elsewhere. Archived
sessions are temporarily unarchived and restored. The built-in `:workspace` and
`:read-only` profiles are supported, along with ordinary legacy read-only and trusted
single-root workspace-write settings. The confirmation shows the permission profile.
Custom profiles, full-access profiles, explicit extra writable roots, and multi-root
workspaces remain blocked. A permission-profile
rejection means tracr does not yet support moving that session with its current
permission settings; it does not mean you selected the wrong folder.

To test built-in-profile support from this checkout, run `./scripts/dev.sh .`.
Restarting an older installed binary does not load these source changes.

Failures remain in a **Couldn't relink session** dialog until you press Enter or
Esc. The dialog explains whether tracr left the folder unchanged. If an operation
may have changed the folder, **Relink needs recovery** directs you to close the
dialog and press `R`. Up/Down scrolls long explanations; technical details are
included below the session and destination. Press `c` to copy the complete message
as plain text, with paragraph breaks but no dialog borders or visual line wrapping.
The dialog reports copy success or failure and stays open. Clipboard support uses
`wl-copy` on Wayland or `xclip` on X11; if the required utility is missing, tracr
names it in the error message. Rejected attempts are recorded
separately from completed moves and do not appear as unfinished recovery work.

Relinking changes Codex's persisted directory through its API. Database-only
listing reflects the current folder; clients using Codex's default rollout listing
may still display the original folder. Historical messages are not rewritten.
tracr does not move files, create destinations, or update project-root associations.
The original folder may still exist or be missing. Selecting the same directory,
including a symlink alias, is rejected. Use the separate **Move folder** action below to move files and descendant sessions together.

Private recovery journals live in `$XDG_STATE_HOME/tracr/relocations` (default
`~/.local/state/tracr/relocations`). If execution is interrupted, press `R` to inspect
and attempt guarded completion. Conflicting later changes are left untouched.
Escape cannot abandon a running mutation; forced termination leaves a journal.

See [the native mechanism](docs/codex-native-relinking-solution.md),
[implementation validation](docs/relinking-validation.md), and the
[permission-profile investigation](docs/codex-profile-relinking.md).
The [built-in profile validation](docs/builtin-profile-relinking-validation.md) covers
the production implementation and its real-Codex acceptance cases.

The session relink picker has **Folders** and **Worktrees** views. With the list
focused, Left selects Folders and Right selects Worktrees without wrapping;
within the path field those keys move the
caret. Worktrees lists all Git-registered checkouts for the session’s original
repository, including external locations. Up/Down or Enter fills the destination
with the selected worktree root; **Use this folder** still opens the preview,
and confirmation is required before relinking.

Each view remembers its own path. Returning to Folders restores the path and
tree position from before visiting Worktrees. Git discovery requires the original
session folder to exist in a repository; otherwise use Folders manually.
Missing worktrees are unavailable, and worktree selections are rechecked before
preview. This feature does not create, delete, or move Git worktrees.

## One-command verification

Run `./scripts/check.sh --offline` for cached-dependency checks, or
`./scripts/check.sh` to allow locked dependency fetching. Add
`--codex /absolute/path/to/codex` for isolated real-Codex acceptance without inference.
The verifier reports passed, failed, and blocked stages and retains logs. See
[validation status](docs/validation.md) for current blockers and coverage.


## Move a folder and its sessions

Select an existing folder in the **Folders** pane and press **m**, or choose
**Move folder** from `?`. Choose a destination folder using the same directory browser as relinking.
End the path with `/` to preserve the original folder name: moving `project`
to `/archive/` produces `/archive/project`. Without a trailing slash, the path
is the exact new location, so `/archive/renamed` also renames the folder.
The previous folder and final path appear above the field. A compact Sessions
table beside the folder tree shows live states throughout the source subtree,
including archives. While this dialog is open, tracr automatically attempts to
release eligible detached idle sessions. Sessions still open in Codex stay
protected; closing them lets a later check release them. PageUp/PageDown scroll
the table. Narrow terminals stack
it below the tree. These hints do not replace the final safety checks. Press Enter to
preview, then Enter to confirm; Escape returns from the preview to editing.
The preview lists the affected sessions, including archived sessions and sessions
in subfolders. Enter confirms; Escape returns to editing, where Escape cancels.

The destination must not exist. This version supports directory renames within
one filesystem; it never merges directories, overwrites a destination, or falls
back to copying across disks. Hidden files and internal symlinks move with the
directory. Relative session paths are preserved, while unrelated sessions stay put.

Every configured Codex source is checked, independent of search and archive
visibility. Active sessions, unsupported permission settings, missing descendant
directories, or incomplete inventories block the move. Detached idle sessions are
released automatically while choosing a destination and checked again after
confirmation, before files move. Loaded archived
sessions and sessions with background terminals require manual closure; automatic
release is not yet verified for those cases. Codex storage and tracr's
recovery storage cannot be inside the folder being moved.

Progress is shown while files and session metadata are updated. These updates
are recoverable but are not an atomic transaction across Codex and the filesystem.
Folder failures use the same scrollable error dialog as session relinking.
Press **c** to copy the complete report, including both paths and all error
details; Enter or Esc closes it. If interrupted, **Move needs recovery** explains
the unfinished operation; close the dialog and press **R** to inspect and finish it. Recovery checks directory identities and session
settings before continuing, and refuses conflicting external changes. It does
not automatically move files back. Folder journals live under tracr's state
directory in `folder-moves/`, alongside existing single-session journals.

Moving between filesystems and reconnecting a folder moved outside tracr are
future increments. Avoid changing the folder or opening affected sessions in
other applications while a move is in progress.
