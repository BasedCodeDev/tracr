# Git worktree browsing validation

Implementation worktree: `/tmp/tracr-git-browser`.

## Lifecycle matrix

| Scenario | Automated evidence |
| --- | --- |
| Two-job limit and cached negatives | Controller concurrency test |
| Inventory deduplication across checkout/subfolder paths | Real temporary Git inventory test |
| External worktree removal and inventory refresh | Real Git inventory test and PTY walkthrough |
| Cancel directory probe and restart | Controlled backend test; awaited shutdown closes the request |
| Forget completed but unapplied same-generation result | Controlled backend token test |
| Generation change with queued result | Controlled backend generation test |
| Pending/unavailable ownership prevents retirement | Browser retirement test |
| Late inventory after retirement | Browser retirement test |
| Cancel inventory and restart | Controlled shutdown/reservation/retry test |
| Both inventory/directory completion orders | Separate-batch convergence test in both orders |
| Independent repository work cannot delay retirement | Controlled unrelated-probe test |
| Physical fallback and ancestor selection | Surviving physical session and removed-worktree project fallback test |
| Stale inventory failure/recovery rendering | TestBackend assertions for stale label, retained worktrees, and recovery |
| Mixed checkout failure/confirmation | Both update orders retain stale membership; retirement requires every associated path to be conclusive |
| Unrelated collapsed-project focus stability | Inventory refresh preserves collapsed projects, selected session, folder, and pane focus; TestBackend confirms hidden rows stay hidden |
| Logical navigation, session handoff, return and removal | `git_lifecycle_check.py` with a real repository, external worktree, and isolated fake session provider |
| Git process cancellation and terminal restoration | PTY matrix: directory/inventory/history reads × quit/Ctrl-C/SIGTERM/session handoff |

The PTY helper uses an isolated PATH wrapper to signal that a Git request is pending, then blocks until cancelled. It asserts the owned processes have exited before session handoff, retries discovery on return, and verifies terminal attributes and process cleanup on exit. All waits use readiness predicates with bounded deadlines. Fixtures and state are disposable; no real session provider is launched.

## Shutdown behavior

Discovery shutdown invalidates request tokens, releases unfinished inventory reservations, aborts requests, and joins their tasks. History requests are also aborted and joined. Git read workers own their child processes and react to requester cancellation by killing and reaping them. The terminal boundary drains those workers before session handoff or exit. This fixes the gap exposed by the PTY test: joining an aborted request alone did not guarantee its child was reaped before handoff.

The worker registry is process-wide and is drained only after all TUI Git request producers have stopped. Ordinary refresh remains nonblocking; there is no added polling or watcher.

## Verification

- Focused library, browsing, and folder-move regression tests passed before the process-reaping fix.
- The Git PTY walkthrough and all 12 cancellation cases passed after that fix, including the explicit logical-navigation and pre-handoff process-exit assertions.
- `./scripts/check.sh --offline`: passed every stage on the final implementation, including the registered Git lifecycle PTY check. Logs: `/tmp/tracr-check-yvm5v2sa`. Rust results: 107 passed, four native-provider tests ignored by the Rust suite as configured.
- `git diff --check`: passed.

Discovery lifecycle acceptance is complete. Every scenario in the matrix above has passing automated coverage.

This document covers discovery lifecycle acceptance. Progressive graph loading, HEAD following, metadata watching, and the other broader feature work remain separate increments.


## Direct worktree layout

Hierarchy assertions verify the `↳` prefix, direct parent depth, immediate placement beneath the project, unique projection, session counts and descendant indentation. Collapse tests verify independent checkout expansion and project-controlled visibility. Navigation recovery covers both a surviving physical folder and direct project fallback. The PTY walkthrough now moves directly between the linked checkout and its project, and continues to verify actual session launch paths and inventory removal. `./scripts/check.sh --offline` passed every stage for this layout update, including Git lifecycle PTY acceptance; logs: `/tmp/tracr-check-29slr3m3`. `git diff --check` passed.


