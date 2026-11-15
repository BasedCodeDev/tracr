# Git worktree browsing validation

Implementation worktree: `/tmp/tracr-git-browser`.

## Current acceptance summary (2026-11-16)

This summary supersedes historical “remaining work” statements below. Current native-ASCII browser functionality has passed acceptance; overall first-slice acceptance remains open because the approved Unicode renderer is unresolved and release gates below are not all complete.

| Gate | Current evidence/status |
| --- | --- |
| Repository ownership and session scopes | Real nested/submodule/independent-clone/bare-worktree integration test passed |
| Navigation, history, watchers and lifecycle | Full offline verifier passed, including native Git terminal tests |
| 30,000 commits and 64 parallel branches | Explicit release and debug acceptance runs passed |
| Idle behavior | Zero settled Git commands and terminal output in all large-fixture samples |
| x86_64 musl release binary | Static linkage, architecture, version and full release smoke passed |
| Dependency notices | Resolved Cargo license notices generated, including notify |
| Runtime notices / packaged assets | Blocked: required Rust-library and/or distribution musl license texts unavailable locally; packaging checks were not bypassed |
| ARM64 musl release binary | Static architecture/linkage/version and full smoke passed under QEMU; native ARM64 execution unverified |
| Approved Unicode graph | Unresolved; separate feasibility work remains authoritative |
| Real user-provider acceptance | Not run in this increment; all session providers were isolated fixtures |

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

Hierarchy assertions verify the worktree prefix, direct parent depth, immediate placement beneath the project, unique projection, session counts and descendant indentation. Collapse tests verify independent checkout expansion and project-controlled visibility. Navigation recovery covers both a surviving physical folder and direct project fallback. The PTY walkthrough now moves directly between the linked checkout and its project, and continues to verify actual session launch paths and inventory removal. `./scripts/check.sh --offline` passed every stage for this layout update, including Git lifecycle PTY acceptance; logs: `/tmp/tracr-check-29slr3m3`. `git diff --check` passed.


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

## Native metadata watching

Implemented native-only active-repository watching; optional polling fallback was explicitly deferred. The watcher has a bounded mailbox and no Git subprocess or filesystem scan in its callback. Access, index, lock and timestamp-only notifications are filtered. Checkpoint/subscription and native rescan events reconcile only the active repository; graph work remains deferred while hidden.

- Controlled tests cover 200 ms debounce, the one-second burst cap, busy-read coalescing, old mailbox isolation, failure/retry checkpoints, runtime errors, overflow, filtered paths and subscription scope.
- Native fixtures cover ordinary and bare Git repositories using both files and reftable reference storage, nested/new refs, packed refs, worktree creation/removal and detached checkout HEADs. Reftable cases report a skip when the installed Git cannot initialize that storage format.
- Atomic replacement and missing-directory fixtures verify HEAD rename, new refs directories, removal and re-creation of refs directories, and subsequent writes.
- The terminal watcher check verifies external commits, branch/tag updates, detached HEAD, manual selection preservation, hidden history with zero graph commands, worktree addition/removal, quiet source/read-only Git activity and active-repository switching.
- Lifecycle acceptance additionally checks that no inotify descriptors survive session handoff. Shutdown unregisters paths and waits for the native callback lifetime to end. Existing worker cancellation/reaping tests remain enabled.
- Footer failure coverage checks that the persistent warning and details remain present alongside transient messages.

Repeated 30-second idle samples with native watching enabled (1,000 sessions, 240×80 terminal; same fixture and method as the CPU correction):

| Settled view | Debug CPU % | Release CPU % |
| --- | ---: | ---: |
| Plain folder | 0.067 | 0.033 |
| Visible Git history | 0.033 | 0.000 |
| Hidden Git history | 0.067 | 0.033 |
| Linked checkout with old HEAD | 0.033 | 0.000 |

Every sample recorded zero idle Git commands, zero reaped-child CPU and zero terminal output. Input-to-output response was 7.47–10.56 ms in release and 21.01–28.26 ms in debug. Raw local artifacts: `/tmp/tracr-watch-cpu-release.jsonl` and `/tmp/tracr-watch-cpu-debug.jsonl`. These are synthetic measurements, not hard CPU guarantees; zero means no CPU tick was observed during the window. Later changes add failure-path retry and warning coverage without changing settled successful-path work.

