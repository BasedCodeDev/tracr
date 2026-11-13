# tracr implementation plan

Build **tracr**, a Rust terminal application for browsing, finding, resuming, and relinking coding-agent sessions. The first release targets Linux and Codex, with a provider interface for supporting other agents later.

Status: foundation, browsing/search, resume, and validation milestones pass automated checks and normal-terminal live acceptance with Codex 0.167.2 (legacy and paginated histories). Evidence is recorded in [validation status](docs/validation.md). Relocation remains blocked by the reproducible listing/read directory mismatch; release work remains open. See [compatibility findings](docs/codex-relocation-compatibility.md).

## 1. Terminal interface

Use **Ratatui and Crossterm** with three areas:

| Area | Contents |
|---|---|
| Folder sidebar | All sessions, Unlinked folders, and an expandable directory tree with session counts |
| Session list | Title, working folder, last activity, provider, and archive status |
| Details pane | Full title, session ID, working directory, timestamps, source, and available Git metadata |

Default to non-archived sessions ordered by recent activity. Include interactive and non-interactive sessions, identifying their source when available. Provide an archive visibility toggle.

Selecting a folder includes sessions in its subdirectories. Keep full paths distinct, even when folders share a name.

Keyboard controls:

- Arrows or `j/k`: navigate.
- Tab: switch panes.
- `/`: global search.
- Enter: resume.
- `r`: refresh.
- `l`: relink or change folder.
- `?`: help.
- `q`: quit.
- Escape: close the current dialog or clear search.

Preserve selection and filters after refresh, resume, and relocation.

## 2. Global metadata search

Search across:

- Session titles and IDs.
- Working-directory paths.
- Provider names.
- Recorded Git branches.

Use case-insensitive matching with whitespace-separated terms. Every term must match some searchable metadata field.

Search spans all folders while respecting archive visibility. Clearing search restores the previous folder selection.

Keep the index in memory. Conversation text and tool outputs are outside the first release.

## 3. Folder availability and relinking

Check working directories at startup, refresh, and before resume or relocation.

| Status | Meaning |
|---|---|
| Available | The directory exists and is accessible |
| Unlinked | The recorded directory no longer exists |
| Inaccessible | Permissions or another filesystem error prevent access |
| Unknown | No usable local directory was recorded |

Do not assume an inaccessible directory has moved. Do not guess replacement directories or relink automatically.

Support two relocation actions:

- **Selected session:** Change only that session’s working directory.
- **Folder group:** Change every inventoried session beneath an old root, including archived sessions, preserving relative subdirectories.

For example:

```text
Old root: /home/seb/Work/project
New root: /home/seb/Projects/project

/home/seb/Work/project
→ /home/seb/Projects/project

/home/seb/Work/project/packages/api
→ /home/seb/Projects/project/packages/api
```

Match path components, so `/work/app` never matches `/work/apple`.

The relink dialog accepts an existing destination and previews affected sessions, resulting paths, project-root changes, missing subdirectories, and blocking loaded sessions.

Require every destination directory to exist. tracr will not move project files or create missing directories. Existing folders may also be reassigned through a “Change folder” action.

Preserve session identity and history. Old paths inside historical messages remain unchanged.

## 4. Persistent Codex relocation

**Relinking must update Codex’s own persisted association**, not merely tracr’s display or resume arguments.

The inspected Codex protocol exposes a working-directory argument on `thread/resume`, but not a direct working-directory patch on `thread/metadata/update`. Project roots are separate metadata. Durable relocation therefore needs a compatibility proof before implementation is considered complete.

The first milestone will use disposable Codex homes to:

1. Create synthetic sessions with legacy and paginated history.
2. Resume them with a new working directory without starting a turn.
3. Release the sessions and restart the server.
4. Verify that listing, reading, and resuming without an override retain the new directory.
5. Verify unchanged session IDs, history, archive state, and unrelated settings.
6. Test associated project-root updates separately.

Enable relocation only for Codex versions and history formats that pass this contract.

Use supported Codex operations. Do not silently fall back to rewriting internal databases or transcripts. If persistent relocation cannot be established, browsing and resume may be delivered independently, but the requested release remains incomplete.

