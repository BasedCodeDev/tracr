# Isolated gleisbau validation

This package is not a Tracer dependency. It pins gleisbau 0.7.5, reads JSON commit records on stdin, and writes owned render rows, commit positions, timings and allocator measurements on stdout. Library state is worker-local; errors/panics are reported as JSON. The Python driver creates disposable Git fixtures and checks actual rendered connections.

Build/run from the repository root:

```sh
cargo +stable build --release --locked --manifest-path scripts/experiments/gleisbau-probe/Cargo.toml --target-dir target/gleisbau-probe
python3 scripts/experiments/gleisbau_probe.py --binary target/gleisbau-probe/release/tracr-gleisbau-probe --output target/gleisbau-probe/results
cargo +stable test --offline --test git_unicode_library_probe -- --ignored
```

Use `--offline` on the build after dependencies are cached. The no-library baseline uses `--no-default-features --bin baseline`. Use fresh, distinct target directories for cold-build comparisons. Build caches are intentionally kept under `target`, not in additional temporary worktrees.

A successful driver exit means the experiment finished, **not** that the library passed. See `docs/gleisbau-git-graph-validation.md` for the decision, limitations and recorded evidence. Do not link this experimental implementation into Tracer.

The validation-first follow-up adds cached viewport requests and an explicit scale
runner. Rendering still runs once; multiple viewport requests clip owned spans and
retain OID-to-row mappings. The Rust tests cover combining marks, wide glyphs and
ZWJ emoji. No output escape sequence is forwarded as an untyped span.

```sh
python3 scripts/experiments/gleisbau_acceptance.py --binary target/gleisbau-probe/release/tracr-gleisbau-probe --output target/gleisbau-probe/acceptance
cargo +stable test --offline --manifest-path scripts/experiments/gleisbau-probe/Cargo.toml --target-dir target/gleisbau-probe
```

See `docs/gleisbau-validation-followup.md` for the extended corpus, actual narrow
and wide previews, pinned source provenance and failed/blocked gates. The
experiment's success exit does not authorize adoption of the candidate.