## Branch labels and folder viewport

New scope/label tests cover branch-only labels, detached HEAD IDs, linked roots versus main checkouts and subfolders, retained inventory during refresh, and rendered Sessions titles. Viewport tests cover 50/50 layout widths, deep nesting, fixed selection markers, shared ancestor clipping, reset after shorter selections, wide/narrow terminal sizes, wide glyphs, combining accents and emoji sequences. The Git PTY fixture now expects branch-only labels while still verifying launch at the actual filesystem path. `./scripts/check.sh --offline` passed all stages (logs `/tmp/tracr-check-dyaajf1f`). An additional exact pane-boundary assertion then passed in the focused viewport test; formatting and `git diff --check` also passed.


## Progressive history loading

- Controlled backend tests cover initial limits (including a 160-commit viewport-derived request), doubled prefixes, single-flight requests, exact-limit versus exhausted responses, no automatic expansion chaining, retained rows on failure, retry limits, hiding/reopening, obsolete results on context changes, and awaited shutdown.
- Selection tests cover OID preservation, nearest surviving commit fallback, viewport offset, connector skipping, Home/End and page movement.
- TestBackend rendering covers loading, empty/error states, retained rows after failure, viewport resizing, refresh and help while the graph has focus.
- A real two-branch merge fixture spans 128 commits and verifies prefix commit/connector continuity through the final requested commit.
- The Git PTY check now includes 301-commit linear history: browsing beyond the first prefix, refreshing while preserving the selected commit, and quitting with a blocked 256-commit expansion. Existing discovery/history cancellation and launch-directory checks continue to run.

Progressive history acceptance is complete. `./scripts/check.sh --offline` passed every stage on the final implementation, including the extended Git lifecycle check and the Sessions margin regression tests. Logs: `/tmp/tracr-check-_fqq56u0`. `git diff --check` passed.


The equal 50/50 split was explicitly confirmed during this slice. The folder overflow fixture was lengthened so it still exercises clipping with the wider folder pane. Sessions right-padding coverage checks long titles and empty states at narrow and wide terminal sizes; existing tiny-terminal tests verify essential Unlinked labels remain visible when the padding contracts. Existing pane top padding is preserved.


## Selected checkout HEAD

- Controlled backend tests verify sequential prefix search for an old HEAD, OID pinning, manual navigation cancellation, same-checkout preservation, manual refresh/reopen, `f`, root reselection, checkout-switch cancellation and unavailable metadata without automatic retries.
- Native Git fixtures verify unborn branches, bare checkout detection and a detached worktree commit with no named ref, included via the explicit revision root.
- TestBackend verifies detached/branch headings, no-commits-yet presentation, a single `[HEAD]` marker and a distinct selected commit.
- Git PTY acceptance uses 301 commits and a linked branch 200 commits behind the main checkout. It verifies automatic location, manual browsing across refresh, `f` return, and switching between the two checkout HEADs. Existing progressive loading and process cleanup cases remain enabled.

Selected checkout HEAD acceptance is complete. `./scripts/check.sh --offline` passed every stage, including the extended Git PTY walkthrough. Logs: `/tmp/tracr-check-8ass6va9`. `git diff --check` passed.


## Graph references, colours and horizontal scrolling

- Parser fixtures verify local/remote/tag ordering, punctuation in names, lightweight and annotated tag targets, ignored namespaces, invalid records and empty sets.
- Graph parsing verifies palette colours, Unicode subjects, connector-only rows, and removal of CSI/OSC control sequences. The real merge-prefix fixture continues to verify stable graph geometry and colour spans across expanding prefixes.
- Controlled reference reads verify reuse during expansion, stale-label retention while history remains usable, successful recovery, and cancellation on repository changes.
- Style and rendered viewport tests verify typed label colours, independent HEAD/selection, fixed borders/selection marker, Unicode clipping, resize clamping and horizontal scrolling stopping following without moving selection.
- The Git PTY suite now includes a merge with local/remote branches and lightweight/annotated tags, horizontal scrolling, refresh and `f`. Existing old-HEAD, progressive-loading, worktree-switching and cancellation cases remain enabled.

