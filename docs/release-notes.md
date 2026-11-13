# tracr 0.1.0 — browse and resume preview

Browse Codex sessions by folder, search session metadata globally, and resume an
active session in the same terminal. `tracr .` opens the current folder and its
subfolders; exiting Codex returns to the browser.

Linux x86_64 and ARM64 musl binaries are provided. Install Codex separately.
Codex CLI 0.167.2 passed isolated legacy and paginated history acceptance on
Linux x86_64. ARM64 receives native automated and fake-provider terminal coverage;
real-Codex ARM64 acceptance is not yet established.

Session relocation is unavailable: Codex can report inconsistent folder metadata
after a directory change. Search covers metadata, not conversation contents.
Archived sessions and fixture sessions are browse-only. macOS and Windows support
are planned, not included in this preview.

Download the matching binary, SHA256SUMS, LICENSE, and THIRD_PARTY_NOTICES.txt.
Verify the binary's checksum before installing it with executable permissions.
See the repository README for installation and configuration.
