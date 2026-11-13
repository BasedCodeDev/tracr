# Browse and resume validation

## Repeatable checks

```sh
./scripts/check.sh --offline
./scripts/check.sh --codex /absolute/path/to/codex
```

Run from any directory. The verifier shares installed-toolchain selection with the
dev launcher and preserves Cargo.lock. Omit `--offline` to attempt locked dependency
fetching. `--codex` opts into isolated real-Codex acceptance; it must name the actual
installed executable. Default checks use only fixture and fake providers.

Stages cover formatting, locked build, all-target Clippy, Rust tests, Python tests,
and terminal cleanup, folder launch, resume, failure paths, and browsing interactions.
Independent stages continue after errors. Terminal stages are blocked if the build
fails, preventing a stale binary from producing misleading passes. Each stage has
a bounded timeout. Logs and a JSON summary are retained in a unique `/tmp/tracr-check-*`
directory. Exit 0 means every requested stage passed; otherwise exit 1. Invalid
shell options exit 2. An unrequested real-Codex stage is not a compatibility claim.

## Current evidence (2026-11-13)

All requested stages passed in the user's normal-terminal run
`/tmp/tracr-check-0m6z4957`. Both legacy and paginated histories passed every live
phase on Codex 0.167.2: fixture, inventory, browser readiness, startup,
session/directory verification, return to tracr, and daemon cleanup.
The result summaries are preserved in [live acceptance evidence](live-acceptance-evidence.json).

- Locked build, formatting, all-target Clippy, and the full Rust suite pass.
- Python probe unit tests pass.
- Compiled terminal checks cover quit/interrupt cleanup, caller-relative folder
  launch, empty folders, resume success/failure, Ctrl-C, SIGTERM, and forced kill
  of a child ignoring SIGTERM.
- Additional PTY checks cover changed-directory review and repeated Enter,
  preparation cancellation, missing destination, direct-input rejection, launch
  failure, archive rejection, global search/clear, resize, repeated refresh, and
  partial-inventory warnings. An unrelated process is kept alive during failure tests.
- Resume checks include restoring the search, selected session, details-pane focus,
  and scroll position after child exit.
- Test-only Rust cases exercise terminal-guard cleanup after setup failure and panic;
  these now pass as part of the Rust suite. No production
  fault-injection switch was introduced.

The user successfully fetched the locked test dependencies. The first full run
exposed a CLI conflict bug: `--codex-home` before the subcommand bypassed the
fixture conflict check. Post-parse validation now rejects that combination, with
regressions for both subcommands and option orderings. The full Rust suite and
all-target Clippy now pass offline. The single ignored Rust test is a child helper
explicitly invoked by the passing terminal-guard parent test.

## Real Codex acceptance

The optional helper constructs validated synthetic legacy and paginated histories
in newly created disposable homes using the existing fixture builder. The builder
returns before relocation. Credentials, user plugins, and user configuration are
not inherited, and inference is never requested. A loopback-only synthetic model
provider is configured. Existing real sessions are never used.

Codex 0.167.2 passes interactive acceptance for both formats in the user's normal
terminal. Earlier sandbox runs were blocked by `Operation not permitted` on the
background-server control socket. The harness does not
silently add `--no-daemon`, approve trust, or submit text to bypass startup prompts.

The real helper saves version, inventory result, terminal evidence, and result JSON
under `/tmp/tracr-real-check-*`. It exits 2 for blocked acceptance. Synthetic history
rendering plus return to the same session is required for a passing interactive run.
An organically generated, sanitized-session manual check remains desirable.

## Remaining acceptance

Foundation, browsing, resume, and live acceptance validation are complete for this
tested version and environment. Broader terminal/version coverage and an organic
session check remain useful release work; synthetic PTY coverage is not a claim
that all user-terminal combinations are validated.
Relinking remains separately disabled by the persistence compatibility verdict.

Recorded combined run: `/tmp/tracr-check-0lp9ysgr/results.json`. Real Codex evidence:
`/tmp/tracr-real-check-jk1g4h9t/results.json`. Subsequent targeted resume tests also
passed the added focus/scroll assertions. Temporary logs are not permanent release artifacts.

## Live acceptance harness follow-up

The following notes describe earlier failures and their fixes. The successful run
recorded above supersedes their requests for another live run.

The live harness now uses a dedicated `prepare_fixture` helper, with no relocation
operation in fixture preparation. Each history-format run has independent phase
results, executable version/hash, expected session ID and directory. Full terminal
bytes and a rendered view are retained; handoff and status checks use offsets into
that evidence rather than clearing it. Readiness requires completed discovery and
fresh metadata. Unexpected terminal closure fails promptly.

