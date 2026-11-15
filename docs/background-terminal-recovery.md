# Background-command recovery during session relinking

## Behavior

“Idle” describes agent activity, not the absence of background processes. If background commands block an already-confirmed relink, Tracr retains the session and destination and opens a separate recovery dialog. It lists sanitized, wrapped commands and working directories with keyboard scrolling. Cancel is the default; Tab/Left/Right choose Cancel, Refresh, or Stop listed commands and retry move. Enter explicitly activates the selected action. Buffered input is discarded when the asynchronous result opens the dialog.

Refresh only inspects. If no commands remain it offers Retry move. Unknown state has Refresh and Cancel, never a termination action. The busy phase disables repeat submissions and cancellation, since stopped commands cannot be restored; it has a 30-second cleanup deadline and displays normal relocation progress afterward. A failed stop may have stopped some commands; refresh/review shows the remaining state. Small terminals retain safe cancellation and request enlargement when necessary.

## Safety and implementation

`codex_terminals` parses all paginated native terminal entries, rejecting malformed records, duplicate identities and repeated cursors. Missing OS PID/CPU fields do not mark an entry stopped. Release distinguishes known commands from unavailable terminal state using a typed transient blocker carried by `relocation::Failure`; serde skips the blocker, so command text and confirmation state never enter journals. Other release callers remain blocking and gain no termination behavior.

Before each stop, cleanup validates source, Codex version, session identity, idle status, original folder, destination, archive state, recorded permission profile and preview fingerprint. Each fresh list must be a subset of the reviewed identities (process ID, item ID, command and working directory). Already-ended commands need no termination; newly appearing or changed commands require a new review. Calls use only `thread/backgroundTerminals/terminate` for those identities, never bulk cleanup or OS signals. A follow-up list must confirm disappearance; a false termination result is accepted only when the command is absent. Malformed responses, timeouts and remaining commands block relocation.

After cleanup, the existing confirmed relocation pipeline runs again. Attached clients, stale previews, ownership conflicts and permission changes can still prevent moving. Cleanup does not change a session's folder or force ownership. Recovery is offered only for normal single-session relinking, not folder renames or unfinished-relocation recovery. No periodic monitoring or configuration option is added.

## Validation

Provider tests cover complete pagination, inspection without termination, reviewed cleanup and relocation, changed/new commands, malformed replies and repeated cursors, active sessions and changed destination/directory, commands that disappear naturally, false termination responses, partial failures and newly arriving processes. Existing attached-client, permission and exclusive-ownership coverage remains in place.

Dialog tests cover the Cancel default, unknown-state restrictions, busy input suppression, long Unicode commands, scrolling/resizing, and serialization excluding command details. The Linux PTY helper `terminal_recovery_check.py` covers explicit confirmation, buffered Enter protection, Cancel, Refresh-to-empty, automatic retry, resizing and the attached-client blocker after cleanup. It is included in the standard verifier.

Native acceptance used Codex 0.169.0 with a disposable home, a localhost scripted model and a harmless `sleep 300` process. The session was idle while its terminal remained listed with null OS PID/CPU statistics. Per-process termination returned true, the subsequent list was empty, and both cwd and relocation fingerprint stayed unchanged. No real user session, credentials or inference service was used. Reproduce with `python3 scripts/check_codex_terminals.py --codex /absolute/path/to/codex`; supply the actual executable, not a project-dependent wrapper. The probe always attempts cleanup of its own terminal and daemon.

Generated native reports and temporary logs are not part of the source. The
reproduction command above creates fresh evidence for the installed Codex version.

The full offline verifier covers Git/worktree, relink/release, folder-move and idle
checks, along with the provider/dialog suites and terminal-recovery PTY helper.
Run `./scripts/check.sh --offline` to validate the current checkout.


## Still-loaded Retry/Cancel

A normal relink that cannot unload its idle session now reports the typed `FailureKind::StillLoaded`. The padded recovery dialog retains session identity, original folder and approved destination, explaining that Codex still reports the session loaded; it does not assert that a client must still be open. Cancel is selected by default. Retry reruns the complete confirmed relocation pipeline using the retained preview, without another destination picker or confirmation. A repeated still-loaded result returns to the same dialog. Buffered input is discarded on asynchronous completion and submissions are disabled during each attempt.

Background commands discovered on retry route to their separate explicit cleanup confirmation. Stale session/destination/permission context is rejected by the existing checks and requires a fresh preview. Folder moves and unfinished-relocation recovery keep their existing handling. No forced detach, automatic retry or additional monitoring is introduced. This improves recovery; it does not establish why a particular closed client left its session loaded.

Retry/Cancel verification: dialog unit tests, release and relocation suites, all-target Clippy, formatting and whitespace checks pass. The updated release PTY helper verifies default cancellation, no automatic retry, repeated still-loaded results, preserved destination, successful retry, stale-context rejection, buffered Enter protection, resizing, and routing into explicit background-command cleanup. The terminal-cleanup PTY helper also passes with the new still-loaded dialog. Logs: `/tmp/tracr-loaded-retry-unit.log`, `/tmp/tracr-loaded-retry-tests.log`, `/tmp/tracr-loaded-retry-pty-final.log`, `/tmp/tracr-loaded-retry-terminals.log`.

The four-scenario idle PTY check also passed with no commands or redraw output (`/tmp/tracr-loaded-retry-idle.log`).
