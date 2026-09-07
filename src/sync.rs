use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use globset::Glob;
use globset::GlobSet;
use globset::GlobSetBuilder;
use walkdir::WalkDir;

use crate::config::Baseline;
use crate::config::Config;
use crate::config::Entry;
use crate::config::State;
use crate::config::Store;

#[derive(Default, Debug)]
pub struct Report {
    pub actions: Vec<String>,
    pub errors: Vec<String>,
}

impl Report {
    pub fn text(&self) -> String {
        self.actions
            .iter()
            .chain(&self.errors)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn exclusions(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern)?);
    }
    Ok(builder.build()?)
}

pub fn run(store: &Store, dry_run: bool, overwrite: Option<&Path>) -> Result<Report> {
    let _lock = store.lock()?;
    let config = store.config()?;
    let repo = crate::git::open(&config)?;
    crate::git::ensure_idle(&repo)?;
    let mut state = store.state()?;
    let mut report = Report::default();
    for entry in config.entries.iter().filter(|entry| entry.enabled) {
        if let Err(error) = sync_entry(&config, entry, &mut state, &mut report, dry_run, overwrite)
        {
            report
                .errors
                .push(format!("error [{}]: {error:#}", entry.id));
        }
    }
    if !dry_run {
        state.last_sync = Some(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs());
        store.save_state(&state)?;
    }
    Ok(report)
}

