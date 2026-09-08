//! Restore the configured repository's reserved layout without running forward sync.
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use walkdir::WalkDir;

use crate::config::Baseline;
use crate::config::Config;
use crate::config::Entry;
use crate::config::Store;
use crate::sync::Report;
use crate::sync::fingerprint;
use crate::sync::key;
use crate::sync::safe_destination;

struct Copy {
    repository_path: PathBuf,
    local: PathBuf,
    directory: bool,
    expected: Option<String>,
    previous: Option<String>,
    entry: Option<u64>,
}

pub fn run(
    store: &Store,
    paths: &[PathBuf],
    track: bool,
    overwrite: bool,
    dry_run: bool,
) -> Result<Report> {
    let _lock = store.lock()?;
    let mut config = store.config()?;
    let repo = crate::git::open(&config)?;
    crate::git::ensure_idle(&repo)?;
    // Even copy-only operations must not hide corrupt synchronization history.
    let mut state = store.state()?;
    let home = crate::config::home_dir()?;
    let roots = if paths.is_empty() {
        let mut roots = Vec::new();
        for name in ["__HOME__", "__ROOT__"] {
            let relative = config.subdir.join(name);
            let absolute = safe_destination(&config.repository, &relative)?;
            match fs::symlink_metadata(absolute) {
                Ok(_) => roots.push(relative),
                Err(error) if error.kind() == ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
        roots
    } else {
        paths
            .iter()
            .map(|path| crate::config::relative(path))
            .collect::<Result<Vec<_>>>()?
    };
    let mut copies = BTreeMap::new();
    let mut local_paths = BTreeMap::new();
    for root in roots {
        local_path(&config, &home, &root)?;
        let absolute = safe_destination(&config.repository, &root)?;
        for item in WalkDir::new(absolute)
            .follow_links(false)
            .follow_root_links(false)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|item| {
                !item
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(".git")
            })
        {
            let item = item?;
            let relative = crate::config::relative(item.path().strip_prefix(&config.repository)?)?;
            if copies.contains_key(&relative) {
                continue;
            }
            let local = local_path(&config, &home, &relative)?;
            let directory = item.file_type().is_dir();
            // Reserved roots encode bases, never links or files to replace Home or /.
            if relative.strip_prefix(&config.subdir)?.components().count() == 1 && !directory {
                bail!(
                    "reserved restore root must be a real directory: {}",
                    relative.display()
                );
            }
            validate_local(store, &config, &local, directory)?;
            let folded = key(&local)?.to_lowercase();
            if let Some(other) = local_paths.insert(folded, relative.clone()) {
                bail!(
                    "restore paths collide: {} and {}",
                    other.display(),
                    relative.display()
                );
            }
            let (expected, previous) = if directory {
                (None, None)
            } else {
                let expected = fingerprint(item.path())?.context("repository file disappeared")?;
                let previous = fingerprint(&local)?;
                if previous.as_ref().is_some_and(|value| value != &expected) && !overwrite {
                    bail!(
                        "local file differs: {}; use --overwrite to replace it",
                        local.display()
                    );
                }
                (Some(expected), previous)
            };
            copies.insert(
                relative.clone(),
                Copy {
                    repository_path: relative,
                    local,
                    directory,
                    expected,
                    previous,
                    entry: None,
                },
            );
        }
    }
    // Plan the entire batch, including management compatibility, before writing.
    if track {
        let mut next = config
            .entries
            .iter()
            .map(|entry| entry.id)
            .chain(state.owned.values().copied())
            .chain(state.files.values().map(|baseline| baseline.entry))
            .max()
            .unwrap_or(0);
        for copy in copies.values_mut().filter(|copy| !copy.directory) {
            let target = crate::config::default_target(&copy.local, &home)?;
            if config.subdir.join(&target) != copy.repository_path {
                bail!(
                    "cannot track {}: its current-system mapping belongs at {}",
                    copy.repository_path.display(),
                    config.subdir.join(target).display()
                );
            }
            let existing = config.entries.iter().find(|entry| {
                copy.local == entry.source
                    || (entry.directory && copy.local.starts_with(&entry.source))
            });
            let id = if let Some(entry) = existing {
                let relative = copy.local.strip_prefix(&entry.source)?;
                if config.destination(entry).join(relative) != copy.repository_path {
                    bail!("incompatible existing mapping for {}", copy.local.display());
                }
                ensure_included(&config, entry, relative)?;
                entry.id
            } else {
                next = next
                    .checked_add(1)
                    .filter(|id| *id <= i64::MAX as u64)
                    .context("no available mapping IDs")?;
                let entry = Entry {
                    id: next,
                    source: copy.local.clone(),
                    target,
                    directory: false,
                    enabled: true,
                    delete: false,
                    exclude: vec![],
                };
                ensure_included(&config, &entry, Path::new(""))?;
                config.entries.push(entry);
                next
            };
            copy.entry = Some(id);
        }
        config.validate()?;
        store.validate_layout(&config)?;
    }
    let mut report = Report::default();
    for copy in copies.values() {
        if copy.directory {
            if !copy.local.exists() {
                report
                    .actions
                    .push(format!("mkdir {}", copy.local.display()));
                if !dry_run {
                    validate_local(store, &config, &copy.local, true)?;
                    fs::create_dir_all(&copy.local)?;
                }
            }
            continue;
        }
        report.actions.push(format!(
            "{} {} -> {}",
            if copy.expected == copy.previous {
                "unchanged"
            } else {
                "restore"
            },
            copy.repository_path.display(),
            copy.local.display()
        ));
        if let Some(id) = copy.entry {
            report
                .actions
                .push(format!("track [{id}] {}", copy.local.display()));
        }
        if dry_run {
            continue;
        }
        validate_local(store, &config, &copy.local, false)?;
        let source = safe_destination(&config.repository, &copy.repository_path)?;
        if fingerprint(&source)? != copy.expected || fingerprint(&copy.local)? != copy.previous {
            bail!(
                "file changed after restore planning: {}; retry restore",
                copy.repository_path.display()
            );
        }
        let expected = copy
            .expected
            .as_deref()
            .context("missing file fingerprint")?;
        if copy.expected != copy.previous {
            crate::sync::copy_atomic_checked(&source, &copy.local, expected, overwrite)?;
        }
        if let Some(entry) = copy.entry {
            let name = key(&copy.repository_path)?;
            state.owned.insert(name.clone(), entry);
            state.files.insert(
                name.clone(),
                Baseline {
                    entry,
                    fingerprint: expected.to_owned(),
                },
            );
            state.conflicts.remove(&name);
        }
    }
    if track && !dry_run && copies.values().any(|copy| copy.entry.is_some()) {
        // Config first: if saving state fails, a later sync can adopt identical
        // copies, without ever having a baseline referencing an unsaved mapping.
        store.save_config(&config)?;
        store.save_state(&state)?;
    }
    Ok(report)
}

fn local_path(config: &Config, home: &Path, relative: &Path) -> Result<PathBuf> {
    let mut components = relative
        .strip_prefix(&config.subdir)
        .context("restore path must be inside the configured subdirectory")?
        .components();
    let root = components
        .next()
        .context("select paths under __HOME__ or __ROOT__")?;
    let base = match root.as_os_str().to_str() {
        Some("__HOME__") => home,
        Some("__ROOT__") => Path::new("/"),
        _ => bail!(
            "restore path must be under __HOME__ or __ROOT__: {}",
            relative.display()
        ),
    };
    Ok(base.join(components.as_path()))
}

fn validate_local(store: &Store, config: &Config, local: &Path, directory: bool) -> Result<()> {
    // Be conservative on both platforms, like mapping validation: macOS often
    // aliases case variants of the same path.
    let folded = PathBuf::from(key(local)?.to_lowercase());
    for protected in [&store.root, &config.repository] {
        let protected = PathBuf::from(key(protected)?.to_lowercase());
        if folded.starts_with(&protected) || (!directory && protected.starts_with(&folded)) {
            bail!(
                "restore location overlaps repository or data directory: {}",
                local.display()
            );
        }
    }
    // Reject symlink ancestors on both sides, including a directory leaf. Leaf
    // symlinks remain links and can only be replaced with explicit overwrite.
    let check = if directory {
        local.join(".filetrail-check")
    } else {
        local.to_owned()
    };
    safe_destination(Path::new("/"), check.strip_prefix("/")?)?;
    key(local)?;
    Ok(())
}

fn ensure_included(config: &Config, entry: &Entry, relative: &Path) -> Result<()> {
    let patterns = config
        .exclude
        .iter()
        .chain(&entry.exclude)
        .cloned()
        .collect::<Vec<_>>();
    let ignored = crate::sync::exclusions(&patterns)?;
    let relative = if entry.directory {
        relative
    } else {
        Path::new(
            entry
                .source
                .file_name()
                .context("missing source filename")?,
        )
    };
    if !entry.enabled || relative.ancestors().any(|path| ignored.is_match(path)) {
        bail!(
            "cannot track disabled or excluded source: {}",
            entry.source.display()
        );
    }
    Ok(())
}
