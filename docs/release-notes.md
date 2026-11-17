# tracr 0.6.0 — embedded terminals and session navigation

- Open sessions full-screen with Enter, or embedded in the Sessions pane with Alt+F. Toggle views without restarting Codex, and use Ctrl+B to return to the list while the session stays running.
- Keep terminal views per folder, browse other folders while sessions run, and switch between open terminals.
- Scroll through terminal history, select text with the mouse, and copy explicitly. Preserve the child application's keyboard, paste, focus, and mouse protocols.
- In embedded mode, Tab and Shift+Tab navigate Tracr panes. In full-screen mode, Tab, Shift+Tab, and Alt+arrow keys reach Codex; use Alt+F to return to the embedded view.
- Focus a folder subtree with Alt+Z, collapse Folders when needed, and use layouts that adapt to narrow or short terminals. Folder selection stays centered while scrolling.
- Browse saved stashes as single labeled Git history nodes beside their base commits.
- Create worktrees with a single approval, and use m consistently to move either a folder or a selected session.

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