fn sync_entry(
    config: &Config,
    entry: &Entry,
    state: &mut State,
    report: &mut Report,
    dry: bool,
    overwrite: Option<&Path>,
) -> Result<()> {
    let metadata = match fs::symlink_metadata(&entry.source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A missing root directory may be an unmounted volume: never mass-delete it.
            if entry.directory || !entry.source.parent().is_some_and(Path::is_dir) {
                bail!(
                    "source unavailable; retaining destination: {}",
                    entry.source.display()
                );
            }
            return handle_missing(
                config,
                entry,
                state,
                report,
                &BTreeSet::new(),
                dry,
                overwrite,
            );
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.is_dir() != entry.directory {
        bail!("source type changed; remove and re-add this entry");
    }
    let patterns = config
        .exclude
        .iter()
        .chain(&entry.exclude)
        .cloned()
        .collect::<Vec<_>>();
    let ignored = exclusions(&patterns)?;
    let mut seen = BTreeSet::new();
    let mut scan_failed = false;
    let walker = WalkDir::new(&entry.source)
        .follow_links(false)
        .into_iter()
        .filter_entry(|item| {
            if item
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(".git")
            {
                return false;
            }
            let relative = item
                .path()
                .strip_prefix(&entry.source)
                .unwrap_or(item.path());
            let relative = if relative.as_os_str().is_empty() && !entry.directory {
                Path::new(item.file_name())
            } else {
                relative
            };
            !ignored.is_match(relative)
        });
    for item in walker {
        let item = match item {
            Ok(item) => item,
            Err(error) => {
                scan_failed = true;
                report.errors.push(format!("scan error: {error}"));
                continue;
            }
        };
        let relative = item.path().strip_prefix(&entry.source)?;
        let mut target = config.destination(entry);
        if !relative.as_os_str().is_empty() {
            target.push(relative);
        }
        let key = key(&target)?;
        seen.insert(key.clone());
        if item.file_type().is_dir() {
            // Checking a hypothetical child also validates the directory itself.
            if let Err(error) = (|| -> Result<()> {
                safe_destination(&config.repository, &target.join(".filetrail-check"))?;
                let destination = config.repository.join(&target);
                if !destination.exists() {
                    report.actions.push(format!("mkdir {key}"));
                    if !dry {
                        fs::create_dir_all(destination)?;
                    }
                }
                Ok(())
            })() {
                scan_failed = true;
                report
                    .errors
                    .push(format!("directory conflict {key}: {error:#}"));
            }
            continue;
        }
        if !item.file_type().is_file() && !item.file_type().is_symlink() {
            report
                .errors
                .push(format!("skipped special file: {}", item.path().display()));
            continue;
        }
        let result = sync_file(
            config,
            entry,
            state,
            &mut report.actions,
            item.path(),
            &target,
            dry,
            overwrite == Some(target.as_path()),
        );
        if let Err(error) = result {
            scan_failed = true;
            state.conflicts.insert(key.clone(), format!("{error:#}"));
            report.errors.push(format!("conflict {key}: {error:#}"));
        }
    }
    if !scan_failed {
        handle_missing(config, entry, state, report, &seen, dry, overwrite)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn sync_file(
    config: &Config,
    entry: &Entry,
    state: &mut State,
    actions: &mut Vec<String>,
    source: &Path,
    target: &Path,
    dry: bool,
    overwrite: bool,
) -> Result<()> {
    let destination = safe_destination(&config.repository, target)?;
    let name = key(target)?;
    let source_hash = fingerprint(source)?.context("source disappeared")?;
    let target_hash = fingerprint(&destination)?;
    if target_hash.as_ref() == Some(&source_hash) {
        if !dry {
            state.owned.insert(name.clone(), entry.id);
            state.files.insert(
                name.clone(),
                Baseline {
                    entry: entry.id,
                    fingerprint: source_hash,
                },
            );
            state.conflicts.remove(&name);
        }
        return Ok(());
    }
    if !overwrite {
        match state.files.get(&name) {
            Some(baseline) if target_hash.as_ref() != Some(&baseline.fingerprint) => bail!(
                "destination changed outside Filetrail; use resolve --use-source to overwrite"
            ),
            None if target_hash.is_some() => {
                bail!("destination already exists with different content")
            }
            _ => (),
        }
    }
    actions.push(format!(
        "{} {name}",
        if target_hash.is_some() {
            "update"
        } else {
            "add"
        }
    ));
    if !dry {
        copy_atomic(source, &destination, &source_hash)?;
        state.owned.insert(name.clone(), entry.id);
        state.files.insert(
            name.clone(),
            Baseline {
                entry: entry.id,
                fingerprint: source_hash,
            },
        );
        state.conflicts.remove(&name);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_missing(
    config: &Config,
    entry: &Entry,
    state: &mut State,
    report: &mut Report,
    seen: &BTreeSet<String>,
    dry: bool,
    overwrite: Option<&Path>,
) -> Result<()> {
    let patterns = config
        .exclude
        .iter()
        .chain(&entry.exclude)
        .cloned()
        .collect::<Vec<_>>();
    let ignored = exclusions(&patterns)?;
    let missing = state
        .files
        .iter()
        .filter(|(name, baseline)| baseline.entry == entry.id && !seen.contains(*name))
        .map(|(name, baseline)| (name.clone(), baseline.clone()))
        .collect::<Vec<_>>();
    for (name, baseline) in missing {
        let target = Path::new(&name);
        // Removing an exclusion or changing a mapping must never turn excluded paths into deletions.
        let Ok(relative) = target.strip_prefix(config.destination(entry)) else {
            continue;
        };
        let excluded = if relative.as_os_str().is_empty() && !entry.directory {
            ignored.is_match(Path::new(
                entry.source.file_name().context("source has no filename")?,
            ))
        } else {
            relative.ancestors().any(|path| ignored.is_match(path))
        };
        if excluded {
            continue;
        }
        if !entry.delete {
            report
                .actions
                .push(format!("retain {name} (source removed; deletion disabled)"));
            continue;
        }
        let result = (|| -> Result<()> {
            let destination = safe_destination(&config.repository, target)?;
            let current = fingerprint(&destination)?;
            if current.is_some()
                && current.as_ref() != Some(&baseline.fingerprint)
                && overwrite != Some(target)
            {
                bail!("destination changed outside Filetrail; refusing deletion");
            }
            report.actions.push(format!("delete {name}"));
            if !dry {
                if current.is_some() {
                    fs::remove_file(destination)?;
                }
                state.files.remove(&name);
                state.conflicts.remove(&name);
            }
            Ok(())
        })();
        if let Err(error) = result {
            state.conflicts.insert(name.clone(), error.to_string());
            report.errors.push(format!("conflict {name}: {error:#}"));
        }
    }
    Ok(())
}

pub fn key(path: &Path) -> Result<String> {
    Ok(path
        .to_str()
        .context("non-UTF-8 paths are not supported")?
        .to_owned())
}

pub fn safe_destination(repository: &Path, relative: &Path) -> Result<PathBuf> {
    let relative = crate::config::relative(relative)?;
    let mut current = repository.to_path_buf();
    if let Some(parent) = relative.parent() {
        for part in parent.components() {
            current.push(part);
            match fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => bail!(
                    "destination ancestor is not a real directory: {}",
                    current.display()
                ),
                Ok(_) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(repository.join(relative))
}

pub fn fingerprint(path: &Path) -> Result<Option<String>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut hash = blake3::Hasher::new();
    if metadata.file_type().is_symlink() {
        hash.update(b"symlink\0");
        hash.update(fs::read_link(path)?.as_os_str().as_encoded_bytes());
    } else if metadata.is_file() {
        hash.update(b"file\0");
        hash.update(&(metadata.mode() & 0o777).to_le_bytes());
        let mut file = fs::File::open(path)?;
        let mut buffer = [0; 65536];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
    } else {
        bail!("not a regular file or symlink: {}", path.display());
    }
    Ok(Some(hash.finalize().to_hex().to_string()))
}

fn copy_atomic(source: &Path, destination: &Path, expected: &str) -> Result<()> {
    let parent = destination.parent().context("missing destination parent")?;
    fs::create_dir_all(parent)?;
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let temporary = temporary.into_temp_path();
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        fs::remove_file(&temporary)?;
        symlink(fs::read_link(source)?, &temporary)?;
    } else {
        fs::copy(source, &temporary)?;
        fs::set_permissions(
            &temporary,
            fs::Permissions::from_mode(metadata.mode() & 0o777),
        )?;
        fs::File::open(&temporary)?.sync_all()?;
    }
    if fingerprint(&temporary)?.as_deref() != Some(expected)
        || fingerprint(source)?.as_deref() != Some(expected)
    {
        bail!("source changed during copy; will retry on the next scan");
    }
    temporary
        .persist(destination)
        .map_err(|error| error.error)?;
    Ok(())
}
