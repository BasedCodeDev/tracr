# Git history and logical worktree browsing

## Agreed scope

Linux-first, read-only Git integration. Keep two upper panes (50% Folders, 50% Sessions at normal terminal widths) and add a full-width bottom graph occupying approximately one-third of content height. Worktrees appear directly beneath the expanded repository in the navigator, regardless of physical location, with a `↳` prefix and one level of indentation. Labels show the branch name only, such as `↳ feature/login`; detached worktrees show `↳ detached HEAD` with a seven-character commit ID when available. Actual filesystem paths remain the navigation and launch identity. There is no synthetic Worktrees grouping row or separate group expansion state. Worktrees precede ordinary project subfolders; each worktree can expand to show its own session-containing folders. Counts describe sessions, and stale/availability indicators belong on the individual worktree rows. Only suppress physical entries after ownership is successfully established. Bare repositories own linked checkouts without a main checkout; independent clones remain independent.

Selecting a checkout root shows all its sessions, excluding nested repositories and other worktrees. Selecting a subfolder shows exact-folder sessions. Expand worktrees to expose session-containing folders. Remember selection per context. Resume and create always use actual paths. Direct launch, restored paths, and search results reveal logical entries without losing the intended session.

## History

Use native Git ASCII graph output with coloured lanes, branch/tag labels, selected checkout HEAD, abbreviated object ID and subject. No details or diffs in this slice. Preserve connector-only rows but select commits only. Follow HEAD until manual navigation; selecting a checkout or pressing f resumes following.

Read on demand using expanding prefixes, never concatenate independent --skip pages. Start with a viewport buffer, double as the user approaches the boundary, preserve selection by full object ID, and extend to find old checkout HEADs. Run reads in background tasks with cancellation and generation checks. Render only visible rows.

## Controls

Left/Right switch upper panes. Tab/Shift-Tab toggle graph and prior upper focus. g shows/hides history; Escape returns to upper focus. Up/Down and j/k select commits. PageUp/PageDown move a viewport; Home/End select newest/oldest loaded commits. Left/Right within graph scroll horizontally. r refreshes sessions and Git. Existing modal bindings retain priority. Narrow terminals retain focused-upper-pane behavior.

## Automatic updates

Use one notify watcher with subscriptions for relevant active-repository shared refs, per-checkout HEADs, and worktree registration. Avoid source-tree and object-store recursive watches. Debounce changes for 200ms with a one-second maximum delay. Reconcile on repository entry, return from sessions, and graph reopening. No default periodic Git polling. Native watching is the first increment. Polling fallback and its configuration are deferred by explicit decision. Unsupported or silently unreliable filesystems may remain stale until manual/checkpoint refresh. Watcher failures show a persistent non-modal warning; retries occur at those checkpoints, not on a timer.

## Failure behavior

Keep session browsing usable. Preserve stale graphs with an explanation after failures. Missing worktrees remain labelled; unresolved ownership retains physical access. Empty repositories show an empty state. No repairs or network fetching. Disable filesystem moves of synthetic nodes and worktree/repository roots requiring Git-aware repair.

## Implementation order

1. Read-only Git command layer, discovery, inventory and parser fixtures.
2. Typed logical navigator and session scopes, ownership projection and deduplication.
3. Graph rendering, controls, prefix loading and HEAD following.
4. Metadata watching, fallback configuration, failure presentation.
5. Automated, terminal and release validation; README updates.

## Validation

Cover nested and bare repositories, unusual paths, detached and missing worktrees, direct entry and search routing, all-session versus exact-folder scope, counts and activity, stable selection, merge connector continuity, old HEAD loading, external updates, watcher failures, cancellation, small terminals and unchanged Git metadata. Reproduce the 30,000-commit benchmark and a wide-branch fixture. Run scripts/check.sh and interactive acceptance.

Research with Git 2.56.0 found --skip pages changed lanes at 9/11 tested boundaries; expanding prefixes preserved output at 11/11. Single-run synthetic 30,000-commit measurements ranged from 53ms for 100 requested commits to 72ms for all commits. These are feasibility observations, not performance guarantees.

## Backlog

Worktree creation/removal/repair, merging, rebasing, fetch/push, project build/run configuration, process management, commit inspection and diffs, custom/image graph rendering, and full Windows/macOS application support.

## Implementation status

