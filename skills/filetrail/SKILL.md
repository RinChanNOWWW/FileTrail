---
name: filetrail
description: Use the FileTrail CLI on macOS or Linux to manage file and dotfile synchronization into a Git repository, restore files to the current system, review and commit managed changes, and operate its daemon and shell integration. Applies to using FileTrail, not developing its Rust implementation.
---

# FileTrail

FileTrail watches selected files and directories and copies changes into a local
Git repository. It provides explicit review and commit commands, and can restore
repository files onto the current system. Normal synchronization goes from local
sources to the repository; it is not automatic bidirectional synchronization.
The daemon never stages, commits, or pushes. One executable supports macOS and
Linux; built-in commands need neither a Rust installation nor system Git.
Only `filetrail git` requires Git on PATH.

## Discover the installed CLI and profile

Run `filetrail --version`, `filetrail --help`, and relevant subcommand help before
using unfamiliar options; the installed version may differ from this guide.
FileTrail must be installed and executable in the environment where the user's
files reside. Loading this skill does not install the executable or grant access
to another machine's files.

The default profile is `$HOME/.filetrail`. Use the same `--data-dir <directory>`
for every command when the user selects another profile:

```sh
filetrail --data-dir ~/filetrail-work status
filetrail --data-dir ~/filetrail-work list
```

Inspect `status` and `list` for an existing setup. Use an isolated temporary profile,
source, and repository for experiments; the data directory must be outside both
source and repository. Real setup examples below apply when configuration is part
of the user's request.

## Set up sources and understand paths

```sh
filetrail init ~/dotfiles --subdir macos
filetrail add ~/.zshrc
filetrail add ~/.config/nvim
filetrail add ~/notes --exclude '**/*.tmp'
```

Use `--subdir linux` for a Linux layout, or omit `--subdir` to put the reserved
roots directly in the repository. `init` creates the destination and initializes
Git if needed; an existing local repository can also be selected. It does not
clone or fetch a remote. `add` immediately synchronizes the source's existing
files. Directories are recursive; `--exclude` is a repeatable glob relative to
each source root.

| Local source | Repository path with `--subdir macos` |
| --- | --- |
| `~/.zshrc` | `macos/__HOME__/.zshrc` |
| `~/.config/nvim/init.lua` | `macos/__HOME__/.config/nvim/init.lua` |
| `/opt/scripts/build.sh` | `macos/__ROOT__/opt/scripts/build.sh` |

`__HOME__` represents the current user's Home; `__ROOT__` represents `/`.
Custom targets are unsupported, and neither reserved name can occur in `--subdir`.
Source arguments are local paths, relative to the current directory when not
absolute. Parent symlinks are resolved when adding sources. Paths for `restore`,
`diff`, `commit`, and `resolve` are repository-relative and include the subdirectory.
Sources, the target repository, and the application data directory must not overlap.

For batch addition, use `filetrail add --from ./files.txt`. Write one source per
line, quoting paths containing spaces; blank lines and `#` comments are allowed.
Relative paths are resolved from the list's directory. `~` expands to Home;
variables and commands are not expanded or executed. The entire list is validated
before adding entries. Editing the list later does not update existing entries.

```text
~/.zshrc
~/.config/nvim
"~/My Notes"
/opt/scripts
```

## Manage and synchronize

| Command | Effect |
| --- | --- |
| `filetrail list` | Show source mappings and IDs. |
| `filetrail disable <id>` | Disable synchronization for a mapping. |
| `filetrail enable <id>` | Enable a mapping. |
| `filetrail remove <id>` | Stop managing a mapping; keep its files. |
| `filetrail sync --dry-run` | Preview forward synchronization. |
| `filetrail sync` | Synchronize immediately, even while automatic sync is paused. |

Use IDs returned by `list`. Source deletion does not delete repository copies by
default. `add <source> --delete` opts into propagating deletion of previously
synchronized files. An unavailable source directory never triggers mass deletion.
Symlinks are copied as links; `.git` is excluded. Empty directories can be copied,
but Git does not track them. Git ignore rules apply when committing.

## Restore from the repository

On a new profile, first select the existing local repository with `init` and the
appropriate `--subdir`. Then use:

```sh
filetrail restore --dry-run
filetrail restore
filetrail restore macos/__HOME__/.zshrc macos/__HOME__/.config/nvim
filetrail restore --track macos/__HOME__/.zshrc
filetrail restore --overwrite macos/__HOME__/.zshrc
```

- Without paths, restore all files under the configured subdirectory's `__HOME__`
  and `__ROOT__`; other platform directories and repository READMEs are not restored.
- Multiple files or directories can be selected. Directories are recursive;
  overlapping selections are copied once. Current working files are used, including
  uncommitted files. `__ROOT__` locations need normal system write permissions.
- Default behavior is copying only, leaving management unchanged. Identical files
  are left alone. Different local contents, types, or permissions require
  `--overwrite`; local directories are never replaced by files or links.
