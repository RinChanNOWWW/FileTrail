use std::collections::BTreeMap;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub repository: PathBuf,
    pub subdir: PathBuf,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default = "interval")]
    pub scan_interval_secs: u64,
    #[serde(default)]
    pub entries: Vec<Entry>,
}

fn interval() -> u64 {
    30
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: u64,
    pub source: PathBuf,
    pub target: PathBuf,
    pub directory: bool,
    pub enabled: bool,
    pub delete: bool,
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct State {
    pub owned: BTreeMap<String, u64>,
    pub files: BTreeMap<String, Baseline>,
    pub conflicts: BTreeMap<String, String>,
    pub last_sync: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Baseline {
    pub entry: u64,
    pub fingerprint: String,
}

#[derive(Clone, Debug)]
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            root: fs::canonicalize(root)?,
        })
    }

    pub fn lock(&self) -> Result<File> {
        let file = self.lock_file("operation.lock")?;
        fs2::FileExt::lock_exclusive(&file)?;
        crate::lifecycle::recover_retarget(self)?;
        Ok(file)
    }

    pub fn lock_file(&self, name: &str) -> Result<File> {
        Ok(OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join(name))?)
    }

    pub fn config(&self) -> Result<Config> {
        let text = fs::read_to_string(self.root.join("config.toml"))
            .context("not initialized; run filetrail init <repository>")?;
        let config: Config = toml::from_str(&text).context("invalid config.toml")?;
        config.validate()?;
        self.validate_layout(&config)?;
        Ok(config)
    }

    pub fn save_config(&self, config: &Config) -> Result<()> {
        config.validate()?;
        self.validate_layout(config)?;
        atomic_write(
            &self.root.join("config.toml"),
            toml::to_string_pretty(config)?.as_bytes(),
        )
    }

    pub fn state(&self) -> Result<State> {
        crate::state::load(&self.root)
    }

    pub(crate) fn validate_layout(&self, config: &Config) -> Result<()> {
        if self.root.starts_with(&config.repository) || config.repository.starts_with(&self.root) {
            bail!("repository and data directory must not overlap");
        }
        for entry in &config.entries {
            if entry.source.starts_with(&self.root) || self.root.starts_with(&entry.source) {
                bail!("source and data directory must not overlap");
            }
        }
        Ok(())
    }

    pub fn save_state(&self, state: &State) -> Result<()> {
        crate::state::save(&self.root, state)
    }
}

impl Config {
    pub fn new(repository: PathBuf, subdir: PathBuf) -> Result<Self> {
        let config = Self {
            version: 1,
            repository,
            subdir: relative(&subdir)?,
            exclude: vec![],
            scan_interval_secs: interval(),
            entries: vec![],
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != 1 || self.scan_interval_secs == 0 {
            bail!("unsupported config version or zero scan interval");
        }
        if !self.repository.is_absolute() {
            bail!("repository must be absolute");
        }
        relative(&self.subdir)?;
        if self.subdir.components().any(|part| {
            let name = part.as_os_str().to_string_lossy();
            name.eq_ignore_ascii_case("__HOME__") || name.eq_ignore_ascii_case("__ROOT__")
        }) {
            bail!("subdirectory must not contain the reserved __HOME__ or __ROOT__ names");
        }
        crate::sync::exclusions(&self.exclude)?;
        let home = home_dir()?;
        for (i, entry) in self.entries.iter().enumerate() {
            let normalized_target = relative(&entry.target)?;
            if normalized_target.as_os_str().is_empty()
                || !entry.source.is_absolute()
                || entry
                    .source
                    .components()
                    .any(|part| matches!(part, Component::ParentDir))
            {
                bail!("invalid source or target for entry {}", entry.id);
            }
            if normalized_target != default_target(&entry.source, &home)? {
                bail!(
                    "entry {} does not use the __HOME__/__ROOT__ layout; initialize a new data directory and re-add the source",
                    entry.id
                );
            }
            crate::sync::key(&entry.source)?;
            crate::sync::key(&entry.target)?;
            crate::sync::exclusions(&entry.exclude)?;
            if self.repository.starts_with(&entry.source)
                || entry.source.starts_with(&self.repository)
            {
                bail!(
                    "source and repository must not overlap: {}",
                    entry.source.display()
                );
            }
            for other in &self.entries[..i] {
                let target_folded =
                    PathBuf::from(normalized_target.to_string_lossy().to_lowercase());
                let other_folded =
                    PathBuf::from(relative(&other.target)?.to_string_lossy().to_lowercase());
                if entry.id == other.id
                    || target_folded.starts_with(&other_folded)
                    || other_folded.starts_with(&target_folded)
                    || entry.source.starts_with(&other.source)
                    || other.source.starts_with(&entry.source)
                {
                    bail!(
                        "overlapping mappings or duplicate ID: {} and {}",
                        other.id,
                        entry.id
                    );
                }
            }
        }
        Ok(())
    }

    pub fn destination(&self, entry: &Entry) -> PathBuf {
        self.subdir
            .join(&entry.target)
            .components()
            .filter(|part| !matches!(part, Component::CurDir))
            .collect()
    }
}

pub fn relative(path: &Path) -> Result<PathBuf> {
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => (),
            Component::Normal(name) if !name.to_string_lossy().eq_ignore_ascii_case(".git") => {
                clean.push(name)
            }
            _ => bail!(
                "target must be a repository-relative path without '..' or '.git': {}",
                path.display()
            ),
        }
    }
    Ok(clean)
}