Development is in `/tmp/tracr-git-browser` on `feature/git-worktree-browser`. Logical worktree browsing, discovery lifecycle acceptance, branch-only navigation labels, folder viewport scrolling, progressive history loading, and checkout HEAD following are implemented. Reference labels, graph colours and horizontal graph scrolling are implemented. Native metadata watching is implemented; polling fallback and broader release acceptance remain unfinished. The checkpoints below record implementation history.

### Discovery implementation checkpoint

A two-job discovery controller now seeds from selected/visible folders, all session directories, and registered worktrees. Outcomes are cached per browser generation and distinguish ordinary directories from failures. Checkout-root resolution now excludes successfully discovered nested repositories from parent aggregates. Graph reads are separate and keyed by common Git directory; errors are no longer silently discarded. Explicit task cancellation runs on shutdown and terminal handoff.

Validation: library and browsing tests pass (24 library plus 30 browsing, one ignored), and the added controller concurrency/negative-cache test passes. Remaining acceptance gaps include real nested-repository integration fixtures, stale inventory presentation/removal, aggregate count/activity reconciliation, and pending-membership UI. This increment is not yet complete.

### Refresh/accounting checkpoint

Ownership now has a Git generation, pending/current outcome, and previous successful outcome. Refresh marks prior ownership stale, failures retain previous membership, and obsolete-generation updates are rejected. The implicit ancestor-prefix ownership fallback was removed. Counts and activity now share a logical-subtree membership calculation, including external worktree sessions beneath project ancestors. Checkout lists expose incomplete/stale scope in their title.

Focused ownership-refresh and logical-hierarchy tests pass, along with all-target Clippy. This checkpoint remains incomplete: inventory requests still need deduplication by repository, authoritative removal of obsolete repositories/ownership needs reconciliation, totals are not cached, and real nested/submodule fixtures plus terminal acceptance remain outstanding.

### Authoritative inventory checkpoint

Directory resolution no longer retrieves a full inventory. The controller deduplicates inventory reads by common Git directory, shares the two-job limit across both operations, and tags updates with their originating generation. Ownership references directory/repository identity rather than embedding inventories. Successful inventories remove obsolete registrations and invalidate previous ownership for removed checkout roots; affected directories are rediscovered. Failed inventory refresh retains the previous list with a stale label.

A real temporary Git test verifies one inventory for multiple checkout/subfolder probes and registration removal on refresh. Full repository retirement, selected-node fallback when no node survives, controlled completion-order/failure tests, and terminal acceptance remain unfinished. This checkpoint is not the completed increment.

### Retirement checkpoint

Repository candidate tracking now permits retirement only after all associated paths have conclusive current-generation results; unavailable and pending records retain stale inventories. Retired IDs reject late inventory publication within that generation. Cancellation releases unfinished inventory reservations. Inventory updates preserve the pre-batch selected session and recover navigation through its captured logical ancestor chain, without unconditional ancestor expansion on each successful inventory.

Library, browsing and folder-move regressions pass; the new controlled retirement test covers pending/unavailable results and late inventory rejection. Clippy passes. The full controlled cancellation/completion-order matrix and feature-specific terminal walkthrough have not been completed; this remains an implementation checkpoint, not completed terminal acceptance.

### Lifecycle matrix checkpoint

Discovery now accepts an injected backend and tags updates with generation/request tokens. Forgetting or cancelling invalidates queued results. Batch application is extracted from the terminal loop. Channel-controlled tests verify directory cancellation/restart, same-generation queued-result invalidation, and generation replacement. See git-worktree-browsing-validation.md for passed and outstanding matrix cases; the completion gate remains open.


### Lifecycle validation update

Inventory cancellation/retry and queued-result invalidation now have controlled coverage. Retirement no longer waits for unrelated directory probes; pending inventory reads are considered per repository, while associated unresolved ownership still prevents retirement. Tests also cover both directory/inventory completion orders, stale failure/recovery on a rendered screen, surviving physical-folder selection, and removed-group ancestor fallback. The standard verifier passed before this final refinement; focused regression tests and all-target Clippy passed afterward. Remaining acceptance is listed in the validation matrix.


### Lifecycle acceptance complete

The lifecycle increment is complete. Mixed checkout failure/confirmation now has coverage in both orders; rendered refresh tests preserve collapsed groups, selected session, folder and focus. Discovery and history requests are cancelled and joined at terminal boundaries. Git subprocess workers kill and reap cancelled commands, and those workers are drained before session launch or exit. This corrects the process-reaping gap found by the new PTY acceptance test.

