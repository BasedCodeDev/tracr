# tracr

tracr is a Linux terminal application for browsing Codex sessions by folder,
searching metadata and conversation content, and resuming work in the same terminal. Browsing and
resume pass automated checks and isolated real-Codex acceptance with Codex 0.167.2.
Relink individual sessions or move a folder and its sessions together, with
recovery support verified on Codex 0.167.2 and 0.169.0.

## Install

On Linux x86_64 or ARM64:

```sh
curl -fsSL https://raw.githubusercontent.com/BasedCodeDev/tracr/main/install.sh | sh
```

The current preview is [v0.6.0](https://github.com/BasedCodeDev/tracr/releases/tag/v0.6.0).
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
  TRACR_INSTALL_DIR="$HOME/bin" TRACR_VERSION=v0.6.0 sh
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
./dev.sh                  # Build and browse live Codex sessions
./dev.sh .                # Build and browse the current folder
./dev.sh list --json      # Build and print the live inventory
```

The script works from any directory, finds rustup in the Cargo bin directory,
uses the pinned toolchain when installed (otherwise installed stable), generates
`Cargo.lock` on the first run, and builds with `--locked` before launching.
Application paths resolve relative to the directory you launch the script from. Set `TRACR_TOOLCHAIN` to
override toolchain selection. The first build needs crates.io connectivity.
Use `./dev.sh --help` for script options or `./dev.sh -- --help`
for the application's help.

Interactive browsing watches source files by default (requires Python 3). Saving a
file rebuilds in the background and restarts tracr after a successful build.
Compilation failures leave the current browser running; diagnostics are written
to the temporary log printed at startup, which you can follow with `tail -f`.
The log remains available after exit. Edits during a build queue another build.
Reload waits for interactive Codex sessions and running file/session operations
to finish. Temporary UI state resets; the existing saved-folder behavior applies.
Quitting tracr exits the watcher, and commands such as `list --json` run once.

Use `./dev.sh --once [arguments...]` for the original build-and-run
workflow without Python or watching. Runner options must precede application arguments; `--` ends runner option
parsing and the remaining arguments are passed unchanged to tracr. Non-terminal
invocations also use the one-shot workflow. The watcher checks `src/`, Cargo
and toolchain configuration, and development-runner scripts every 500 ms, with a
300 ms debounce. Build outputs and unrelated files do not trigger rebuilds.
Changes to the runner itself require restarting `dev.sh` to load its new logic.

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

## Agent activity from the command line

Run `tracr worktrees [PATH]` to get a one-shot JSON snapshot of a repository's
registered worktrees and loaded Codex agents. PATH defaults to the current
folder; main checkouts, linked worktrees, and their subfolders resolve to the
same repository. Tracr's UI does not need to be running.

```sh
tracr worktrees
tracr worktrees /path/to/repository
tracr worktrees --text
tracr --codex-home ~/.codex worktrees
```

JSON has `schema_version: 1`, `repository` (the absolute common Git directory),
`worktrees`, `complete`, `error`, and per-source diagnostics in `sources`.
Each worktree has `path`, `branch` (a full ref or null), `head`, `bare`, `locked`,
`prunable`, and `agents`. Agents include their composite `key` (provider, source,
native ID), `title`, `cwd`, and structured `status`, for example
`{"status":"running"}` or
`{"status":"waiting","approval":true,"input":false}`. Idle, error, and unknown
loaded sessions are included; unloaded history and guardian sessions are omitted.
Worktrees are sorted by path and agents by key. The calling agent is included if
visible to the configured sources.

The command queries existing Codex daemons passively, without resuming sessions,
starting turns, or answering approvals. Existing `--config`, `--codex-home`, and
`--codex-bin` options apply. Independently hosted agents and older Codex versions
without daemon support may not be observable.

Always check `complete` before interpreting an empty agent array. Source failures,
timeouts, or unresolved session directories produce an incomplete report while
preserving available results. Exit codes are 0 for a complete snapshot, 1 for
incomplete results or operational errors, and 2 for invalid arguments. Operational
errors in JSON mode use the same envelope. `--text` prints a readable table and
reports failures on stderr. This snapshot is an observation, not a worktree lock
or reservation; activity can change immediately after it is read.

## Terminal browsing

`tracr` and `tracr browse` launch the terminal interface. Use
`tracr browse --fixture tests/fixtures/sessions/catalog.json` for synthetic data.
A terminal is required on stdin and stdout; use `tracr list` in pipelines.

Use `tracr .`, `tracr ~/Work/Seb.SysAdmin`, or `tracr browse PATH` to select a
folder immediately, showing only sessions recorded directly there. The session
pane starts focused: choose a session and press Enter to resume. Without a path, tracr restores the last visited folder; the first launch starts in your current directory. You can
navigate to other folders or search globally; selecting a result opens its folder.

In Folders or Sessions, press **n** or choose **Create new session** from `?` to
start a new session in the selected folder, including folders with existing
sessions. From Folders, the new session opens full-screen; from Sessions, it
opens embedded.
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


Linked worktrees use `◇` before their names in Folders and before their local branch
labels in Git history. The main checkout and branches without linked worktrees stay
unmarked. An amber `*` after a checkout or branch name means it has staged,
unstaged, or untracked changes (ignored files are excluded). This also applies to
the main checkout. File notifications refresh status without reloading history;
press `i` on a folder for counts, or view them in session information. Counts can
overlap when a file has both staged and unstaged changes.

Git branches, worktree rows and history update automatically for the active
repository using native metadata notifications. Hidden history pauses graph reads;
worktree rows continue updating. Manual graph navigation preserves your selected
commit until you press `f` to follow HEAD again.

Tracr does not poll Git periodically or recursively watch project files. If native
watching fails, the footer reports it; `r`, repository re-entry or reopening history
retries and refreshes the data. Filesystems that silently omit notifications may
require those refresh actions. Polling fallback is not implemented yet.

Saved stashes appear in Git history as `▣` nodes with muted-magenta
`[stash@{n}]` labels. All saved stashes are included, grouped immediately above
their original base commit, newest first within each group. A stash saved today
against an old commit therefore appears beside that old commit. Each stash has
one connection to its base; Git's internal index and untracked-file snapshots
are hidden unless independently referenced by ordinary history or a checkout.
Stash rows are read-only: Enter describes the stash, and worktree creation is
unavailable. Creating or removing stashes refreshes the history automatically.

The folder and session panes show partial results while loading.
Sessions starts focused, with both available side panels expanded. At 160 columns
and 12 rows or larger, Folders sits on the left, Sessions in the center, and Git
Worktrees on the right. Otherwise, terminals at least 80 columns by 48 rows stack
Folders above Sessions above Git. Smaller terminals show only the focused panel.
Tab cycles through Folders, Sessions, and Git (when available); Shift+Tab reverses it.

- Up / Down or `k/j`: select the previous / next item. In Folders, Right / `l`
  expands a collapsed folder without changing selection; when already expanded,
  it focuses Sessions for that folder. Left / `h` collapses the folder or selects
  its displayed parent. Enter opens the only session embedded, or creates one
  embedded when the folder has none. With multiple sessions, it focuses the
  session list. While discovery is incomplete, it only focuses Sessions.
  Left, Escape, or Backspace returns to Folders from Sessions.
  Right and `h/l` do nothing in Sessions.
- `b` in Folders or Git collapses that panel and returns focus to Sessions.
  Collapsed panels leave a two-column rail in the wide layout or a one-row strip
  in the stacked layout. Navigating to a collapsed panel reopens it. Selection,
  expanded folders, and Git scroll positions are preserved. Collapse choices last
  for the current run; resizing does not reset them.
- Alt+Left/Right moves between adjacent panels in the wide layout; Alt+Up/Down
  does so in the stacked layout. Both pairs work in single-panel mode. Directions
  do not wrap; unused directions do nothing. These shortcuts do nothing during
  dialogs, search, or resume preparation.
- Tab cycles Folders → Sessions → Git → Folders; Shift-Tab reverses the order.
  Without a Git panel, both keys switch between Folders and Sessions. Entering
  Git expands collapsed history, and leaving preserves its expansion state.
  Page Up / Page Down scroll through history when it is focused.
- In Git history, Home / End selects the first / last loaded commit or session; approaching
  the end loads more history. Left / Right or `h/l` scrolls horizontally. Manual movement
  stops HEAD following; `f` resumes it. Selecting another worktree or its checkout
  root follows HEAD automatically. `b` collapses Git and focuses Sessions;
  Escape also returns to Sessions. Leaving Git preserves its expansion state
  across folders for this run.
  The top-right `↑` border marker indicates rows above the preview; the
  bottom-right `↓` indicates rows below it or more history available to load.
- In Git history (WorkTrees), `i` opens information for the highlighted commit:
  full hash, parents, refs, author and committer dates, full message, and file
  change statistics. Overview, Files, and Details tabs separate the message,
  changed paths, and full metadata. Switch tabs with Left/Right or `1`/`2`/`3`;
  use Up/Down, `j`/`k`, or Page Up/Down to scroll. Press `c` to copy all details,
  `y` to copy the full hash, and Escape or `i` to close.
- Loaded sessions appear directly beneath their worktree's HEAD commit, including
  sessions in the main project folder and subfolders. Rows show title, provider,
  and live status; a worktree label appears when several worktrees share a commit.
  Waiting sessions appear first, followed by errors, running, and idle sessions;
  newer sessions come first within each status. Unloaded and unknown sessions are
  omitted. Updates use the same live-status feed as Sessions, without reloading Git.
- Enter on a Git session row selects its folder and resumes the session,
  or offers relinking if its folder is missing. Enter on a commit selects its worktree folder,
  offering a chooser when multiple worktrees match, then applies the same
  zero/one/many-session behavior as Enter in Folders. Session rows appear as their
  HEAD commits become available in loaded history.
- Enter in Sessions: resume a session, relink one whose folder is missing, or
  create a session when the selected folder has none and creation is available.
- `n` in Sessions: create a new session in the selected folder.
- `w` (shown and enabled only when the selection supports creation) in Folders or expanded Git history: create a new worktree from checkout HEAD
  or the selected commit, respectively.
- `m` in Sessions: move the selected session. Choose an existing destination,
  press Enter to preview, then Enter again to confirm. Escape cancels before execution. If background
  commands block the move, Tracr lists them and offers **Stop listed commands and
  retry move**, Refresh, or Cancel. Cancel is selected by default. Stopping commands
  can interrupt builds or servers; only explicitly reviewed commands are stopped.
  The move is retried only after fresh relocation checks pass. An unknown command
  state offers Refresh without a stop action. “Idle” describes agent activity,
  not whether background commands remain. If Codex still reports the session
  loaded after release, a Retry/Cancel dialog keeps the approved destination.
  Retry repeats all relocation checks; it does not force-detach clients or retry
  automatically. You can retry even if you already closed the client.
- `u`: inspect/recover an unfinished relocation or folder move. Only shown and enabled
  when unfinished work exists. Recovery never recreates the old folder.
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

By default, search requires every whitespace-separated term to match a metadata field:
title, native ID, path, provider, or Git branch. Search respects folder focus;
without folder focus it searches globally. Choosing a result returns to exact-folder browsing; sessions
without a recorded folder are selected under Unknown folders.

**Deep search**

Press `/` and enable **Deep search** with `Ctrl+D` (or Tab to the checkbox and
press Space). Search includes your messages, proposed plans, and final Codex
replies, alongside the usual title, folder, session ID, provider, and branch.
Tool output, reasoning, and progress updates are excluded. Older completed turns
without final-answer markers use their last unmarked agent reply.

Focus a folder first to limit the search to its descendants and displayed
worktrees—for example, focus sysadmin and search for `AltTab Ninja`. Every
space-separated term must occur somewhere in the session; matching ignores case.
Results show a matching excerpt. Enter opens the folder and selects the session.

Content loads in the background only for a nonempty deep-search query. Progress
and unavailable-history warnings appear above the results. Content is cached
only while Tracer runs; refresh clears the cache. The toggle is remembered during
the current run and defaults to off on launch. Claude content search is not yet
supported.


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
Folders, Sessions, and Git history each have their own
action context: only the focused pane is highlighted, and its footer and `?` menu
show the applicable actions. Git offers Collapse/Expand and, when expanded,
commit navigation and horizontal scrolling. Small terminals show only the focused panel. The footer and `?` action menu
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
Enter opens the selected session embedded in the Sessions workspace, keeping
other panels visible when space permits. **Alt+F** on a highlighted session opens
it full-screen. The open-terminals picker uses the same shortcuts when reattaching
a running terminal.
Once a terminal is displayed, **Alt+F** toggles between embedded and full-screen
from Sessions, Folders, or Git history, focusing it without restarting the process.
The action menu also offers full-screen and embedded opening.
**Ctrl+B** returns to the list. In embedded mode, **Tab / Shift+Tab** cycle
Tracer panes. In full-screen mode, they reach Codex for completion, context, or
other application actions without revealing the panels. Use **Alt+F** to return
to embedded mode before using Tab to navigate panels. Tracer dialogs keep their
own Tab navigation. **Alt+arrow keys** move panel focus in inline mode.
In full-screen mode, they are forwarded to the session without revealing panels
or leaving full-screen. Alt+Enter inside Codex is forwarded to Codex.
In Folders, **Alt+Z** zooms into the selected folder as the tree root, hiding ancestors
and unrelated branches. Its descendants and displayed worktrees remain navigable,
including worktrees outside that directory. Search is restricted to this context;
the search dialog and footer show the focused path. Press Alt+Z in Folders again
to clear focus without changing the current selection. Navigation outside the
context through another panel clears focus with a notice.

Focus is remembered separately for each launch folder (the explicit path, or the
current directory for plain `tracr`). A new explicit launch path starts focused
on that folder; a new plain-launch context restores the last visited folder with
the full tree. Clearing focus is remembered too.

Sidebar sizes depend on the window dimensions, so tree expansion, selection,
and scrolling do not move the dividers. In the wide layout, Folders uses 24–32
columns and Git uses 36–48; Sessions receives the remaining width. In the stacked
layout, Folders uses 20% of the available height (up to 12 rows), and Git uses
twice that height (up to 24 rows). Both shrink proportionally in shorter windows,
leaving at least 40% for Sessions.
Collapsing either panel gives its space to Sessions. Git is omitted outside a
repository. Resizing changes the existing terminal dimensions without restarting Codex.
The mouse wheel over a visible terminal scrolls its output history, even when
Folders has keyboard focus. Up to 2,000 lines are kept per terminal. New output
keeps your reading position; typing or pasting returns to the live screen. An
indicator appears while viewing older output. CLIs that request mouse reporting
receive wheel events themselves; their alternate-screen behavior is respected.
Tracr dialogs block scrolling into the terminal behind them.

Keyboard input preserves modifiers and negotiates the child application's keyboard
protocol, including Ctrl+Backspace, modified navigation/function keys, and
press/repeat/release events. Legacy terminals retain their normal byte mappings;
keys intercepted by the desktop or outer terminal cannot reach Tracr. Each running
session keeps independent keyboard, paste, focus, and mouse modes.

Drag to select terminal text, double-click a word, or triple-click a wrapped logical
line. Hold **Shift** while dragging to select locally when the child captures the
mouse. Dragging past the top or bottom scrolls the selection through retained history.
**Alt+C** copies a selection; **Ctrl+Shift+C** also works if the outer terminal
forwards it. Copying is explicit and uses the desktop clipboard; **Ctrl+C** continues
to interrupt the child. Selections stay with their folder's terminal, but clear when
you type, resize, reset the screen, or the selected text changes or leaves history.


Each folder remembers its Sessions panel for the current Tracr run. Navigating to
another folder shows that folder's session list or its last displayed terminal;
returning restores the same running process inline without changing keyboard focus.
Parent folders and worktrees have independent views. If a terminal exits, its
folder returns to the session list. Choosing an open terminal from the terminal
picker navigates to its folder.
**Ctrl+B** backgrounds the terminal and remembers the session-list view for that folder; selecting an
open session reattaches without restarting it. **Ctrl+C** still interrupts Codex. **Tab / Shift+Tab** move focus between panels in embedded mode
without backgrounding the terminal; input goes to whichever panel is focused.
From the browser, press **g** for Open terminals (including newly started sessions).
The navigator lists open terminals with aligned provider and status columns; use
↑/↓ or j/k to select, Enter to attach, and g or Escape to return.
Use **x** in Sessions or the navigator to close a terminal. Closure requires confirmation.
New terminals keep their own entries until closed; tracr does not infer their
Codex session identity from timing or directory names.

Each tracr instance owns its terminals. Quitting with open terminals asks before
closing them, while another instance can continue monitoring. Resuming an identified
session already owned by another tracr is blocked. Terminals do not persist after
their owning tracr exits. Inside tmux with its default prefix, press Ctrl+B twice
to send the background shortcut through to tracr.

Exiting Codex restores the session list and refreshes metadata.
Ctrl-C belongs to Codex while its terminal is focused; SIGTERM to tracr terminates
its owned child process groups, with a five-second grace period. See the relinking section below
for supported versions and permission configurations.

Resume and inline PTY checks (no real sessions or inference):

```sh
python tests/fixtures/app-server/resume_check.py target/debug/tracr
python tests/fixtures/app-server/inline_check.py target/debug/tracr
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

## Dialog controls

Dialogs share padded frames, bordered text fields, and a fixed bottom action row.
The focused control has a stronger border or highlighted button. Tab/Shift-Tab
moves between controls; Enter advances through form fields and activates a focused
button. Space also activates buttons. Search, action lists, and harness selection
retain Enter-to-open/run behavior. Escape cancels, closes, or returns to editing
according to the current step.

Long content scrolls independently of the action row. Copy feedback and progress
stay above the buttons. In small terminals spacing is reduced; an explicit resize
message appears when controls cannot fit. Running file/session mutations retain
their existing cancellation restrictions.

## Relink or relocate a session

Select the session and press `m`, or choose **Move session** from `?`. Supply an
existing directory using the folder tree or the editable **New folder** field
(absolute, `~/…`, or relative to tracr's launch directory).

- Up/Down selects a folder and fills the path field, including while typing.
- Enter in the picker tree expands or collapses the folder.
- Tab switches between the path field, tree, **Cancel**, and **Use this folder**.
- Type or paste a path to reveal it in the tree. Ctrl+U clears the field.
- Enter in the field focuses **Use this folder**; Enter on that button opens
  the preview. Esc cancels.

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

To test built-in-profile support from this checkout, run `./dev.sh .`.
Restarting an older installed binary does not load these source changes.

Failures remain in a **Couldn't relink session** dialog until you press Enter or
Esc. The dialog explains whether tracr left the folder unchanged. If an operation
may have changed the folder, **Relink needs recovery** directs you to close the
dialog and press `u`. Up/Down scrolls long explanations; technical details are
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
`~/.local/state/tracr/relocations`). If execution is interrupted, press `u` to inspect
and attempt guarded completion. Press `u` in recovery to cycle unfinished operations. Conflicting later changes are left untouched.
Escape cannot abandon a running mutation; forced termination leaves a journal.

See [relocation compatibility](docs/codex-relocation-compatibility.md) for the native
mechanism, supported permission profiles, and validation. Built-in profile checks cover
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


## Create a Git worktree

In **Folders**, press **w** or choose **Create worktree** from `?` to start from
that checkout's HEAD. In expanded **Git history**, use **w** or **Create worktree
from selected commit** to start from the highlighted commit. Both actions open
the same dialog and show the source checkout and exact starting commit.

Enter a new branch name and review the destination. By default, tracr suggests
`<main checkout>/.worktrees/<branch>` even when creating from another worktree.
For example, `feature/login` creates `.worktrees/feature/login` under the main
checkout. The suggestion follows the branch name until you edit the destination.
Custom paths may be absolute, `~/…`, or relative to the source checkout root.
Tab/Shift-Tab switches controls; Ctrl-U clears a field. Enter advances from
New branch to Destination, then focuses **Create worktree**. Enter on
**Create worktree** validates the inputs and creates the worktree directly.
Escape cancels before creation starts. Creation itself cannot be cancelled through the dialog.

Opening the dialog fixes the starting commit: later HEAD changes do not silently alter
it. The new checkout contains committed content; uncommitted changes remain in
the source checkout. After success, tracr selects the new worktree in Folders.
Switch to Sessions and use the existing New session action when ready.

Git 2.48 or newer with relative-worktree support is required for creation.
Relative links enable Git's `extensions.relativeWorktrees` repository extension;
older Git versions and incompatible Git tools cannot open that repository.
Ordinary browsing remains available without this creation capability.

The default container is ignored locally using `/.worktrees/` in the repository's
`info/exclude`, without editing tracked `.gitignore` files, making commits, or
changing global Git settings. Existing effective exclusions are reused. A custom
path inside another checkout of the same repository receives an exact subtree
exclusion. Outside paths need no exclusion and do not travel with the project.
Conflicting tracked ignore rules, tracked container content, and exclusion-write
failures stop creation and leave the inputs available for correction.

The main checkout must contain its ordinary `.git` directory. Bare repositories,
external Git directories, submodule checkouts, repositories containing submodules,
symlink destinations/parents, existing destinations, existing branch names, and
custom destinations inside unrelated repositories are not supported in this slice.
tracr does not force creation, reset branches, copy dirty files, or automatically
delete branches or directories after failure. Errors report observed partial state;
local exclusions or Git compatibility settings may remain after a failed attempt.

Moving the entire main checkout with its nested worktrees preserves the new Git
links. This does not update Codex session paths after a move outside tracr. Use
**Move folder** below to update supported descendant sessions along with the files,
or relink affected sessions after an external move. Existing absolute-linked
worktrees are not converted or repaired by creating a new one.

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


Large Git browser acceptance (separate from the normal verifier):

```sh
python3 scripts/git_acceptance.py target/release/tracr --output /tmp/tracr-git-acceptance.json
```

This generates disposable 30,000-commit and 64-branch repositories, checks
progressive loading, graph prefix continuity, old checkout HEADs and cancellation,
and records display/expansion latency, peak memory, command counts and settled
CPU. Timing values are measurements rather than CI speed thresholds. The normal
verifier includes real nested-repository, local-submodule, independent-clone and
bare-worktree ownership checks. Release smoke tests include Git lifecycle,
native notifications and deterministic idle checks.
