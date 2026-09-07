use std::collections::BTreeMap;
use std::fs;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use notify::RecursiveMode;
use notify::Watcher;

use crate::config::Config;
use crate::config::Store;

pub fn request(store: &Store, command: &str) -> Result<String> {
    let mut stream = UnixStream::connect(store.root.join("daemon.sock"))
        .context("daemon is not running; use filetrail daemon start")?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    writeln!(stream, "{command}")?;
    let mut result = String::new();
    BufReader::new(stream).read_line(&mut result)?;
    if result.is_empty() {
        bail!("daemon disconnected without responding");
    }
    Ok(result.trim_end().to_owned())
}

pub fn start(store: &Store, poll: bool) -> Result<String> {
    if let Ok(status) = request(store, "status") {
        return Ok(status);
    }
    store.config()?;
    let output = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(store.root.join("daemon-output.log"))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--data-dir")
        .arg(&store.root)
        .args(["daemon", "run"]);
    if poll {
        command.arg("--poll");
    }
    let mut child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(status) = request(store, "status") {
            return Ok(status);
        }
        if let Some(status) = child.try_wait()? {
            bail!(
                "daemon exited ({status}); inspect {}",
                store.root.join("daemon-output.log").display()
            );
        }
        thread::sleep(Duration::from_millis(50));
    }
    child.kill()?;
    child.wait()?;
    bail!("daemon did not become ready within 10 seconds")
}

pub fn stop(store: &Store) -> Result<String> {
    let response = request(store, "stop")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let lock = store.lock_file("daemon.lock")?;
        if fs2::FileExt::try_lock_exclusive(&lock).is_ok() {
            return Ok(response);
        }
        if Instant::now() >= deadline {
            bail!("daemon is still stopping");
        }
        thread::sleep(Duration::from_millis(50));
    }
}

pub fn log(store: &Store, message: &str) -> Result<()> {
    let path = store.root.join("filetrail.log");
    if fs::metadata(&path).is_ok_and(|metadata| metadata.len() > 2 * 1024 * 1024) {
        fs::rename(&path, store.root.join("filetrail.log.1"))?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(
        file,
        "{} {message}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
    )?;
    Ok(())
}

fn watcher(
    config: &Config,
    tx: mpsc::SyncSender<()>,
    store: &Store,
) -> Result<notify::RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |_: notify::Result<notify::Event>| {
        let _ = tx.try_send(());
    })?;
    let mut roots = BTreeMap::new();
    for entry in config.entries.iter().filter(|entry| entry.enabled) {
        if entry.directory && entry.source.is_dir() {
            roots.insert(entry.source.clone(), RecursiveMode::Recursive);
        }
        // Watch parents so atomic replacement and recreation do not lose the subscription.
        if let Some(parent) = entry.source.parent() {
            roots
                .entry(parent.to_path_buf())
                .or_insert(RecursiveMode::NonRecursive);
        }
    }
    for (path, mode) in roots {
        if let Err(error) = watcher.watch(&path, mode) {
            log(
                store,
                &format!(
                    "watch failed for {}: {error}; periodic scans remain active",
                    path.display()
                ),
            )?;
        }
    }
    Ok(watcher)
}

pub fn run(store: &Store, poll: bool) -> Result<()> {
    let singleton = store.lock_file("daemon.lock")?;
    fs2::FileExt::try_lock_exclusive(&singleton).context("another daemon is already running")?;
    let socket = store.root.join("daemon.sock");
    if socket.exists() {
        fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket).context(
        "cannot bind daemon socket; choose a shorter --data-dir if its path is too long",
    )?;
    listener.set_nonblocking(true)?;
    let stopped = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&stopped);
    ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst))?;
    let mut config = store.config()?;
    let (tx, rx) = mpsc::sync_channel(1);
    let mut active_watcher = if poll {
        None
    } else {
        Some(watcher(&config, tx.clone(), store)?)
    };
    let mut config_text = fs::read(store.root.join("config.toml"))?;
    let mut config_check = Instant::now();
    let mut last_scan = Instant::now();
    let mut dirty = Some(Instant::now());
    let mut latest_event = Instant::now();
    let mut paused = false;
    log(store, "daemon started")?;
    while !stopped.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_read_timeout(Some(Duration::from_millis(500)))?;
                stream.set_write_timeout(Some(Duration::from_secs(1)))?;
                let mut input = String::new();
                if BufReader::new(&stream).read_line(&mut input).is_ok() {
                    let response = match input.trim() {
                        "status" => format!(
                            "running pid={} paused={paused} mode={}",
                            std::process::id(),
                            if poll { "poll" } else { "native+periodic" }
                        ),
                        "pause" => {
                            paused = true;
                            "paused".into()
                        }
                        "resume" => {
                            paused = false;
                            dirty = Some(Instant::now());
                            "resumed".into()
                        }
                        "stop" => {
                            stopped.store(true, Ordering::SeqCst);
                            "stopped".into()
                        }
                        _ => "unknown command".into(),
                    };
                    let _ = writeln!(stream, "{response}");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
            Err(error) => return Err(error.into()),
        }
        if stopped.load(Ordering::SeqCst) {
            break;
        }
        if rx.try_recv().is_ok() {
            dirty.get_or_insert_with(Instant::now);
            latest_event = Instant::now();
        }
        if config_check.elapsed() >= Duration::from_secs(1) {
            config_check = Instant::now();
            if let Ok(bytes) = fs::read(store.root.join("config.toml"))
                && bytes != config_text
            {
                match store.config() {
                    Ok(updated) => {
                        config = updated;
                        active_watcher = if poll {
                            None
                        } else {
                            Some(watcher(&config, tx.clone(), store)?)
                        };
                        config_text = bytes;
                        dirty.get_or_insert_with(Instant::now);
                        log(store, "configuration reloaded")?;
                    }
                    Err(error) => {
                        log(store, &format!("invalid configuration: {error:#}"))?;
                        config_text = bytes;
                    }
                }
            }
        }
        let due = last_scan.elapsed() >= Duration::from_secs(config.scan_interval_secs);
        let settled = dirty.is_some_and(|first| {
            latest_event.elapsed() >= Duration::from_millis(350)
                || first.elapsed() >= Duration::from_secs(2)
        });
        if !paused && (due || settled) {
            match crate::sync::run(store, false, None) {
                Ok(report) => {
                    if !report.text().is_empty() {
                        log(store, &report.text())?;
                    }
                }
                Err(error) => log(store, &format!("sync failed: {error:#}"))?,
            }
            dirty = None;
            last_scan = Instant::now();
        }
        thread::sleep(Duration::from_millis(50));
    }
    drop(active_watcher);
    fs::remove_file(socket)?;
    log(store, "daemon stopped")?;
    Ok(())
}