The registered `git_lifecycle_check.py` verifies logical navigation, actual worktree launch directory, return after external registration removal, and all twelve combinations of pending directory/inventory/history reads with quit, Ctrl-C, SIGTERM and session handoff. It verifies process exit before handoff, retries after return and terminal restoration.

Final verification: `./scripts/check.sh --offline` passed every stage (logs `/tmp/tracr-check-yvm5v2sa`); `git diff --check` passed. The earlier incomplete lifecycle checkpoints above are historical and are superseded by this result. The broader feature remains unfinished; progressive history loading is the next increment.


### Direct worktree rows

The synthetic Worktrees row and its collapse state have been removed. Project expansion now controls visibility of direct `↳` worktree children; existing checkout expansion controls their descendants. Counts, branch names and availability remain on each checkout, and inventory staleness is displayed there too. Navigation recovery goes directly to the project when a removed checkout has no surviving physical entry. Earlier references to groups above describe the superseded layout.

Validation for direct rows: the full offline verifier passed, including updated navigation, collapse, stale rendering, missing-folder recovery and Git lifecycle terminal checks (logs `/tmp/tracr-check-29slr3m3`).


### Branch labels and horizontal folder viewport

Linked worktree roots use the Sessions — worktree title, main checkouts use Sessions — checkout, and subfolders use Sessions — folder. Existing incomplete/stale membership suffixes are preserved. The folder pane now receives 50% of the upper width and Sessions 50%; narrow terminals keep the focused-pane layout.

When the selected folder row overflows, all folder rows share a horizontal offset that reveals the selected row’s right edge, including indicators and count. Borders, title and selection marker remain fixed. Selecting a row that fits resets the offset to zero. Resizing or changing labels recomputes the offset during rendering. Grapheme-aware clipping preserves combining characters and replaces partial wide glyphs with padding so all rows remain aligned. This adds no persisted state or user configuration.

Validation: full offline verifier passed (logs `/tmp/tracr-check-dyaajf1f`), including the branch-only Git terminal walkthrough. The added exact 50/50 pane-boundary assertions passed afterward in the focused viewport test.


## Progressive history loading slice

Replace the fixed 128-commit limit with demand-driven prefixes. Initial requests use at least 128 commits and four graph viewports; navigation within the final viewport arms one doubled request. Each response replaces the entire graph prefix, never concatenating `--skip` pages. Connector rows remain visible but are excluded from commit selection and exhaustion accounting. Returning fewer commits than requested marks the end; exact-limit responses permit another expansion.

`git_history::History` owns the active repository, refresh generation, request token, single in-flight task, prefix limit, error/end states, selection and viewport. Its backend is injectable for deterministic tests. Selection is preserved by full commit ID, with nearest surviving old commit fallback (older wins ties), then the first commit. Replacement preserves the selected row's screen offset where possible. Expansion completion never arms another expansion.

Up/Down and j/k select commits; PageUp/PageDown move by visible rows and skip connectors; Home/End select the newest/oldest loaded commit. End can arm one expansion. Refresh and help remain accessible with graph focus. Rendering constructs only visible graph items and shows initial loading, loading more, empty, exhausted and retry/error states. Failed reads preserve existing rows; `r` retries the requested prefix without an automatic loop.

Hidden/too-small graph panes pause reads and cancel pending requests. Reopening refreshes the retained prefix. Repository changes discard the previous repository's rows. Terminal handoff and exit join history requests and retain the existing Git worker drain/reaping contract. No polling, persistent settings or new external dependency is added.

Acceptance includes controlled sizing/doubling/deduplication/exhaustion/failure/cancellation tests, selection and connector navigation, rendered loading/error/empty states and resizing, a real merge graph spanning 128 commits, and PTY navigation beyond 128 commits with refresh preservation and exit during expansion. Checkout HEAD following, reference labels, colours, horizontal graph scrolling and metadata watchers remain later increments.


### Layout confirmation during progressive history work

The folder/session split remains 50/50, as confirmed during implementation. Sessions have two columns of right padding for list content and empty states, matching the selection-marker space on the left. On exceptionally narrow panes the margin shrinks enough to preserve eight columns for essential status labels. Existing top padding is retained. History request sizing shares the rendered content-height calculation, including pane padding.