### Concurrency and recovery

Require affected sessions to be unloaded before relocation. An idle session open elsewhere still counts as loaded. Never terminate or interrupt it automatically.

Use shared-server status where available. An isolated server’s status cannot establish that no other Codex process is using the session. If exclusive access cannot be established, block relocation with an actionable explanation.

Before execution:

- Refresh affected metadata.
- Compare it against the preview.
- Require a new preview if the affected sessions or paths changed.
- Write a durable operation journal.

Apply changes sequentially and verify each through fresh provider reads. A batch is not globally atomic.

On failure:

- Stop processing further sessions.
- Report completed, failed, and untouched sessions.
- Retain the journal across restarts.
- Offer retry and guarded reversal.

Reverse only changes whose current metadata still matches the recorded target, avoiding overwriting subsequent user changes. Report complete success only after durable verification of every affected session and project root.

## 5. Resume in the same terminal

When the user presses Enter:

1. Refresh the selected session and validate its directory.
2. Leave raw mode and the alternate screen.
3. Launch `codex resume <session-id>` with an explicit working directory.
4. Wait for Codex to exit.
5. Restore tracr and refresh the inventory.

Use process arguments rather than shell interpolation.

Do not submit prompts or override model and approval settings. Missing directories lead to relinking. Launch failures return to tracr with an actionable error.

Ensure terminal restoration after normal exit, interrupts, errors, and panic.

## 6. Architecture and storage

Create a single Cargo package and executable named **`tracr`**, with modules for:

- Domain types and path handling.
- Agent providers.
- Application state and search.
- Terminal UI.
- Child-process management.
- Relocation journals and recovery.

Use Ratatui, Crossterm, Tokio, Serde, Clap, structured errors, and XDG directory handling. Commit the dependency lockfile.

### Provider interface

Define:

- `ProviderId`
- `SessionKey { provider, source_id, native_id }`
- `SessionSummary`
- `FolderStatus`
- `ResumeCommand`
- `RelinkPlan`
- `RelinkProgress`
- `RelinkResult`
- `ProviderCapabilities`

Providers implement discovery, refresh, resume-command construction, relocation preview, execution, and verification. Unsupported capabilities remain explicit.

Include the storage source in session identity to prevent collisions across separate agent homes.

### Codex discovery

Use the app-server protocol for listing, reading, and supported metadata operations. Implement initialization, request IDs, pagination, timeouts, cancellation, and process cleanup.

Honor `CODEX_HOME`, falling back to the current user’s default Codex home. Allow additional homes through configuration and repeatable CLI options.

Do not crawl other users’ directories or the whole filesystem.

Fetch active and archived inventories as needed, paginate fully, and deduplicate by `SessionKey`. Display results progressively with an incomplete-discovery indicator. Errors must not erase already loaded results.

Refresh manually and after resume or relocation. No filesystem watcher or background daemon in the initial release.

Store tracr preferences and journals in application-owned XDG directories under `tracr`. Avoid logging session content or reading authentication secrets directly.

