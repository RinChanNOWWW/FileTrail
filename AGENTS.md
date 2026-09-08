# Working on Filetrail

Filetrail is a Rust CLI and background file synchronization daemon distributed as
one executable for macOS and Linux. Read README.md before changing its behavior.

## Development

- README.md is the primary English README; README_zh.md is the Chinese version.
  Every README change must update both files in the same change, keeping behavior,
  examples, and section coverage equivalent. Preserve their language-switch links.
- Use the exact toolchain in rust-toolchain.toml. Keep Cargo.lock checked in.
- Building requires a C compiler for vendored libgit2 and SQLite. The distributed executable
  needs no separate Rust installation, and its built-in commands need no system Git.
  Only the explicit `filetrail git` passthrough and its integration tests require Git on PATH.
- Run `cargo fmt --all`, `cargo clippy --locked --all-targets -- -D warnings`,
  `cargo test --locked --all-targets`, and `cargo test --locked --doc`.
- Completion integration tests require Bash, Zsh, and Fish on PATH.
- Run `taplo fmt` and `taplo fmt --check` with taplo-cli 0.10.0.
- Run `taplo lint Cargo.toml` to validate the manifest schema. `.taplo.toml` selects
  `.schemas/cargo.json`, which works around Taplo's loading of external references
  inside `anyOf` while preserving the complete official Cargo schema validation.
- Every Rust import must be its own `use` statement. Do not use grouped braces.
  rustfmt's `imports_granularity = "Item"` enforces this on the pinned nightly.
- Keep TOML keys alphabetically sorted, particularly `[package]` and dependency
  names in Cargo.toml. Use the repository's .taplo.toml; do not format Cargo.lock.
- Put filesystem/Git regression tests in tests/workflow.rs using temporary
  directories and a temporary Git identity. Never test against real dotfiles.
- Keep OS service installation out of automated tests. Test rendered definitions.
- Do not introduce a dependency on an external `git` executable for core commands.

## Publishing to crates.io

- PR CI runs `cargo publish --dry-run --locked --registry crates-io` on macOS and
  Linux without credentials. The final `aborting upload due to dry run` warning is
  Cargo's expected confirmation that nothing was uploaded.
- `.github/workflows/publish.yml` runs on pushed `v*` tags and reuses the complete
  CI workflow. Only after all checks pass does it require an exact
  `v<package.version>` match (for example, `v0.1.0`), then publish to crates.io.
- After crates.io publishing succeeds, the workflow creates a draft GitHub
  Release with generated notes. It attaches the tested macOS and Linux binaries
  as `filetrail-v<version>-<Rust target>.tar.gz` archives, including both READMEs
  and the license, plus a `SHA256SUMS` file. Archives preserve executable permissions.
  Publish the GitHub Release manually after reviewing it. A rerun can replace
  assets on an existing draft but refuses to modify a published release. If this
  job fails after crates.io publishing, rerun only failed jobs to avoid publishing
  the same crate version again.
- In GitHub repository Settings → Environments, create `crates-io`, allow release
  tags matching `v*`, and add an environment secret named `CARGO_REGISTRY_TOKEN`.
  Its value must be a crates.io API token, not a GitHub personal access token.
  No extra GitHub credentials are needed. Only the draft Release job grants the
  built-in `GITHUB_TOKEN` `contents: write`; other jobs use `contents: read`.
- The crates.io account must have a verified email and permission to publish
  `filetrail`. The token needs permission to publish new crates for the first
  release, and publish new versions for later releases. Scope it to `filetrail`
  where supported and set an expiration date. Create tokens at
  <https://crates.io/settings/tokens>.
- Update `Cargo.toml` and the root package version in `Cargo.lock` together,
  commit the release changes, then push the matching tag. For version `0.1.0`:
  `git tag v0.1.0` followed by `git push origin v0.1.0`.
  A published version cannot be overwritten; each new release needs a new version.
- For local verification of uncommitted changes only, add `--allow-dirty` to the
  dry-run command. CI and actual publishing deliberately require a clean checkout.

## Architecture and invariants

Core modules: `config.rs` manages mappings, `state.rs` persists sync state,
`sync.rs` and `daemon.rs` handle synchronization, and `git.rs` handles Git operations.
Consult the source and tests for implementation details.

- CLI, daemon, and services share the same data directory and serialize mutations.
- Keep sync destinations inside the configured repository. Never follow symlinks
  outside it or modify `.git` through synchronization.
- Protect external destination edits. Deletion is opt-in and limited to managed
  files; an unavailable source directory must never trigger mass deletion.
- Background sync never stages or commits. Explicit commits include only managed
  changes and must preserve the user's existing staging.
- Validate before writing, update state atomically, and keep dry runs read-only.
  Never silently discard corrupt state.
- Shell setup must preserve existing user configuration and safely quote paths.

## Using Filetrail as an agent

Use `filetrail --help` and subcommand help to discover the installed CLI. Select
an isolated profile with `--data-dir <directory>` when testing. This directory
must be outside the source and target repository.

Example real-user setup (execute only when the user requests configuration):

```sh
filetrail init ~/dotfiles --subdir macos
filetrail add ~/.zshrc
filetrail add ~/.config/nvim
filetrail daemon start
```

On Linux, use `--subdir linux`, or omit it to put the reserved directories at the
repository root. Home sources go under `__HOME__`; external sources go under
`__ROOT__`, preserving their original relative paths. Custom targets are not
supported. Paths passed to `diff`, `commit`, and `resolve` are repository-relative.
For example, `/opt/scripts/build.sh` with `--subdir macos` is stored at
`macos/__ROOT__/opt/scripts/build.sh`.

Use `retarget <repository> [--subdir <path>]` to change the destination while keeping
sources; omitting `--subdir` preserves its current value. `deinit` stops the daemon,
uninstalls its service, and forgets the profile while keeping sources and repositories.

Keep both READMEs focused on how to use the product. Database schemas, state-file
layouts, internal locking/hashing details, toolchain versions, and formatter
configuration belong in source/configuration files and this guide, not the READMEs.

For review, run `filetrail status` and `filetrail diff`. If a stable working tree
is needed, run `filetrail pause` first and `filetrail resume` afterward. Explicit
`filetrail sync` and `add` still synchronize while automatic sync is paused.
Only run `filetrail commit` when a commit is within the user's requested scope.
Omit `-m` to use the generated message. Do not run `resolve --use-source`, enable
deletion, or install a service merely to make a test or diagnostic pass.

`filetrail cd` jumps to the repository root when the Bash, Zsh, or Fish integration
is loaded; `command filetrail cd` prints its path. `filetrail git <args...>` runs
system Git in that root under the operation lock. Put `--data-dir` before `git`.
This explicit passthrough follows normal Git behavior, including unmanaged files;
only run mutations such as commits or pushes when the user requests them.

Report actual test outcomes and distinguish local checks from GitHub-hosted CI.
