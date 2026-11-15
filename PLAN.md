# Remaining work

Current behavior and reproducible checks live in README.md and docs/. Completed
milestones and superseded investigations are available in Git history.

- Review Unicode graph appearance in actual user terminals, especially crossing
  bridges and font fallback; profile wide-history memory and old-HEAD latency.
- Validate native ARM64 releases and satisfy Rust-runtime/distribution-musl license
  notice requirements wherever local packaging remains blocked. Do not bypass gates.
- Broaden native Codex-version and terminal coverage; add a sanitized organic-session
  acceptance case without changing real user sessions during automated checks.
- Keep custom/multi-root permission relinking gated until complete native policy
  previews, missing-directory writer acquisition, explicit consent and guarded
  recovery are verified. See docs/codex-relocation-compatibility.md.
- Design cross-filesystem folder moves, externally moved folder repair and
  project-root association updates before enabling them.
- Evaluate watcher polling fallback for unreliable filesystems and detection of
  repositories initialized while browsing; retain manual/checkpoint refresh meanwhile.
- Validate background-terminal recovery work in its existing feature worktree;
  preserve that unfinished work and relocate the checkout outside /tmp only when idle.
- Audit and implement macOS and Windows terminal/process/path support with native
  acceptance and packaging; cross-compilation alone does not establish support.
- Evaluate package registries and update integrations after release validation.
