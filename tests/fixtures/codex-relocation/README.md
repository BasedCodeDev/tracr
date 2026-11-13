# Synthetic relocation fixtures

`messages.json` defines the user and assistant messages. The runner replaces
`OLD_PATH` with its disposable source directory; both messages must be present
before a fixture is considered valid.

The runner creates legacy and paginated fixtures through Codex, then adds completed
turn boundaries while stopped. Paginated fixtures also seed the version-specific
history database. These writes are confined to newly created temporary homes.
Never use the fixture builder with an existing or real Codex home.

See `docs/codex-relocation-compatibility.md` for validation and limitations.
