use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;

/// Parse one source per line, using quotes for whitespace in paths.
/// This only tokenizes text: it never invokes a shell or expands variables.
pub fn parse_line(line: &str) -> Result<Option<PathBuf>> {
    let words = shlex::split(line).context("invalid quoting or trailing escape in file list")?;
    if words.is_empty() {
        return Ok(None);
    }
    if words.len() != 1 {
        bail!(
            "expected one source per line; quote paths containing spaces; custom targets are not supported"
        );
    }
    if words
        .iter()
        .any(|word| word.is_empty() || word.contains('\0'))
    {
        bail!("source must be a nonempty path without NUL characters");
    }
    Ok(Some(PathBuf::from(&words[0])))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::parse_line;

    #[test]
    fn parses_sources_quotes_escapes_and_comments() {
        for (line, source) in [
            ("~/.zshrc", "~/.zshrc"),
            (" /opt/scripts  ", "/opt/scripts"),
            ("\"My Notes\"", "My Notes"),
            ("My\\ Notes", "My Notes"),
            ("\"file\\\"name\"", "file\"name"),
            ("'hash#file' # comment", "hash#file"),
            ("\"$HOME/$(command)\"", "$HOME/$(command)"),
        ] {
            assert_eq!(
                parse_line(line).unwrap(),
                Some(PathBuf::from(source)),
                "{line}"
            );
        }
        assert_eq!(parse_line("  # comment").unwrap(), None);
        assert_eq!(parse_line("   ").unwrap(), None);
    }

    #[test]
    fn rejects_ambiguous_and_malformed_entries() {
        for line in [
            "a b c",
            "source target",
            "source\ttarget",
            "\"\"",
            "\"unterminated",
            "'unterminated",
            "source \\",
            "\"\" target",
            "source ''",
            "source\0",
        ] {
            assert!(parse_line(line).is_err(), "{line:?}");
        }
    }
}
