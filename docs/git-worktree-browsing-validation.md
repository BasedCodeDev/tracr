# Git worktree browsing validation

## Current implementation

The browser groups registered worktrees under their project while preserving real
launch paths and session identity. Discovery handles nested repositories,
submodules, independent clones and bare repositories. Incomplete/stale ownership
remains visible; unrelated requests cannot block repository retirement.

Unicode is the production history renderer. Ordered commit parents feed a
deterministic lane model and typed coloured spans in a bounded worker. Completed
snapshots cache geometry, commit metadata, OID positions and selectable indices.
Navigation and resize do not rebuild the graph. Prefix expansion retains completed
geometry; unresolved ancestry has explicit boundary decoration. Shallow parentless
records use a cancellable batched raw-object read to distinguish missing ancestry
from true roots.

History supports progressive expansion, stable OID selection, horizontal scrolling,
reference labels, checkout-HEAD following and explicit refresh. Cancellation joins
construction and Git children before replacement, session handoff or exit. Tokens
reject obsolete results; errors retain same-repository data with retry presentation.
Reference-only refresh can reuse geometry. Native metadata watchers monitor refs,
HEAD and worktree registration rather than recursively watching source/object trees.
Watch errors remain visible; checkpoint/manual refresh retries without periodic
polling. Hidden history defers graph reads.

## Renderer decision and limitations

Gleisbau 0.7.5 was rejected: the final experimental corpus had 24 failing renders out
of 50 and seven unstable prefix transitions. Partial histories could imply incorrect
ancestry; full-history successes did not repair prefix correctness. Its independent
experimental crate, adapters and generated captures have been removed. Production
layout/rendering tests and the independent topology oracle remain authoritative.

The selected renderer uses directed joins, a `⌢` crossing bridge, explicit root
separation and `┊` unresolved ancestry. Terminal/font fallback still merits user
review. Complex merges intentionally use extra rows rather than ambiguous routes.
Full-prefix reconstruction has a measurable cost: a historical 30,000-commit old-HEAD
fixture took about 1.29 seconds and 75,700 KiB peak RSS, versus roughly 0.87 seconds
and 35,260 KiB for the earlier ASCII implementation. These local observations are
not CI thresholds. Wide-history memory and old-HEAD latency remain optimization
work; settled historical samples emitted zero Git commands and terminal output.

## Coverage and reproduction

Production tests cover ordered parents, shallow boundaries, exact rendered ancestry,
negative topology controls, deterministic generated DAGs, stable prefixes, wide
routing cancellation, cached navigation, stale replies and HEAD following. Real Git
fixtures cover worktree registration/removal, detached/shallow history and repository
ownership. PTY checks cover watcher changes, session handoff, resize, blocked-read
cancellation/reaping, hidden history and idle behavior.

```sh
./scripts/check.sh --offline
cargo +stable test --test git_layout --test git_render --test git_parents
cargo +stable run --example git_graph_preview -- target/git-graph-preview
cargo +stable test --release --test git_layout layout_cost_probe -- --ignored --nocapture
cargo +stable test --release --test git_render rendering_cost_probe -- --ignored --nocapture
# Explicit large-fixture acceptance, separate from normal checks:
python3 scripts/git_acceptance.py target/release/tracr --output target/git-acceptance.json
```

Preview SVG/ANSI/text comes from the actual renderer with illustrative labels;
it is not a live browser screenshot. PNG conversion is optional with `rsvg-convert`.
Keep generated previews under `target/`, not in tracked docs. Large-fixture acceptance
uses 30,000 linear commits and 64 branches, checks selection/prefix continuity,
old-HEAD search and cancellation, and records timings, RSS and settled activity.

No historical development run certifies current packaged assets or another platform.
Native ARM64, runtime-license packaging prerequisites and broader real-provider/font
review require their own release checks. See [releasing](releasing.md) and the
[actionable backlog](../PLAN.md).


## History scroll indicators (2026-11-16)

The history borders now show bold cyan arrows at the right edge: `↑` for off-screen commits above, `↓` for off-screen commits below or a prefix whose end is not yet confirmed. The computation reads cached selectable indices and the final visible range; it performs no traversal, Git reads or background scheduling. The title reserves space for the upper marker without reducing graph row capacity.

Validation passed: all 15 history-controller/rendering tests, all-target Clippy with warnings denied, formatting and whitespace checks, and the native Git lifecycle, watcher and four-scenario idle PTY helpers. Added assertions cover boundary transitions, empty/initial loading, unknown exhaustion, retained errors, connector-only tails, middle/older-HEAD views, long Unicode titles, focused/unfocused styling, widths 140/100/40, resizing and clearing markers. Cached snapshot construction and selected commit remain unchanged by rendering indicators. Idle views issued no commands or redraw output.

Logs: `/tmp/tracr-arrows-tests.log`, `/tmp/tracr-arrows-clippy.log`, `/tmp/tracr-arrows-lifecycle.log`, `/tmp/tracr-arrows-watch.log`, `/tmp/tracr-arrows-idle.log`.

## Commits above the viewport (2026-11-21)

