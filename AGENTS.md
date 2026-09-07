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

- config.rs owns editable TOML mappings, validation, atomic config writes, and the shared
  operation lock. Repository path, init subdirectory, and entry target are
  separate concepts: destination = repository / subdir / target / relative file.
  Without `add --to`, sources inside Home use their Home-relative path; sources
  outside Home use their absolute path with the leading `/` removed. The same
  default applies to list imports. Explicit targets override either default.
- Application data defaults to `$HOME/.filetrail` on both macOS and Linux. The
  `--data-dir` option overrides this location for all configuration, mappings,
  synchronization state, locks, sockets, and logs. Use the same resolved data
  directory when spawning the daemon or rendering system service definitions.
- sync.rs reconciles current filesystem contents, records ownership and content
  baselines, protects external destination edits, and copies without following
  symlinks. Events are hints; periodic scans recover missed events.
- state.rs persists synchronization state in `state.db` using bundled SQLite.
  Ownership, baselines, conflicts, and the last sync timestamp live in separate
  tables. Apply related changes in one transaction, updating only changed rows.
  Ownership survives a source deletion so Git can still commit that deletion.
  Conflicts may refer to files not yet owned by FileTrail.
  Validate application_id and user_version before accessing a database; never
  silently reset damaged or unknown schemas. Publish a new database only after
  its initial transaction succeeds. Reads and dry runs must not create a database.
  There is no legacy JSON state reader or migration path. Callers hold the shared
  operation lock across a state read/modify/write sequence; SQLite also provides
  transactional consistency for state readers and writers.
- manifest.rs parses file-list lines as `source [target]` separated by whitespace.
  Single/double quotes and escapes support spaces in either path; comments and
  blank lines are allowed. Never execute a shell or expand variables in the list.
  Reject extra fields, empty paths, and malformed quotes with a line-numbered
  error, and validate the complete list before saving any mappings.
- git.rs handles local status/diff/commit. Background sync never stages or commits.
  A commit includes only previously synchronized, still-managed paths. Preexisting
  staged changes cause a refusal, without changing the index.
- daemon.rs owns native watching, periodic reconciliation, and the local socket.
  All disk mutations share operation.lock; daemon.lock prevents duplicate daemons.
  CLI config edits are atomic and picked up by the daemon without restarting it.
- service.rs renders/installs user-level launchd or systemd definitions.
- completion.rs installs explicitly requested Bash, Zsh, and Fish completion hooks.
  Keep generation derived from the Clap command tree, including nested commands.
  Generation and installation must work before init without creating application data.
  Preserve existing shell configuration, symlinks, and permissions; replace only
  FileTrail's marked block and refuse malformed markers. Respect ZDOTDIR and
  XDG_CONFIG_HOME. Hooks invoke the absolute executable path with shell-specific
  quoting, so upgrades at the same location update completion automatically.
  install.sh wraps cargo install followed by completion installation. Never use
  build.rs to modify shell configuration during builds. Test with isolated HOME,
  ZDOTDIR, and XDG_CONFIG_HOME; never modify the developer's real shell profiles.
- Default synchronization preserves deleted source files in the destination.
  Opt-in deletion applies only to previously synchronized paths. A missing source
  root directory must never trigger mass deletion.
- Never permit repository-relative paths to escape via `..`, `.git`, or a
  destination ancestor symlink. Do not overwrite external destination edits
  unless the user explicitly requests conflict resolution for that path.
- Default commit messages start with `filetrail: ` and list every selected file
  change. Explicit user messages are preserved. No automatic commit or push.

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

On Linux, use `--subdir linux`, or omit it to write at the repository root. An
`add --to` path is relative to that configured subdirectory; paths passed to
`diff`, `commit`, and `resolve` are relative to the repository root.
For example, `filetrail add /opt/scripts/build.sh` with `--subdir macos` configured
stores `macos/opt/scripts/build.sh`; no explicit target is required.

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
