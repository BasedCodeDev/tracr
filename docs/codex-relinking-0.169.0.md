# Native relinking investigation — Codex 0.169.0

> **2026-11-15 correction:** native relocation passes with `thread/list` using
> `useStateDbOnly: true`. The default listing mode still reports the original
> directory. See the [working mechanism and new evidence](codex-native-relinking-solution.md).
> The older findings below concern default listing, not all native approaches.

## Verdict

**Native relinking remains unavailable.** The session and folder-group feature
still requires Codex's own list, read, and override-free resume to agree on the
new directory. No tracr-only path map or production storage rewrite is introduced.

On 2026-11-14, 32 disposable fixture cases validated and then failed that
compatibility gate. The [versioned evidence](codex-relinking-0.169.0-evidence.json)
records executable/schema fingerprints, the pinned upstream source commit,
per-case checks, and retained evidence locations. These results supplement,
rather than replace, the [0.167.2 investigation](codex-relinking-investigation.md).

## Results

Each strategy ran twice in both legacy and paginated history formats (24 cases):

| Strategy | Result in all four trials |
|---|---|
| Resume with cwd override | Read/resume new directory; list original directory |
| Load, then settings/update cwd | Same mismatch |
| Settings/update while unloaded | Rejected: thread not found |
| Resume with cwd and runtime workspace roots | Same mismatch |
| Resume, unsubscribe, observe unload, restart | Same mismatch |
| Update project root, then resume with cwd | Same mismatch |

The investigation intentionally skipped expanded scenarios after those basic
gates failed. A separate baseline/missing-folder run bypassed that early gate:
ordinary and missing-old scenarios, twice per format (8 additional cases).
All eight preserved the read/list discrepancy, including all four cases where
the original directory had been removed.

All 28 completed update cases preserved the checked identity, history, metadata,
archive state, and disposable configuration. Directory reversal persisted through
read. This is not proof of permission/trust preservation or transactional rollback.
The four rejected unloaded-settings attempts did not complete relocation.

## Version-specific interface and source review

The generated experimental schema still exposes cwd on resume and settings
updates, but not on `ThreadMetadataUpdateParams`. That metadata method accepts
project association, Git metadata, and the Daybreak preference. `project/import`
accepts thread IDs and project roots, not a per-thread working directory. Fork
and start create another identity and cannot satisfy this feature's contract.
The internal workspace-switch helper concerns account-workspace runtime teardown,
not a public project-directory relocation method.

Source reviewed at tag `rust-v0.169.0`, commit
`d32063f96c2dff1dc96af839b3952abb6acfeda4`:

- [Listing builds responses from stored threads](https://github.com/openai/codex/blob/d32063f96c2dff1dc96af839b3952abb6acfeda4/codex-rs/app-server/src/request_processors/thread_processor.rs#L2893), with [cwd taken from the stored thread](https://github.com/openai/codex/blob/d32063f96c2dff1dc96af839b3952abb6acfeda4/codex-rs/app-server/src/request_processors/thread_processor.rs#L6731).
- [Resume prefers the latest owned settings snapshot's cwd](https://github.com/openai/codex/blob/d32063f96c2dff1dc96af839b3952abb6acfeda4/codex-rs/app-server/src/request_processors/thread_processor.rs#L4185), falling back to session history metadata.

These distinct read paths are consistent with the observed discrepancy. This is
not a claim that the complete underlying indexing defect has been proven, nor
that every possible native operation has been exhausted. Upstream clarification
is needed on the canonical directory association and a supported reassignment API.

## Reproduce

Run from this repository with Python 3.10+ and the actual Linux Codex executable:

```sh
python3 scripts/probe_codex_relocation.py \
  --codex /home/seb/.local/share/mise/installs/codex/0.169.0/bin/codex \
  --output /tmp/tracr-relink-0169-investigation \
  --investigate --repeat 2 --scenario ordinary --scenario missing-old

python3 scripts/probe_codex_relocation.py \
  --codex /home/seb/.local/share/mise/installs/codex/0.169.0/bin/codex \
  --output /tmp/tracr-relink-0169-missing \
  --strategy resume --repeat 2 --scenario ordinary --scenario missing-old
```

Both commands return **1** for the reproduced compatibility failure, not a test
harness crash. Every run creates a unique disposable child directory; neither
command uses the caller's Codex home or credentials. The loopback provider is
never invoked. Fixture preparation seeds synthetic completed history only in
these disposable homes; relocation itself uses supported protocol requests.

See the [minimal reproducer](codex-relinking-0.169.0-reproducer.md) and
[upstream issue draft](codex-relinking-upstream-issue.md), which remains unsubmitted.
The existing 38 Python tests passed, including probe isolation and RPC allowlist
checks. No production code or release was changed by this investigation.

## Gate for implementation

Once a supported native operation passes list/read/resume persistence, complete
the deferred safety matrix before enabling either action: shared-server and
independent-writer exclusion; archived-state preservation; nested groups and
unrelated controls; interruption journals and guarded reversal; multi-root
projects; workspace-write permission roots and trust. Validate against an
organically generated, sanitized conversation as well as synthetic fixtures.

The accepted feature remains: relink one unlinked session, or every inventoried
session below a missing root (including archives) while preserving relative
subdirectories. Require existing destinations, complete inventory, preview and
confirmation, durable journaling, per-session verification, and actionable partial
failure reporting. Do not move project files, guess destinations, or silently
substitute a tracr-only override.
