# Minimal directory-association reproducer

> **2026-11-15 correction:** native relocation passes with `thread/list` using
> `useStateDbOnly: true`. The default listing mode still reports the original
> directory. See the [working mechanism and new evidence](codex-native-relinking-solution.md).
> The older findings below concern default listing, not all native approaches.

Tested with Codex CLI **0.169.0**, Linux. No production session is needed.

## Runnable reproduction

From this repository:

```sh
python3 scripts/probe_codex_relocation.py \
  --codex /home/seb/.local/share/mise/installs/codex/0.169.0/bin/codex \
  --output /tmp/tracr-cwd-repro \
  --strategy resume --scenario ordinary --scenario missing-old --repeat 1
```

This is the minimal harness invocation covering both history formats and the
unlinked-folder use case: four fresh cases. Expected exit: **1**. Each case must
have a validated baseline, `read_cwd: true`, `resume_cwd: true`, and
`listing_cwd: false`. A setup/fixture failure is inconclusive, not this result.
The printed evidence directory contains the exact JSON-RPC transcripts and
`results.json`; it is safe to retain or remove after inspection.

## Essential protocol sequence

Precondition: a disposable persistent thread `ID` with validated history is
listed at existing directory `A`. Directory `B` exists. Close the fixture server.
For the unlinked variant, remove the empty directory `A` first.

Start `codex app-server --stdio` against that **disposable CODEX_HOME**, initialize
with `experimentalApi: true`, and send `initialized`. Then:

```json
{"id":2,"method":"thread/resume","params":{"threadId":"ID","cwd":"B","excludeTurns":true}}
```

Close that server and start a new one with the same disposable home. Initialize
again. Send these requests sequentially, reading each response before the next:

```json
{"id":2,"method":"thread/read","params":{"threadId":"ID","includeTurns":false}}
{"id":3,"method":"thread/list","params":{"limit":100,"archived":false,"modelProviders":[],"sourceKinds":[]}}
{"id":4,"method":"thread/resume","params":{"threadId":"ID","excludeTurns":true}}
```

Page `thread/list` if necessary to find `ID`. `ID`, `A`, and `B` above are
placeholders; the harness creates real synthetic values automatically.

| Response | Expected for native relocation | Observed |
|---|---|---|
| `thread/read` → `thread.cwd` | B | B |
| `thread/list` → matching `data[].cwd` | B | A |
| Override-free `thread/resume` → `thread.cwd` | B | B |

No turn is started by this sequence. Historical text containing A is expected
to remain unchanged; the discrepancy concerns the top-level thread directory.

The harness seeds completed synthetic histories in disposable storage to ensure
valid nonempty legacy/paginated fixtures, and verifies them before this sequence.
Those fixture writes are not part of relocation or a proposed production repair.
An organic sanitized conversation remains a required later acceptance case.
