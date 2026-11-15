# Permission-profile relinking investigation — 2026-11-15

**The built-in `:workspace` profile can be relocated natively, including from a
missing child directory to its existing parent. Arbitrary custom profiles need
more than removing tracr's current rejection.** This increment adds a reproducible
probe, versioned evidence, and the implementation specification below. That research increment left production eligibility unchanged and moved no real
user session. The subsequent [built-in implementation](builtin-profile-relinking-validation.md)
now enables `:workspace` and `:read-only`; the custom-profile findings below still apply.

## Evidence and reproduction

[Machine-readable evidence](codex-profile-relinking-evidence.json) contains **88
final observations**: 36 cwd-only cases and eight explicit-profile cases per
version, covering Codex 0.167.2 and 0.169.0 and both legacy and paginated history.
There are 69 completed observations and 19 captured native rejections; these are
research outcomes, not 69 certifications of safe relinking. Four completed
observations are competing-writer rejection checks rather than moves. All 65
completed moves preserved synthetic history and agreed on the directory through
native database listing and read. No fixture was inconclusive in the final runs.
Binary and probe SHA-256 hashes are included. Preliminary exploratory runs are not
included in those counts.

Run from the repository, supplying an installed version explicitly:

```sh
python3 scripts/probe_codex_profiles.py --codex /absolute/path/to/codex
python3 scripts/probe_codex_profiles.py --codex /absolute/path/to/codex \
  --apply-profile --case destination-layer-existing --case custom-existing \
  --case missing-profile --case workspace-parent
```

Use `--mode legacy` or `--mode paginated` and repeat `--case` to narrow a run.
Each run prints its disposable evidence directory. Raw RPC transcripts remain
there; the committed report includes only selected metadata/configuration and
normalizes test paths to `$CASE`. There are no credentials or real conversations
in the fixtures. The probe forbids `turn/start`, uses a loopback-only dummy model
provider, and closes its owned servers. It never reads the user's Codex home.
Fixture construction reuses the existing synthetic history builder; all writes
are confined to newly created temporary homes.

Run on a normal host where Codex's sandbox helper can operate. The outer agent
sandbox masks `/tmp/codex-daemon-<uid>` and caused a socket-directory permission
error during early custom-profile trials. The final runs used the host context,
without altering that directory or weakening any permission profile. Native
missing-directory failures described below remained in that context.

## Findings

| Case | Observation on both versions/history formats | Consequence |
| --- | --- | --- |
| `:workspace`, `:read-only`, and `workspace-parent` | Directory and profile survive restart; built-in workspace guards rebind to the new root | A verified built-in support increment is feasible |
| Archived workspace session | Native move, verification, and archive restoration work | Retain existing archive handling |
| Multiple runtime roots | Cwd-only update replaces the old cwd root and preserves the unrelated root | Do not replace the entire roots list with the destination |
| Simple custom profile extending `:workspace` | Relocation succeeds even when the old directory is missing | Not every custom profile has the richer-profile failure |
| Rich custom profile, extra roots, or destination rules with old folder missing | 15 of 16 final cwd-only cases reject during override-free resume | Do not claim general missing-folder custom support |
| Same rich profiles with the original folder still present | Moves complete, with relative rules rebound and explicit absolute rules retained | Existing-folder controls isolate the missing-folder problem |
| Destination project adds a deny rule | Cwd-only update lacks that rule in the live snapshot; restart applies it | Checking only live state misses destination changes |
| Config changes after preview | Reopened state differs from the previously resolved destination snapshot | Preview must be invalidated on configuration changes |
| Custom profile removed after preview | Override-free resume falls back to `:workspace`; cwd-only update proceeds | Detect profile fallback before applying any move |
| Explicit `permissions: "project"` with missing definition | Settings update rejects the unknown profile | Preserve profile identity explicitly; never accept silent fallback |
| Competing writer | Second override-free resume is rejected | Preserve native writer exclusion |

