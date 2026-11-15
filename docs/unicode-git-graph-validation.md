# Coloured Unicode graph feasibility

The follow-up [compiled library validation and implementation decision](gleisbau-git-graph-validation.md) is complete. The report below records the first, ASCII-transformer experiment; its recommendation to evaluate the library has now been carried out.

## Decision — 2026-11-16

**Do not integrate this prototype. The small presentation-only renderer claim is not yet validated.** The agreed coloured, connected Unicode graph remains a first-slice requirement; this result does not defer its appearance again.

A bounded Python prototype retained Git's positions and converted local connections into rounded orthogonal strokes. Simple full graphs preserved parent relationships, but the renderer failed the boundary/complex-topology gates and the output is too vertically expanded. This is evidence about this candidate, not proof that all small custom renderers are infeasible. No application source, dependency, key binding, or default UI path was changed for this experiment.

Recommendation: evaluate gleisbau's generic track builder and graph-only rendering path next, using this fixture corpus as the acceptance baseline. Do not adopt its repository-owning legacy builder wholesale or extend the custom prototype into a new layout engine without a separate decision. A small attributed adaptation remains an option if the reusable portion proves substantially narrower than the crate.

## Reproduction and scope

Run from the feature checkout:

```sh
python3 scripts/experiments/unicode_graph_probe.py --output target/unicode-graph-probe
python3 -m unittest discover -s tests -p test_unicode_graph_probe.py -v
cargo +stable test --offline --lib git_history
cargo +stable test --offline --lib graph_viewport
```

The probe uses Python's standard library and installed Git. All fixture repositories live in one automatically cleaned temporary directory; output is retained only at the requested destination. Ambient Git routing/configuration is removed in the probe process. It never reads or modifies the development repository's Git history. No external provider is launched.

A zero probe exit means the experiment completed, **not** that the renderer passed. Individual outcomes and `accepted_for_integration: false` are explicit in the report. Harness crashes still fail the command. The experiment is deliberately not added as a renderer-acceptance stage in the release verifier; its harness tests are discoverable by the existing Python test stage.

Recorded evidence:

- [Machine-readable results](fixtures/unicode-graph-probe/results.json), including Python/platform/Git versions.
- [Actual four-branch prototype output](fixtures/unicode-graph-probe/parallel-four-prototype.svg). This is experimental output, not the approved design or a Tracer screenshot.
- Native counterexamples: [lane collapse](fixtures/unicode-graph-probe/four-lanes-native.txt), [criss-cross merge](fixtures/unicode-graph-probe/crossing-native.txt), [octopus merge](fixtures/unicode-graph-probe/octopus-native.txt).

## Method and results

