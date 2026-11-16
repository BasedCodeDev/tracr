# tracr 0.5.0 — worktree creation and live Git navigation

- Create reviewed Git worktrees from folders or selected history commits, with worktree creation offered only where available.
- Browse worktrees in graph order, select their folders from history, and resume sessions directly from the Git pane.
- See live sessions in Git history, linked worktrees marked with `◇`, and live uncommitted changes.
- Navigate all panes consistently with Tab, Shift-Tab, directional Alt-arrow keys, and folder tree shortcuts. Right expands folders before focusing their sessions.
- Use clearer modal controls, Git history scroll indicators, and explicit recovery for blocked session relinks.
- Development launches watch for changes and reload gracefully by default.

Install or update:

```sh
curl -fsSL https://raw.githubusercontent.com/BasedCodeDev/tracr/main/install.sh | sh
```

Linux x86_64 and ARM64 static musl binaries are provided. Install Codex separately.
Relocation is verified with Codex 0.167.2 and 0.169.0, legacy and paginated histories,
and supported built-in permission profiles. Isolated native-Codex acceptance ran
on Linux x86_64; ARM64 receives native automated and fake-provider terminal checks.

Folder moves currently require the same filesystem and a destination that does
not exist. Active sessions, unsupported permission configurations and uncertain
ownership block moves. Loaded archived sessions and sessions with background
terminals require manual closure before relocation. Relinking changes the session's
folder, not historical conversation text; folder moves also rename the files.

This remains a preview. Search covers metadata, not conversation contents.
macOS and Windows binaries are not provided. Installation verifies SHA-256
checksums and replaces the executable without migrating session history.
