# Automatic idle release before relocation

Implemented for Codex 0.167.2 and 0.169.0. Release happens while the user has the folder-move destination dialog open,
and after confirmed relinks, folder moves, and recovery. Ordinary browsing and
passive status monitoring never resume sessions. The move dialog serializes
guarded checks, retries eligible idle sessions no more frequently than every
five seconds, and cancels its worker before preview, confirmation, or dismissal.

A dedicated connection to the existing daemon checks fresh loaded status,
identity, directory, archive state, recorded permission profile, and background
terminals. For eligible idle sessions it calls `thread/resume` with only
`threadId`, `excludeTurns: true`, and the per-runtime override
`config: {"thread_unload_delay_secs": 0}`, then `thread/unsubscribe`.
Codex replaces a detached idle runtime; an attached or active runtime retains its
existing lifecycle. No persistent config is edited, model turn started, or session
archived to force release.

Release has a 30-second total deadline and a two-second unload observation limit.
The operational connection is dropped and its byte proxy killed/reaped on success,
failure, timeout, or cancellation. Other clients and the daemon stay untouched.
If no daemon connection can be established, the existing exclusive native writer
check remains authoritative. Errors after connection do not silently fall back.
All directory changes still require exclusive native writers. Batch moves acquire
all writers before renaming files. A racing client therefore still blocks mutation.

Loaded archived sessions and background terminals block automatic release because
preserving those runtime states is not yet verified. Unloaded archived sessions
retain the existing move and archive-restoration support. Unknown profiles,
changed metadata and unsupported versions remain blockers.

## Evidence

[Native evidence](idle-release-evidence.json) records 20 passing cases across both
versions and both legacy/paginated history formats: detached session relocation,
attached-session refusal, folder move, Workspace and Read Only profiles.
The tests check history and permission/model settings, directory outcomes,
unchanged persistent configuration, and restoration of the default unload delay
on a later ordinary resume. Successful fixture moves took approximately 0.5–0.9
seconds; attached cases stopped after approximately 2.3 seconds. These are local
fixture timings, not performance guarantees.

The native harness uses isolated homes, synthetic history, a local TCP app server,
and a fixture-only proxy forwarding bytes to that server. Production uses Codex's
`app-server proxy`. The fixture bypasses the machine-global Unix daemon directory
so acceptance cannot join or modify the user's existing daemon.

Rust regression tests cover busy/waiting/unavailable states, already-unloaded
sessions, archived/background-terminal blockers, attached and racing owners,
recovery, cancellation, timeout, and refusal to rename a blocked folder. Terminal
acceptance covers confirmation before release, progress and copyable errors.
Existing passive-monitor tests continue to forbid resume and model turns.

## Reproduce

Run the normal checks:

```sh
./scripts/check.sh --offline
```

Include the native matrix for a supported binary:

```sh
./scripts/check.sh --offline --codex /absolute/path/to/codex
```

That adds the `real-release` stage alongside the existing native checks. For a
focused run, build `cargo test --test release --no-run` and pass its reported test
executable to `python3 scripts/check_codex_release.py --codex /path/to/codex
--test-binary /path/to/release-test`. Localhost sockets must be permitted.