The transformation receives coloured graph cells and commit IDs, never parent IDs. It infers connections from adjacent rows using the basic `* | / \` vocabulary. It expands coordinates onto an orthogonal grid to produce `│ ─ ╭ ╮ ╰ ╯` and junctions. Parent IDs come separately from `git rev-list --parents` and are used only by an oracle that traverses the rendered directed strokes to the next commit. The oracle rejects extra/missing loaded parents, dangling internal connectors, and missing open tails for unloaded ancestry. Four-way ambiguity and unsupported glyphs fail explicitly rather than drawing a plausible but unverified graph.

The small candidate contains 75 lines in the transformation function, 18 in its parser, and 31 in the independent oracle (including comments/blank lines). This is experimental Python, not a production Rust size estimate. It handles ordinary forks/joins and uses a doubled grid; it does not implement Git's full graph state machine. Unknown syntax is intentionally not patched over.

| Fixture | Full-history result | Prefixes tested |
|---|---|---:|
| Linear | Parent relationships pass | 4 |
| Ordinary merge | Both parents and common ancestor pass | 4 |
| Four branches with merge/collapse | Unsupported `_` routing | 11 |
| Four parallel branches | Parent relationships pass; appearance fails desired lane presentation | 9 |
| Octopus merge | Unsupported `-` and `.` routing | 6 |
| Criss-cross merges | Ambiguous crossing/junction rejected | 6 |
| Disconnected roots | Parent relationships pass | 4 |
| Detached HEAD outside named refs | Parent relationships pass | 4 |

Across all 48 fixture/prefix cases: **14 passed, 16 rejected missing boundary continuations, 18 unsupported**. All 16 oracle rejections were missing open tails, not an observed wrong loaded-parent connection. When Git ends a prefix on a commit, the graph text alone may omit its outgoing ancestry. Fixing that requires explicit continuation policy or parent metadata; blindly adding a line would incorrectly extend real roots. This is a specific gap in this candidate, not a claim that it requires a new layout engine.

The `_` counterexample crosses existing lanes while collapsing a merge. The criss-cross case shows why a four-way cell cannot automatically become `┼`: a visual crossing is not necessarily a topological join. Supporting these safely needs additional edge-routing identity beyond the candidate's local occupancy rules. Octopus graphs need further horizontal fan-out handling.

### Appearance and stability

The simple merge and four-branch SVGs were rasterized and visually inspected. Corners render, but full outputs occupy roughly 1.75–1.92 times the native rows and twice the horizontal coordinate spacing. The four parallel branches are serialized by Git's layout, not displayed as the approved four simultaneous long lanes. Native commit stars are often uncoloured; preserving raw colours leaves some nodes/stretches neutral and does not achieve the preview's consistent coloured branch appearance. The approved appearance therefore **has not passed**.

A 541-commit repeated-merge fixture was read at 128, 256 and 512 commits. Repeated unchanged reads matched exactly. Native and transformed completed rows, including colours, matched across both expansions when excluding the previous last commit and trailing boundary rows. The 128- and 512-commit boundaries still lacked an open continuation. This is limited positive stability evidence, not coverage of arbitrary crossing graphs or repository changes.

Cached viewport slicing is deterministic after horizontal/vertical movement and does not rerun transformation. Existing Ratatui tests pass for styled Unicode clipping and fixed selection. The experimental doubled-row layout was not installed into the UI, so new-renderer paging, selection styling, narrow-pane appearance and terminal font compatibility are **not certified**. Those integration checks are gated on topology and appearance passing; they are not silently counted as successes.

### Timing and memory

Seven transformation samples per size; reported time is the median without tracing. Memory is a separate `tracemalloc` run including returned grid/path/node dictionaries, excluding source rows. Git-read time includes subprocess and Python parsing and is one sample, not a pure Rust baseline. Recorded values are local feasibility measurements, not performance guarantees.

| Commits / shape | Git read + parse ms | Transformation ms | Retained bytes | Peak bytes |
|---|---:|---:|---:|---:|
| 128 linear | 17.394 | 0.226 | 78,032 | 168,552 |
| 512 linear | 17.377 | 1.006 | 624,016 | 985,032 |
| 2,048 linear | 21.429 | 4.497 | 2,412,816 | 4,401,096 |
| 10,000 linear | 32.588 | 37.303 | 11,025,728 | 21,305,592 |
| 512 across eight branch refs | — | 2.108 | 1,149,280 | 1,964,416 |

The initial 128-commit transformation meets the 10 ms exploratory target. Output cell count scales with input cells, and the algorithm visits each source cell with bounded local work; timings show allocation effects and are not perfectly proportional. The eight-ref graph is not equivalent to eight simultaneously occupied lanes. None of this establishes performance for the unsupported wide routing cases or a Rust implementation. A cached result can be reused for cursor movement; there is no background process.

### Existing behavior baseline

Ten existing history tests and two graph-viewport tests passed. They cover prefix loading and selection by OID, connector skipping, paging, old-HEAD seeking, worktree changes, stopping/resuming follow mode, refresh preservation, cancellation/stale results, reference caching, and horizontal styled clipping. These establish the unchanged UI baseline, not acceptance of the prototype. Eight new harness tests independently verify correct and incorrect connections, partial boundaries, explicit unsupported input, colour parsing and repeatable viewport slicing.

## Reuse assessment

Source/API review was performed on 2026-11-16. Upstream default-branch links can change; gleisbau's manifest reported 0.7.5. No upstream code was copied and no library benchmark or compiled integration was performed.

| Option | Findings and cost | Assessment |
|---|---|---|
| Extend the local ASCII transformer | Preserves current Git ordering and loading; needs more routing states, explicit crossing semantics, boundary policy, colour treatment and denser output | Not justified as a production increment by this experiment |
| gleisbau generic tracks + graph-only output | Can receive commit IDs and parents separately from repository loading; has subset layout and terminal graph output | Strongest next candidate, subject to stability/coverage tests |
| gleisbau legacy repository builder | Owns repository traversal, branch assignment, labels and layout | Poor fit for replacing only Tracer's presentation layer |
| Adapt a small licensed component | Could avoid adopting an entire dependency stack if an independent grid/renderer component exists | Inspect isolation and maintenance cost before choosing; preserve MIT notices if copying |
| Embed git-igitt or serie | Application-level integration would duplicate existing browser behavior; serie's image path differs from the agreed text rendering | Study designs, do not embed either application for this increment |

Concrete library evidence:

- Gleisbau's [generic track builder](https://github.com/git-bahn/gleisbau/blob/master/src/track.rs) accepts commit IDs/messages/parents in child-before-parent order and maintains missing-parent state. This corrects the earlier overly broad suggestion that reuse necessarily requires its repository-loading machinery. Its `Rc<RefCell<…>>` ownership would need to remain within a worker, returning owned render data to Tracer.
- The [layout module](https://github.com/git-bahn/gleisbau/blob/master/src/layout.rs) assigns columns/colours for a commit range. Its range-dependent packing means stable prefix expansion must be demonstrated, not assumed.
- The [printing API](https://docs.rs/gleisbau/latest/gleisbau/print/index.html) documents a graph-only terminal function and a 2D drawing grid, distinct from legacy commit formatting. Exact row/OID mapping, clipping and typed-colour adaptation still need a compiled trial.
- The [legacy builder](https://github.com/git-bahn/gleisbau/blob/master/src/graph.rs) performs its own traversal and explicitly rejects shallow clones. This limitation must not automatically be attributed to the generic track API without testing it.
- The [manifest](https://github.com/git-bahn/gleisbau/blob/master/Cargo.toml) makes `git2` mandatory and includes regex, TOML, terminal/colour and formatting dependencies. The generic API avoids using repository traversal but does not remove that dependency cost. License: MIT; no new dependency was added here.
- [git-igitt's manifest](https://github.com/git-bahn/git-igitt/blob/master/Cargo.toml) shows shared gleisbau use and a separate `tui` UI stack. Its [README](https://github.com/git-bahn/git-igitt#limitations) documents branch-model assumptions and octopus/shallow restrictions. These are application-level warnings, not proof about every current generic API. Detailed UI adapter source could not be retrieved in this environment; that part of the comparison remains unverified.
- [serie](https://github.com/lusingander/serie) remains a useful visual reference. Its terminal image approach is outside the chosen Unicode rendering requirement; no image-protocol dependency is proposed.

## Next decision

Keep the appearance and existing navigation contract fixed. Before expanding custom work, run the same topology and prefix corpus through gleisbau's generic builder/layout/graph-only renderer in an isolated executable, including shallow, detached and octopus histories. Measure dependency/build cost and map output rows to OIDs without taking over Tracer's history controller. If that path is unsuitable, compare an isolated MIT-licensed grid/renderer adaptation with a fully specified custom edge router. Do not choose on this experiment's line count alone.


## Compiled-library follow-up

The isolated compiled trial has been rerun and extended with 30,000 commits, 64 branches, cached Unicode viewports, explicit shallow boundaries, provenance and a static-build attempt. See [the follow-up report and actual previews](gleisbau-validation-followup.md). It reconfirms rejection of Gleisbau 0.7.5; no production integration or visual approval is implied.