A live pass requires foreground ownership, synthetic history rendering, then
matching session ID and directory in the local `/status` output. This is a CLI
inspection command, not a model prompt. The [official CLI command documentation](https://learn.chatgpt.com/docs/developer-commands?surface=cli)
describes `/status` as inspecting the active session. Unsupported or incomplete
status output fails acceptance rather than guessing. Interrupts are sent one at a
time, observing return, one composer-clear redraw, or the CLI's second-interrupt
hint (at most three interrupts). Return checks include
folder/session preservation, refresh, terminal attributes, and foreground ownership.

Only recognized authentication/trust prompts and the observed control-socket
restriction are classified as blocked startup. Wrong metadata, invalid fixtures,
unexplained timeouts, unexpected exits, and cleanup errors fail. Failure takes
precedence over blocked cases: helper exit codes are 0 passed, 1 failed, 2 blocked.
The check runner preserves those codes for the real-Codex stage instead of scanning
its output for generic dependency-error strings.

Cleanup identifies same-user app-server processes by the exact disposable
CODEX_HOME and invokes `codex app-server daemon stop` using that same isolated
environment when needed. It checks for surviving test-home daemons and never stops
an unrelated daemon. Test homes and evidence are retained for inspection.

Unit/simulated-flow coverage includes successful startup, recognized rejection,
wrong session/directory, unknown timeout, cleanup failure, stale output, per-case
state reset, early terminal closure, and failure precedence. These simulations do
not count as live acceptance.

On this machine, both real history formats still pass fixture, inventory, and
browser readiness, then block at the same control-socket restriction. No test-home
daemons remained. Browsing/resume live acceptance stays open. Run from your normal
terminal to obtain the missing evidence:

```sh
./scripts/check.sh --offline --codex /home/seb/.local/share/mise/installs/codex/0.167.2/bin/codex
```

This uses the installed Codex unchanged: no `--no-daemon`, automatic trust approval,
credentials, real sessions, or inference. Synthetic acceptance and manual testing
with ordinary sessions remain distinct evidence.

Latest follow-up evidence is preserved in [live acceptance evidence](live-acceptance-evidence.json).
All standard verifier stages pass; only the requested real-Codex stage is blocked.
The run also exposed an executable-copy race in parallel Rust tests (`ETXTBSY`);
they now share an immutable executable fixture while retaining separate settings/homes.
The refactored relocation probe still reproduces the original listing mismatch in
both formats, confirming that fixture extraction did not change its baseline verdict.

### Corrections from the normal-terminal run (2026-11-13)

The user's run reached real Codex, exposing two fixture/harness defects hidden by
this environment's earlier socket restriction. Startup incorrectly waited for
`Synthetic fixture complete.`, a turn-completion marker rather than the actual
assistant message. Startup now requires the actual user and assistant texts returned
by the fixture builder. The paginated fixture also lacked complete turn metadata:
items were readable, but `thread/turns/list` returned no turns. Synthetic root-turn
identity, item references, timing and rollout bounds are now populated, and fixture
validation requires both messages through turn summaries as well as history reads.
All storage edits remain confined to newly created, stopped disposable fixtures.

The daemon stop command reported successful shutdown, but the previous five-second
process-disappearance deadline expired. The old test-home processes were no longer
running when inspected afterward. Cleanup now waits up to 30 seconds, records elapsed
time and remaining PIDs in `daemon-cleanup.json`, and still fails if any survive.
Tests cover delayed shutdown beyond five seconds and a daemon surviving the new bound.
This change does not establish that cleanup passes on the user's machine; a fresh
normal-terminal run is still needed.

### Status input and updater cleanup correction

The next normal-terminal run passed real startup for both formats. `/status`
remained in the composer, however: the harness sent the entire command and Enter
as one input burst. It now waits for a fresh composer redraw after each character,
then sends Enter separately. This preserves strict session/directory verification;
the user's successful startup alone is not counted as a full acceptance pass.

The remaining PIDs were identified from the disposable homes' `daemon-updater.pid`
records as daemon updaters, rather than the stopped main app servers. After the
supported daemon-stop command, cleanup now terminates only that recorded updater,
verifying its same-user process, exact CODEX_HOME, boot ID and start ticks. A pidfd
pins the identity while signalling to prevent PID-reuse mistakes. SIGTERM gets five
seconds before SIGKILL; all other surviving home processes still fail cleanup.
Tests verify that a mismatched start identity is rejected and an unrelated updater
is untouched. No processes from the user's normal Codex home are targeted.

These fixes need a new normal-terminal live run; local automated tests cannot
establish real interactive success while the control socket remains prohibited.

### Composer clearing during exit

The user run in `/tmp/tracr-real-check-rytfiprx` passed every legacy phase.
Paginated history also passed session/directory verification and daemon cleanup,
but timed out returning to tracr. Its terminal trace showed a residual `s` in the
composer after `/status`; the first interrupt replaced it with the empty-composer
placeholder, without displaying the second-interrupt hint.

The harness now recognizes that fresh composer-clear redraw once before continuing
the observed exit sequence. It sends at most three interrupts and still requires
actual return to tracr; stale placeholders, repeated clearing, unknown responses,
and failure to return after confirmation all fail. Regression tests cover these
paths. This changes only the acceptance harness, not tracr's resume behavior.
The paginated live pass still requires a new normal-terminal run.

## Distribution tooling validation (2026-11-13)

The standard runner passed every requested stage in
`/tmp/tracr-check-vcno5q_i`. After adding download/install verification and draft
publication regressions, the Python suite passed all 34 tests. Both workflow YAML
files parse, shell entrypoints pass `bash -n`, and the version-tag validator accepts
`v0.1.0`. Dependency notice generation succeeds against the cached locked graph.
The ELF verifier rejects the existing dynamically linked application as intended.

The attempted x86_64 musl release build failed with Rust E0463 because the musl
standard-library target is not installed. musl-gcc is also absent. No native musl
artifact, ARM64 run, hosted workflow, or draft release has been verified here.
The publishing preflight correctly fails on this environment's inaccessible Git
metadata. Follow [the release runbook](releasing.md) from the writable checkout;
GitHub CLI authentication must also be restored there.

## Published v0.1.0 preview (2026-11-14)

The corrected [release workflow](https://github.com/BasedCodeDev/tracr/actions/runs/43429321965)
passed native x86_64 and ARM64 checks, static musl packaging, and downloaded-asset
verification. The first attempt exposed the offline development-launcher test's
missing build cache in the final job; preparing its locked debug build resolved it.
The v0.1.0 source tag was not moved.

All final draft checksums verified. The final x86_64 asset is byte-identical to the
binary that passed real-Codex 0.167.2 acceptance for both history formats, including
session/directory verification, return to browser, and daemon cleanup. The curated
[release evidence](release-0.1.0-evidence.json) records its hash and results.

The prerelease is now public. The README's curl-to-sh installer succeeded against
its public assets in a fresh temporary home, and the installed binary matched the
release asset. The local Python suite still passes all 38 tests. ARM64 real-Codex
acceptance remains unverified; relocation remains unsupported.

## Exact-folder session filtering (unreleased)

Folder selection uses exact working-directory equality for session lists.
Folder-tree counts include the selected directory and all its descendants. Child folders remain navigable; global search still spans all
folders and clearing search restores the direct-folder filter. CLI help and the
TUI label describe the new behavior. Regression checks cover child/sibling
exclusion, child selection, empty parents, archived direct sessions, counts, and
empty refreshes. Folder-launch PTY checks reject child sessions, and browse/resume
checks use the new “this folder only” label.

The full standard runner passed in `/tmp/tracr-check-6opt5d6a`; the expanded archive
count regression also passed separately. This is a source change after v0.1.0;
that published release and existing installations retain their original behavior
until upgraded to a later build.

The folder-count follow-up preserves descendant totals independently of exact
session filtering. Tests assert parent totals of two active/three with archives
while showing one active/two archived-inclusive direct sessions, and a parent
with three descendant sessions but no direct sessions. Component-based counting
excludes similarly prefixed sibling paths.

## Navigation while filtering (unreleased)

Arrow keys and Tab/Shift-Tab now operate while search text is being edited.
Up/Down navigate the focused pane; Left/Right switch panes with the same wrapping
as Tab/Shift-Tab. Enter in the folder pane toggles expansion even during search
editing. Enter elsewhere ends editing without resuming; a subsequent Enter can
resume as before. Printable shortcut characters remain search text while editing.
Regression checks cover folder navigation, expansion, pane switching, details
scrolling, session selection, query preservation, and safe Enter behavior.

## Guardian review exclusion (unreleased)

Codex 0.167.2's generated `ThreadListParams` schema has source-kind allowlists,
but no Guardian-specific source filter. Guardian shares `subAgentOther` with
unrelated sessions. The adapter therefore excludes the exact structured
`source.subAgent.other = guardian` marker (and legacy `subagent` spelling) before
adding records to discovery. It also returns no record when a fresh read identifies
Guardian, preventing a stale selection from preparing resume. Titles are not used.

Regression coverage includes a Guardian-only first page followed by ordinary
sessions, active/archive partitions, both source spellings, unrelated subagents,
ordinary sessions named Guardian review, and fresh-read/resume rejection. A
metadata-only run against the real Codex home completed with 20 visible sessions
and zero remaining Guardian source markers. No histories were deleted or resumed.

## Modal session search (unreleased)

The permanent search row has been removed. `/` opens a modal with an input field
and globally matched session titles/paths below it. Up/Down moves the modal's
selection; Enter opens the selected session's exact folder, expands its ancestors,
focuses the sessions pane, and highlights the selected session without launching
Codex. Sessions without an absolute working directory use Unknown folders.
Escape cancels without changing the underlying folder, session, pane, or scroll.
Empty input lists sessions under the existing archive setting; Ctrl-U clears the
input. Search text and selected result are separate from browser navigation state.

Tests cover cancellation, Unicode matching, empty results, result navigation,
missing/unknown folders, archived sessions, narrow layouts, and search-to-folder
navigation without resume. The standard checks passed in
`/tmp/tracr-check-2mtjo7ta`; additional missing/unknown/archive cases passed as a
targeted test. Real-Codex acceptance passed for legacy and paginated histories in
`/tmp/tracr-real-check-zamvt8lu`. Terminal return checks now recognize the footer
instead of relying on the removed search/header row.