The initial counter implementation showed the number of real commits strictly above the visible
viewport, immediately left of the upward arrow. At the beginning it shows `0`
without an arrow. Session annotations, routing rows and stash entries do not
contribute. The count refers to the start of displayed history, not checkout HEAD
or the number of navigation keystrokes. Empty history has no counter.

Composed commit positions are cached with the presentation and counted with a
binary search after viewport clamping. Rendering adds no Git reads or history
expansion. The title reserves the counter's width and preserves border corners.

A one-column visual scrollbar was prototyped using only cached row counts.
Alternating release-mode runs of `cached_scroll_render_cost` against a 30,000-row
history measured median times of 155.99 ms per 1,000 frames for the original arrows
and 158.40 ms for the counter plus scrollbar (32 warmed samples each). These are
local TestBackend measurements, not terminal latency guarantees. Because the
combined prototype was consistently slower, the initial implementation retained
the arrows, as requested at that stage. The manual ignored test remains available for future
comparisons; timing is not a CI threshold.

Validation: 189 library tests passed (three manual tests ignored), all-target
Clippy with warnings denied, formatting and whitespace checks passed. The four
terminal idle scenarios issued no Git commands or redraw output. Test scratch
directories were confined to `.tmp/`, with `GIT_CEILING_DIRECTORIES` preventing
negative-discovery fixtures from inheriting this checkout and the stale inherited
`TRACR_DEV_CONTROL` removed from terminal-test environments.

The supplemental `git_sessions_check.py` terminal script fails while searching
for “Select worktree folder”; the same failure was reproduced on the unmodified
base binary. This pre-existing fixture/action mismatch is outside the counter
change. Library session-navigation tests pass.

## Shared thin scrollbars (2026-11-21)

The expanded scope adds the application's shared visual scrollbar to Git,
retaining its border arrows and exact commit counter on the top border. The
track and thumb use the same quarter-cell `🮇` glyph, colored `#383838` and
`#707070` respectively, without bold styling.
Scrollbars appear only when loaded content exceeds the viewport. Git proportions
refer to the loaded prefix; computing them never queries total Git history.

The same helper now covers folders, sessions, search results, worktree/harness/path
pickers, Actions, the open-terminal switcher, information and commit dialogs,
relocation previews and recovery dialogs. Tracks occupy the column immediately inside the right border alongside
scrolling bodies where available, preserving the frame and reserving text space. Independent unbordered bodies reserve a column
on overflow and rewrap at the reduced width. Fixed summaries and embedded terminal
screens do not acquire scrollbars. Keyboard navigation and terminal geometry are
unchanged. Multiline lists use rendered-line offsets after the widget updates its
viewport, rather than the selected item index.

The revised performance requirement permits imperceptible rendering overhead.
Alternating warmed release runs with the counter present in **both** versions
measured 156.04 ms per 1,000 frames for counter-plus-arrows and 157.91 ms for
counter-plus-scrollbar (32 samples each, 30,000 cached Git rows). The 95th-percentile
batch times were 159.91 and 159.83 ms respectively. Median incremental cost was
about 1.9 microseconds per frame. The separate `large_widget_render_cost` probe,
including construction of 10,000 multiline list entries and a long wrapped Unicode
paragraph, rendered both regions together in roughly 6.6–6.8 ms per frame. These
are local TestBackend observations, not hardware-independent thresholds.

Validation passed: 194 library tests, all-target Clippy with warnings denied,
formatting and whitespace checks; terminal idle, scrollback/input isolation,
information-dialog and worktree-creation scripts. Tests cover scrollbar endpoints,
tiny tracks, shrinking content, Unicode wrapping, multiline offsets, folder/session
selection, Git counter preservation and resizing. Idle scenarios emitted no Git
commands or redraw output. The supplemental Git-session script still has the
previously reproduced baseline failure searching for “Select worktree folder”.
Logs and benchmark samples are in this worktree's ignored `.tmp/uniform-*` files.

### Scrollbar visibility correction

The thumb now uses a solid half-cell `▐`, so it differs visibly from the thin `│`
track even when foreground colors are difficult to distinguish. Its terminal-column
allocation is unchanged. Git also retains its upper/lower border arrows alongside
the scrollbar, with the commit counter immediately left of the upper arrow.
The lower arrow still indicates unconfirmed/unloaded history at the loaded edge.

The scrollbar is positioned one column left of the outer right border. Bordered
lists reserve or reuse right padding so long text cannot extend beneath the track;
Git and folder horizontal clipping use the reduced content width. Unbordered
scrolling bodies retain their existing reserved scrollbar column. All 194 library
tests, Clippy, formatting and the build passed after this placement correction.

### Fine graphite styling

Design A uses three seamlessly aligned quarter-cell rectangles (`🮇`): graphite
`#383838` above and below, and a slightly brighter `#707070` thumb. The shared
renderer uses explicit RGB grays and removes bold styling. Placement, proportions,
Git arrows/counter and reserved content space are unchanged. Rendering tests assert
the exact glyph, both gray colors and absence of bold, including viewport endpoints.

The graphite segments use U+1FB87 RIGHT ONE QUARTER BLOCK (`🮇`). This moves the
unused portion of the reserved cell to the text side without changing text width,
scrollbar proportions or the surrounding border. The glyph's single-column width
is asserted by the rendering test.
