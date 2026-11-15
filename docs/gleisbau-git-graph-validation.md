# Git graph implementation decision: compiled library validation

## Decision — 2026-11-16

**Implement a parent-aware Unicode renderer in Tracer. Do not integrate gleisbau 0.7.5 or continue the ASCII-to-Unicode prototype.** Git should still provide topologically ordered commits, parent IDs and references. Tracer should own only deterministic lane assignment, edge routing, styled rows and the OID-to-row mapping needed by its existing history controller.

This is an implementation direction, not a claim that the new renderer already passes acceptance. The approved connected, coloured Unicode appearance remains in the first slice. Production enablement still requires the existing topology/prefix corpus, compact visual review, and navigation/following acceptance. No production renderer or dependency changed in this validation increment.

The compiled library is fast enough and has a usable commit-to-row API. It does not pass our partial-history correctness and stable-prefix requirements under any of its four built-in ordering policies. Isolating its drawing grid would not fix the missing-parent/track-packing problems. Maintaining those fixes plus the integration adapter is not a demonstrated reduction in custom work.

## Reproduce the isolated experiment

```sh
cargo +stable build --release --locked \
  --manifest-path scripts/experiments/gleisbau-probe/Cargo.toml \
  --target-dir target/gleisbau-probe
python3 scripts/experiments/gleisbau_probe.py \
  --binary target/gleisbau-probe/release/tracr-gleisbau-probe \
  --output target/gleisbau-probe/results
python3 -m unittest discover -s tests -p 'test_*probe.py' -v
cargo +stable test --offline --test git_unicode_library_probe -- --ignored
```

After dependencies are fetched, add `--offline` to the build. The first sandboxed fetch could not resolve crates.io; the approved network fetch succeeded. The experiment has its own manifest and lockfile. Tracer's Cargo files are unchanged. Git fixtures are automatically cleaned, including the shallow clone. Retained results and build products stay in `target/`; no extra worktree or background service is created.

The probe exits successfully when measurements complete; this does not mean renderer acceptance. The JSON explicitly records `accepted_for_integration: false`, individual errors and rejected configurations. The application adapter test is opt-in because it requires generated experiment output. It is not part of production rendering.

Evidence: [results](fixtures/gleisbau-probe/results.json), [build/dependency costs](fixtures/gleisbau-probe/build-cost.json), [actual coloured output](fixtures/gleisbau-probe/four-lanes.svg), [octopus output](fixtures/gleisbau-probe/octopus.svg), [prefix position changes](fixtures/gleisbau-probe/prefix-changes.json).

## What was actually integrated for testing

The executable uses the pinned crate's generic `track::Builder<Oid>`, `layout_track_range`, and `print_graph_terminal`. Commits and references come from Git CLI output; it never creates a gleisbau repository object or replaces Tracer's discovery. All `Rc<RefCell<…>>` state is created and destroyed inside one worker; only owned JSON render data leaves it.

The adapter uses public APIs to complete child/source/target metadata, associates reference names with track targets, selects rounded characters and Tracer's six-colour palette, and requests compact output. Anonymous track persistence is normalized to the one equal-priority group so the grid paints coloured lines rather than leaving them neutral. A spare last row is requested when parents are outside the prefix, allowing continuation strokes. These are explicit adapter choices; no upstream source was edited.

`GraphLines.commit2line` supplies the commit-to-row map. The opt-in Tracer test converts the library's ANSI-256 colours into the existing typed basic-colour spans and preserves connector-only rows. Arbitrary terminal escapes are not passed through to Tracer.

The independent Python oracle follows the **rendered Unicode cells**, comparing reachable next commits with Git's parent IDs. It treats `┼` as a crossing, and respects the library's arrows pointing toward merge children so it does not walk backwards into another child. Unit tests cover crossings, merge arrows, wrong/missing parents, open tails, malformed mappings and colour decoding. Correct library input alone is never counted as correct drawing.

## Correctness, appearance and stability

