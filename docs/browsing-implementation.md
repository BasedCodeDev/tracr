> Update: all standard checks and normal-terminal live acceptance pass on Codex 0.167.2 for both legacy and paginated histories. Earlier blocked/pending notes below are historical; see [current validation](validation.md).

# Browsing milestone implementation status

The application now compiles and has passed fixture CLI, fake-Codex discovery,
and terminal cleanup smoke checks. The full Rust suite remains blocked by missing
test dependencies and crates.io DNS. Milestones 2 and 3 remain open.

## Components

- `providers/codex`: serialized JSON-RPC requests, initialization, bounded stderr
  and frame capture, timeouts, explicit source kinds, both archive partitions,
  per-record warnings, metadata reads, owned-process shutdown.
- `browser`: generation-scoped updates, four concurrent source discoveries,
  staged refresh, stable selection, folder hierarchy, global search, independent
  details, and bounded background directory checks.
- `tui`: three panes or focused-pane fallback, keyboard controls, resize handling,
  help, safe text rendering, terminal restoration guard and panic hook.
- `main`: live and fixture `list`/`browse` commands, binary override, per-source
  JSON diagnostics, incomplete-inventory exit status.

The existing provider page type now carries warnings. The collect-all helper
retains valid sessions and reports incomplete results if any record is malformed.
Production discovery sends only initialize/initialized, thread/list and thread/read.

## Validation actually performed

- Rust 1.99.1 formatting applied; `cargo fmt --check` passes.
- The five existing Python probe unit tests pass.
- Python fake-server and pseudo-terminal helper scripts pass syntax compilation.
- Cargo.lock is present and the application builds with it.
- Application-target Clippy passes with warnings denied.
- The dev script produces the expected five active fixture sessions.
- A compiled CLI smoke test pages both fake-Codex archive partitions.
- The PTY helper passes for q, Ctrl-C, and SIGTERM; direct signal flags fix the
  async signal-delivery failure observed on this machine.
- The full Rust suite stops at the uncached fastrand test dependency.

## Remaining acceptance work

1. Restore registry access for test dependencies and run the remaining tests.
2. Run all foundation and browsing Rust tests, Clippy, and formatting checks.
3. Run the compiled fixture TUI in a real terminal: search, archives, resize,
   pane switching, rapid refresh, details, and clean exit.
4. Exercise live listing/browsing against a disposable Codex home with legacy
   and paginated sessions; compare inventory and directory mismatches.
5. Verify panic/setup-error cleanup manually (automated PTY cases cover normal
   quit, Ctrl-C, and SIGTERM). Confirm no user-owned Codex process is terminated.
6. Retain Cargo.lock and update milestone status only after remaining checks pass.

No claims of real-Codex compatibility or a passing full Rust suite are made yet. In
particular, the fake server verifies our request shape, not every Codex version.
The adapter targets the locally inspected 0.167.2 schema. Unknown protocol
changes fail visibly instead of silently producing an empty inventory.

Updated consolidated verification and real-Codex results: [validation status](validation.md).
