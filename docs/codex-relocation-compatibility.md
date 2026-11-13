# Codex relocation compatibility

Follow-up: [2026-11-12 relinking investigation](codex-relinking-investigation.md) tested five additional strategies; the basic persistence gate still fails.

## Decision

**Do not enable production relocation for Codex CLI 0.167.2.** The supported
`thread/resume` directory override persists for `thread/read` and subsequent
resume, but `thread/list` still reports the original directory after server
restart. This fails tracr's requirement that Codex's own browsing interfaces
observe the new association.

This is a conclusive negative compatibility result for the tested sequence,
not a claim that every possible Codex operation has been exhausted. No real
sessions were changed. No model inference was requested.

## Reproduce

Python 3.10+ and the installed Linux Codex executable are required. Pass the
actual executable, not a mise/npm launcher; launchers may try to install tools
when run with the isolated HOME.

```bash
python scripts/probe_codex_relocation.py \
  --codex /absolute/path/to/codex \
  --output /tmp/tracr-evidence \
  --repeat 2
python -m unittest discover -s tests -v
```

The probe creates a unique child directory, never reuses an existing Codex home,
and prints the evidence location. Exit codes: 0 means the implemented behavioral
checks passed; 1 means a compatibility failure; 2 means setup or fixture validity
was inconclusive. Even exit 0 does not approve production relocation: untested
coverage in `results.json` must also be resolved.

Each case retains JSON-RPC transcripts, stderr, synthetic storage, and a result
file. The run retains generated schemas, their SHA-256 fingerprint, executable
version, and aggregate results. Temporary evidence may be deleted by the OS;
`codex-relocation-evidence.json` preserves the compact findings in this project.

## Fixture validity and isolation

The harness uses an environment allowlist and disposable HOME, CODEX_HOME, and
XDG directories. Its provider points to loopback port 1, requires no credentials,
and is never invoked. A method allowlist forbids inference and command execution.
Only harness-owned server processes are stopped. Configuration disables analytics
and the OpenTelemetry exporter.

Codex creates the initial session and injects synthetic user/assistant messages.
Injection alone does not create a completed turn. With the server stopped, the
fixture builder adds synthetic turn boundaries and regenerates disposable derived
indexes. For paginated history it seeds completed history rows using the tested
0.167.2 schema. These fixture-only storage writes are **not a migration fallback**.
The relocation operation itself exclusively uses the protocol.

Before testing relocation, every fixture must survive server restart, appear in
listing, expose both expected messages through history APIs, and resume without
a directory override in the original directory. Fixture rejection is inconclusive,
not an unsupported-relocation verdict. These synthetic fixtures do not substitute
for a future test against an organically generated, sanitized conversation.

## Observed behavior

The following results reproduced in both fresh runs for both legacy and paginated
history. The new directory includes a space.

| Scenario | Observation |
|---|---|
| Ordinary relocation | Read and override-free resume retain new directory; listing retains old directory |
| Old folder removed | Same mismatch; missing source directory does not prevent reassignment |
| Archived session | Direct resume rejected with an instruction to unarchive |
| Unarchive → relocate → rearchive | Directory mismatch remains; archive state can be restored |
| Associated project | Project root update is separate; session listing still retains old directory |
| Session already loaded | Resume accepts the request but retains the original directory |
| Independent server holds session | Other server's resume rejected: `already has an active writer` |
| Reversal after unloaded relocation | Original directory restored and observed after restart |

Conversation history, identity, name, creation timestamp, model/provider, recorded
Git metadata, history format, and the disposable config file remained unchanged
in completed relocation cases. This is not a general permission-profile audit:
the fixtures use read-only permissions, and workspace-write trust side effects
remain untested.

The archive-cycle case verifies archived listing after the operation, unarchives
for the override-free resume check, and restores archived state at the end. A
plain archived-resume rejection is an expected protocol behavior, but fails the
naive relocation sequence; it does not invalidate the archive-cycle experiment.

Project experiments change the root before resuming the session and record a
fresh project read after restart. They do not establish atomicity between project
and thread changes or compatibility for multi-root projects.

## Coverage and remaining work

Implemented: 7 scenarios × 2 history formats × 2 fresh runs = 28 experiments,
plus 5 unit tests for environment isolation and RPC safety/error handling.

The core gate fails before production batch work is justified. The following
planned extensions are explicitly deferred and are **not claimed as passing**:

- Two simultaneous clients on a shared server transport.
- Nested folder groups with unrelated-session controls.
- Interruption midway through a batch and guarded reversal after an independent change.
- Multi-root project preservation and thread-only project-update behavior.
- Workspace-write permission roots and trust changes.

The writer rejection is useful evidence of cross-process protection, but does not
prove every shared-client race is excluded. No production exclusivity guarantee
is made.

The investigation has identified a reproducible blocker; the full originally
planned experiment matrix is not implemented. Continue browser foundations
independently if desired, while keeping relocation unavailable. Revisit the
listing inconsistency through an upstream-supported metadata operation or fix,
then complete the deferred matrix before enabling relocation. Do not patch real
Codex databases, rewrite historical text, or silently substitute a tracr-only map.

Reference: [Codex App Server documentation](https://learn.chatgpt.com/docs/app-server).
The installed executable's generated schema is the version-specific evidence;
public documentation can describe newer behavior.