Progressive history slice completed and validated: `./scripts/check.sh --offline` passed all stages (logs `/tmp/tracr-check-_fqq56u0`), including loading beyond 128 commits, stable refresh selection, cancellation during expansion and Sessions margin checks. `git diff --check` passed. Checkout HEAD following and reference presentation remain the next history work.


## Selected checkout HEAD slice

History context now carries the selected checkout identity independently of the common Git directory. Subfolders use their discovered checkout root; unresolved subfolder discovery retains the existing context until ownership is known. Bare roots have no checkout HEAD to follow. Read-only HEAD resolution distinguishes an attached branch, detached commit, unborn branch and unavailable metadata. The resolved commit is explicitly included alongside `--all` in each graph prefix.

Entering a checkout starts follow mode. Selecting a checkout root again or pressing `f` while the graph has focus resumes it. The heading identifies the branch or detached HEAD and following/manual state; `[HEAD]` marks the target commit independently of the selection cursor. If necessary, one prefix at a time doubles until HEAD is found. The heading shows locating HEAD during that search; exhausted or failed searches stop with an explanation.

Every manual graph movement stops following, even at a boundary, and cancels outstanding HEAD-search work while retaining loaded rows. Moving between subfolders does not rearm follow mode. Refresh and graph reopening preserve manual selection; in follow mode they resolve HEAD again and select it. Checkout changes abort and join prior metadata/history requests, including changes within the same repository. Hidden or undersized panes pause requests; terminal handoff and exit retain the Git worker reaping guarantees.

Unborn branches show no commits yet without searching for a HEAD. Other repository history may still be browsed. Failed HEAD reads retain same-checkout data with a stale-context error and retry only on refresh or `f`. There are no new watchers, polling, Git mutations or persisted follow settings. The 50/50 upper layout, Sessions right margin and branch-only worktree rows remain unchanged.

Acceptance covers controlled follow/manual transitions, same-checkout navigation, root reselection, refresh/reopen, prefix seeking, cancellation and stale results; real merge, unborn, bare and detached-without-named-ref fixtures; rendered HEAD markers distinct from selection; and terminal switching between a recent main checkout and a linked checkout whose HEAD lies beyond 128 commits. General reference labels, graph colours, horizontal graph scrolling and automatic metadata watching remain future work.

Selected checkout HEAD slice completed: full offline verifier passed (logs `/tmp/tracr-check-8ass6va9`), including old-HEAD location, detached/unborn/bare fixtures, manual refresh preservation, worktree switching and process cleanup. `git diff --check` passed.


## Graph presentation slice

Commit rows now show the native graph, abbreviated object ID, selected-checkout `[HEAD]`, local branches, remote-tracking branches, tags and subject. Label examples are `[main]`, `[remote: origin/main]`, and `[tag: v1.0]`. Types sort local/remote/tag, then alphabetically. Names retain their full short reference names. Annotated tags use the peeled targets from `git show-ref --dereference`; unrelated namespaces are omitted. HEAD remains tied to the selected checkout, independently of the reference snapshot.

Native Git emits graph colours with the explicit cyan/magenta/yellow/green/blue/red palette. The parser converts permitted colour sequences into typed graph spans and discards terminal control sequences; rendering never forwards escape bytes. Local references are green, remote references cyan, tags yellow and HEAD bold magenta, with text preserving meaning independently of colour. Commit selection remains bold and connector-only rows remain unselectable.

One reference snapshot is retained for the active repository. Context refreshes retrieve it asynchronously; expanding prefixes reuse it. Reads retain existing cancellation and token checks. Reference failures do not block graph reads: same-repository labels remain with a references-unavailable/stale warning, new repositories begin without labels, and successful refresh clears the warning. An empty reference set is valid.

Left/Right with graph focus scroll all graph content by four columns while title, borders and selection cursor remain fixed. Styled clipping is grapheme-aware and pads partially clipped wide glyphs. Horizontal scrolling stops HEAD following but does not move the selected commit or request more history. Offsets survive vertical movement, refresh and expansion, clamp on replacement/resizing, and reset on checkout/repository changes or `f`. Maximum row width is cached until graph, reference or HEAD data changes; only visible styled rows are built during rendering.

Acceptance includes typed/peeled reference parsing, colour/control-sequence handling, cache reuse and failure recovery, obsolete reference-result cancellation, reference ordering/style, Unicode viewport clipping, fixed selection markers and follow-mode transitions. A terminal merge fixture includes local and remote branches, lightweight and annotated tags, scrolling, refresh and return to HEAD. Metadata watching is the next feature increment; no polling, network operation, Git mutation or new configuration is added here.

