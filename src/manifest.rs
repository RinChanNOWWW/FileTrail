use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;

/// Parse one list entry as source [target], using quotes for whitespace in paths.
/// This only tokenizes text: it never invokes a shell or expands variables.
pub fn parse_line(line: &str) -> Result<Option<(PathBuf, Option<PathBuf>)>> {
    let words = shlex::split(line).context("invalid quoting or trailing escape in file list")?;
    if words.is_empty() {
        return Ok(None);
    }
    if words.len() > 2 {
        bail!("expected source [target]; quote paths containing spaces");
    }
    if words
        .iter()
        .any(|word| word.is_empty() || word.contains('\0'))
    {
        bail!("source and target must be nonempty paths without NUL characters");
    }
    Ok(Some((
        PathBuf::from(&words[0]),
        words.get(1).map(PathBuf::from),
    )))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::parse_line;

    #[test]
    fn parses_optional_targets_quotes_escapes_and_comments() {
        for (line, source, target) in [
            ("~/.zshrc", "~/.zshrc", None),
            (" /opt/scripts   scripts ", "/opt/scripts", Some("scripts")),
            (
                "\"My Notes\" 'backup notes'",
                "My Notes",
                Some("backup notes"),
            ),
            (
                "My\\ Notes backup\\ notes",
                "My Notes",
                Some("backup notes"),
            ),
            ("\"file\\\"name\" target", "file\"name", Some("target")),
            ("'hash#file' target # comment", "hash#file", Some("target")),
            (
                "\"$HOME/$(command)\" target",
                "$HOME/$(command)",
                Some("target"),
            ),
            ("source\ttarget", "source", Some("target")),
        ] {
            assert_eq!(
                parse_line(line).unwrap(),
                Some((PathBuf::from(source), target.map(PathBuf::from))),
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
