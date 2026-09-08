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

const BEGIN: &str = "# >>> FileTrail completions >>>";
const END: &str = "# <<< FileTrail completions <<<";

/// Install startup hooks without requiring an initialized repository or data directory.
/// Hooks ask the installed binary for current definitions, so upgrades need no regeneration.
pub fn install(shell: Shell, binary: &Path) -> Result<Vec<PathBuf>> {
    let home = dirs::home_dir().context("cannot determine home directory")?;
    let hook = hook(shell, binary, &home)?;
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
        Shell::Fish => {
            let fish = environment_directory("XDG_CONFIG_HOME", &home.join(".config")).join("fish");
            vec![
                fish.join("completions/filetrail.fish"),
                fish.join("functions/filetrail.fish"),
            ]
        }
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
            .with_context(|| format!("cannot update {}", display_path(&path)))?;
    }
    Ok(paths)
}

fn environment_directory(name: &str, fallback: &Path) -> PathBuf {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| fallback.to_owned())
}

fn home_relative(path: &Path, home: &Path) -> Option<PathBuf> {
    path.strip_prefix(home)
        .ok()
        .map(Path::to_owned)
        .or_else(|| {
            // current_exe may resolve symlinks that are still present in HOME
            // (for example /var versus /private/var on macOS).
            let home = fs::canonicalize(home).ok()?;
            fs::canonicalize(path)
                .ok()?
                .strip_prefix(home)
                .ok()
                .map(Path::to_owned)
        })
}

/// Abbreviate Home paths in completion installation output.
pub fn display_path(path: &Path) -> String {
    match dirs::home_dir().and_then(|home| home_relative(path, &home)) {
        Some(relative) if relative.as_os_str().is_empty() => "$HOME".to_owned(),
        Some(relative) => format!("$HOME/{}", relative.display()),
        None => path.display().to_string(),
    }
}

fn executable_expression(shell: Shell, binary: &Path, home: &Path) -> Result<String> {
    if let Some(relative) = home_relative(binary, home) {
        let relative = relative
            .to_str()
            .context("executable path must be valid UTF-8")?;
        let mut quoted = String::from("\"$HOME");
        if !relative.is_empty() {
            quoted.push('/');
        }
        for character in relative.chars() {
            if matches!(character, '\\' | '"' | '$') || (character == '`' && shell != Shell::Fish) {
                quoted.push('\\');
            }
            quoted.push(character);
        }
        quoted.push('"');
        return Ok(quoted);
    }
    let binary = binary
        .to_str()
        .context("executable path must be valid UTF-8")?;
    Ok(match shell {
        Shell::Fish => format!("'{}'", binary.replace('\\', "\\\\").replace('\'', "\\'")),
        _ => format!("'{}'", binary.replace('\'', "'\\''")),
    })
}

/// A child process cannot change its parent's directory. These shell functions
/// intercept `cd`, while all other commands keep their original arguments.
pub fn shell_integration(shell: Shell, binary: &Path) -> Result<String> {
    let template = match shell {
        Shell::Bash | Shell::Zsh => include_str!("shell_integration.sh"),
        Shell::Fish => include_str!("shell_integration.fish"),
        _ => return Ok(String::new()),
    };
    let home = dirs::home_dir().context("cannot determine home directory")?;
    Ok(template.replace("@FILETRAIL@", &executable_expression(shell, binary, &home)?))
}

fn hook(shell: Shell, binary: &Path, home: &Path) -> Result<String> {
    let quoted = executable_expression(shell, binary, home)?;
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
             compinit -i\n\
             fi\n\
             if (( $+functions[compdef] )); then\n\
             eval \"$({quoted} completions zsh)\"\n\
             fi\n\
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
                .with_context(|| format!("cannot resolve {}", display_path(path)))?;
            let metadata = fs::metadata(&resolved)?;
            if !metadata.is_file() {
                bail!("{} is not a regular file", display_path(path));
            }
            let content = fs::read_to_string(&resolved)
                .with_context(|| format!("cannot read {}", display_path(path)))?;
            (resolved, content, Some(metadata.permissions()))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => (path.to_owned(), String::new(), None),
        Err(error) => return Err(error.into()),
    };
    let updated = replace_hook(&original, hook).with_context(|| {
        format!(
            "invalid FileTrail completion block in {}",
            display_path(&path)
        )
    })?;
    Ok((path, updated, permissions))
}

fn replace_hook(original: &str, hook: &str) -> Result<String> {
    let mut begin = None;
    let mut end = None;
    let mut offset = 0;
    for line in original.split_inclusive('\n') {
        let marker = line.trim_end_matches(['\r', '\n']);
        if marker.eq_ignore_ascii_case(BEGIN) {
            if begin.is_some() || end.is_some() {
                bail!("duplicate or out-of-order markers; repair the marked block first")
            }
            begin = Some(offset);
        } else if marker.eq_ignore_ascii_case(END) {
            if begin.is_none() || end.is_some() {
                bail!("duplicate or out-of-order markers; repair the marked block first")
            }
            end = Some(offset + line.len());
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
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::Path;

    use clap_complete::Shell;

    use super::BEGIN;
    use super::END;
    use super::executable_expression;
    use super::replace_hook;

    #[test]
    fn executable_paths_use_home_only_at_component_boundaries() {
        let home = Path::new("/home/alice");
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
            assert_eq!(
                executable_expression(shell, &home.join(".cargo/bin/filetrail"), home).unwrap(),
                "\"$HOME/.cargo/bin/filetrail\""
            );
            for external in ["/opt/bin/filetrail", "/home/alice-other/bin/filetrail"] {
                assert_eq!(
                    executable_expression(shell, Path::new(external), home).unwrap(),
                    format!("'{external}'")
                );
            }
        }
    }

    #[test]
    fn resolved_executable_paths_match_a_symlinked_home() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("real-home");
        fs::create_dir(&home).unwrap();
        let binary = home.join("filetrail");
        fs::write(&binary, "").unwrap();
        let alias = temp.path().join("home-alias");
        symlink(&home, &alias).unwrap();
        assert_eq!(
            executable_expression(Shell::Zsh, &fs::canonicalize(binary).unwrap(), &alias).unwrap(),
            "\"$HOME/filetrail\""
        );
    }

    #[test]
    fn replaces_only_its_own_block_and_preserves_surrounding_content() {
        let hook = format!("{BEGIN}\nnew\n{END}\n");
        let original = format!("before\n{BEGIN}\nold\n{END}\nafter\n");
        let updated = replace_hook(&original, &hook).unwrap();
        assert_eq!(updated, format!("before\n{hook}after\n"));
        assert_eq!(
            replace_hook(&original.to_lowercase(), &hook).unwrap(),
            updated
        );
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
