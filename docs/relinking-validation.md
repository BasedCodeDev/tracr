# Single-session relinking validation — 2026-11-15

The current source implements native relinking for one unlinked Codex session.
This is an unreleased source change; no GitHub publication or installer update was
performed. The [mechanism investigation](codex-native-relinking-solution.md)
explains why current-directory discovery uses database listing.

## Behavior

- `l`, the action menu, or Enter on an unlinked session opens destination input.
  Enter previews the resolved path; a separate Enter confirms. Escape cancels
  before execution. Input accepts absolute, home-relative, and launch-relative paths.
- Only existing accessible destinations and missing original folders are accepted.
  Source, identity, metadata fingerprint, archive state, supported history format,
  executable version, and destination are checked before changes.
- A dedicated Codex server first resumes **without a cwd override**, obtaining
  Codex's native exclusive writer. It checks resolved permissions/workspace roots
  before requesting a settings update. The passive live monitor is not repurposed
  to mutate a session open in another client. Its known loaded sessions are blocked
  in the UI; the native writer lock is the authoritative race-safe exclusion.
- Settings application is polled after acknowledgement. A fresh connection checks
  database listing, read, and override-free resume. Settings are compared across
  restart, and archive restoration is verified through listing.
- A private atomic journal (0600 files in a 0700 directory) records checkpoints
  before mutation. Source-level file locks serialize tracr operations. `R` offers
  guarded recovery after interruption; it never recreates a missing original folder.
  A conflicting later directory, identity, archive state, or saved permission/model
  settings prevents recovery. A timeout retains recovery information.
- Successful relinking selects the destination and same session, enabling archive
  visibility when necessary. No interactive session or model turn is launched.

Supported configurations: Codex CLI 0.167.2 and 0.169.0; legacy/paginated histories;
legacy read-only or trusted workspace-write, plus built-in `:workspace` and
`:read-only` profiles, with one runtime root and no explicit extra writable roots.
Custom/full-access profiles and multi-root workspaces fail closed. See
[built-in profile acceptance](builtin-profile-relinking-validation.md). Project-root associations and historical text are not changed.

## Tests

`./scripts/check.sh --offline` includes Rust, Python, formatting, Clippy, and all
terminal tests, including the new relinking PTY flow. Fake-provider cases verify
native metadata wins over original rollout metadata, default-only sessions are
reconciled, and database failure preserves browsing with incomplete-source warnings.

`./scripts/check.sh --offline --codex /absolute/path/to/codex` additionally runs
real relinking acceptance. Cargo identifies the exact compiled acceptance binary;
no stale binary is selected by filename guessing.

The real matrix has 18 cases per version: both history formats × normal relinking,
archived relinking, trusted workspace-write, stale preview, independent writer,
shared idle writer, shared working writer, shared waiting writer, and custom
permission rejection. Successful cases verify discovery after reopening and replay
recovery from a journal deliberately left at verification. Rejected cases verify
unchanged directory and archive state. Transcript preservation is checked; active
writer fixtures additionally retain their deliberately created synthetic turn.

The complete standard suite passed. The combined live run passed status and relinking;
its older new-session harness expected placeholder text removed in Codex 0.169.0.
After recognizing the explicit idle notification, the real resume/create rerun
passed legacy, paginated, and new-session cases.

All sessions and daemon homes are disposable. Working/waiting fixtures use a
loopback scripted provider with no inference or user credentials. No user session
was relocated during acceptance. Evidence is retained in
[the compact acceptance record](relinking-acceptance-evidence.json).

For targeted acceptance:

```sh
cargo test --offline --test relocation --no-run --message-format=json
python3 scripts/check_codex_relink.py \
  --codex /absolute/path/to/codex \
  --test-binary /absolute/path/from/cargo/compiler-artifact/executable
```

Optional `--mode legacy|paginated` and repeatable `--scenario` restrict the matrix.

## Remaining work

Folder-group relocation, explicit project-root migration, custom/multi-root
permissions, and additional Codex versions require later increments. Clients using
default rollout listing may continue to show the original folder. This change does
not rewrite Codex transcripts to change those clients' behavior. Release artifact
validation and publishing remain separate work.

## Permission-profile follow-up

The [profile investigation](codex-profile-relinking.md) adds 88 disposable native
observations and an implementation specification. It verifies a native :workspace
relocation recipe and identifies concrete custom-profile and preview limitations.
That research increment did not expand eligibility. The subsequent
[built-in implementation](builtin-profile-relinking-validation.md) now enables the
verified built-in profiles; custom profiles remain gated.
