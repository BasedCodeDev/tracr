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