The missing-folder error is raised while loading AGENTS.md through Codex's
filesystem sandbox helper: `Not a directory (os error 20)` or a bubblewrap
mount/directory-creation error. The original cwd remains missing and no settings
update has been sent at this point. One 0.167.2 paginated extra-root trial completed,
so this behavior is not a deterministic success/failure contract. Keep these cases
incompatible until the native failure is understood and reliably addressed. Do not
recreate the old folder, relax rules, or resume with a destination override to
bypass this gate.

### Complete rules and misleading summaries

`thread/resume` exposes `activePermissionProfile` (identity and inheritance), runtime
roots, and a **legacy compatibility** `sandbox`. `permissionProfile/list` returns
IDs, descriptions, and allowed flags. Neither returns a complete materialized
filesystem policy or enforced network domain policy.

The probe reads **synthetic** `thread_settings_applied` events after closing the
server. These record materialized permission profiles, cwd, roots, and approval
settings. A research-only destination twin is created and given inert injected
history so Codex persists a comparable snapshot; it never runs a model turn.
An ephemeral thread is also inspected and confirms the public-response limitation.
Creating persistent preview twins in a user's home is **not** the proposed product
implementation.

Snapshots show that the built-in `:workspace` profile retains read-only rules for
`.git`, `.agents`, `.codex`, and `.aws` at the new root. The tested custom inherited
profiles did **not** include those same entries in their stored materialized
snapshots, even with those directories present. Do not infer builtin-equivalent
protections from an `extends` label. This investigation did not execute writes to
prove runtime enforcement of either case.

Custom profiles contain deny globs, an explicit read-only subdirectory, optional
additional roots, explicit absolute rules, and enabled/disabled network settings
with allow/deny domains. `config/read` exposes merged definitions and their network
policy, while the materialized permission snapshot only reports coarse network
access. No outbound requests or sandbox enforcement certification were attempted.
Domain enforcement, proxy configuration, and managed requirements therefore remain
part of the required complete-policy resolver, not a proven property of this probe.

### Native recipes established

For the built-in workspace reproduction of “Test this”:

1. Resume with no cwd override to acquire the native writer and inspect current state.
2. Check the expected profile and runtime roots; never accept an unexpected fallback.
3. Send `thread/settings/update` with only `threadId` and the selected `cwd`.
4. Close the owned server and reopen. Verify native database listing, read, resume,
   roots, profile, and the recorded rebinding of protected directory rules.

The `workspace-parent` case reproduces the missing-child-to-existing-parent shape
using synthetic paths and preserves the profile's guards in all four final runs.

For the rich existing-directory controls, an explicit update with
`{threadId, cwd, permissions: <same profile id>}` resolves the destination profile
before updating. The live result can retain an **additional old-root deny rule**;
restart drops that extra deny rule. Duplicate identical rules also occur. All four
custom/destination-rule explicit-update controls (two cases × two formats) per
version match the native destination twin after restart once identical duplicates
and rule ordering are normalized. This comparison does not discard changed paths,
access modes, missing-path behavior, or real extra rules.

This is a verified native recipe for those controls, not a solution to acquiring
a writer on a missing directory under arbitrary custom rules. It also does not
supply a complete non-mutating public preview API.

## Implementation specification for a subsequent increment

### Resolution and data model

Replace `simple_permissions` only when the relevant compatibility gate passes.
Introduce a versioned `PermissionSnapshot` with profile ID/inheritance, complete
resolved filesystem rules and provenance, runtime and profile-defined roots,
resolved network/proxy/domain policy, approval policy/reviewer, and fingerprints
of configuration layers, requirements, and session overrides. Distinguish unknown
fields from empty/disabled fields. Keep model/provider and other non-permission
settings as independent invariants.

Introduce `PermissionDelta` with before/after values and a classification:
workspace-relative rebinding, unchanged explicit rule/root, or substantive access
change. Use path components and native rule provenance, never string-prefix
replacement. Preserve explicit absolute paths even if they happen to be under the
old cwd. Exact duplicate rules can be deduplicated for comparison; unknown rule
forms cannot be silently ignored.

