# tracr 0.3.0 — session relocation and folder moves

- Relink sessions whose folders went missing, or relocate a session to another existing folder with `l`. A filesystem picker makes choosing the destination easier.
- Move an entire folder, its files and all descendant sessions with `m` in Folders. Relative paths are preserved; recovery journals support interrupted operations.
- Automatically release detached idle Codex sessions after confirming a move. Active and attached sessions stay protected, and Codex's persistent settings stay unchanged.
- See Running, Waiting, Idle and Unloaded states. Running and waiting activity also colors the containing folders.
- Copy detailed failure reports from reusable, scrollable dialogs. Relocation progress and recovery instructions explain what happened.
- Navigate the compact, centered `?` action menu with arrow keys and Enter. Session activity uses relative times, ordered newest first.

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
