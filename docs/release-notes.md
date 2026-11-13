# tracr 0.2.0 — folder-first browsing and session creation

- Open an exact folder with `tracr .` or `tracr PATH`. Plain `tracr` remembers the last visited folder, defaulting to the current directory on first use.
- Create a Codex session from an empty folder. Choose a Codex home when multiple sources are configured, and return to the browser after exiting Codex.
- Press `/` for global search in a modal with session titles and paths. Enter opens the matching folder and selects the session.
- Folder counts include descendants; the Sessions pane shows only the selected folder's sessions.
- Navigate panes with Left/Right or Escape; Enter expands and collapses folders. Heavy borders identify the focused pane.
- Cleaner session titles, fewer banners, and hidden internal Guardian review sessions.

Install or update:

```sh
curl -fsSL https://raw.githubusercontent.com/BasedCodeDev/tracr/main/install.sh | sh
```

Linux x86_64 and ARM64 musl binaries are provided. Install Codex separately.
Validated with Codex 0.167.2 in isolated homes on Linux x86_64, including
legacy/paginated resume and new-session creation. ARM64 receives native
automated and fake-provider terminal coverage; real-Codex ARM64 acceptance is
not yet established.

Session relocation remains unavailable. Missing folders appear as unlinked and
cannot be resumed until the path exists again. Search covers metadata, not
conversation contents. Archived and fixture sessions are browse-only. macOS and
Windows are not included in this preview.

The installer verifies SHA256 checksums and replaces the installed binary.
No session history is migrated or rewritten. Browser folder preferences are
stored separately under the XDG state directory.