All 50 default-policy fixture/prefix calls returned a render; none panicked. **26 passed the output checks; 24 failed.** This includes the original eight shared fixtures plus shallow and empty histories. Full ordinary merges, the full four-branch fixture, full criss-cross history, detached HEAD and the shallow/empty cases passed. Failures concentrate in partial ancestry, octopus routing and disconnected roots.

Concrete failures retained for review:

- [Two-commit partial history](fixtures/gleisbau-probe/four-lanes-prefix-2.json): two unrelated tips with unloaded parents are packed into one visible column and appear directly connected. An extra bottom row alone cannot repair that topology.
- [Octopus merge](fixtures/gleisbau-probe/octopus.json): the displayed paths from `bd4c5b4` reach two of its four parents. The SVG visibly contains upward-facing junctions where downward connections are needed.
- [Disconnected roots](fixtures/gleisbau-probe/disconnected.json): the end of one history and start of another occupy adjacent rows in the same column without a separating gap. In a compact graph this implies a parent connection that does not exist.
- With the default policy, commit `4e0c314` moves from column 2 to column 4, still on row 1, when the prefix grows from 128 to 256 commits. This is far above the unfinished boundary. Unchanged repeated reads match, but expanding prefixes do not.

All built-in ordering policies were tested on the difficult fixtures and repeated-merge prefixes:

| Ordering policy | Rejected prefix cases in this comparison | Unstable expansions |
|---|---:|---:|
| Shortest first, reverse | 24 | 9 |
| Shortest first, forward | 24 | 12 |
| Longest first, reverse | 21 | 12 |
| Longest first, forward | 21 | 7 |

These policy totals are a separate six-fixture comparison, not the 50-case default matrix. Longest-forward stabilizes the large repeated-merge fixture, but still changes layout in smaller fixtures and retains correctness failures. No policy passes the full requirements. The test rebuilds prefixes as Tracer currently does; no claim is made about an independently patched incremental integration.

The four-branch and octopus SVGs were rasterized and inspected. Rounded lines, colours, compact rows and explicit commit nodes are much closer to the approved design than the first prototype. Hollow merge nodes and directional arrows are library conventions, not newly approved product requirements. The correctness failures prevent accepting the visual candidate.

### Existing UI behavior

`git_unicode_library_probe.rs` passes using real library output through Tracer's existing `History` backend interface. It verifies:

- locating a worktree HEAD beyond the initial 128 commits;
- Up/Down and page navigation select commits rather than connectors;
- manual navigation stops following, refresh preserves the OID, and `f` resumes following;
- expansion preserves selection even when the library relocates rows/columns;
- horizontal movement keeps selection and does not request another render;
- worktree switching selects the new HEAD, and cancelled/obsolete replies cannot replace it;
- shutdown drops pending replies.

This is a controlled delivery test, not a claim that gleisbau supports interrupting an in-progress synchronous layout call. Its public rendering API has no cancellation token; dropping a future around a thread would not stop that thread. An integration would need bounded worker completion or additional cooperative checkpoints. No permanent subprocess is proposed.

The experiment does not install an interactive renderer. Full-pane selection styling, terminal font compatibility and production integration remain acceptance work for the selected implementation. Existing graph clipping/following tests provide the baseline, not substitute visual approval.

## Performance and dependency cost

Release build, Rust 1.99.1, Git 2.56.0. Seven runs per input; medians below exclude input JSON parsing and process startup. Ingestion includes fresh settings/regex construction and track building. Memory uses a Rust allocator counter relative to the parsed input, including working tracks/layout/rendered strings; it excludes native allocations outside that allocator. These are local feasibility measurements, not product guarantees.

| Linear commits | Ingestion ms | Layout ms | Rendering ms | Retained bytes | Peak bytes |
|---|---:|---:|---:|---:|---:|
| 128 | 3.601 | 0.006 | 0.048 | 149,904 | 416,556 |
| 512 | 3.851 | 0.011 | 0.171 | 300,432 | 416,556 |
| 2,048 | 4.996 | 0.030 | 0.681 | 902,544 | 1,224,115 |
| 10,000 | 10.959 | 0.123 | 3.366 | 4,256,012 | 5,634,412 |

