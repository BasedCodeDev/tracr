# tracr 0.4.0 — Git history and worktree browsing

- Browse Git repositories and their registered worktrees alongside sessions, including checkouts outside the project folder.
- Explore cached, colored Unicode commit graphs with branch and tag labels, progressive history loading, and horizontal scrolling. Press `g` to toggle history and `f` to follow the selected checkout's HEAD.
- Refresh active repository branches, worktrees, and history automatically through native metadata notifications, with low idle CPU use. Manual navigation preserves the selected commit.
- Choose registered worktrees as session relink destinations. Folders and Worktrees retain separate paths, and the destination is revalidated before the existing confirmation preview.
- Use directional arrow navigation without wrapping between the folder and session panes.

Native watching does not poll Git or recursively watch project files. If notifications
are unavailable, use `r`, re-enter the repository, or reopen history to refresh.

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