pub fn expand(path: &Path, base: &Path) -> Result<PathBuf> {
    let expanded = if path == Path::new("~") {
        dirs::home_dir().context("cannot determine home directory")?
    } else if let Ok(tail) = path.strip_prefix("~") {
        dirs::home_dir()
            .context("cannot determine home directory")?
            .join(tail)
    } else if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    // Resolve parent symlinks, but preserve a leaf symlink as a link to copy.
    let parent = expanded.parent().context("source must have a parent")?;
    Ok(fs::canonicalize(parent)?.join(
        expanded
            .file_name()
            .context("source cannot be filesystem root")?,
    ))
}

pub fn home_dir() -> Result<PathBuf> {
    Ok(fs::canonicalize(
        dirs::home_dir().context("cannot determine home directory")?,
    )?)
}

/// Encode the restoration base in the destination without renaming the source.
pub fn default_target(source: &Path, home: &Path) -> Result<PathBuf> {
    if !source.is_absolute() || !home.is_absolute() {
        bail!("source and Home paths must be absolute");
    }
    match source.strip_prefix(home) {
        Ok(path) => Ok(Path::new("__HOME__").join(relative(path)?)),
        Err(_) => Ok(Path::new("__ROOT__").join(relative(source.strip_prefix("/")?)?)),
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("missing parent")?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::path::PathBuf;

    use super::default_target;
    use super::relative;

    #[test]
    fn default_targets_preserve_home_relative_and_external_absolute_paths() {
        let home = Path::new("/home/alice");
        for (source, expected) in [
            ("/home/alice/.zshrc", "__HOME__/.zshrc"),
            ("/home/alice/.config/nvim", "__HOME__/.config/nvim"),
            ("/home/alice", "__HOME__"),
            ("/opt/scripts/build.sh", "__ROOT__/opt/scripts/build.sh"),
            ("/opt/scripts", "__ROOT__/opt/scripts"),
            (
                "/home/alice-other/settings",
                "__ROOT__/home/alice-other/settings",
            ),
        ] {
            assert_eq!(
                default_target(Path::new(source), home).unwrap(),
                PathBuf::from(expected)
            );
        }
        assert!(default_target(Path::new("relative"), home).is_err());
        assert!(default_target(Path::new("/opt/../escape"), home).is_err());
        assert!(default_target(Path::new("/opt/.git/config"), home).is_err());
    }

    #[test]
    fn rejects_escaping_and_git_targets() {
        for path in [
            "../escape",
            "/absolute",
            "foo/../../bad",
            ".git/config",
            "foo/.GIT/config",
        ] {
            assert!(relative(Path::new(path)).is_err(), "{path}");
        }
        assert_eq!(
            relative(Path::new("./macos/.config")).unwrap(),
            PathBuf::from("macos/.config")
        );
        assert_eq!(relative(Path::new(".")).unwrap(), PathBuf::new());
    }
}
