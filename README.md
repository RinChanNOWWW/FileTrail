# FileTrail

English | [简体中文](README_zh.md)

FileTrail is a file synchronization tool with Git version control. It watches the
files and directories you choose, syncs changes into a local Git repository, and
lets you review and commit them on your terms.

Use it for dotfiles, scripts, notes, or other files spread across your machine.
Keep separate macOS and Linux configurations in the same repository. Everything
runs from a single executable, with no separate Git installation required.

## Install

Run the following from the project directory:

```sh
cargo install --path . --locked
filetrail --help
```

To install FileTrail and enable Tab completion in one step:

```sh
./install.sh
```

The script detects Bash, Zsh, or Fish from `$SHELL`. You can select one explicitly
with `./install.sh zsh`. It installs with Cargo, then configures that shell's
completion. Open a new shell afterward. The installation root defaults to
`${CARGO_HOME:-$HOME/.cargo}`; set `CARGO_INSTALL_ROOT` to override it.

## Tab completion

If you installed FileTrail with `cargo install`, enable completion with:

```sh
filetrail completions --install
```

This detects your shell from `$SHELL`. To select a shell explicitly:

```sh
filetrail completions zsh --install
filetrail completions bash --install
filetrail completions fish --install
```

Run the command for the shell you use, then open a new shell. Tab completes
subcommands (including `daemon` and `service` actions), options, and file paths.
For example, try `filetrail da<Tab>`, `filetrail daemon st<Tab>`, or
`filetrail add --f<Tab>`.

Installation preserves existing shell configuration and is safe to repeat. It
uses `.zshrc` (respecting `ZDOTDIR`), `.bashrc` and Bash's active login profile,
or Fish's completion directory (respecting `XDG_CONFIG_HOME`). Home paths use
`$HOME` in the installed hooks and command output, so your username is not embedded.
Paths outside Home retain their absolute location. Completion stays
in sync when you upgrade the executable at the same location. Run installation
again if you move it. To remove completion, delete the marked FileTrail block
from the configured files printed by the install command.

To print a completion script for manual setup, omit `--install`:

```sh
filetrail completions zsh
```

## Get started

```sh
filetrail init ~/dotfiles --subdir macos
filetrail add ~/.zshrc
filetrail add ~/.config/nvim
filetrail daemon start
```

On Linux, use `--subdir linux`. Omit `--subdir` to save at the repository root.
FileTrail creates the destination and initializes Git if needed. Adding a source
immediately copies its existing files; the daemon keeps subsequent changes in sync.

## Choose where files go

Sources inside HOME keep their Home-relative paths. Sources outside HOME keep their
absolute hierarchy without the leading `/`. Use `--to` to choose a different target,
relative to the subdirectory selected during `init`.

| Source | Subdirectory | Target | File in the repository |
| --- | --- | --- | --- |
| `~/.zshrc` | `macos` | Default | `macos/.zshrc` |
| `~/.config/nvim` | `linux` | Default | `linux/.config/nvim/init.lua` |
| `/opt/scripts/build.sh` | `macos` | Default | `macos/opt/scripts/build.sh` |
| `/opt/scripts` | `macos` | `scripts` | `macos/scripts/build.sh` |

```sh
filetrail add /opt/scripts --to scripts
filetrail add ~/notes --to notes --exclude '**/*.tmp'
```

Directories are watched recursively. Exclusions are relative to the source root.
Relative source paths are resolved from your current directory; parent-directory
symlinks are resolved to their actual locations. Sources, destinations, and the
application data directory must not overlap.

## Add sources from a list

```sh
filetrail add --from ./files.txt
```

Write one entry per line as `source` or `source target`, separated by spaces.
Use single or double quotes around paths containing spaces. Targets are optional;
omitting one uses the defaults above.

```text
# source [target]
~/.zshrc
~/.config/nvim
/opt/scripts scripts
"~/My Notes" "notes backup"
'./local scripts' 'scripts backup'
```

Relative source paths are resolved from the list's directory. Blank lines and
`#` comments are allowed. Use `~` for HOME; environment variables and commands in
the list are not expanded or executed. The whole list is checked before any entries
are added. Editing it later does not update previously imported entries.

## Manage sources

```sh
filetrail list
filetrail disable 1
filetrail enable 1
filetrail remove 1
filetrail sync
filetrail sync --dry-run
```

Use IDs from `list`. `remove` stops tracking a source and keeps its destination
files. `sync` copies current changes immediately; `--dry-run` previews them.

Source deletions are retained at the destination by default. Enable deletion
propagation when adding a source:

```sh
filetrail add ~/scripts --to scripts --delete
```

Only previously synchronized files can be deleted. If an entire source directory
becomes unavailable, FileTrail keeps its destination files. Symlinks are copied as
links, not followed; Git does not track empty directories. `.git` is always excluded,
and destination Git ignore rules apply when committing.

## Review and commit

```sh
filetrail status
filetrail diff
filetrail diff -- macos/.config/nvim
filetrail commit
filetrail commit -m 'Update shell configuration'
filetrail commit -- macos/.zshrc
```

The daemon never commits or pushes automatically. `diff` includes new file contents.
`commit` includes only managed files and refuses to proceed if other changes are
already staged. Set your Git name and email before your first commit.

Without `-m`, FileTrail generates a message listing the selected changes:

```text
FileTrail: sync 3 files (+1 ~1 -1)

add "macos/.config/nvim/init.lua"
delete "macos/.oldrc"
modify "macos/.zshrc"
```

To keep the destination stable while reviewing:

```sh
filetrail pause
filetrail diff
filetrail commit
filetrail resume
```

`resume` catches up with changes made while paused. Explicit `sync` and `add`
commands still copy files while automatic synchronization is paused. Paths passed
to `diff`, `commit`, and `resolve` are relative to the repository root.

## Resolve conflicts

FileTrail reports a conflict if a destination differs from an existing source on
first sync, or if you modify the destination outside FileTrail. To explicitly use
the source version:

```sh
filetrail conflicts
filetrail resolve macos/.zshrc --use-source
```

You can also make both copies identical yourself and run `filetrail sync` again.
Finish any Git merge/rebase or unresolved Git conflicts before resuming synchronization.

## Run in the background

```sh
filetrail daemon start
filetrail daemon status
filetrail daemon restart
filetrail daemon stop
filetrail daemon run          # Foreground mode
filetrail daemon start --poll # Use periodic scans instead of filesystem events
```

For automatic startup at login, install a user service after placing the executable
in a stable location:

```sh
filetrail service show
filetrail service install
filetrail service uninstall
```

macOS uses launchd and Linux uses systemd user services. Once installed, use
`service uninstall` to stop the service and prevent automatic restarts.

## Data directory and troubleshooting

FileTrail stores its application data in `$HOME/.filetrail` on both platforms.
Your synchronized files and Git history live in the repository chosen during `init`.
To use a different data directory, pass the same `--data-dir` to each command:

```sh
filetrail --data-dir ~/filetrail-work init ~/work-dotfiles --subdir macos
filetrail --data-dir ~/filetrail-work daemon start
```

Use one data directory per repository. To inspect problems or discover more options:

```sh
filetrail doctor
filetrail logs --follow
filetrail --help
filetrail add --help
```

For development instructions, see [AGENTS.md](AGENTS.md).
