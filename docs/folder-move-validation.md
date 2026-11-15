# Folder-move validation

The same-filesystem folder move uses Linux `renameat2(RENAME_NOREPLACE)`
and Codex native session settings. It never copies/deletes a tree or edits a
user's Codex database directly.

## Coverage

- Unit/integration tests in `tests/folder_move.rs`: multiple Codex sources,
  archived descendants, hidden files, symlinks, source/destination conflicts,
  unsupported profiles, exclusive writers, partial updates, recovery before the
  first session update, replaced-directory refusal, zero-session moves,
  cross-filesystem rejection, and coordination locking.
- Terminal acceptance in `tests/fixtures/app-server/folder_move_check.py`:
  parent picker, editable name, preview, cancellation, confirmation, final
  selection and restart recovery.
- `scripts/check_codex_folder_move.py`: disposable native homes with a workspace
  session, a nested archived session and an unrelated control. Both history
  formats run on Codex 0.167.2 and 0.169.0. Scenarios cover successful moves,
  competing writers before rename, and interruption after rename followed by
  recovery. History, archive state, control paths, and a file sentinel are checked.
  Results are recorded in [folder-move-evidence.json](folder-move-evidence.json).

Run `./scripts/check.sh --offline` for standard checks. Add
`--codex /absolute/path/to/codex` to include native folder tests alongside
the existing native acceptance checks. All native fixtures use isolated temporary
Codex homes and synthetic conversations; no model turns are submitted.

## Recovery contract

Before filesystem mutation, the journal records the directory device/inode,
destination-parent identity, complete session mappings, original archive state,
and validated native settings. All affected native writers are acquired before
rename. Archive cleanup is attempted on every normal error exit.

Recovery determines whether rename occurred by inspecting directory identities.
It can finish forward with sessions at either recorded path, but refuses changed
directories, unexpected sessions, settings conflicts, or changed source
configuration. Recovery does not merge directories or automatically roll files
back.

Native unarchive can update the activity timestamp. The pre-mutation inventory
check therefore tolerates that timestamp change only after this operation has
recorded its own unarchive; other metadata and settings remain checked.

The filesystem and Codex are not a single atomic transaction. External programs
can still change files or create sessions while the operation runs. A failed move
may require resolving a reported conflict before recovery can continue.
Cross-filesystem copying and externally moved folder repair are deferred.
