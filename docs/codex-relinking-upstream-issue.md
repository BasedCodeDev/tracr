# Draft: persisted thread cwd differs between list and read after restart

Status: draft only; not submitted.

Codex CLI 0.167.2 on Linux returns different working directories for the same
persistent thread after changing its cwd and restarting app-server.

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
are in `codex-relinking-investigation.md` and its evidence JSON. Probe exit 1 is the
expected reproduction outcome. No real Codex storage was edited.

Is there a supported persistent cwd reassignment operation that keeps these APIs
consistent, or is this a listing/index consistency defect? If cwd overrides are
intentionally scoped differently, please clarify which API provides the canonical
persistent association for clients grouping sessions by folder.