Graph presentation slice completed: the full offline verifier passed all stages (logs `/tmp/tracr-check-zjlqf4v0`), including the merge/reference/scrolling terminal walkthrough. `git diff --check` passed. Metadata watching and broader release acceptance remain.

## Idle CPU correction before metadata watching

The htop command `/tmp/tracr-git-browser/...` identifies the running executable; it does not attribute CPU to a Git subsystem. Measure parent CPU and Git children separately before adding watchers. This increment addresses observed idle work without introducing a general event framework or a broad session cache.

Implemented changes:

- Retain Git discovery candidates until inventory, navigation, expansion, archive visibility or refresh context changes. Poll bounded task completions separately; settled discovery does not repeatedly rebuild candidates and scan cached results.
- Cache the checkout HEAD row when history or HEAD changes, avoiding a full graph search on every loop.
- Skip identical provider snapshots and redraw only after input, meaningful state changes, completed work, resize signals or a relative-time label deadline. Preserve existing asynchronous operations and their cancellation boundaries.
- Wait up to 250 ms when settled and 40 ms during pending work; keyboard input interrupts terminal polling immediately. Keep the runtime timer yield so asynchronous I/O remains serviced. Resize has a signal flag as well as terminal events.
- Recover folder navigation only when discovery changes the folder structure. Ordinary discovery must not move a deliberately selected missing folder.
- Add reproducible Linux CPU sampling and deterministic idle terminal checks. Numeric CPU percentages are evidence, not flaky CI thresholds. Existing modal, input, resize, live status, Git lifecycle and shutdown checks remain required.

Acceptance: compare debug and release builds using 1,000 fixture sessions, 301 commits and a linked checkout 200 commits behind HEAD. Sample settled plain-folder, visible-history, hidden-history and old-HEAD views for 30 seconds each. Record parent and reaped-child CPU, Git command counts, terminal output and input response. Require no repeated idle Git commands or unchanged-screen output; investigate any responsiveness or lifecycle regressions. Measurements and final validation are recorded in `git-worktree-browsing-validation.md`.

Metadata watching remains the next feature increment. No new background monitor, Git mutation, network operation or dependency is introduced by this correction.


## Native metadata watching slice

One `notify::RecommendedWatcher` serves the active common Git directory. Subscriptions cover directory entries for common and per-worktree metadata, recursive refs only, reftable directories, packed refs, HEAD, shallow boundaries and worktree registration. The common directory's parent detects replacement; callbacks ignore unrelated siblings. Source trees, object stores and logs are never recursively watched. Subscription reconciliation covers new or atomically replaced metadata directories. Both files and reftable reference storage are supported.

Callbacks perform no Git work. A bounded mailbox coalesces changes for 200 ms, capped at one second from the first event; active reads retain one follow-up batch. Access, metadata timestamp changes, index files and lock-file churn do not request refreshes. Each subscription context owns a fresh mailbox, so callbacks from previous contexts cannot publish into the current one. Rescan/overflow notifications request active-repository reconciliation. Subscription failure stops native watching, retains data and displays `Git auto-update unavailable — r to refresh` with diagnostic detail. Explicit refresh, graph reopening and repository re-entry retry setup; there is no automatic retry or polling loop.

Discovery invalidation is scoped to the active repository: reference events refresh inventory and visible history, while topology events additionally rediscover its known ownership paths. Unrelated discovery caches and session inventories remain current. Watch registration precedes a checkpoint reconciliation. Moving between folders within one repository retains subscriptions. Manual history selection and scrolling survive automatic refresh; follow mode follows updated HEAD. Hidden/small graph panes defer graph reads while inventory stays current.

Watchers are released on repository exit, session handoff and application exit. Shutdown removes subscriptions and waits for callback lifetime completion; Git cancellation/reaping retains its existing boundary. Returning from a session attaches and reconciles again. No new config is added. Polling fallback, automatic discovery of Git initialization in ordinary folders, and Git mutations remain deferred.

Acceptance adds bounded debounce/filter/failure tests, real files/reftable and bare fixtures, metadata replacement and directory creation, terminal external changes/manual selection/hidden history/project switching, handoff descriptor cleanup and the existing four-scenario idle CPU probe. See the validation document for measured results and final verification.