The first full offline verifier passed all stages (logs `/tmp/tracr-check-j5kup7lw`). Expanded project-switch/manual-selection terminal checks also passed. The final run additionally covers repository-scoped recovery of stale ownership after a transient discovery failure. Native x86_64 Linux debug/release builds are available; musl cross-builds were not run because only the x86_64 GNU Rust target is installed. Live Codex and broad release acceptance remain separate from this increment.

Final verification passed every offline verifier stage, including the expanded watcher and handoff checks: `/tmp/tracr-check-6fxt1zno`. Final native release build, formatting and `git diff --check` passed. The increment is complete; changes are not committed yet.


## Coloured Unicode feasibility (2026-11-16)

See [the isolated prototype report](unicode-git-graph-validation.md) for reproducible fixtures, measurements, source/API comparison and recorded previews. The experiment completed, but the candidate renderer is **not accepted for integration**: 14 of 48 fixture/prefix cases passed, 16 lacked explicit unloaded-ancestry continuation, and 18 were unsupported. Existing history (10 tests) and styled graph viewport (2 tests) baselines passed; the new renderer is not installed and its UI behavior is not certified. Eight harness tests passed. The Unicode appearance remains in scope while the implementation choice is reconsidered.


## Browser baseline acceptance increment (2026-11-16)

Full offline verifier: passed every stage, logs `/tmp/tracr-check-1w7nokqh`. The real-repository scope fixture passed with five independent common Git directories: project, nested repository, local submodule, independent clone and bare repository. Project checkout scope contained only its own two sessions; hierarchical counts included nested and logical worktree descendants exactly once. External and bare-linked selections retained their actual paths. Native worktree launch correctness remains covered by the lifecycle PTY helper.

Large-repository release results (single runs, 240×80 terminal, one fixture session per view):

| Fixture | Commits | Initial display ms | Expansion ms | Peak RSS KiB | Settled CPU % |
| --- | ---: | ---: | ---: | ---: | ---: |
| Linear | 30,000 | 178.33 | 91.53 | 10,324 | 0.033 |
| 64 branches + merges | 1,089 | 134.56 | 49.79 | 10,524 | 0.000 |
| Linked checkout 25,000 commits behind | 30,000 | 870.26 | — | 35,260 | 0.033 |

Each sample measured roughly 30 seconds of settled time and recorded zero Git commands, zero child CPU and zero terminal output. Initial limits were 128; ordinary browsing expanded to 256. Old-HEAD location doubled through 32,768, then stopped. A checkpoint reconciliation repeated the retained prefix once; settled operation did not keep reading. Cancellation at the blocked 256-commit expansion killed and reaped its Git process. Prefix comparisons exclude artificial trailing connector rows, matching the existing native-graph continuity contract.

A debug walkthrough also passed all four cases, including selected-commit assertions after expansion and two-second deterministic idle windows. Debug initial display was 202 ms linear, 152 ms wide and 1,016 ms old HEAD; peak RSS was 16,488 / 16,752 / 41,748 KiB respectively. These timings include fixture-provider startup and instrumented Git invocation overhead. They are evidence, not a guaranteed performance budget. The native graph baseline is the renderer measured here.

Reproduction:

```sh
python3 scripts/git_acceptance.py target/release/tracr --output /tmp/tracr-large-acceptance.json
python3 scripts/git_acceptance.py target/debug/tracr --seconds 2 --output /tmp/tracr-large-debug.json
```

