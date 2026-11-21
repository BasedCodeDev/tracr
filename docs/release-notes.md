# tracr 0.7.0 — coding drawer, Git inspection, and session tools

- Keep coding sessions central with a responsive drawer, searchable actions, clearer shortcuts, and improved keyboard paging. Open sessions in multiple Tracr instances with independent terminal sizes.
- Browse full-screen Git history with commit details beside it. Inspect changes and references, follow branch-grouped sessions and live terminal previews, and navigate without losing the previous preview while new data loads.
- Search conversation contents with opt-in deep search, alongside the existing metadata search.
- Query worktrees and live agent activity with `tracr worktrees [PATH]`, using JSON by default or `--text` for a table. Check `complete` before treating an empty result as authoritative.
- Move session directory associations with `tracr move-session SESSION_ID DESTINATION`. Use `--dry-run` to validate, `--from` to check the starting directory, and `--live` for verified relocation through a shared Codex daemon. Output is JSON; project files are not moved by this command.
- Delete an individual Codex conversation from Sessions while preserving its folder and other sessions. Discard linked feature worktrees with explicit confirmation and verified conversation deletion before filesystem cleanup. These are permanent operations; loaded clients and uncertain state can block them.
- Improve startup responsiveness, folder navigation, live worktree labels, and terminal dialogs. New Codex sessions enable automatic approval review.

Install or update:

```sh
curl -fsSL https://raw.githubusercontent.com/BasedCodeDev/tracr/main/install.sh | sh
```

Linux x86_64 and ARM64 static musl binaries are provided. Install Codex separately.
This remains a preview; macOS and Windows binaries are not provided. Installation
verifies SHA-256 checksums and replaces the executable without migrating session
history.

Folder moves require the same filesystem and a destination that does not exist.
Unsupported permission configurations, active sessions, and uncertain ownership
can block moves. Session relocation changes the directory association, not
historical conversation text; folder moves also rename project files.

Acceptance was exercised with Codex 0.171.0 on Linux x86_64, including legacy
and paginated history, live status, shared sessions, relocation, folder moves,
deep search, and conversation/worktree deletion in disposable fixtures. Both
Linux architectures receive native automated checks; real-Codex acceptance
coverage is limited to x86_64.