- `--track` adds restored files and symlinks individually, with deletion disabled.
  Extra local files are not added. Compatible existing mappings retain their settings;
  disabled or excluded mappings must be adjusted before tracking. Empty directories
  are created but not tracked. A `__ROOT__` path that now belongs inside Home must
  use the `__HOME__` layout to be tracked.
- `--dry-run` previews both copying and optional tracking. Restore does not delete
  extra local files, start the daemon, commit, or run forward synchronization.
- Permissions and symlinks are preserved; link targets are not rewritten or followed.
  Symlink ancestors and writes into the repository, application data directory, or
  `.git` are refused. Nested `.git` entries are skipped.

Selections and conflicts are checked before copying. An I/O failure can leave
some files restored; fix the error and rerun. Subsequent normal synchronization
still goes from local sources to the repository.

## Review, commit, and resolve conflicts

```sh
filetrail status
filetrail diff
filetrail diff -- macos/__HOME__/.config/nvim
filetrail commit
filetrail commit -m 'Update shell configuration'
filetrail commit -- macos/__HOME__/.zshrc
```

`diff` includes untracked file contents. `commit` includes only managed changes
and refuses unrelated staged changes; preserve the user's staging. A Git name
and email must be configured before committing. Omit `-m` for an automatically
generated message describing the selected changes. Commits do not push.

For stable review while the daemon runs, `pause` automatic sync, inspect with
`status`/`diff`, then `resume` if this task paused it. Preserve a preexisting paused
state. `resume` catches up; explicit `sync` and `add` still synchronize while paused.

```sh
filetrail conflicts
filetrail resolve macos/__HOME__/.zshrc --use-source
```

A conflict means the repository copy differs on first sync or changed outside
FileTrail. `resolve --use-source` explicitly overwrites that copy with the local
source. To choose the repository copy instead, use `restore --overwrite --track`
for that path, subject to tracking compatibility. Another option is to make the
copies identical and synchronize. Finish Git merge/rebase operations and unresolved
Git conflicts before synchronizing or restoring.

Run commits, pushes, overwrites, deletion propagation, and service installation
only within the user's requested scope; do not use them just to clear diagnostics.
Existing authorization need not be requested again.

## Background operation and shell integration

| Command | Effect |
| --- | --- |
| `filetrail daemon start` | Start background synchronization. |
| `filetrail daemon start --poll` | Use periodic scans instead of filesystem events. |
| `filetrail daemon status` | Inspect the daemon. |
| `filetrail daemon restart` | Restart it; `--poll` is also supported. |
| `filetrail daemon stop` | Stop it. |
| `filetrail daemon run` | Run in the foreground; `--poll` is also supported. |
| `filetrail pause` / `filetrail resume` | Pause automatic sync / resume and catch up. |
| `filetrail service show` | Preview the platform's service definition. |
| `filetrail service install` | Install login startup using launchd on macOS or systemd on Linux. |
| `filetrail service uninstall` | Stop the installed service and prevent automatic restarts. |

Place the executable at a stable location before installing its service.

`filetrail completions --install` detects the shell from `$SHELL`; use
`filetrail completions bash --install`, `zsh --install`, or `fish --install` to
choose explicitly. It preserves existing shell configuration. Open a new shell
after installation. Omit `--install` to print a completion script.

With Bash, Zsh, or Fish integration loaded, `filetrail cd` jumps to the repository
root. `command filetrail cd` prints the path; `--print0` selects NUL termination.
For an agent shell without integration, use `cd "$(command filetrail cd)"` in
Bash/Zsh or use the printed path as the next command's working directory.

`filetrail git <args...>` runs system Git in the repository root and prevents
concurrent FileTrail synchronization for the duration. Put FileTrail options
before `git`, e.g. `filetrail --data-dir ~/filetrail-work git status`. All following
arguments, I/O, and exit codes belong to Git. This passthrough uses normal Git
staging, including unmanaged files, and adds no automatic sync or commit.

## Change the target, stop using a profile, or troubleshoot

```sh
filetrail retarget ~/new-dotfiles
filetrail retarget ~/new-dotfiles --subdir linux
filetrail retarget ~/dotfiles --subdir .
```

`retarget` keeps sources, exclusions, deletion settings, and paused/running status.
Omitting `--subdir` preserves its value; `--subdir .` selects the repository root.
It immediately syncs enabled sources. The old repository and history remain.
If initial sync reports conflicts, the target has still changed: review `conflicts`
and resolve against the new repository.

`filetrail deinit` stops the daemon, uninstalls its service, and forgets the profile.
It preserves source files, repositories, Git history, logs, and shell completion.
It is repeatable. To start over, run `init`, add or restore-and-track sources, and
start the daemon or install a service as requested.

Use `filetrail doctor` to validate the setup, `filetrail logs` to read the log,
`filetrail logs --follow` to follow it, and `filetrail <command> --help` for details.
