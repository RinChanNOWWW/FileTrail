use std::env;
use std::fs;
use std::io::ErrorKind;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use clap_complete::Shell;

const BEGIN: &str = "# >>> filetrail completions >>>";
const END: &str = "# <<< filetrail completions <<<";

/// Install startup hooks without requiring an initialized repository or data directory.
/// Hooks ask the installed binary for current definitions, so upgrades need no regeneration.
pub fn install(shell: Shell, binary: &Path) -> Result<Vec<PathBuf>> {
    let hook = hook(shell, binary)?;
    let home = dirs::home_dir().context("cannot determine home directory")?;
    let paths = match shell {
        Shell::Bash => {
            // Bash reads .bashrc for interactive shells and the first available
            // login profile for login shells (including macOS Terminal).
            let mut login = home.join(".bash_profile");
            for name in [".bash_profile", ".bash_login", ".profile"] {
                let candidate = home.join(name);
                match fs::symlink_metadata(&candidate) {
                    Ok(_) => {
                        login = candidate;
                        break;
                    }
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            vec![home.join(".bashrc"), login]
        }
        Shell::Zsh => vec![environment_directory("ZDOTDIR", &home).join(".zshrc")],
        Shell::Fish => vec![
            environment_directory("XDG_CONFIG_HOME", &home.join(".config"))
                .join("fish/completions/filetrail.fish"),
        ],
        _ => bail!("automatic installation supports bash, zsh, and fish only"),
    };

    // Validate every file before writing any of them. Resolve existing symlinks
    // so common dotfile setups retain both their symlinks and file permissions.
    let updates = paths
        .iter()
        .map(|path| prepare_update(path, &hook))
        .collect::<Result<Vec<_>>>()?;
    for (path, content, permissions) in updates {
        let parent = path
            .parent()
            .context("missing shell configuration parent")?;
        fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(content.as_bytes())?;
        if let Some(permissions) = permissions {
            file.as_file().set_permissions(permissions)?;
        }
        file.as_file().sync_all()?;
        file.persist(&path)
            .with_context(|| format!("cannot update {}", path.display()))?;
    }
    Ok(paths)
}

fn environment_directory(name: &str, fallback: &Path) -> PathBuf {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| fallback.to_owned())
}

fn hook(shell: Shell, binary: &Path) -> Result<String> {
    let binary = binary
        .to_str()
        .context("executable path must be valid UTF-8")?;
    let quoted = match shell {
        Shell::Fish => format!("'{}'", binary.replace('\\', "\\\\").replace('\'', "\\'")),
        _ => format!("'{}'", binary.replace('\'', "'\\''")),
    };
    let body = match shell {
        Shell::Bash => format!(
            "if [ -n \"${{BASH_VERSION-}}\" ] && [ -x {quoted} ]; then\n\
             case $- in\n\
             *i*) eval \"$({quoted} completions bash)\" ;;\n\
             esac\n\
             fi\n"
        ),
        Shell::Zsh => format!(
            "if [[ -o interactive && -x {quoted} ]]; then\n\
             if (( ! $+functions[compdef] )); then\n\
             autoload -Uz compinit\n\
             compinit\n\
             fi\n\
             eval \"$({quoted} completions zsh)\"\n\
             fi\n"
        ),
        Shell::Fish => format!(
            "if test -x {quoted}\n\
             {quoted} completions fish | source\n\
             end\n"
        ),
        _ => bail!("automatic installation supports bash, zsh, and fish only"),
    };
    Ok(format!("{BEGIN}\n{body}{END}\n"))
}

fn prepare_update(path: &Path, hook: &str) -> Result<(PathBuf, String, Option<fs::Permissions>)> {
    let (path, original, permissions) = match fs::symlink_metadata(path) {
        Ok(_) => {
            let resolved = fs::canonicalize(path)
                .with_context(|| format!("cannot resolve {}", path.display()))?;
            let metadata = fs::metadata(&resolved)?;
            if !metadata.is_file() {
                bail!("{} is not a regular file", path.display());
            }
            let content = fs::read_to_string(&resolved)
                .with_context(|| format!("cannot read {}", path.display()))?;
            (resolved, content, Some(metadata.permissions()))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => (path.to_owned(), String::new(), None),
        Err(error) => return Err(error.into()),
    };
    let updated = replace_hook(&original, hook)
        .with_context(|| format!("invalid FileTrail completion block in {}", path.display()))?;
    Ok((path, updated, permissions))
}

fn replace_hook(original: &str, hook: &str) -> Result<String> {
    let mut begin = None;
    let mut end = None;
    let mut offset = 0;
    for line in original.split_inclusive('\n') {
        match line.trim_end_matches(['\r', '\n']) {
            BEGIN if begin.is_none() && end.is_none() => begin = Some(offset),
            END if begin.is_some() && end.is_none() => end = Some(offset + line.len()),
            BEGIN | END => {
                bail!("duplicate or out-of-order markers; repair the marked block first")
            }
            _ => {}
        }
        offset += line.len();
    }
    match (begin, end) {
        (Some(begin), Some(end)) => Ok(format!("{}{hook}{}", &original[..begin], &original[end..])),
        (None, None) => {
            let separator = if original.is_empty() || original.ends_with('\n') {
                ""
            } else {
                "\n"
            };
            Ok(format!("{original}{separator}{hook}"))
        }
        _ => bail!("incomplete markers; repair the marked block first"),
    }
}

#[cfg(test)]
mod tests {
    use super::BEGIN;
    use super::END;
    use super::replace_hook;

    #[test]
    fn replaces_only_its_own_block_and_preserves_surrounding_content() {
        let hook = format!("{BEGIN}\nnew\n{END}\n");
        let original = format!("before\n{BEGIN}\nold\n{END}\nafter\n");
        let updated = replace_hook(&original, &hook).unwrap();
        assert_eq!(updated, format!("before\n{hook}after\n"));
        assert_eq!(replace_hook(&updated, &hook).unwrap(), updated);
        assert_eq!(
            replace_hook("no newline", &hook).unwrap(),
            format!("no newline\n{hook}")
        );
        for broken in [
            format!("{BEGIN}\n"),
            format!("{END}\n"),
            format!("{hook}{hook}"),
        ] {
            assert!(replace_hook(&broken, &hook).is_err());
        }
    }
}
