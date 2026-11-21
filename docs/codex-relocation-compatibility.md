# Codex relocation compatibility

## Supported behavior

Production relinking uses native API validation with legacy and paginated
history. Codex version numbers are diagnostic only, including across preview,
execution and recovery. Failed version queries are recorded as `unknown`.
Relinking moves a session's directory association; it does not rewrite history,
move project files, update associated project roots, or maintain a tracr-only path
map. Original directories may exist or be missing. Destinations must exist and be
accessible; inaccessible/unknown source paths and equivalent destinations fail.
The separate Move folder action handles an actual same-filesystem directory rename
and its descendant sessions across configured sources.

Supported permissions are validated legacy read-only/trusted workspace-write and
built-in `:workspace` or `:read-only`, with exactly one runtime root matching cwd.
Workspace configurations must have canonical sandbox flags, no extra writable
roots and no network access; read-only must remain read-only with network disabled.
Custom/full-access profiles, unknown policy forms and multi-root workspaces remain
unsupported. Native acceptance tests provide regression evidence without gating
new Codex releases. API failures and state mismatches use the existing rejection
and recovery flow.

## Live CLI relocation

`tracr move-session ID DESTINATION --live [--from OLD]` uses the shared Codex daemon.
Tracr validates the API responses at runtime. The daemon's reported Codex home
must resolve to the selected source. The exact ID must resolve uniquely through a
complete source inventory. The session must already be loaded, unarchived, and use
a single local environment.
Supported profile/root restrictions above still apply.

An operational proxy attaches using override-free `thread/resume` with
`excludeTurns: true`, validates identity, cwd and recorded/native permissions, then
issues only a cwd update through `thread/settings/update`. It does not acquire an
independent writer, stop tools, interrupt turns, or unload other clients. It polls
resolved runtime settings and verifies native read plus a separate database-first
listing. Model, permission and other returned settings are compared before/after.
Cooperating Tracr operations share the folder-move lock; another Codex client can
still change settings concurrently. Disagreement is unresolved, not rolled back.

Before mutation, a private audit journal records source/session identity, original
and target directories and model/permission settings. The journal is marked complete
only after readback. Transport loss after issuing the update is unresolved.
Live journals are inspection records, not inputs to the exclusive-writer recovery
engine. Inspect native state and the journal before retrying. An active turn can
retain its earlier tool context and existing processes retain their OS cwd, so
branch-closing skills should relocate as their final step and use explicit paths
for remaining work. Project-root/client sidebar changes are not part of this API.

`check_codex_live_move.py` verifies 12 combinations of legacy/paginated history,
legacy/read-only/workspace permissions, and attached idle/active sessions with a
local scripted model. The matrix has passed on Codex 0.170.0 and 0.171.0. It checks history/settings preservation,
continued active status, expected-source rejection, dry runs, repeat invocations,
owner readback and persisted directory/roots after daemon restart. CLI protocol
fault tests cover absent or differing version information, daemon home mismatches,
unloaded/multi-root sessions, API failures, and concurrent settings changes.
The tests move only disposable sessions.

## Native mechanism and invariants

Codex's default scan-based listing can report the original rollout directory after
a native move. Database listing with `useStateDbOnly: true` reports the persisted
current directory. Tracr uses database-first discovery and reconciles default-only
sessions; database failure produces an incomplete-source warning rather than a
false relocation verdict. Earlier blanket claims that relocation was impossible
were superseded by this distinction.

After preview and separate confirmation, acquire a dedicated native writer by
resuming **without a cwd override**. Validate identity, directory, history format,
archive state, settings and permission provenance; update cwd through
`thread/settings/update`. Poll application, close/reopen, and verify database
listing, read and override-free resume. Preserve model/provider, approval settings,
profile, roots, history and session identity. Archived sessions require journaled
unarchive/update/rearchive and archive restoration verification.

Profile provenance is read from the supplied regular rollout file beneath the
canonical configured sessions/archive directory, with identity checks, bounded
metadata-only parsing (8 MiB record, 256 MiB file, five-second waiting), and pinned
owned-settings/legacy-checkpoint precedence. Messages are not retained. Re-read
before writer acquisition; a custom profile silently falling back to a built-in
must never authorize a move. Built-in workspace protections for `.git`, `.agents`,
`.codex` and `.aws` rebind to the new root in the verified native cases.

Private journals use 0600 files in a 0700 directory and record checkpoints before
mutation. Coordination locks serialize operations. `u` performs guarded recovery;
changed identity, paths, settings, profiles or source configuration prevent it.
Journal format 1 remains readable with additive profile/permission-validation
fields. Old journals cannot implicitly authorize built-in or custom-policy changes.

## Folder moves and idle release

Folder moves obtain all affected native writers before a no-overwrite rename.
Batch journals retain device/inode identities, destination-parent identity,
session mappings, settings and archive progress. Recovery finishes forward only
when recorded identities and current inventories agree; it never merges folders,
automatically rolls files back or rewrites Codex storage. Filesystem and session
updates are not one atomic transaction. Cross-filesystem copying, externally moved
folder repair and project-root association changes remain unsupported.

Detached idle runtimes can be released using a dedicated daemon connection,
`thread/resume` with `excludeTurns: true` and transient
`config: {"thread_unload_delay_secs": 0}`, followed by `thread/unsubscribe`.
Ordinary browsing remains passive. The move dialog serializes readiness checks,
retries eligible sessions no faster than five seconds, and cancels before preview,
confirmation or dismissal. Confirmed operations/recovery still require writers.
Release has a 30-second total deadline and two-second unload observation limit.
Attached, active, uncertain, loaded archived and background-terminal states remain
protected in this checkout. Persistent config, other clients and daemons are not
changed. Missing daemon connectivity leaves native writer exclusion authoritative;
errors after connection do not silently fall back.

## Custom-profile constraints

Research found that richer custom profiles can fail native writer acquisition when
the original folder is missing. Existing-folder successes do not resolve that gate.
Destination rules may appear only after restart, removed profiles may silently
fall back, and `extends` does not prove built-in-equivalent protections.

The public resume/profile-list responses do not expose a complete effective
filesystem/network/domain policy. Future support requires a native read-only
source/destination resolver, configuration fingerprints, preserved absolute rules
and unrelated roots, explicit review of substantive permission changes, and
journaled before/approved-after snapshots. Revalidate under the writer and after
restart; do not normalize away unexplained access changes. Never recreate the old
folder, weaken rules, create durable preview sessions or patch real databases to
bypass these constraints.

## Reproduction

```sh
./scripts/check.sh --offline
# Opt-in native acceptance in disposable homes:
./scripts/check.sh --offline --codex /absolute/path/to/codex
python3 scripts/probe_codex_listing.py --codex /absolute/path/to/codex
python3 scripts/probe_codex_profiles.py --codex /absolute/path/to/codex
python3 scripts/probe_codex_relocation.py --codex /absolute/path/to/codex \
  --output target/relocation-report --repeat 2
```

Use `--help` for focused scenarios. The original relocation research probe includes
negative controls/default-listing strategies; its failure is not proof that the
production database-listing sequence fails. A successful research process does not
replace production runtime validation. Retention is documented in
[validation](validation.md#temporary-files-and-evidence).

Rust integration tests cover relocation, folder moves and release. Opt-in native
helpers check both formats, archive restoration, competing writers, stale previews,
history/settings preservation and interrupted recovery using synthetic fixtures.
Synthetic fixture storage construction is not a production migration fallback.