Graph presentation acceptance is complete. `./scripts/check.sh --offline` passed every stage, including the extended Git PTY suite. Logs: `/tmp/tracr-check-zjlqf4v0`. `git diff --check` passed.

## Idle CPU investigation and correction

The executable path in htop is not a per-subsystem attribution. The baseline showed **no Git commands during settled sampling**, including visible history and old-HEAD views. Plain folders also consumed substantial CPU in the synthetic fixture. Repeated application-side discovery bookkeeping, history checks and rendering were the actionable work; evidence did not support blaming repeatedly spawned Git processes. The scheduling-only intermediate debug Git sample was 37.388%, compared with 39.782% initially; skipping unchanged rendering and settled-loop work produced the major reduction. This is an experimental comparison, not a stack-profile attribution.

Reproduce on Linux:

```sh
python3 tests/fixtures/app-server/cpu_probe.py target/debug/tracr --seconds 30 --sessions 1000
python3 tests/fixtures/app-server/cpu_probe.py target/release/tracr --seconds 30 --sessions 1000
```

The helper creates isolated repositories and fixture sessions, waits three seconds after initial presentation, then measures `/proc/<pid>/stat` CPU ticks over a 30-second window. Percentages represent one CPU core. It records parent CPU, reaped-child CPU, native Git command counts through a wrapper, terminal output bytes and post-sample Tab response. Git scenarios have 301 commits; old HEAD is in a linked checkout 200 commits behind the main branch. The terminal is 240×80. These 1,000-session stress results are not a reproduction of the user's exact machine/session count; zero means no CPU ticks observed at this sampling resolution.

Regression coverage includes settled discovery, unchanged provider snapshots, wake/deadline behavior and relative-time boundaries (minutes through years). `idle_check.py`, included in the offline verifier, requires no idle Git commands and no terminal output in all four scenarios, then checks that input wakes rendering. Numeric CPU or latency thresholds are deliberately excluded from CI. Existing relink and menu checks also exposed timing-sensitive navigation and resize behavior, now corrected. The PTY information helper waits for the session action to be available rather than assuming an initial partial screen is loaded.

Measured parent CPU (% of one core), before → after:

| Settled view | Debug | Release |
| --- | ---: | ---: |
| Plain folder | 37.299 → 0.033 | 15.390 → 0.000 |
| Visible Git history | 39.782 → 0.067 | 16.946 → 0.033 |
| Hidden Git history | 39.912 → 0.033 | 16.728 → 0.000 |
| Linked checkout with old HEAD | 43.798 → 0.067 | 21.167 → 0.033 |

All final samples recorded zero idle Git commands, zero reaped-child CPU and zero terminal output bytes. Post-sample input-to-output response was 7.70–12.79 ms in release and 21.25–29.94 ms in debug. Raw final records: `/tmp/tracr-cpu-verified-release.jsonl` and `/tmp/tracr-cpu-verified-debug.jsonl`. These are temporary local artifacts; this table preserves the results. The final relative-time correction was subsequently verified with expanded boundary tests and a rebuilt debug idle check; it does not affect the old fixture timestamps used for these measurements.

Validation completed: the offline verifier passed formatting, build, Clippy, Rust/Python tests and all terminal checks except an outdated empty-folder text expectation (`folder_check`, expecting “matching”). That expectation now matches the existing “No sessions currently shown” UI; the complete folder helper passed on rerun, including `scripts/dev.sh`. The rebuilt debug idle helper also passed. Expanded relative-time unit tests, final Clippy, formatting and `git diff --check` passed. Full run logs: `/tmp/tracr-check-ivm_f5b2`; focused reruns: `/tmp/tracr-folder-final.log`, `/tmp/tracr-idle-final.log`, `/tmp/tracr-cpu-clippy-final.log`. No live Codex acceptance was requested or run in this increment.
