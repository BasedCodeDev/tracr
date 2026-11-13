> Update: all standard checks and normal-terminal live acceptance pass on Codex 0.167.2 for both legacy and paginated histories. Earlier blocked/pending notes below are historical; see [current validation](validation.md).

# Resume increment

Implemented: Enter in session/details panes prepares a resume using fresh metadata,
validates the destination, and launches Codex in the same terminal. A changed
folder requires review and another Enter. Archived sessions, fixtures, and threads
explicitly rejecting direct input cannot launch. Escape cancels preparation.

The provider constructs an argument vector with an explicit `--cd`, session ID,
and source-specific `CODEX_HOME`. No shell or inference prompt is involved.
Discovery servers are closed before launch and reconnected afterward. Browser
filters, focus, selection, and scroll state are retained.

The child owns a separate foreground process group. tracr restores terminal
attributes and foreground ownership after exit or launch failure. SIGTERM is
forwarded to the owned group, followed by SIGKILL after five seconds if needed.
The development script now defaults to live browsing; fixture mode is explicit.

## Validation

- Offline locked application build, formatting, and application-target Clippy pass.
- Python PTY checks pass for normal exit, nonzero exit, Ctrl-C, and SIGTERM.
  They verify argv, source home, fresh directory, foreground ownership, child
  cleanup, return to the browser, and original terminal attributes.
- Rust tests added for command preparation, missing directories, wrong sources,
  archives, direct-input restrictions, fixtures, and keyboard focus/search rules.
- Full Rust tests and all-target Clippy remain blocked by uncached `fastrand 2.5.0`.
- Real Codex interactive resume acceptance remains pending; fake-process checks
  do not prove compatibility with every Codex CLI version.

Run the independent terminal checks after building:

```sh
python tests/fixtures/app-server/resume_check.py target/debug/tracr
python tests/fixtures/app-server/terminal_check.py target/debug/tracr tests/fixtures/sessions/catalog.json
```

For manual acceptance, run `./scripts/dev.sh`, select an active disposable session,
press Enter, confirm its identity/directory in Codex, then exit and verify tracr's
selection and filters. No message needs to be submitted. Also verify that an
archived session or fixture displays its browse-only reason.

Relinking remains blocked by the separately documented Codex listing mismatch.

Updated consolidated verification and real-Codex results: [validation status](validation.md).
