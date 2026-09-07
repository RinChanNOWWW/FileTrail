#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use clap::CommandFactory;
use clap::Parser;
use clap::Subcommand;
use clap_complete::Shell;
use filetrail::config::Config;
use filetrail::config::Entry;
use filetrail::config::Store;

#[derive(Parser)]
#[command(
    version,
    about = "Watch files, sync into Git, and commit on your terms"
)]
struct Cli {
    /// Store configuration, mappings, synchronization state, sockets, and logs here [default: $HOME/.filetrail].
    #[arg(long, global = true, value_hint = clap::ValueHint::DirPath)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Set the target repository and its optional platform subdirectory.
    Init {
        #[arg(value_hint = clap::ValueHint::DirPath)]
        repository: PathBuf,
        /// Repository-relative destination root, e.g. macos or linux.
        #[arg(long, default_value = ".")]
        subdir: PathBuf,
    },
    /// Add a source and immediately synchronize existing files.
    Add {
        #[arg(required_unless_present = "from", conflicts_with = "from")]
        source: Option<PathBuf>,
        /// Override the default Home-relative or root-relative destination.
        #[arg(long, conflicts_with = "from")]
        to: Option<PathBuf>,
        /// Import source [target] lines separated by spaces; quote paths containing spaces.
        #[arg(long, value_hint = clap::ValueHint::FilePath)]
        from: Option<PathBuf>,
        /// Propagate source deletions for files previously synchronized.
        #[arg(long)]
        delete: bool,
        /// Exclude a glob relative to each source root; may be repeated.
        #[arg(long)]
        exclude: Vec<String>,
    },
    /// List source mappings and IDs.
    List,
    /// Stop tracking an entry, keeping its destination files.
    Remove {
        id: u64,
    },
    Enable {
        id: u64,
    },
    Disable {
        id: u64,
    },
    /// Synchronize now, even if automatic synchronization is paused.
    Sync {
        #[arg(long)]
        dry_run: bool,
    },
    #[command(subcommand)]
    Daemon(DaemonCommands),
    #[command(subcommand)]
    Service(ServiceCommands),
    /// Pause automatic synchronization after any current operation finishes.
    Pause,
    /// Resume automatic synchronization and catch up with source changes.
    Resume,
    /// Show repository changes, ownership, conflicts, and daemon status.
    Status,
    /// Show staged and working tree diffs, including untracked file contents.
    Diff {
        paths: Vec<String>,
    },
    /// Commit managed changes; generates a filetrail: message by default.
    Commit {
        #[arg(short, long)]
        message: Option<String>,
        paths: Vec<String>,
    },
    Conflicts,
    /// Resolve one conflict by replacing its target with the source version.
    Resolve {
        path: PathBuf,
        #[arg(long, required = true)]
        use_source: bool,
    },
    Logs {
        #[arg(long)]
        follow: bool,
    },
    /// Validate configuration, Git state, source availability, and mappings.
    Doctor,
    /// Generate completions or install Tab completion for Bash, Zsh, or Fish.
    Completions {
        /// Shell to generate/install for; --install defaults to $SHELL.
        #[arg(required_unless_present = "install")]
        shell: Option<Shell>,
        /// Configure shell startup files (safe to repeat); open a new shell afterward.
        #[arg(long)]
        install: bool,
    },
}

#[derive(Subcommand)]
enum DaemonCommands {
    Start {
        #[arg(long)]
        poll: bool,
    },
    Stop,
    Restart {
        #[arg(long)]
        poll: bool,
    },
    Status,
    /// Run in the foreground (also used by launchd/systemd).
    Run {
        #[arg(long)]
        poll: bool,
    },
}

#[derive(Subcommand)]
enum ServiceCommands {
    Install,
    Uninstall,
    /// Print the service definition without installing it.
    Show,
}

