# Building and releasing tracr

The first distribution is a **draft prerelease** for public repository
`BasedCodeDev/tracr`. It is a browse-and-resume preview, not completion of the
original relocation goal. No workflow publishes a draft automatically.

## Repository and authentication setup

Git is installed and already uses `Sebastiaan Fehr <seb@actuator.digital>`.
The managed workspace exposes an empty, read-only `.git`; do not initialize over
it or discard real history. Run the following from a normal terminal in the real,
writable checkout. If no checkout exists, clone the existing remote into a new
writable directory and copy the project changes there. Only initialize a new Git
repository if both local history and the remote are confirmed absent.

```sh
gh auth login --hostname github.com --git-protocol https --web
gh auth setup-git --hostname github.com
gh auth status
git rev-parse --show-toplevel
git status --short
git remote -v
gh repo view BasedCodeDev/tracr
```

Authentication failure is not evidence that a repository does not exist. Once
authenticated, reuse an existing repository, history, and default branch. For a
confirmed new project only, use `git init -b main` in the writable source checkout,
then `gh repo create BasedCodeDev/tracr --public --source . --remote origin`
without `--push`. For an existing repository lacking an origin, add
`https://github.com/BasedCodeDev/tracr.git`; never overwrite a different origin.

Before committing or pushing, review `git ls-files`, `git status --short`, and
`git diff --cached`. Stage explicit project paths, not the whole directory.
Exclude `.aws`, `.codex`, `.agents`, credentials, caches, compiled binaries, and
raw session/terminal evidence. Ignore rules do not untrack existing files.
Preserve the curated synthetic validation summaries. Run
`./scripts/publish-preflight.sh` to check Git metadata access, identity, account,
origin, remote access, and tracked local-only directories. This command is
read-only and does not replace the staged-content review. Inspect all staged content,
commit it, and push the existing branch (or `main` for a new repository).

## Native builds

Requires Python 3.11+, binutils, the exact Rust version in `rust-toolchain.toml`,
the matching Rust musl target, and `musl-gcc`. On Ubuntu install `musl-tools` and
`binutils`. Build each architecture on a matching native machine; packaging runs
the binary and terminal tests, so cross-compilation alone is insufficient.

```sh
rustup show
rustup target add x86_64-unknown-linux-musl
rustup component add rust-docs
CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
  ./scripts/release.sh build --target x86_64-unknown-linux-musl --output dist/x86_64
```

For ARM64 substitute `aarch64-unknown-linux-musl` and
`CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER`. Use a fresh output directory;
existing outputs are not overwritten. `--offline` forbids dependency fetching.
`TRACR_TOOLCHAIN=stable` is permitted only when that installed toolchain's reported
version exactly matches the pin. Release packaging never chooses a different
Rust version as a fallback.

Packaging validates architecture, static linkage, version, fixture inventory and
all five PTY checks. It includes verbatim dependency license texts from the
resolved Cargo graph (a superset including test dependencies). Missing license
texts fail packaging and require review. Rust standard-library and Ubuntu musl
copyright notices are also included; local packaging expects their installed
documentation, as provided by the CI prerequisite steps. Target artifacts contain their own
checksums; assembly verifies both before producing the combined SHA256SUMS.

## Draft workflow

1. Update Cargo version, the default preview version in `install.sh`, and release
   notes together; keep Cargo.lock committed. The installer downloads an explicit
   version so GitHub prereleases work without a latest-stable API dependency.
   Its default version is checked by the Python test suite. After the first public
   release, remove the README notice that binary installation is not yet available.
2. Run `./scripts/check.sh` and build/test both native musl artifacts. CI performs
   these checks on Ubuntu 24.04 x86_64 and ARM64, with exact pinned Rust.
3. Commit the release changes, create an annotated `v0.1.0` tag for the first
   preview, and push that tag. The workflow also accepts manual dispatch with an
   existing version tag; it resolves the tag to a commit before building.
4. The workflow creates or refreshes a draft prerelease after both builds pass.
   A version mismatch, failed check, missing artifact, checksum mismatch, or
   existing published release blocks the job. Reruns replace assets only in drafts.
5. Download assets with `gh release download v0.1.0 --repo BasedCodeDev/tracr
   --dir /tmp/tracr-release-review` (choose a new directory), and run
   `sha256sum -c SHA256SUMS` there. Install the native binary to a temporary bin
   directory with mode 755; confirm `--version`, fixture listing, and terminal use.
   Alternatively run `./scripts/release.sh verify --target x86_64-unknown-linux-musl
   --input /tmp/tracr-release-review` from the matching release checkout. This
   checks all checksums and exercises a temporary installation without changing
   your PATH. The draft job runs this check after artifact download too.
6. Before publishing, run the existing real-Codex helper against the downloaded
   x86_64 release binary in a normal terminal:

```sh
python3 tests/fixtures/app-server/real_check.py \
  /absolute/path/to/downloaded/tracr-linux-x86_64 /absolute/path/to/codex
```

Make the downloaded binary executable first. Require all phases to pass for both
history formats. Record results for this release artifact; earlier debug-binary
acceptance is supporting evidence, not a substitute. No inference or real home is
used. ARM64 real-Codex coverage remains explicitly unverified until run there.

Review the draft notes and assets, then publish deliberately through GitHub or
`gh release edit v0.1.0 --repo BasedCodeDev/tracr --draft=false --prerelease`.
Do not edit assets on a published release; ship a new version for corrections.
No crates.io, AUR, Homebrew, mise registry, or automatic updater integration is
part of this slice.

## Publishing setup status

GitHub authentication for BasedCodeDev was verified outside the managed sandbox.
That check confirmed this project had no Git repository; the empty read-only
`.git` seen inside the sandbox was an isolation artifact. A new `main` repository
has now been initialized from the reviewed project source.

Local musl targets and musl-gcc were absent at the last packaging attempt. Native
artifact builds, hosted CI, and the first draft must pass before declaring
release distribution complete. A public source push does not publish a binary
release.

## One-command installer

The public entrypoint is `install.sh` on `main`, served by raw.githubusercontent.com.
It supports POSIX sh on Linux x86_64 and ARM64, downloads the selected release's
binary and license notices, and verifies each against that release's SHA256SUMS.
It checks the binary version before replacing an existing installation via a
same-directory rename. No Rust, GitHub CLI, root access, or shell-profile edits
are needed. `TRACR_VERSION` selects a published `vMAJOR.MINOR.PATCH` tag;
`TRACR_INSTALL_DIR` overrides the default `~/.local/bin`. Notices use
`${XDG_DATA_HOME:-$HOME/.local/share}/tracr`.

Tests execute the installer through sh stdin with isolated homes and stubbed curl,
cover both architecture aliases, unsupported systems, custom directories,
unavailable releases, checksum failures, version mismatches and cleanup. Before
publishing a release, also run the installer against the actual published assets
in a temporary home; stubbed-download tests do not establish release availability.