A native **read-only resolver** must provide source and destination snapshots in
the actual source-home/config/session context. The current public API is insufficient
for complete custom-policy preview. Required extension: resolve effective rules
and requirements for a thread and proposed cwd/profile/roots, returning configuration
fingerprints without changing the thread, starting a turn, or creating a durable
preview session. Codex already has internal permission-config loading and thread
settings preview functions, but they are not exposed as a full public dry-run.
A version-pinned helper using that same native resolver is another implementation
route; do not approximate inheritance, managed requirements, or network enforcement
with a new independent TOML interpreter. Until one route is available and tested,
complete custom-policy previews stay gated with this specific explanation.

### Preview, execution, and recovery

- Retain the active profile and unrelated roots by default. Show the destination,
  workspace rebinding, and any substantive changes in filesystem access, roots,
  network, approval policy, or reviewer. A profile fallback is a conflict.
- For substantive changes, require the explicit **Apply permission changes and
  relink** action after displaying before/after values. Enter must not approve an
  unseen change. Preserve the existing plain-language failure and copy UI.
- Re-read configuration fingerprints and source identity before mutation under the
  native writer. Rebuild the preview if anything differs. Send the approved profile
  explicitly when destination policy re-resolution is required.
- Record before/approved-after snapshots, fingerprints, consent to substantive
  changes, schema/adapter version, and execution checkpoints in a private journal
  before updating. Continue reading old journals; legacy entries without snapshots
  cannot authorize custom-policy changes during recovery.
- Observe the post-update state, then restart and compare with the approved
  destination snapshot. Treat only verified transient *additional deny rules* as
  allowable before restart; final state must match the reviewed snapshot. Never
  normalize away a permissive or unexplained difference.
- Recovery revalidates configuration and compares current state against recorded
  before/after states. A changed profile/configuration requires fresh review;
  unexpected state remains unresolved. Do not automatically weaken permissions or
  rewrite Codex's databases or history to finish a move.

### Compatibility gates

Enable built-in support only after production checks and tests consume the complete
required evidence for those profiles. Enable each custom configuration class only
when it has a complete pre-mutation preview, reliable writer acquisition with a
missing source directory, stable approved state after restart, preserved history,
and exclusion/recovery coverage. New Codex versions require their own evidence.
Existing-directory controls alone do not satisfy tracr's unlinked-session feature.

## Pinned source references

The executable schemas and these matching upstream sources explain the observations:

- [0.169.0 protocol: thread responses and settings parameters](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/app-server-protocol/src/protocol/v2/thread.rs): profile provenance and legacy sandbox response; no full policy response.
- [0.169.0 permission profile protocol](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/app-server-protocol/src/protocol/v2/permissions.rs): list summaries and profile identity.
- [0.169.0 settings builder](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/app-server/src/request_processors/turn_processor.rs): explicit `permissions` loads destination configuration and calls an internal preview.
- [0.169.0 config manager](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/app-server/src/config_manager.rs): `load_permission_config_for_thread` preserves session flags and resolves cwd-specific layers.
- [0.169.0 session updates](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/core/src/session/session.rs) and [0.167.2 equivalent](https://github.com/openai/codex/blob/rust-v0.167.2/codex-rs/core/src/session/session.rs): runtime-root replacement, richer policy preservation, and deny-preserving legacy projection.
- [0.169.0 persisted resume settings](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/app-server/src/request_processors/persisted_resume_settings.rs) and [0.167.2 equivalent](https://github.com/openai/codex/blob/rust-v0.167.2/codex-rs/app-server/src/request_processors/persisted_resume_settings.rs): named profiles preserve identity rather than freezing the resolved profile as an unnamed policy.
- [0.169.0 profile compiler](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/core/src/config/permissions.rs): inheritance compilation and the distinction between builtin and inherited profile conversion.

Validation: the native probe matrix above and Python unit tests for metadata-only
capture, normalization, archived snapshots, owner identity, profile controls, and
the no-model-turn method allowlist. The production relocation code is unchanged.