Reference: [Codex App Server documentation](https://learn.chatgpt.com/docs/app-server). Protocol observations were made during planning on 2026-11-12; verify them against the supported installed versions during implementation.

## 7. Testing and validation

Use disposable Codex homes for every write test. Never run relocation tests against the user’s real sessions.

### Domain and search tests

- Directory trees, ancestor selection, and identical folder names.
- Component-aware path replacement.
- Missing, inaccessible, symlinked, and non-UTF-8 paths.
- Search matching, ordering, archive filters, and selection persistence.
- Duplicate IDs across storage sources.

### Provider and terminal tests

- Pagination, optional metadata, malformed responses, and disconnections.
- Legacy and paginated histories.
- Archived, non-interactive, and child sessions.
- Missing executable and unsupported protocol capabilities.
- Paths containing spaces and shell metacharacters.
- Terminal restoration after all exit paths.

### Relocation tests

- Single sessions and directory subtrees.
- Archived sessions and associated project roots.
- Existing destination sessions without identity merging.
- Missing destination subdirectories.
- Loaded-session and changed-state rejection.
- Mid-batch failure, crash recovery, retry, and reversal.
- Persistence across server restart.
- Unchanged session identity and history.

Run formatting checks, Clippy, automated tests, and manual terminal validation before release.

## 8. Delivery milestones

- [ ] **Relocation compatibility proof:** Core probe and negative verdict recorded for Codex 0.167.2; listing consistency blocks support. Complete deferred experiment coverage before enabling relocation.
- [x] **Foundation:** Rust package, provider contracts, configuration, fixture CLI, and tests implemented. Rust 1.99.1 build, Cargo.lock, formatting, and application-target Clippy verified. Full Rust tests and all-target Clippy pass. See README.md.
- [x] **Browsing and search:** Live provider, folder/session/details TUI, global metadata search, archive filtering, and CLI listing implemented. Automated browsing checks and real-Codex inventory/browser readiness pass.
- [x] **Resume:** Terminal handoff, fresh-directory validation, source-specific launch, cancellation, and recovery implemented. Both real history formats pass session/directory verification and return to tracr.
- [x] **Relinking investigation:** Six supported-operation strategies tested on installed Codex 0.167.2, both history formats, twice. None passes durable listing consistency. Evidence and an unsubmitted upstream issue draft are in [investigation results](docs/codex-relinking-investigation.md). Advanced safety coverage remains deferred.
- [x] **Folder launch shortcut:** `tracr PATH` and `tracr browse PATH` select a folder and descendants with session focus; empty folders remain navigable. Dev launcher preserves caller directory. Compiled CLI and PTY smoke checks pass; full Rust tests now pass.
- [ ] **Relinking:** Implement previews, validation, journals, execution, verification, and recovery.
- [x] **Validation increment:** Unified runner, Rust/all-target checks, Python tests, and expanded PTY checks pass, including real Codex acceptance.
- [x] **Live acceptance follow-up:** Both legacy and paginated histories pass every phase on Codex 0.167.2, including identity/directory verification, browser return, and daemon cleanup. See [validation evidence](docs/live-acceptance-evidence.json).
- [ ] **Release:** Validate behavior and document installation, usage, compatibility, and limitations.

## 9. Release acceptance criteria

- [ ] Users can browse all sessions exposed by configured local Codex homes without knowing their project paths.
- [ ] Metadata search finds sessions globally.
- [ ] Missing folders appear as unlinked while preserving their sessions.
- [ ] Relinking preserves relative directories and survives a fresh Codex restart.
- [ ] Codex’s own interfaces observe the new folder association after refresh.
- [x] Resuming opens the original session in the correct directory and returns to tracr afterward.
- [ ] Partial relocation failures remain visible and recoverable.
- [ ] Another provider can be added without changing folder navigation, search, or terminal-navigation logic.

## 10. First-release boundaries

Linux only; Rust with Ratatui; Codex only; current-user local storage; metadata search only.

The initial scope excludes full-text search, project-file moves, cloud synchronization, automatic folder matching, session deletion, and an embedded agent chat interface.

## 11. Distribution and platform backlog

- [ ] **Linux preview distribution:** Public `BasedCodeDev/tracr`, MIT attribution
  to Sebastiaan Fehr (seb@actuator.digital), native x86_64/ARM64 musl artifacts,
  checksums/notices, CI and a draft-only GitHub prerelease workflow. Implementation
  and runbook are present; writable Git setup, authentication, native artifact
  verification, hosted CI and the first draft still need confirmation. See
  [release runbook](docs/releasing.md).
- [ ] **macOS support:** Audit Unix process/terminal APIs and configuration paths;
  implement required portability changes; test native Intel and Apple Silicon
  builds, terminal recovery, and real-Codex resume; add matching release assets.
- [ ] **Windows support:** Introduce platform-specific terminal/process handling,
  filesystem/path behavior and configuration discovery; validate native console
  lifecycle and real-Codex resume; add Windows packaging and CI. Do not claim
  support from cross-compilation alone.
- [ ] **Package distribution follow-up:** Evaluate package registries and update
  integrations after the GitHub release contract is established.

The Linux preview does not complete the relocation goal. Keep relocation
compatibility, relinking implementation, and full-release criteria open.
