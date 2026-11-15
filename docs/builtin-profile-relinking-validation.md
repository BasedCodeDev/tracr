# Built-in permission-profile relinking — 2026-11-15

This source increment enables `:workspace` and `:read-only` in the actual Rust
relocation service. It is the implementation follow-up to the
[profile investigation](codex-profile-relinking.md), not another research-only change.
No real user session was moved and no installed binary or release was replaced.

## Contract

Both built-ins are supported on Codex 0.167.2 and 0.169.0, with legacy or paginated
history, exactly one runtime root matching the cwd, and their expected sandbox
configuration. Workspace must have no extra writable roots, no network access,
and the canonical temporary-directory flags. Read-only must remain read-only with
network disabled. Existing legacy configurations keep their previous validation.
Custom/full-access profiles, unknown fields/inheritance, and extra roots are rejected.

Preview reads the profile provenance from Codex's supplied rollout path. The
reader accepts only regular files beneath the canonical configured `sessions` or
`archived_sessions` directory, validates session identity, and follows the pinned
Codex owned-settings/legacy-checkpoint precedence before falling back to turn
context. Foreign-owner grants are not accepted. Unknown record payloads—including
messages—are not deserialized or retained in previews/journals. Reads run off the
UI thread, with five-second waiting, 8 MiB record and 256 MiB file bounds. Missing,
malformed, oversized, or incomplete metadata rejects the operation with an explanation.

The profile is re-read before writer acquisition and matched against the native
resume response. Custom-profile fallback to a built-in cannot pass this gate.
The native writer is acquired without a cwd override. Only cwd is updated, then
profile, single root, sandbox, model/provider, approval policy/reviewer and directory
are verified in live state and across a fresh server. Existing archive restoration,
source locking, directory canonicalization, and native listing/read verification remain.

The confirmation labels the profile and keeps confirmation controls in a fixed
footer when paths wrap. Known unsupported profiles fail before confirmation in the
persistent, copyable error dialog. Success selects the same session in its new folder.

Journal format 1 remains readable with additive fields: `preview.profile` and
`permission_validation { version: 1, profile }`. Built-in recovery requires its
recorded validation. Older journals cannot authorize built-in moves by inference;
legacy journals with explicit saved null profile identity can retain their previous
recovery behavior. Unexpected current state remains unresolved for inspection.

## Validation

The production-service matrix exercises six new scenarios on each version and each
history format (24 cases): workspace, read-only, child-to-parent, archived, competing
writer, and stale preview. Successful cases additionally reopen discovery, replay
guarded recovery from a `verifying` journal, check private journal permissions,
confirm the original folder stays missing, and compare synthetic history. Workspace
cases inspect the complete synthetic recorded profile after completion to check
read-only `.git`, `.agents`, `.codex`, and `.aws` guards at the destination.

A further 24 production-service regression cases cover legacy read-only, archived,
workspace-write, unsupported extra writable roots, competing writers, and stale
previews across both versions/formats. The associated results are in
[built-in acceptance evidence](builtin-profile-relinking-evidence.json).

Unit/integration tests cover provenance precedence, ignored message payloads,
malformed/inherited/unknown profiles, canonical sandbox/root checks, unsupported
profiles rejected before resume, changed runtime profiles/fallback, built-in journal
validation, and recovery of old legacy journals. The terminal check relinks a
built-in workspace session, checks the preview label, restarts tracr at the new
folder, and verifies the session is linked there. It also retains rejection,
recovery, copy, cancellation, and narrow-layout coverage.

Run the standard suite:

```sh
./scripts/check.sh --offline
```

The optional real-Codex check now includes built-in cases by default. For targeted
acceptance, `scripts/check_codex_relink.py` accepts `--scenario builtin-parent`
(and the other scenario names listed by `--help`) plus an exact Cargo-produced
`tests/relocation.rs` test executable via `--test-binary`.

To try the updated application without changing the installed version:

```sh
./scripts/dev.sh .
```

Select an eligible unlinked session, press `l`, choose its existing destination,
review **Permissions: Workspace** or **Permissions: Read only**, and confirm.
