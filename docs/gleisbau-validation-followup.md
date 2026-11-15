# Gleisbau validation: scale and preview follow-up

## Result

**Reject Gleisbau 0.7.5 for Tracer integration.** This validation-first increment is complete; it does not approve a visual design or integrate a renderer. The existing compiled trial was reused and independently rerun, then extended with the remaining scale, viewport, shallow-boundary and build checks. Separate application-renderer changes already present in the worktree were preserved and were not implemented by this follow-up.

The default corpus again produced 50 renders: 26 passed and 24 failed the output checks; seven recorded prefix transitions were unstable. The stronger oracle also rejects duplicate mapping entries and genuinely dangling strokes, while distinguishing valid upward/child-facing connections. All four library ordering policies remain recorded in the corpus report; none passes the full existing requirements.

## Actual previews

These are output from the compiled library plus a cached viewport adapter, not design mockups. Rounded connections, explicit coloured nodes and compact rows can look promising on passing fixtures, but failed topology prevents adoption. The SVG previews were rasterized and visually inspected. Emoji shape is font-dependent; combining-character, wide-glyph and ZWJ clipping correctness is checked separately in Rust.

- [Four parallel branches, wide](fixtures/gleisbau-acceptance/parallel-four-9-wide.svg), [narrow](fixtures/gleisbau-acceptance/parallel-four-9-narrow.svg), [scrolled](fixtures/gleisbau-acceptance/parallel-four-9-scrolled.svg).
- [Ordinary merge](fixtures/gleisbau-acceptance/merge-4-wide.svg) and [criss-cross history](fixtures/gleisbau-acceptance/crossing-6-wide.svg).
- [Incorrect partial ancestry](fixtures/gleisbau-acceptance/four-lanes-2-narrow.svg): unrelated tips appear connected in one column.
- [Octopus output](fixtures/gleisbau-acceptance/octopus-6-wide.svg): the independent oracle still reports missing parent connectivity.
- [Explicit shallow boundary](fixtures/gleisbau-acceptance/shallow-boundary.svg).

Every preview also has adjacent `.txt`, `.ansi` and/or input/output `.json` evidence. Narrow/scrolled views are slices of one render; returning to the same viewport yields identical owned spans and OID-to-row mappings. The preview encoder preserves exact span whitespace and escapes label text. No layout is invoked by viewport slicing.

## Extended measurements

Seven optimized runs per input; medians exclude subprocess startup and input JSON parsing. The allocator measurements cover Rust working tracks/layout/rendered strings relative to parsed input. They exclude native allocations and are not whole-process RSS or a complete serialized-output memory budget. Passing topology and fast execution are separate gates.

| Shape | Commits | Ingestion ms | Layout ms | Render ms | Retained working bytes | Peak working bytes | Topology errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| linear-30000 | 128 | 3.808 | 0.006 | 0.048 | 161584 | 416556 | 0 |
| linear-30000 | 512 | 4.161 | 0.012 | 0.175 | 312112 | 416556 | 0 |
| linear-30000 | 2048 | 5.366 | 0.031 | 0.669 | 914224 | 1235795 | 0 |
| linear-30000 | 10000 | 11.955 | 0.125 | 3.533 | 4267760 | 5646147 | 0 |
| linear-30000 | 30000 | 30.030 | 0.357 | 11.526 | 11826732 | 16174828 | 0 |
| wide-64 | 128 | 3.623 | 0.010 | 0.067 | 151686 | 416556 | 14 |
| wide-64 | 512 | 3.854 | 0.019 | 0.251 | 314045 | 416556 | 60 |
| wide-64 | 1089 | 4.374 | 0.040 | 5.780 | 1333848 | 1905689 | 0 |

The 64-branch fixture has 16 commits per branch and 64 successive merge commits (1,089 commits total). Its full output passes the oracle, but prefixes of 128 and 512 commits have 14 and 60 topology errors. Expansion to the full history moves earlier commits and changes completed rows. These failing-prefix timings are not evidence of an acceptable renderer. Linear history remains stable through 30,000 commits.

The added shallow test reads raw commit parents with `git cat-file`, because Git log suppresses ancestry beyond a shallow boundary. It labels that boundary explicitly and requires an open continuation rather than treating it as a genuine root; this case passed. No fetch or repository-owning library API is used.

## Provenance and build cost

The isolated manifest pins Gleisbau `0.7.5`. Published crate checksum: `ac933c4eb1ee130b83108cd6b750f2092e96fd0c65883c40451933af5975c48e`. Its packaged VCS metadata identifies revision `735db6a060aafb4174dab1f79c7b1c99c62dfcc0`. License: MIT, Copyright 2020 Martin Lange; upstream MSRV 1.83.0. The trial was built with `rustc 1.99.1 (58cf8f9c3 2026-10-13)`. No upstream source was copied. The experiment has its own Cargo lockfile and is not a Tracer dependency.

With downloaded dependencies but fresh target directories, the final library+viewport executable built in 3.579 s (0.025 s warm); the same-package JSON/worker baseline took 1.66 s (0.015 s warm). Stripped sizes were 1,261,840 and 443,656 bytes respectively. This includes the experimental adapter and is not a prediction of Tracer's binary growth. The lock has 103 dependency packages across targets and 60 name/version pairs absent from the application lock at measurement time.

**Static musl build: blocked, not certified.** `libgit2-sys`/`libz-sys` require `x86_64-linux-musl-gcc`, which is missing locally. The experiment was attempted against the installed Rust musl target; no production build flags or packaging requirements were weakened. The mandatory native dependency is an integration cost, but correctness is the reason for rejection.

## Reproduction and verification

```sh
cargo +stable build --release --locked --offline --manifest-path scripts/experiments/gleisbau-probe/Cargo.toml --target-dir target/gleisbau-probe
python3 scripts/experiments/gleisbau_probe.py --binary target/gleisbau-probe/release/tracr-gleisbau-probe --output target/gleisbau-probe/revalidation
python3 scripts/experiments/gleisbau_acceptance.py --binary target/gleisbau-probe/release/tracr-gleisbau-probe --output target/gleisbau-probe/acceptance
cargo +stable test --offline --manifest-path scripts/experiments/gleisbau-probe/Cargo.toml --target-dir target/gleisbau-probe
python3 -m unittest discover -s tests -p 'test_*probe.py' -v
python3 -m unittest discover -s tests -p 'test_gleisbau_acceptance.py' -v
```

Passing checks: two compiled viewport tests; 17 original/probe oracle tests plus one preview-serialization test; isolated all-target Clippy with warnings denied; ten existing history tests and two existing graph-viewport tests. Those application tests exercised the contemporaneous worktree, including separately developed renderer changes, and do not certify Gleisbau integration. A successful probe process means evidence collection completed; `accepted_for_integration` stays false.

Artifacts: [default corpus and policies](fixtures/gleisbau-acceptance/corpus-results.json), [scale and viewport results](fixtures/gleisbau-acceptance/supplement.json), [build/provenance record](fixtures/gleisbau-acceptance/build-cost.json).

The earlier assessment of a grid-only extraction still applies: it would not supply stable missing-parent tracking or fix the demonstrated partial-history packing failures. No attributed adaptation is selected by this run. The existing separate parent-aware-renderer direction requires its own correctness and visual acceptance; this experiment does not authorize or certify it.