fn main() {
    if let Err(error) = execute(Cli::parse()) {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn execute(cli: Cli) -> Result<()> {
    if let Commands::Completions { shell, install } = cli.command {
        let shell = shell.or_else(Shell::from_env).context(
            "cannot detect shell; specify bash, zsh, or fish, e.g. completions zsh --install",
        )?;
        if install {
            let paths = filetrail::completion::install(shell, &std::env::current_exe()?)?;
            for path in paths {
                println!("Configured {}", path.display());
            }
            println!("{shell} Tab completion installed. Open a new shell to activate it.");
        } else {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "filetrail",
                &mut std::io::stdout(),
            );
        }
        return Ok(());
    }
    let store = Store::new(data_root(cli.data_dir)?)?;
    match cli.command {
        Commands::Init { repository, subdir } => {
            let _lock = store.lock()?;
            if store.root.join("config.toml").exists() {
                bail!("already initialized; edit config.toml or use a different --data-dir");
            }
            let subdir = filetrail::config::relative(&subdir)?;
            let repository = if repository.starts_with("~") {
                dirs::home_dir()
                    .context("cannot determine home directory")?
                    .join(repository.strip_prefix("~")?)
            } else {
                repository
            };
            fs::create_dir_all(&repository)?;
            let repository = fs::canonicalize(repository)?;
            if repository.starts_with(&store.root) || store.root.starts_with(&repository) {
                bail!("repository and data directory must not overlap");
            }
            let config = Config::new(repository.clone(), subdir)?;
            let repo = if repository.join(".git").exists() {
                git2::Repository::open(&repository)?
            } else {
                git2::Repository::init(&repository)?
            };
            drop(repo);
            filetrail::git::open(&config)?;
            filetrail::sync::safe_destination(
                &repository,
                &config.subdir.join(".filetrail-check"),
            )?;
            store.save_config(&config)?;
            println!(
                "Initialized {} (subdirectory: {})\nData directory: {}\nConfig: {}",
                repository.display(),
                if config.subdir.as_os_str().is_empty() {
                    ".".to_owned()
                } else {
                    config.subdir.display().to_string()
                },
                store.root.display(),
                store.root.join("config.toml").display()
            );
        }
        Commands::Add {
            source,
            to,
            from,
            delete,
            exclude,
        } => {
            {
                let _lock = store.lock()?;
                let mut config = store.config()?;
                let state = store.state()?;
                let mut next = config
                    .entries
                    .iter()
                    .map(|entry| entry.id)
                    .chain(state.owned.values().copied())
                    .max()
                    .unwrap_or(0)
                    + 1;
                let cwd = std::env::current_dir()?;
                let mut sources = Vec::new();
                if let Some(list) = from {
                    let list = filetrail::config::expand(&list, &cwd)?;
                    let base = list.parent().context("list has no parent")?;
                    for (line_number, line) in fs::read_to_string(&list)?.lines().enumerate() {
                        let Some((source, target)) = filetrail::manifest::parse_line(line)
                            .with_context(|| format!("{}:{}", list.display(), line_number + 1))?
                        else {
                            continue;
                        };
                        sources.push((
                            filetrail::config::expand(&source, base).with_context(|| {
                                format!("{}:{}", list.display(), line_number + 1)
                            })?,
                            target,
                        ));
                    }
                } else {
                    sources.push((
                        filetrail::config::expand(&source.context("missing source")?, &cwd)?,
                        to,
                    ));
                }
                if sources.is_empty() {
                    bail!("source list contains no entries");
                }
                for (source, target) in sources {
                    if source.starts_with(&store.root) || store.root.starts_with(&source) {
                        bail!("source and data directory must not overlap");
                    }
                    filetrail::sync::key(&source)?;
                    let metadata = fs::symlink_metadata(&source)?;
                    if !metadata.is_file()
                        && !metadata.is_dir()
                        && !metadata.file_type().is_symlink()
                    {
                        bail!("source must be a regular file, directory, or symlink");
                    }
                    let target = match target {
                        Some(target) => filetrail::config::relative(&target)?,
                        None => {
                            let home = fs::canonicalize(
                                dirs::home_dir().context("cannot determine home directory")?,
                            )?;
                            filetrail::config::default_target(&source, &home)?
                        }
                    };
                    config.entries.push(Entry {
                        id: next,
                        source,
                        target,
                        directory: metadata.is_dir(),
                        enabled: true,
                        delete,
                        exclude: exclude.clone(),
                    });
                    next += 1;
                }
                store.save_config(&config)?;
            }
            print_report(filetrail::sync::run(&store, false, None)?)?;
        }
        Commands::List => {
            let _lock = store.lock()?;
            let config = store.config()?;
            for entry in &config.entries {
                println!(
                    "{} [{}] {} -> {} (delete={})",
                    entry.id,
                    if entry.enabled { "enabled" } else { "disabled" },
                    entry.source.display(),
                    config.destination(entry).display(),
                    entry.delete
                );
            }
        }
        Commands::Remove { id } | Commands::Enable { id } | Commands::Disable { id } => {
            let _lock = store.lock()?;
            let mut config = store.config()?;
            let entry = config
                .entries
                .iter_mut()
                .find(|entry| entry.id == id)
                .context("unknown entry ID")?;
            match cli.command {
                Commands::Remove { .. } => {
                    config.entries.retain(|entry| entry.id != id);
                    let mut state = store.state()?;
                    state
                        .conflicts
                        .retain(|path, _| state.owned.get(path) != Some(&id));
                    store.save_state(&state)?;
                }
                Commands::Enable { .. } => entry.enabled = true,
                _ => entry.enabled = false,
            }
            store.save_config(&config)?;
        }
        Commands::Sync { dry_run } => print_report(filetrail::sync::run(&store, dry_run, None)?)?,
        Commands::Daemon(command) => match command {
            DaemonCommands::Start { poll } => {
                println!("{}", filetrail::daemon::start(&store, poll)?)
            }
            DaemonCommands::Stop => println!("{}", filetrail::daemon::stop(&store)?),
            DaemonCommands::Restart { poll } => {
                if filetrail::daemon::request(&store, "status").is_ok() {
                    filetrail::daemon::stop(&store)?;
                }
                println!("{}", filetrail::daemon::start(&store, poll)?);
            }
            DaemonCommands::Status => println!("{}", filetrail::daemon::request(&store, "status")?),
            DaemonCommands::Run { poll } => filetrail::daemon::run(&store, poll)?,
        },
        Commands::Service(command) => println!(
            "{}",
            match command {
                ServiceCommands::Install => filetrail::service::install(&store)?,
                ServiceCommands::Uninstall => filetrail::service::uninstall(&store)?,
                ServiceCommands::Show => filetrail::service::render(
                    &store,
                    std::env::current_exe()?
                        .to_str()
                        .context("invalid executable path")?,
                    cfg!(target_os = "macos")
                ),
            }
        ),
        Commands::Pause => println!("{}", filetrail::daemon::request(&store, "pause")?),
        Commands::Resume => println!("{}", filetrail::daemon::request(&store, "resume")?),
        Commands::Status => {
            println!("{}", filetrail::git::status(&store)?);
            println!(
                "Daemon: {}",
                filetrail::daemon::request(&store, "status").unwrap_or_else(|_| "stopped".into())
            );
        }
        Commands::Diff { paths } => print!("{}", filetrail::git::diff(&store, &paths)?),
        Commands::Commit { message, paths } => println!(
            "{}",
            filetrail::git::commit(&store, message.as_deref(), &paths)?
        ),
        Commands::Conflicts => {
            let _lock = store.lock()?;
            for (path, reason) in store.state()?.conflicts {
                println!("{path}: {reason}");
            }
        }
        Commands::Resolve { path, .. } => {
            let path = filetrail::config::relative(&path)?;
            {
                let _lock = store.lock()?;
                if !store
                    .state()?
                    .conflicts
                    .contains_key(&filetrail::sync::key(&path)?)
                {
                    bail!(
                        "no recorded conflict for {}; run sync first",
                        path.display()
                    );
                }
            }
            print_report(filetrail::sync::run(&store, false, Some(&path))?)?;
        }
        Commands::Logs { follow } => {
            let path = store.root.join("filetrail.log");
            let mut printed = 0;
            loop {
                match fs::read(&path) {
                    Ok(bytes) => {
                        if bytes.len() < printed {
                            printed = 0;
                        }
                        use std::io::Write;
                        std::io::stdout().write_all(&bytes[printed..])?;
                        std::io::stdout().flush()?;
                        printed = bytes.len();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error.into()),
                }
                if !follow {
                    break;
                }
                thread::sleep(Duration::from_millis(500));
            }
        }
        Commands::Doctor => {
            let _lock = store.lock()?;
            let config = store.config()?;
            let repo = filetrail::git::open(&config)?;
            filetrail::git::ensure_idle(&repo)?;
            for entry in &config.entries {
                fs::symlink_metadata(&entry.source)
                    .with_context(|| format!("source unavailable: {}", entry.source.display()))?;
                filetrail::sync::safe_destination(&config.repository, &config.destination(entry))?;
            }
            store.state()?;
            if repo.signature().is_err() {
                println!("Git identity missing: set user.name and user.email before committing");
            }
            println!("Configuration, repository, source paths, and state are valid");
        }
        Commands::Completions { .. } => unreachable!(),
    }
    Ok(())
}

fn print_report(report: filetrail::sync::Report) -> Result<()> {
    if report.actions.is_empty() && report.errors.is_empty() {
        println!("Up to date");
    } else {
        println!("{}", report.text());
    }
    if !report.errors.is_empty() {
        bail!(
            "synchronization completed with {} error(s); see above",
            report.errors.len()
        );
    }
    Ok(())
}

fn data_root(override_dir: Option<PathBuf>) -> Result<PathBuf> {
    match override_dir {
        Some(directory) => Ok(directory),
        None => Ok(dirs::home_dir()
            .context("cannot determine home directory; specify --data-dir")?
            .join(".filetrail")),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;

    use super::Cli;
    use super::data_root;

    #[test]
    fn data_directory_defaults_to_dotfile_in_home() {
        let expected = dirs::home_dir().unwrap().join(".filetrail");
        assert_eq!(data_root(None).unwrap(), expected);
        let cli = Cli::try_parse_from(["filetrail", "status"]).unwrap();
        assert_eq!(data_root(cli.data_dir).unwrap(), expected);
    }

    #[test]
    fn data_directory_override_is_global_and_old_flag_is_removed() {
        for arguments in [
            ["filetrail", "--data-dir", "/tmp/filetrail-test", "status"],
            ["filetrail", "status", "--data-dir", "/tmp/filetrail-test"],
        ] {
            let cli = Cli::try_parse_from(arguments).unwrap();
            assert_eq!(
                data_root(cli.data_dir).unwrap(),
                PathBuf::from("/tmp/filetrail-test")
            );
        }
        assert!(
            Cli::try_parse_from(["filetrail", "--config-dir", "/tmp/filetrail-test", "status"])
                .is_err()
        );
    }
}
