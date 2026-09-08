use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;

use crate::config::Store;

fn identity(store: &Store) -> String {
    format!(
        "filetrail-{}",
        &blake3::hash(store.root.as_os_str().as_encoded_bytes()).to_hex()[..12]
    )
}

pub fn render(store: &Store, executable: &str, macos: bool) -> String {
    if macos {
        let escape = |value: &str| {
            value
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
                .replace('\'', "&apos;")
        };
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{}</string>\n<key>ProgramArguments</key><array><string>{}</string><string>--data-dir</string><string>{}</string><string>daemon</string><string>run</string></array>\n<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><true/>\n<key>StandardErrorPath</key><string>{}</string>\n</dict></plist>\n",
            identity(store),
            escape(executable),
            escape(&store.root.to_string_lossy()),
            escape(&store.root.join("service-error.log").to_string_lossy())
        )
    } else {
        let quote = |value: &str| {
            format!(
                "\"{}\"",
                value
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('%', "%%")
                    .replace('$', "$$")
                    .replace('\n', "\\n")
            )
        };
        format!(
            "[Unit]\nDescription=Filetrail file synchronization\n\n[Service]\nType=simple\nExecStart={} --data-dir {} daemon run\nRestart=on-failure\nRestartSec=3\n\n[Install]\nWantedBy=default.target\n",
            quote(executable),
            quote(&store.root.to_string_lossy())
        )
    }
}

fn location(store: &Store) -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot determine home directory")?;
    Ok(if cfg!(target_os = "macos") {
        home.join("Library/LaunchAgents")
            .join(format!("{}.plist", identity(store)))
    } else {
        dirs::config_dir()
            .context("cannot determine systemd user service directory")?
            .join("systemd/user")
            .join(format!("{}.service", identity(store)))
    })
}

fn execute(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program).args(args).status()?;
    if !status.success() {
        bail!("{program} {} failed: {status}", args.join(" "));
    }
    Ok(())
}

fn domain() -> Result<String> {
    let output = Command::new("id").arg("-u").output()?;
    if !output.status.success() {
        bail!("cannot determine user ID");
    }
    Ok(format!("gui/{}", String::from_utf8(output.stdout)?.trim()))
}

pub fn install(store: &Store) -> Result<String> {
    store.config()?;
    let location = location(store)?;
    if location.exists() {
        bail!(
            "service already installed at {}; uninstall it first",
            location.display()
        );
    }
    if crate::daemon::request(store, "status").is_ok() {
        crate::daemon::stop(store)?;
    }
    let executable = std::env::current_exe()?;
    crate::config::atomic_write(
        &location,
        render(
            store,
            executable
                .to_str()
                .context("executable path is not UTF-8")?,
            cfg!(target_os = "macos"),
        )
        .as_bytes(),
    )?;
    if cfg!(target_os = "macos") {
        execute(
            "launchctl",
            &[
                "bootstrap",
                &domain()?,
                location.to_str().context("invalid service path")?,
            ],
        )?;
    } else {
        execute("systemctl", &["--user", "daemon-reload"])?;
        execute(
            "systemctl",
            &[
                "--user",
                "enable",
                "--now",
                &format!("{}.service", identity(store)),
            ],
        )?;
    }
    Ok(format!("installed {}", location.display()))
}

pub fn is_installed(store: &Store) -> Result<bool> {
    Ok(location(store)?.try_exists()?)
}

pub fn uninstall(store: &Store) -> Result<String> {
    let location = location(store)?;
    if !location.exists() {
        bail!("service is not installed");
    }
    if cfg!(target_os = "macos") {
        execute(
            "launchctl",
            &[
                "bootout",
                &domain()?,
                location.to_str().context("invalid service path")?,
            ],
        )?;
    } else {
        execute(
            "systemctl",
            &[
                "--user",
                "disable",
                "--now",
                &format!("{}.service", identity(store)),
            ],
        )?;
    }
    fs::remove_file(&location)?;
    if !cfg!(target_os = "macos") {
        execute("systemctl", &["--user", "daemon-reload"])?;
    }
    Ok(format!("removed {}", location.display()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::render;
    use crate::config::Store;

    #[test]
    fn templates_quote_paths() {
        let store = Store {
            root: PathBuf::from("/tmp/a & b/%x"),
        };
        assert!(render(&store, "/a & b/filetrail", true).contains("/a &amp; b/filetrail"));
        assert!(render(&store, "/a b/filetrail", false).contains("ExecStart=\"/a b/filetrail\""));
        assert!(render(&store, "/filetrail", false).contains("%%x"));
    }

    #[test]
    fn services_pass_the_data_directory_to_the_daemon() {
        let store = Store {
            root: PathBuf::from("/tmp/filetrail-data"),
        };
        assert!(
            render(&store, "/filetrail", true)
                .contains("<string>--data-dir</string><string>/tmp/filetrail-data</string>")
        );
        assert!(
            render(&store, "/filetrail", false)
                .contains("--data-dir \"/tmp/filetrail-data\" daemon run")
        );
    }
}
