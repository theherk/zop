//! The sectioned config file: CLI arguments per subcommand, one per line.
//!
//! ```text
//! [refresh]
//! --root=~/projects
//!
//! [list]
//! --refresh-if-older=1d
//! ```

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sections {
    pub list: Vec<String>,
    pub refresh: Vec<String>,
}

pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("zop").join("config")
}

/// Loads the config. The default path may be absent. An explicit one may not.
pub fn load(path: &Path, explicit: bool) -> Result<Sections> {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text).map_err(|e| Error::new(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound && !explicit => Ok(Sections::default()),
        Err(e) => Err(Error::io(path, e)),
    }
}

/// Values are kept as written. `~` and `$VAR` are expanded where the flag is parsed, so
/// a `\$` escape is only ever processed once.
pub fn parse(text: &str) -> Result<Sections> {
    let mut sections = Sections::default();
    let mut current: Option<&str> = None;
    for (number, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = match name {
                "list" => Some("list"),
                "refresh" => Some("refresh"),
                other => {
                    return Err(Error::new(format!(
                        "line {}: unknown section [{other}]",
                        number + 1
                    )));
                }
            };
            continue;
        }
        if !line.starts_with("--") {
            return Err(Error::new(format!(
                "line {}: expected an argument like --flag or --flag=value, got {line:?}",
                number + 1
            )));
        }
        match current {
            Some("list") => sections.list.push(line.to_string()),
            Some("refresh") => sections.refresh.push(line.to_string()),
            _ => {
                return Err(Error::new(format!(
                    "line {}: argument before any [section]",
                    number + 1
                )));
            }
        }
    }
    Ok(sections)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sections_and_ignores_comments() {
        let text = "# top\n\n[refresh]\n--root=~/projects\n--add=~/x\n\n[list]\n# c\n--refresh-if-older=1d\n";
        let sections = parse(text).unwrap();
        assert_eq!(sections.refresh, ["--root=~/projects", "--add=~/x"]);
        assert_eq!(sections.list, ["--refresh-if-older=1d"]);
    }

    #[test]
    fn unknown_section_is_an_error() {
        assert!(
            parse("[global]\n--index=x\n")
                .unwrap_err()
                .to_string()
                .contains("global")
        );
        assert!(parse("[tmp]\n").is_err());
    }

    #[test]
    fn argument_before_a_section_is_an_error() {
        assert!(parse("--json\n").is_err());
    }

    #[test]
    fn non_flag_lines_are_errors() {
        assert!(parse("[list]\njson\n").is_err());
    }

    #[test]
    fn missing_default_file_is_empty_and_explicit_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        assert_eq!(load(&path, false).unwrap(), Sections::default());
        assert!(load(&path, true).is_err());
    }
}
