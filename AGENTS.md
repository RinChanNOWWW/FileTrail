# Working on Filetrail

Filetrail is a Rust CLI and background file synchronization daemon distributed as
one executable for macOS and Linux. Read README.md before changing its behavior.

## Development

- README.md is the primary English README; README_zh.md is the Chinese version.
  Every README change must update both files in the same change, keeping behavior,
  examples, and section coverage equivalent. Preserve their language-switch links.
- Use the exact toolchain in rust-toolchain.toml. Keep Cargo.lock checked in.
- Building requires a C compiler for vendored libgit2 and SQLite. The distributed executable
  does not require a separate Rust or Git installation.
- Run `cargo fmt --all`, `cargo clippy --locked --all-targets -- -D warnings`,
  `cargo test --locked --all-targets`, and `cargo test --locked --doc`.
- Completion integration tests require Bash, Zsh, and Fish on PATH.
- Run `taplo fmt` and `taplo fmt --check` with taplo-cli 0.10.0.
- Every Rust import must be its own `use` statement. Do not use grouped braces.
  rustfmt's `imports_granularity = "Item"` enforces this on the pinned nightly.
- Keep TOML keys alphabetically sorted, particularly `[package]` and dependency
  names in Cargo.toml. Use the repository's .taplo.toml; do not format Cargo.lock.
- Put filesystem/Git regression tests in tests/workflow.rs using temporary
  directories and a temporary Git identity. Never test against real dotfiles.
- Keep OS service installation out of automated tests. Test rendered definitions.
- Do not introduce a dependency on an external `git` executable for core commands.

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

Report actual test outcomes and distinguish local checks from GitHub-hosted CI.
