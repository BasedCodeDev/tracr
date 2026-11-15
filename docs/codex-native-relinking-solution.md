# Native relinking: the listing-mode solution

Investigated 2026-11-15. **A native persisted relocation mechanism works on Codex
0.167.2 and 0.169.0.** The previous conclusion that native relocation was blocked
was too broad: the probe used only the default `thread/list` behavior.

## Working sequence

For an unloaded, unarchived session, call:

```json
{"method":"thread/resume","params":{"threadId":"ID","cwd":"/existing/new/folder","excludeTurns":true}}
```

After releasing the writer and restarting the server, verify with:

```json
{"method":"thread/list","params":{"useStateDbOnly":true,"limit":100,"modelProviders":[],"sourceKinds":[],"archived":false}}
{"method":"thread/read","params":{"threadId":"ID","includeTurns":false}}
{"method":"thread/resume","params":{"threadId":"ID","excludeTurns":true}}
```

All three report the new folder. Alternatively, load the session and call
`thread/settings/update` with `threadId` and `cwd`. This also passes. No extra
model turn, tracr path map, symlink, or direct database/transcript modification is
needed for relocation. History and identity survive.

Archived sessions reject resume directly. The tested sequence is unarchive,
relocate, rearchive; verification temporarily unarchives for resume and restores
the original archive state. That multi-step sequence needs journaled recovery
before being exposed as a product feature.

## Why the earlier tests failed

The distinction is public API behavior: [`useStateDbOnly`](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/app-server-protocol/schema/typescript/v2/ThreadListParams.ts)
selects database-only discovery instead of scan-and-repair listing.

Pinned source at `d32063f96c2dff1dc96af839b3952abb6acfeda4` shows:

- [Default rollout listing](https://github.com/openai/codex/blob/d32063f96c2dff1dc96af839b3952abb6acfeda4/codex-rs/rollout/src/list.rs#L977) reads the initial session metadata; its cwd assignment is at line 1034.
- [The database-only branch](https://github.com/openai/codex/blob/d32063f96c2dff1dc96af839b3952abb6acfeda4/codex-rs/thread-store/src/local/list_threads.rs#L244) queries the state database.
- [Settings events update indexed cwd](https://github.com/openai/codex/blob/d32063f96c2dff1dc96af839b3952abb6acfeda4/codex-rs/thread-store/src/thread_metadata_sync.rs#L335).

Default listing continues to report the original folder even after an additional
completed turn. In these experiments, calling default listing did **not** undo the
new database cwd. The old evidence is valid for the default-list contract; it does
not establish absence of a supported persistent directory update.

## Research and experiments

An [upstream recovery report](https://github.com/openai/codex/issues/24855)
led us to test resuming with an additional turn. See the [issue conversation](https://github.com/openai/codex/issues/24855)
for reports of both turn-based recovery and settings-only updates. The broader
[folder-rename issue](https://github.com/openai/codex/issues/26836) also distinguishes
single-thread updates from atomic project-wide migration. These reports were
leads, not proof for our installed version.

Two independent fixture styles now support the result:

| Experiment | Version | Result |
|---|---|---|
| No-turn relocation, resume/settings × legacy/paginated × ordinary/missing/archive × two repeats | 0.169.0 | 24/24 pass |
| Same no-turn matrix, one repeat | 0.167.2 | 12/12 pass |
| Protocol-created conversations with local scripted responses; resume-only versus extra turn × both formats | 0.169.0 | All four show DB/read/resume at new cwd; default list at old cwd |

The no-turn matrix queries DB listing **before** read or resume on a fresh server,
then default listing, then DB listing again. It checks new-folder filtering,
old-folder exclusion, a second restart, unchanged history, an unrelated thread's
cwd, reversal, and archive restoration. Listing pagination and repeated-cursor
handling have unit tests; these small fixtures are not a scale/performance test.

The first no-turn fixture style seeds synthetic completed history in disposable
storage, as in earlier probes. The independent protocol-created fixtures use real
`turn/start` operations against a loopback scripted HTTP server, with no inference,
credentials, tools, or direct history writes. Only fixture setup and the explicitly
compared extra-turn branch start turns. The successful resume-only branch does not.

See [machine-readable evidence](codex-native-relinking-evidence.json). The first
archived trial rejected direct resume, as expected; the final matrix tests explicit
unarchive/rearchive. An injected control was not discoverable as a completed user
conversation, so its isolation check reads it by ID rather than treating its absence
from listing as mutation. No real user session was changed.

## Reproduce

```sh
python3 scripts/probe_codex_listing.py --codex /absolute/path/to/codex --repeat 2
python3 scripts/probe_codex_organic_listing.py --codex /absolute/path/to/codex
python3 -m unittest discover -s tests
```

Both scripts create fresh temporary homes and print evidence locations. The first
retains the original probe's prohibition on turn/start. The second needs permission
to bind a loopback socket. The no-turn matrix returns nonzero for failed or
inconclusive cases. The organic script records observations for comparison; its
exit code alone is not a compatibility verdict. All 41 Python tests passed.

## Implementation implications

1. Use native current-directory metadata for folder grouping. Prefer database-only
   listing for indexed sessions; establish how to discover unindexed legacy
   sessions without replacing their current directory with the original rollout
   header. A possible fallback is default discovery followed by read-based
   reconciliation, but that completeness strategy still needs tests.
2. Enable relocation only after establishing exclusive ownership. An isolated
   app-server's loaded list is insufficient; shared-server sessions and independent
   writers must be tested. Never silently relocate an idle session open elsewhere.
3. Add one-session and folder-group previews, existing-directory validation,
   descendant path mapping, archived-session restoration, durable operation journals,
   sequential verification, partial-failure reporting, and guarded reversal.
4. Verify workspace permission roots, trust, multi-root projects, nested groups,
   interrupted operations, and normal CLI handoff. Do not assume Desktop sidebar
   grouping changes: clients using default listing may still show the old folder.

The subsequent [single-session implementation](relinking-validation.md) now enables eligible unlinked sessions after native writer, permission, journal, and UI checks. Folder-group relocation remains deferred.
The persistence blocker is resolved for the tested API mode; this research does not
claim that the whole relocation feature is already implemented. No commit, push,
release, or upstream issue submission was performed in this research increment.
