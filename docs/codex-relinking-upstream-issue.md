# Draft: persisted thread cwd differs between list and read after restart

> **2026-11-15 correction:** native relocation passes with `thread/list` using
> `useStateDbOnly: true`. The default listing mode still reports the original
> directory. See the [working mechanism and new evidence](codex-native-relinking-solution.md).
> The older findings below concern default listing, not all native approaches.

Status: draft only; not submitted.

Codex CLI 0.169.0 on Linux (also reproduced on 0.167.2) returns different working directories for the same
persistent thread after changing its cwd and restarting app-server.

## Latest evidence

The 0.169.0 investigation validated 32 fresh synthetic fixtures: six strategies
× two history formats × two repeats (24), plus separate ordinary/missing-folder
resume trials × two formats × two repeats (8). All failed the native persistence
gate. Twenty-eight completed updates retained the read/list discrepancy; four
unloaded-settings attempts returned thread-not-found. No model prompts were sent.

The executable and generated-schema fingerprints, individual results, and
upstream source commit are in [the 0.169.0 evidence](codex-relinking-0.169.0-evidence.json).
A [minimal runnable reproduction and RPC sequence](codex-relinking-0.169.0-reproducer.md)
is provided separately. The issue remains a draft; no upstream report was sent.

## Reproduction

Use an isolated HOME/CODEX_HOME with an existing persistent thread whose directory
is A. Ensure the thread appears in thread/list, has readable history, and resumes
without a cwd override in A. Stop that server, then:

1. Start app-server and initialize with experimentalApi enabled.
2. Send `thread/resume` with `{threadId: ID, cwd: B, excludeTurns: true}`.
3. Close app-server and start a fresh server against the same disposable home.
4. Call `thread/read`, page `thread/list` with empty modelProviders/sourceKinds
   filters, and call `thread/resume` with the ID and no directory override.

Expected for durable reassignment: all three APIs agree on B.
Observed: read and resume report B; listing reports A.

The same result occurs when loading the thread and using `thread/settings/update`
with `{threadId: ID, cwd: B}` instead. Explicit unsubscribe and observed unload,
`runtimeWorkspaceRoots: [B]`, and updating an associated project's roots do not
resolve the mismatch. Settings updates on unloaded threads return thread-not-found.

Both legacy and paginated histories reproduced this twice per strategy. No turns
were started. Session identity and the checked history/metadata remained unchanged.
The fixtures are synthetic: completed turn boundaries and paginated history rows
are seeded only while the disposable server is stopped. Every fixture is validated
through listing, read, history, and override-free resume before attempting relocation.
An organically generated sanitized reproduction remains a useful follow-up.

The repository harness command and exact version/schema/executable fingerprints
are in [the current investigation](codex-relinking-0.169.0.md) and its evidence JSON. Probe exit 1 is the
expected reproduction outcome. No real Codex storage was edited.

Is there a supported persistent cwd reassignment operation that keeps these APIs
consistent, or is this a listing/index consistency defect? If cwd overrides are
intentionally scoped differently, please clarify which API provides the canonical
persistent association for clients grouping sessions by folder.

## Source observations (0.169.0)

At upstream commit `d32063f96c2dff1dc96af839b3952abb6acfeda4`, listing converts
stored thread records and uses their cwd, while resume prefers the latest owned
settings snapshot's cwd. These distinct paths fit the observed discrepancy;
this is not a complete root-cause proof. The generated metadata-update schema
still has no cwd patch. See [the pinned source review](codex-relinking-0.169.0.md#version-specific-interface-and-source-review).

Requested regression coverage: an existing session reassigned A→B, optional removal
of A, server restart, and agreement between list/read/override-free resume in both
history formats, without changing identity or historical text. Please clarify
whether directory overrides are intentionally separate from the listed association
and, if so, which supported API can persistently reassign that association.
