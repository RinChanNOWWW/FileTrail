use std::fs;
use std::fs::File;
use std::io::ErrorKind;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;

use crate::config::Config;
use crate::config::State;
use crate::config::Store;
use crate::config::atomic_write;
use crate::sync::Report;

const PENDING_TARGET: &str = "pending-retarget.toml";

// Resolve existing ancestors before creating anything, including symlinks and .. .
fn repository_path(path: &Path) -> Result<PathBuf> {
    let path = if path.starts_with("~") {
        crate::config::home_dir()?.join(path.strip_prefix("~")?)
    } else {
        std::path::absolute(path)?
    };
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            _ => {
                resolved.push(component);
                match fs::symlink_metadata(&resolved) {
                    Ok(_) => resolved = fs::canonicalize(&resolved)?,
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
    Ok(resolved)
}

pub fn retarget(store: &Store, repository: &Path, subdir: Option<&Path>) -> Result<Report> {
    let _lock = store.lock()?;
    let old = store.config()?;
    let mut config = old.clone();
    config.repository = repository_path(repository)?;
    if let Some(subdir) = subdir {
        config.subdir = crate::config::relative(subdir)?;
    }
    config.validate()?;
    store.validate_layout(&config)?;
    if config.repository != old.repository
        && (config.repository.starts_with(&old.repository)
            || old.repository.starts_with(&config.repository))
    {
        bail!("the old and new repositories must not be nested inside each other");
    }
    if config.repository == old.repository && config.subdir == old.subdir {
        // Keep baselines and ownership when the target has not changed.
        return crate::sync::run_locked(store, false, None);
    }
    // Refuse corrupt state before preparing a switch; never silently discard it.
    store.state()?;
    fs::create_dir_all(&config.repository)?;
    if !config.repository.join(".git").exists() {
        git2::Repository::init(&config.repository)?;
    }
    let repo = crate::git::open(&config)?;
    crate::git::ensure_idle(&repo)?;
    crate::sync::safe_destination(&config.repository, &config.subdir.join(".filetrail-check"))?;
    for entry in &config.entries {
        crate::sync::safe_destination(&config.repository, &config.destination(entry))?;
    }
    // A durable intent makes the two-file update recoverable. No synchronization
    // may use either configuration until recovery has reset the old baselines.
    atomic_write(
        &store.root.join(PENDING_TARGET),
        toml::to_string_pretty(&config)?.as_bytes(),
    )?;
    recover_retarget(store).context("target switch pending; retry to complete it")?;
    crate::sync::run_locked(store, false, None)
        .context("target changed, but initial synchronization failed; run filetrail sync to retry")
}

// Called only with operation.lock held, before any operation can use sync state.
pub(crate) fn recover_retarget(store: &Store) -> Result<()> {
    let path = store.root.join(PENDING_TARGET);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let config: Config = toml::from_str(&text).context("invalid pending target switch")?;
    config.validate()?;
    store.validate_layout(&config)?;
    store.save_state(&State::default())?;
    store.save_config(&config)?;
    fs::remove_file(path)?;
    File::open(&store.root)?.sync_all()?;
    Ok(())
}

pub fn deinit(store: &Store) -> Result<()> {
    // Service removal comes first: launchd's KeepAlive could otherwise restart
    // a stopped daemon. If unloading fails, retain the profile for a retry.
    if crate::service::is_installed(store)? {
        crate::service::uninstall(store)?;
    }
    let singleton = store.lock_file("daemon.lock")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match fs2::FileExt::try_lock_exclusive(&singleton) {
            Ok(()) => break,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
        // Also handles a daemon still starting up, before its socket is ready.
        if crate::daemon::request(store, "status").is_ok() {
            crate::daemon::stop(store)?;
        }
        if Instant::now() >= deadline {
            bail!("cannot stop daemon; profile has been kept");
        }
        thread::sleep(Duration::from_millis(50));
    }
    // Explicit deinit can remove invalid configuration or an interrupted switch.
    // Keep lock files: unlinking them would allow concurrent locks on new inodes.
    let operation = store.lock_file("operation.lock")?;
    fs2::FileExt::lock_exclusive(&operation)?;
    for name in [
        PENDING_TARGET,
        "config.toml",
        "state.db",
        "state.db-journal",
        "state.db-wal",
        "state.db-shm",
        "daemon.sock",
    ] {
        match fs::remove_file(store.root.join(name)) {
            Ok(()) => File::open(&store.root)?.sync_all()?,
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
