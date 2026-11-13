# Durable relinking investigation — 2026-11-12

## Verdict

Production relinking remains disabled for Codex 0.167.2. Six strategies were tested
in both legacy and paginated history, twice each, in independent disposable homes.
All 24 fixtures validated before the relocation attempt. No strategy passed the
basic persistence contract.

| Strategy | Sequence | Result in all four trials |
| --- | --- | --- |
| `resume` | Resume with new cwd; restart | Read/resume new cwd, list old cwd |
| `settings` | Load; settings/update cwd; restart | Read/resume new cwd, list old cwd |
| `settings-unloaded` | settings/update without loading | Rejected: thread not found |
| `resume-roots` | Resume with cwd and runtimeWorkspaceRoots; restart | Read/resume new cwd, list old cwd |
| `resume-unsubscribe` | Resume with cwd; unsubscribe; verify unloaded; restart | Read/resume new cwd, list old cwd |
| `project-resume` | Associate project; change project root; resume with cwd; restart | Read/resume new cwd, list old cwd |

The 20 completed update trials preserved the checked session identity, history,
name, creation time, model/provider, Git metadata, history mode, active archive
state, and disposable config file. Directory reversal persisted through read.
These checks do not establish permission/trust preservation or transactional
reversal. The project strategy's reversal restores the thread directory only;
it does not claim to reverse the project root.

## Supported surface reviewed

The installed schema has `cwd` on `ThreadSettingsUpdateParams`, described as an
override for subsequent turns. This is a candidate, not a promise of relocation.
`ThreadMetadataUpdateParams` has no cwd field. Project roots and runtime workspace
roots are separate fields; neither corrected the listing in these experiments.
Forking changes identity, so it does not meet the contract. History reconstruction,
turn execution, and editing production storage are excluded.

The [official App Server documentation](https://learn.chatgpt.com/docs/app-server)
was reviewed on 2026-11-12. It documents resume, metadata updates, project-adjacent
session behavior, and unsubscribe. Unsubscribe removes this connection's
subscription; unloading follows an inactivity delay after the last subscriber
leaves. The probe sets that delay to one second in disposable config and checks
`thread/loaded/list` before restart. This is not proof of cross-client exclusivity.
The public page did not describe `thread/settings/update`; its installed generated
schema supplies the version-specific evidence for that experiment.

Installed aliases `0`, `0.167`, `global`, and `latest` all resolve to `0.167.2` in
mise's Codex installation directory. There is one distinct installed executable;
no newer version was installed or downloaded. The evidence report records its
resolved path, hash, version, aliases, schema hash, and relevant schema fields.

## Reproduce

```sh
python scripts/probe_codex_relocation.py \
  --codex /home/seb/.local/share/mise/installs/codex/0.167.2/bin/codex \
  --output /tmp/tracr-relink-investigation --investigate --repeat 2 \
  --scenario ordinary --scenario missing-old
python -m unittest discover -s tests -v
```

The expected probe exit is **1**, indicating a reproduced compatibility failure.
The explicitly requested missing-old cases are skipped because the basic gate
fails; the report records each skip. Without `--investigate`, original defaults
remain: resume strategy, seven scenarios, both formats, two repetitions. Repeat
`--strategy` and `--scenario` to select cases. Unknown choices are rejected before
starting Codex. Missing required schema fields produce inconclusive cases.
Exit 0 describes only selected checks, never production approval; exit 2 indicates
inconclusive fixture/setup. Missing basic coverage is recorded as an inconclusive
strategy verdict even when the selected individual cases pass.

The versioned compact report is `codex-relinking-investigation-evidence.json`.
Full per-case JSON, RPC transcripts, stderr, and schemas remain at the evidence
path recorded there. Temporary evidence may be removed by the operating system.

## Deferred and completion

The basic gate failed, so shared-transport concurrency, nested batches and unrelated
controls, interruption recovery, guarded reversal conflicts, multi-root preservation,
and workspace-write permission/trust tests remain explicitly deferred. They must
pass before any future production implementation. Synthetic fixtures are not a
substitute for later organically generated sanitized-session acceptance.

The investigation increment is complete with a negative verdict. Durable relinking
is not complete. Next action: obtain an upstream-supported operation or fix that
keeps listing consistent, rerun the basic gates, then implement the deferred matrix.
An upstream issue draft is saved separately and has not been submitted.
