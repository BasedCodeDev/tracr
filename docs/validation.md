# Validation and development hygiene

## Repeatable checks

```sh
./scripts/check.sh --offline
```

The runner selects an installed Rust toolchain, preserves Cargo.lock, and runs
formatting, locked builds, all-target Clippy, Rust/Python tests, and Linux PTY
acceptance with synthetic providers. Independent stages continue after failures;
terminal stages require a successful build. Exit 0 means all requested stages
passed; otherwise the runner exits 1. Invalid shell options exit 2.

Omit `--offline` to fetch locked dependencies. Add `--codex /absolute/path/to/codex`
only when deliberately running native acceptance with an installed executable,
not a package-manager launcher. Native checks use isolated homes and synthetic
sessions; they must never target real user sessions. The live-status fixture uses
a loopback scripted provider. Interactive acceptance does not submit model prompts,
auto-approve trust, or bypass normal daemon behavior. A blocked native fixture is
not a compatibility pass. Native interactive helpers distinguish failed (1) and
blocked (2) runs.

## Coverage and limitations

- Domain/provider tests cover discovery, pagination, metadata search, archives,
  stale/partial inventories, path handling, and source-specific identity.
- PTY checks cover browse/create/resume, session information, contextual controls,
  changed-directory review, resize, cancellation, terminal restoration and child
  reaping. Synthetic providers cannot certify every real terminal or Codex version.
- Relinking, permission provenance, same-filesystem folder moves, idle release,
  journal compatibility and guarded recovery have Rust and synthetic terminal
  coverage. See [relocation compatibility](codex-relocation-compatibility.md).
- Worktree creation checks cover local exclusions, relative links, reviewed commits,
  collision/race rejection, partial failures, both terminal entry points, and moving
  a project with worktree sessions through the existing folder-move flow.
- Git inventory, graph topology, watchers, progressive loading, HEAD following,
  cancellation and idle behavior have independent regression coverage. See
  [Git validation](git-worktree-browsing-validation.md).
- Release tests cover packaging, checksums/notices and installation. See
  [releasing](releasing.md). A successful development build does not certify
  packaging, another architecture, or a new native-provider version.

Previously recorded native checks covered Codex 0.167.2 and 0.169.0, legacy and
paginated histories. These are historical observations, not a fresh run against
any newer installed version. Broader terminal/font review, sanitized organic
session acceptance and native ARM64 acceptance remain explicit follow-ups.

## Temporary files and evidence

Diagnostic entrypoints use `scripts/diagnostics.py`. Successful runs delete their
scratch environments after owned subprocesses close. Failed or blocked runs retain
only reports and log tails in ignored `target/diagnostics/<helper>/`; the latest
three completed runs per helper are kept. Each report/log is bounded to 2 MiB,
with a 16 MiB budget per scratch environment; oversized JSON is omitted rather
than truncated. Raw RPC captures, session storage, downloaded runtimes and schemas
are not retained by default. Terminal captures are available with full retention.

Use `--keep-temp` on `scripts/check.sh` or a diagnostic Python entrypoint to keep
full scratch deliberately. The runner forwards that preference to child checks.
Retained full scratch is outside automatic pruning and must be removed when its
investigation ends. Explicit `--output` locations keep their compact reports even
on success, but not generated environments. Printed final locations distinguish
retained reports from removed scratch. A child cleanup failure or a visible
process still referencing scratch prevents deletion and prints its location.
Uncatchable termination can leave scratch; inspect ownership before removing it.

Keep build products, generated previews and ad hoc acceptance reports under
`target/`. Never commit generated evidence by default: document the behavior,
reproduction command and unresolved constraint instead. Historical investigations
remain available in Git history. Keep only actionable work in [PLAN.md](../PLAN.md).
Create future worktrees in a stable workspace location outside `/tmp`; do not
relocate or delete an active worktree. `cargo clean` is appropriate for an inactive
checkout when disk space matters, at the cost of rebuilding.