The initial 128-commit target is met. A genuine eight-parent, concurrent-branch fixture is also measured: Git renders a 16-column graph, but this integration collapses it to 1–3 columns with wrong/missing connections. Its fast timings are recorded but **must not be interpreted as a valid wide-graph performance result**.

The dependency-cold optimized probe build took 3.602 seconds after downloads; a warm no-change build took 0.023 seconds. A fresh-target JSON/worker baseline without the library took 1.595 seconds. Both use the same toolchain and release settings. Stripped executable sizes are 1,173,872 bytes for the probe and 443,656 for the baseline (730,216-byte difference). This is not an exact Tracer binary-size prediction: an application build could share or eliminate more code.

The isolated lock contains 103 dependency packages across targets, with 61 name/version pairs not present in Tracer's current lock. Some are alternate versions or target-specific, so this is not a count of additional runtime libraries. `git2` remains mandatory within gleisbau even when only its generic API is used. Dependency size and speed are tradeoffs, **not the reason this candidate was rejected**.

## Reuse and maintenance assessment

The actual downloaded 0.7.5 source was inspected, superseding the earlier web-only API review. Its generic track builder explicitly notes incomplete handling of missing merged parents during incremental construction (`src/track.rs`, near the `add_branch_if_missing` closure). Layout packing uses track extents and ordering policy; the public API does not expose a stable column assignment to retain across prefixes. The observed failures are consistent with these constraints, but the experiment does not claim every failure has a diagnosed upstream root cause.

A grid-only adaptation would avoid some dependency coupling, but it would still need Tracer-owned topology, open-parent tracking, stable lane placement and row mapping. In this version the grid module is 705 lines; terminal printing is 1,116, layout 563 and track construction 721, including comments/tests. Extracting those modules together is effectively maintaining a substantial renderer fork. Grid-only reuse also does not resolve the observed octopus stroke problem. Consequently no extracted component is selected.

The crate is MIT-licensed (Copyright 2020 Martin Lange). No upstream source was copied into the prototype. If future code is adapted, retain the full applicable copyright/license notice. The upstream designs remain useful references for separating track layout from drawing, representing cell connections explicitly and mapping commits to inserted connector rows.

## Implementation scope selected

1. Extend the Git history record to include ordered parent IDs; keep Git responsible for repository traversal and topological ordering. Preserve current worktree pinning, cancellation and stale-result checks.
2. Build a small deterministic lane/edge model from those records, with explicit unresolved-parent targets. Do not infer topology from ASCII cells, merge messages or branch-name conventions.
3. Render that model into compact coloured Unicode cells, preserving crossing versus junction semantics and a commit-ID-to-row map. Reuse existing Tracer span clipping and selection styling.
4. Preserve already completed prefix rows and lane colours as older history loads. Treat disconnected roots and incomplete boundaries explicitly. Cache render results so navigation does no layout work.
5. Use the shared fixtures and independent output oracle as regression gates. Add targeted tests for the exact failures recorded here before enabling the renderer. Preserve Up/Down, paging, manual/follow transitions, worktree HEAD seeking and refresh selection.

This direction is intentionally larger than a character substitution, but narrower than adopting branch-model grouping or a full Git UI. The next increment is implementation against these concrete acceptance cases, not another general comparison of Git clients.

## Verification

- Isolated crate release build and Clippy (`-D warnings`): passed.
- Fifteen Python probe/oracle tests: passed.
- Opt-in Tracer history adapter test with compiled library output: passed.
- Clippy for the Tracer adapter test: passed.
- Experimental renderer acceptance: **failed**, as recorded above; not enabled.


## Validation-first follow-up

The isolated compiled trial has been rerun and extended with 30,000 commits, 64 branches, cached Unicode viewports, explicit shallow boundaries, provenance and a static-build attempt. See [the follow-up report and actual previews](gleisbau-validation-followup.md). It reconfirms rejection of Gleisbau 0.7.5; no production integration or visual approval is implied.
