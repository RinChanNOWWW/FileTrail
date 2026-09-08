use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::process::ExitStatus;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use git2::DiffFormat;
use git2::DiffOptions;
use git2::Repository;
use git2::RepositoryState;
use git2::Status;
use git2::StatusOptions;

use crate::config::Config;
use crate::config::State;
use crate::config::Store;

pub fn open(config: &Config) -> Result<Repository> {
    let repository = Repository::open(&config.repository)?;
    if repository.is_bare() || repository.workdir() != Some(config.repository.as_path()) {
        bail!("target must be the root of a non-bare Git repository");
    }
    Ok(repository)
}

/// Explicit passthrough only; built-in Git operations continue to use libgit2.
pub fn run(store: &Store, args: &[OsString]) -> Result<ExitStatus> {
    // Keep sync and retarget from changing the working tree while Git is running.
    let _lock = store.lock()?;
    let config = store.config()?;
    open(&config)?;
    Command::new("git")
        .args(args)
        .current_dir(&config.repository)
        .status()
        .context("cannot run system Git; install git and make sure it is on PATH")
}

pub fn ensure_idle(repository: &Repository) -> Result<()> {
    if repository.state() != RepositoryState::Clean || repository.index()?.has_conflicts() {
        bail!("repository has an ongoing Git operation or unresolved conflicts");
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct Change {
    pub path: String,
    pub status: Status,
}

pub fn changes(repository: &Repository) -> Result<Vec<Change>> {
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(false)
        .renames_index_to_workdir(false);
    let statuses = repository.statuses(Some(&mut options))?;
    let mut changes = statuses
        .iter()
        .map(|entry| {
            Ok(Change {
                path: entry.path().context("Git path is not UTF-8")?.to_owned(),
                status: entry.status(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

fn managed(config: &Config, state: &State, path: &str) -> bool {
    state.owned.get(path).is_some_and(|id| {
        config
            .entries
            .iter()
            .any(|entry| entry.id == *id && Path::new(path).starts_with(config.destination(entry)))
    })
}

fn code(status: Status, index: bool) -> char {
    let (new, modified, deleted, renamed, kind) = if index {
        (
            Status::INDEX_NEW,
            Status::INDEX_MODIFIED,
            Status::INDEX_DELETED,
            Status::INDEX_RENAMED,
            Status::INDEX_TYPECHANGE,
        )
    } else {
        (
            Status::WT_NEW,
            Status::WT_MODIFIED,
            Status::WT_DELETED,
            Status::WT_RENAMED,
            Status::WT_TYPECHANGE,
        )
    };
    if status.is_conflicted() {
        'U'
    } else if status.contains(new) {
        'A'
    } else if status.contains(deleted) {
        'D'
    } else if status.contains(renamed) {
        'R'
    } else if status.contains(kind) {
        'T'
    } else if status.contains(modified) {
        'M'
    } else {
        ' '
    }
}

pub fn status(store: &Store) -> Result<String> {
    let _lock = store.lock()?;
    let config = store.config()?;
    let state = store.state()?;
    let repository = open(&config)?;
    let mut lines = vec![
        format!("Repository: {}", config.repository.display()),
        format!(
            "Subdirectory: {}",
            if config.subdir.as_os_str().is_empty() {
                ".".to_owned()
            } else {
                config.subdir.display().to_string()
            }
        ),
        format!("Last sync (Unix seconds): {:?}", state.last_sync),
    ];
    for change in changes(&repository)? {
        lines.push(format!(
            "{}{} [{}] {}",
            code(change.status, true),
            code(change.status, false),
            if managed(&config, &state, &change.path) {
                "managed"
            } else {
                "other"
            },
            change.path
        ));
    }
    for path in state.owned.keys() {
        if managed(&config, &state, path) && repository.is_path_ignored(Path::new(path))? {
            lines.push(format!("!! [ignored] {path}"));
        }
    }
    for (path, reason) in state.conflicts {
        lines.push(format!("conflict {path}: {reason}"));
    }
    Ok(lines.join("\n"))
}

pub fn diff(store: &Store, paths: &[String]) -> Result<String> {
    let _lock = store.lock()?;
    let config = store.config()?;
    let repository = open(&config)?;
    let head = match repository.head() {
        Ok(head) => Some(head.peel_to_tree()?),
        Err(error)
            if matches!(
                error.code(),
                git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
            ) =>
        {
            None
        }
        Err(error) => return Err(error.into()),
    };
    let options = || -> Result<DiffOptions> {
        let mut options = DiffOptions::new();
        options
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true);
        options.disable_pathspec_match(true);
        for path in paths {
            options.pathspec(crate::config::relative(Path::new(path))?);
        }
        Ok(options)
    };
    let staged = repository.diff_tree_to_index(head.as_ref(), None, Some(&mut options()?))?;
    let worktree = repository.diff_index_to_workdir(None, Some(&mut options()?))?;
    let mut output = Vec::new();
    for (label, mut diff) in [
        ("Staged changes", staged),
        ("Working tree changes", worktree),
    ] {
        diff.find_similar(None)?;
        if diff.deltas().len() == 0 {
            continue;
        }
        output.extend_from_slice(format!("{label}\n").as_bytes());
        diff.print(DiffFormat::Patch, |_, _, line| {
            if matches!(line.origin(), '+' | '-' | ' ') {
                output.push(line.origin() as u8);
            }
            output.extend_from_slice(line.content());
            true
        })?;
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
}

pub fn default_message(changes: &[Change]) -> String {
    let mut added = 0;
    let mut modified = 0;
    let mut deleted = 0;
    let mut details = Vec::new();
    for change in changes {
        let action = if change.status.contains(Status::WT_NEW) {
            added += 1;
            "add"
        } else if change.status.contains(Status::WT_DELETED) {
            deleted += 1;
            "delete"
        } else {
            modified += 1;
            "modify"
        };
        // Debug quoting prevents filenames containing line breaks from forging message lines.
        details.push(format!("{action} {:?}", change.path));
    }
    details.sort();
    format!(
        "FileTrail: sync {} files (+{added} ~{modified} -{deleted})\n\n{}",
        changes.len(),
        details.join("\n")
    )
}

pub fn commit(store: &Store, message: Option<&str>, paths: &[String]) -> Result<String> {
    let _lock = store.lock()?;
    let config = store.config()?;
    let state = store.state()?;
    let repository = open(&config)?;
    ensure_idle(&repository)?;
    let all = changes(&repository)?;
    let staged = Status::INDEX_NEW
        | Status::INDEX_MODIFIED
        | Status::INDEX_DELETED
        | Status::INDEX_RENAMED
        | Status::INDEX_TYPECHANGE;
    if all.iter().any(|change| change.status.intersects(staged)) {
        bail!(
            "repository already contains staged changes; commit or unstage them first (Filetrail leaves the index untouched)"
        );
    }
    let paths = paths
        .iter()
        .map(|path| crate::config::relative(Path::new(path)))
        .collect::<Result<Vec<_>>>()?;
    let selected = all
        .into_iter()
        .filter(|change| {
            managed(&config, &state, &change.path)
                && (paths.is_empty()
                    || paths
                        .iter()
                        .any(|path| Path::new(&change.path).starts_with(path)))
        })
        .collect::<Vec<_>>();
    if selected.is_empty() {
        bail!("no managed changes to commit; run filetrail sync/status first");
    }
    for change in &selected {
        if state.conflicts.contains_key(&change.path) {
            bail!("unresolved synchronization conflict: {}", change.path);
        }
        crate::sync::safe_destination(&config.repository, Path::new(&change.path))?;
    }
    let message = match message {
        Some(message) if message.trim().is_empty() => bail!("commit message cannot be empty"),
        Some(message) => message.to_owned(),
        None => default_message(&selected),
    };
    let signature = repository
        .signature()
        .context("configure Git user.name and user.email before committing")?;
    let parent = match repository.head() {
        Ok(head) => Some(head.peel_to_commit()?),
        Err(error)
            if matches!(
                error.code(),
                git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
            ) =>
        {
            None
        }
        Err(error) => return Err(error.into()),
    };
    let mut index = repository.index()?;
    for change in &selected {
        if change.status.contains(Status::WT_DELETED) {
            index.remove_path(Path::new(&change.path))?;
        } else {
            index.add_path(Path::new(&change.path))?;
        }
    }
    let tree = repository.find_tree(index.write_tree()?)?;
    let parents = parent.iter().collect::<Vec<_>>();
    let oid = repository.commit(
        Some("HEAD"),
        &signature,
        &signature,
        &message,
        &tree,
        &parents,
    )?;
    index
        .write()
        .context("commit created, but index update failed; inspect Git status before continuing")?;
    Ok(format!("{oid}\n{message}"))
}

#[cfg(test)]
mod tests {
    use git2::Status;

    use super::Change;
    use super::default_message;

    #[test]
    fn automatic_message_lists_all_changes_deterministically() {
        let message = default_message(&[
            Change {
                path: "z".into(),
                status: Status::WT_DELETED,
            },
            Change {
                path: "a".into(),
                status: Status::WT_NEW,
            },
            Change {
                path: "m\nname".into(),
                status: Status::WT_MODIFIED,
            },
        ]);
        assert_eq!(
            message,
            "FileTrail: sync 3 files (+1 ~1 -1)\n\nadd \"a\"\ndelete \"z\"\nmodify \"m\\nname\""
        );
    }
}
