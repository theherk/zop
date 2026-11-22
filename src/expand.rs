//! `~`, `$VAR` and `${VAR}` expansion. A literal `$` is written `\$`.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

use crate::error::{Error, Result};

/// Expands `~`, `$VAR` and `${VAR}` in a path-like argument. Works on raw bytes, so a
/// path that isn't valid UTF-8 passes through unchanged.
pub fn expand(input: &OsStr) -> Result<OsString> {
    let input = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(input.len());
    let rest = if input == b"~" || input.starts_with(b"~/") {
        out.extend_from_slice(env("HOME")?.as_bytes());
        &input[1..]
    } else {
        input
    };

    let shown = || String::from_utf8_lossy(input).into_owned();
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            b'\\' if rest.get(i + 1) == Some(&b'$') => {
                out.push(b'$');
                i += 2;
            }
            b'$' => {
                i += 1;
                let name: &[u8] = if rest.get(i) == Some(&b'{') {
                    let start = i + 1;
                    let Some(len) = rest[start..].iter().position(|c| *c == b'}') else {
                        return Err(Error::new(format!("unterminated ${{ in {:?}", shown())));
                    };
                    i = start + len + 1;
                    &rest[start..start + len]
                } else {
                    let start = i;
                    while rest
                        .get(i)
                        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
                    {
                        i += 1;
                    }
                    &rest[start..i]
                };
                if name.is_empty() {
                    out.push(b'$');
                } else {
                    out.extend_from_slice(env(OsStr::from_bytes(name))?.as_bytes());
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(OsString::from_vec(out))
}

/// `expand` for a clap value parser. clap hands over the raw `OsStr`, so a path that
/// isn't valid UTF-8 is a normal argument, not a panic.
#[derive(Clone, Copy, Debug)]
pub struct PathParser;

impl clap::builder::TypedValueParser for PathParser {
    type Value = PathBuf;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> std::result::Result<PathBuf, clap::Error> {
        expand(value).map(PathBuf::from).map_err(|e| {
            let mut err = clap::Error::new(clap::error::ErrorKind::ValueValidation).with_cmd(cmd);
            if let Some(arg) = arg {
                err.insert(
                    clap::error::ContextKind::InvalidArg,
                    clap::error::ContextValue::String(arg.to_string()),
                );
            }
            err.insert(
                clap::error::ContextKind::InvalidValue,
                clap::error::ContextValue::String(value.to_string_lossy().into_owned()),
            );
            err.insert(
                clap::error::ContextKind::Custom,
                clap::error::ContextValue::String(e.to_string()),
            );
            err
        })
    }
}

pub fn parse_path(s: &OsStr) -> Result<PathBuf> {
    expand(s).map(PathBuf::from)
}

fn env(name: impl AsRef<OsStr>) -> Result<OsString> {
    let name = name.as_ref();
    std::env::var_os(name)
        .ok_or_else(|| Error::new(format!("undefined variable ${}", name.to_string_lossy())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x(s: &str) -> Result<String> {
        expand(OsStr::new(s)).map(|o| o.into_string().unwrap())
    }

    #[test]
    fn expands_tilde_and_vars() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(x("~").unwrap(), home);
        assert_eq!(x("~/x").unwrap(), format!("{home}/x"));
        assert_eq!(x("$HOME/x").unwrap(), format!("{home}/x"));
        assert_eq!(x("${HOME}x").unwrap(), format!("{home}x"));
    }

    #[test]
    fn leaves_tilde_elsewhere_alone() {
        assert_eq!(x("a~b").unwrap(), "a~b");
        assert_eq!(x("~user/x").unwrap(), "~user/x");
    }

    #[test]
    fn literal_dollar() {
        assert_eq!(x(r"\$HOME").unwrap(), "$HOME");
        assert_eq!(x("cost$").unwrap(), "cost$");
        assert_eq!(x("a$ b").unwrap(), "a$ b");
    }

    #[test]
    fn undefined_variable_is_an_error_naming_it() {
        let err = x("$ZOP_SURELY_UNSET_VAR/x").unwrap_err().to_string();
        assert!(err.contains("ZOP_SURELY_UNSET_VAR"), "{err}");
    }

    #[test]
    fn unterminated_brace_is_an_error() {
        assert!(x("${HOME").is_err());
    }

    #[test]
    fn non_utf8_bytes_pass_through_unchanged() {
        let raw = OsStr::from_bytes(b"/p/caf\xe9/$HOME/\xff");
        let home = std::env::var("HOME").unwrap();
        let mut want = b"/p/caf\xe9/".to_vec();
        want.extend_from_slice(home.as_bytes());
        want.extend_from_slice(b"/\xff");
        assert_eq!(expand(raw).unwrap().as_bytes(), want);
        let plain = OsStr::from_bytes(b"/p/\xfe\xff");
        assert_eq!(expand(plain).unwrap().as_bytes(), b"/p/\xfe\xff");
    }
}