Machine-readable artifacts: `/tmp/tracr-large-acceptance.json` and `/tmp/tracr-large-debug.json`. The measured toolchain was `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, matching the repository pin through the installed stable alias. Both musl targets were installed for this acceptance run. Native x86_64 static verification and the full release smoke passed. Dependency license extraction passed, but the packaging runtime-notice check reported missing license texts; no asset package, tag or release was created.


Final platform evidence: ARM64 musl built with the pinned compiler's `rust-lld`; `readelf` confirmed AArch64 and absence of an ELF interpreter/dynamic dependencies. `qemu-aarch64-static` ran the version check and full release smoke suite, including Git lifecycle/watch/idle checks, successfully (`/tmp/tracr-arm64-smoke.log`). This is emulated runtime evidence, not native ARM64 validation or a benchmark. x86_64 musl ran natively. Existing packaging guards remain unchanged. Formatting, Clippy, the full Rust/Python/PTY verifier and `git diff --check` passed; current renderer acceptance is complete, while overall first-slice acceptance remains open for Unicode rendering and packaged-release gates.

The final release probe, including explicit selected-commit assertions after expansion and defensive cleanup, passed all four cases with two-second idle windows (`/tmp/tracr-large-final.json`). The original 30-second release measurements above remain the performance record. Git version: 2.56.0. Acceptance changes are uncommitted; the prior watcher increment is committed as `6fcf2c6`.


## Compiled library comparison and Unicode direction (2026-11-16)

The isolated gleisbau 0.7.5 experiment is complete; see [the decision report](gleisbau-git-graph-validation.md). The default output matrix passed 26/50 cases; all four ordering configurations retain correctness or prefix-stability failures. This is an experimental acceptance failure, not a release-verifier failure. Fifteen harness/oracle tests, the opt-in real-output Tracer history adapter test, and targeted Clippy checks pass. The adapter confirms existing commit navigation, expansion, manual/follow transitions, checkout HEAD selection and rejection of obsolete replies remain usable. This was the selection checkpoint. The later Navigator integration checkpoint below records production enablement.


## Unicode renderer acceptance

Parent records, deterministic layout, Unicode drawing and Navigator integration are implemented. The integration checkpoint below supersedes the isolated-renderer statuses: Unicode is the sole production path. User terminal approval remains open; existing ASCII evidence is historical.

| Gate | Required evidence | Status |
| --- | --- | --- |
| Parent records | Ordered parent IDs, full hash validation, unresolved parents retained across prefix expansion, and shallow-history semantics | Complete; see parent-record checkpoint below |
| Rendered topology | Correct displayed connections for linear, parallel, ordinary/octopus merge, crossing, disconnected-root, shallow and detached histories | Pass; native Git input and integrated PTY checks also pass |
| Loading boundaries | Explicit unresolved ancestry; no false joins, lost parents or extensions of true roots; recorded prototype/library counterexamples pass | Pass for isolated renderer |
| Prefix and refresh stability | Completed rows and lane colours remain stable across 128/256/512 prefixes and unchanged refreshes; only unfinished boundary geometry resolves | Styled-prefix, repeated-render and integrated refresh checks pass |
| Approved appearance | Actual coloured multi-branch previews with rounded corners, clear nodes, compact rows, labels, separate cursor/HEAD and readable selected commits | Actual wide/narrow Navigator captures inspected; user terminal approval pending |
| Small panes and clipping | Narrow/short panes, resizing and horizontal clipping preserve borders, styled connections and usable navigation | Integrated controller and terminal checks pass |
| Navigation and following | Commit-only Up/Down and paging; OID selection survives refresh/expansion; checkout changes seek HEAD; manual navigation stops following; `f` resumes it | Integrated controller and terminal checks pass |
| Cancellation and lifecycle | Layout/history cancellation, stale-result rejection, hiding/reopening, session handoff and terminal shutdown preserve existing guarantees | Cancellation/join, stale-result and PTY checks pass |
| Performance and idle | Cached navigation performs no layout/read work; large linear and genuinely wide graphs are measured; no periodic rendering or added monitoring processes | Integrated measurements and cache/idle checks recorded below |
| Enablement and release checks | Applicable automated/PTY checks and visual review pass; user documentation updated; applicable large-repository, idle and release checks rerun against Unicode | Pending |

No new graph-library dependency is planned. Worktree mutations, commit inspection, build/run configuration and terminal-image rendering remain outside this acceptance increment. Packaged-release/platform requirements remain separate and are not waived by renderer acceptance.


## Parent-aware history records (2026-11-16)

`GraphRow` now carries `parents: Vec<String>`. The existing log invocation includes `%P` between the commit ID and subject; no additional Git request or object lookup is introduced. Parent order is preserved, roots and connector-only rows have empty lists, and `oid` continues to distinguish commits from connectors. Parent IDs outside the requested prefix are retained. The parser accepts full 40- or 64-character hexadecimal IDs, requires parent widths to match the commit, and rejects malformed parent tokens or framing. Subject sanitization and coloured graph parsing are unchanged.

`tests/git_parents.rs` covers root/single/ordered octopus parent records, both hash lengths, connectors and malformed fields. Disposable real repositories compare parent lists against `git rev-list --parents` for ordinary and octopus merges, independent roots, detached HEAD outside branch refs and shallow clones. Shared commits retain parent lists across limits 1–4 and the full fixture. Displayed graph cells are compared against the old log format at the same limit; Git can vary boundary glyphs between short prefixes, so the test does not confuse native boundary differences with metadata regressions.

Synthetic history rows and the opt-in library adapter now include parent metadata. The application still renders its native-ASCII graph. This checkpoint completes the data prerequisite only; deterministic layout, Unicode topology, appearance and enablement remain pending.

Verification passed:

- Three parent-record tests, including the real-repository matrix.
- Thirty-one Git/discovery/history/watcher library tests.
- Two styled graph viewport tests and the opt-in library-output history adapter test.
- Git lifecycle PTY acceptance: navigation, session return, removal, cancellation and terminal restoration.
- Formatting, all-target Clippy with warnings denied, development build and `git diff --check`.

Subprocess review confirms `history_at` still performs one `read` of the existing `git log` command. `%P` supplies metadata in that same response. No dependency, configuration, watcher, persistent state or user-facing control was added. The next increment is deterministic lane-and-edge layout from these parent records.


## Deterministic lane model (2026-11-16)

`src/git_layout.rs` adds a pure, unused-by-the-UI model over child-before-parent commit records. It does not read Git, parse ASCII, spawn processes, or change selection/follow behavior. Each row records the node, incoming/outgoing active lanes, and distinct ordered parent connections. The result includes an OID-to-logical-row map, maximum column width and unresolved boundary lanes.

Lane columns never compact. First parents continue the node lane unless already active elsewhere; other new parents use the lowest vacant column, excluding the current node column until the following row. Existing targets share one lane while retaining distinct child/parent-index connections. Reused columns receive fresh identities and the six-colour palette cycles by identity. Roots have no outgoing parent edges. Unknown parents remain explicit boundary targets. Duplicate commits, self-parenting, repeated parents and parents already processed return typed errors before mutation.

Validation covers linear history, ordinary/octopus merges, crossing parent orders, disconnected roots, unloaded parents, and the parallel/interleaved fixture shapes from the earlier probes. Every prefix is compared against both incremental append and the corresponding rows of a full build. Tests verify straight unrelated lanes, target uniqueness, ordered edge identity, reserved columns, colour continuity/cycling and atomic errors. The real Git fixture checks also feed the model, including detached and shallow histories, comparing parent edges against `git rev-list --parents`.

Local release probe, median of seven runs (not a CI timing assertion):

| Commits | Maximum lanes | Layout time | Row representation bytes |
| --- | --- | --- | --- |
| 128 | 4 | 0.043 ms | 59,968 |
| 512 | 4 | 0.200 ms | 244,288 |
| 2,048 | 4 | 0.927 ms | 981,568 |
| 10,000 | 4 | 4.757 ms | 4,798,528 |
| 2,048 | 128 | 7.227 ms | 19,982,336 |

The byte count includes row structs and populated lane/edge entries, excluding string allocations, maps, spare capacity and retained builder state; it is not total heap or RSS. Active-lane snapshots cost O(commits × active width), so wide-history memory remains a consideration for integration. All measured cases met the initial local 10 ms target. Reproduce with `cargo +stable test --release --test git_layout layout_cost_probe -- --ignored --nocapture`.

Checks: focused lane and parent-record tests, formatting and all-target Clippy with warnings denied. This completes the logical model only. Unicode glyph routing, connector rows, visible crossing correctness, clipping and rendered-prefix acceptance remain pending. Navigator caching, cancellation and obsolete-result rejection must be integrated before enablement; the current native graph remains visible.


## Unicode renderer (2026-11-16)

`git_render::render(&Layout)` returns typed coloured `GraphSpan` rows, optional commit OIDs, a rendered-row position map, terminal-cell width and separate replaceable boundary decoration. It consumes the model directly and performs no Git reads, terminal access or UI mutations. The production Navigator still uses its existing native graph.

Lane pitch is four cells. Linear continuations use one row per commit. Horizontal routes receive separate connector rows in parent order; fresh lanes opened by the same commit may share forks, removing avoidable octopus crossings. Pre-existing lanes are never used as routing forks because that could invent ancestry for other children. Unrelated intervening lanes remain straight and use a bridge, with independent vertical/horizontal paths. Directed arrows make incoming joins distinct from outgoing forks. Root separation rows prevent a later reused column from visually connecting unrelated commits. `┊` marks unloaded ancestry outside the immutable completed blocks.

The initially proposed `⏜` passed Unicode width checks but visibly overhung its cell with local font fallback. Visual validation selected the smaller `⌢` arc instead; its width test passes and the rasterized preview fits. Bridge geometry and font fallback still require actual terminal review at integration. All other glyphs are tested as single-cell characters.

### Evidence and previews

- The independent test oracle traces final glyphs and rendered commit positions, not routing metadata. It checks physical connector ports and exact reachable parent IDs, treating bridge axes separately and respecting horizontal arrows. Unloaded endpoints are checked against the independently validated lane model's boundary targets.
- The shared feasibility fixture shapes pass at every prefix: empty, linear, ordinary merge, four branches, parallel branches, octopus, crossing, disconnected and detached. The real Git fixture passes for ordinary/octopus, disconnected, detached and shallow histories, with parent expectations compared against `git rev-list --parents`.
- Another 128 deterministic generated DAGs pass at four prefixes each, including reversed parent orders and unloaded targets. Styled rows/positions remain stable across small prefixes and 128/256/512 expansions. Repeated renders match. Negative controls reject missing strokes, a crossing replaced with a junction and incorrect commit mappings.
- Compactness checks retain one row per linear commit plus root separation, no bridge in the octopus example, and explicit gaps between disconnected roots. Preview heights: 9 parallel commits → 13 rows; 6 octopus commits → 13 rows; 6 crossing commits → 12 rows. Complex merges intentionally use extra height rather than ambiguous overlapping routes.

[Four parallel branches](fixtures/git-renderer/parallel-four.png), [octopus merge](fixtures/git-renderer/octopus.png), [crossing](fixtures/git-renderer/crossing.png), and [unloaded ancestry](fixtures/git-renderer/unresolved.png) are actual renderer output. Their example labels, HEAD marker, cursor and selection background are preview composition, not screenshots of Navigator integration. SVG, ANSI and text versions accompany each PNG. The previews were rasterized and visually inspected; approval by the user remains open.

Reproduce from this checkout:

```sh
cargo +stable test --test git_layout --test git_render --test git_parents
cargo +stable run --example git_graph_preview -- target/git-graph-preview
cargo +stable test --release --test git_render rendering_cost_probe -- --ignored --nocapture
cargo +stable fmt --check
cargo +stable clippy --all-targets -- -D warnings
```

The generator writes only to its requested output directory (default `target/git-graph-preview`); it creates no Git repositories, worktrees or background processes. SVG/ANSI generation has no new dependency. Optional PNG conversion uses the locally installed `rsvg-convert`.

### Cost and remaining work

Rendering-only medians over seven release runs, excluding model construction:

| Commits | Lanes | Output rows | Width (cells) | Time | Row representation bytes |
| --- | --- | --- | --- | --- | --- |
| 128 | 4 | 132 | 13 | 0.047 ms | 36,355 |
| 512 | 4 | 516 | 13 | 0.189 ms | 145,795 |
| 2,048 | 4 | 2,052 | 13 | 0.763 ms | 583,555 |
| 10,000 | 4 | 10,004 | 13 | 4.045 ms | 2,849,875 |
| 2,048 | 128 | 2,176 | 509 | 16.708 ms | 17,865,187 |

The representation count includes row/span structs and text lengths; it excludes spare capacity, OIDs, maps, model memory and allocator overhead. It is not RSS. The wide case exceeds the earlier aspirational 10 ms target and compounds the model's own memory cost. This is evidence for cached worker-side construction with cancellation during integration, not permission to recompute in the input/draw loop. No timing threshold was added to CI.

Focused tests (14 passed, two manual cost probes ignored), formatting, all-target Clippy and diff whitespace checks pass. This completes isolated drawing, not enablement. Next: adapt these rows into History while retaining metadata/references, wire cached computation with cancellation and stale-result rejection, verify clipping and every navigation/follow behavior, and review the actual terminal UI before changing the default renderer. Existing production code paths and key bindings are unchanged.


## Navigator Unicode integration (2026-11-16)

Unicode is now the sole production history renderer. `git log` returns commit records without `--graph` or ANSI colour output. Ordered parent IDs feed the existing deterministic layout and Unicode drawer in a bounded blocking task. The completed snapshot owns rendered spans, commit metadata, OID positions, selectable indices and width. Viewport padding is presentation-only, preserving completed graph geometry across prefix expansion.

Cancellation is cooperative between commits, parent edges and wide routing segments. History invalidation cancels and joins construction before replacement; normal hide, repository switch, handoff and exit use the same cleanup. Request tokens reject obsolete completion. Ref/HEAD-only refreshes compare commit records and reuse geometry; navigation uses cached selectable indices and never constructs a graph. Construction errors retain prior same-repository data with the existing retry presentation. No renderer setting, fallback, new dependency, periodic rendering or monitoring process was introduced.

Shallow history is explicitly covered: formatted Git parents may omit ancestry at shallow boundaries. When a shallow repository returns parentless records, one cancellable `cat-file --batch` read obtains their raw ordered parents. These remain unresolved endpoints rather than invented roots. Real shallow-clone tests compare against raw commit objects. Normal history never performs per-commit Git subprocess reads.

### Integrated validation

- Parent parsing and raw shallow ancestry; independent rendered-topology oracle and generated DAGs; stable prefixes; cancellation during wide routing.
- Thirteen history-controller checks, including no reconstruction during navigation/resize/idle or identical refresh, shutdown joining construction, stale result rejection, commit-only movement, reference caching, refresh selection and HEAD following.
- Full Rust/Python verifier, Clippy with warnings denied, formatting and Linux PTY checks. The verifier includes watcher changes, hidden history, session handoff, cancellation/reaping, resizing and four idle scenarios.
- Native release and static x86_64 musl builds; static release smoke including Git lifecycle, watcher and idle checks. The existing runtime-notice packaging requirements remain unresolved; no packaging checks were bypassed. ARM64 evidence above predates this renderer integration and is not fresh certification of it.

The old library history-adapter test is retained only as an archival experiment source in `scripts/experiments/gleisbau-probe/history_adapter_test.rs.txt`; it is no longer compiled against the production history API. The ASCII parser/colour helpers and native graph generation are removed from application code.

### Actual Navigator captures

[Wide history pane](fixtures/git-renderer/navigator/wide.png) and [narrow history pane](fixtures/git-renderer/navigator/narrow.png) are reconstructed from an actual PTY run of the integrated release binary using the lifecycle presentation fixture, not composed renderer examples. Raw ANSI streams, screen text and SVG accompany them. The capture runs with `NO_COLOR` unset; inherited `NO_COLOR=1` correctly produces monochrome output. Captures show rounded merge connections, coloured lanes, HEAD and cursor distinction, reference labels and clipped Unicode subjects. They have been visually inspected here; user approval in their own terminal remains open, particularly font fallback for the crossing bridge.

### Large-repository measurements

The explicit release probe used 30-second settled samples. These are measurements, not portable CI thresholds. All three settled scenarios emitted **zero Git commands and zero terminal output**; cached-construction tests separately assert no idle/layout work. The blocked expansion process was cancelled and reaped.

| Scenario | Initial display | Expansion | Peak process RSS | Settled Tracr CPU |
| --- | --- | --- | --- | --- |
| linear (30,000 commits) | 222.42 ms | 182.61 ms | 10,612 KiB | 0.033% |
| wide (1,089 commits) | 177.52 ms | 92.81 ms | 10,792 KiB | 0.000% |
| old-head (30,000 commits) | 1291.14 ms | — | 75,700 KiB | 0.000% |

Raw evidence: [acceptance JSON](fixtures/git-renderer/navigator/acceptance.json). Reproduce with `python3 scripts/git_acceptance.py target/release/tracr --output /tmp/unicode-acceptance.json`.

The earlier ASCII sample located the old checkout HEAD in about 870 ms with 35,260 KiB peak RSS; this Unicode run took about 1.29 seconds with 75,700 KiB. Typical initial/expanded views remain around 11 MiB, and settled CPU stays low, but loading the full 30,000-commit prefix has a real transient memory and latency cost. This is a recorded limitation, not a claim of performance parity. Full-prefix reconstruction remains the deliberate initial implementation; any future memory optimization should target these measurements.

Verification logs: `/tmp/tracr-check-ef_chrpw` (all stages passed), `/tmp/unicode-history-refresh.log`, `/tmp/unicode-parent-final.log`, `/tmp/unicode-clippy-final.log`, `/tmp/unicode-final-lifecycle.log`, `/tmp/unicode-final-watch.log` and `/tmp/unicode-musl-final.log`. The final refresh-invalidation change was followed by targeted controller, parser, Clippy and native release lifecycle/watcher checks; static release smoke was run on the integrated renderer build immediately before that change. Capture review is not a substitute for the user's actual terminal approval.
