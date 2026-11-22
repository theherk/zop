//! The index: the last refresh's inputs plus the projects it found.
//!
//! Private, versioned, tab-separated text. Written atomically, under a lock.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::discover::{Kind, Project};
use crate::error::{Error, Result};

pub const VERSION: u32 = 1;
const MAGIC: &str = "zop-index";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inputs {
    pub roots: Vec<PathBuf>,
    pub adds: Vec<PathBuf>,
    pub max_depth: usize,
    pub worktrees: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Index {
    pub inputs: Inputs,
    pub projects: Vec<Project>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Loaded {
    Missing,
    /// An older format, or unreadable. The stored inputs are kept when they could be read.
    Stale(Option<Inputs>),
    Ok(Index),
}

/// Reads the index. A missing file is `Missing`. A file that can't be understood,
/// including one that isn't valid UTF-8, is `Stale`, so a refresh can replace it.
/// Any other failure to read it, such as a permission error, is an error.
pub fn load(path: &Path) -> Result<Loaded> {
    match fs::read(path) {
        Ok(bytes) => Ok(match String::from_utf8(bytes) {
            Ok(text) => parse(&text),
            Err(_) => Loaded::Stale(None),
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Loaded::Missing),
        Err(e) => Err(Error::io(path, e)),
    }
}

pub fn age(path: &Path) -> Option<Duration> {
    fs::metadata(path).ok()?.modified().ok()?.elapsed().ok()
}

fn parse(text: &str) -> Loaded {
    let mut lines = text.lines();
    let version_ok = lines
        .next()
        .and_then(|l| l.strip_prefix(MAGIC))
        .and_then(|v| v.trim().parse::<u32>().ok())
        == Some(VERSION);

    let mut roots = Vec::new();
    let mut adds = Vec::new();
    let mut max_depth = None;
    let mut worktrees = None;
    let mut projects = Vec::new();
    let mut malformed = false;

    for line in lines {
        if let Some(rest) = line.strip_prefix("# ") {
            let Some((key, value)) = rest.split_once('=') else {
                continue;
            };
            match key {
                "root" => roots.push(unescape(value)),
                "add" => adds.push(unescape(value)),
                "max-depth" => max_depth = value.parse().ok(),
                "worktrees" => worktrees = value.parse().ok(),
                _ => {}
            }
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        let [path, real, kind, parent] = fields[..] else {
            malformed = true;
            continue;
        };
        let Some(kind) = Kind::parse(kind) else {
            malformed = true;
            continue;
        };
        projects.push(Project {
            path: unescape(path),
            realpath: unescape(real),
            kind,
            parent: (parent != "-").then(|| unescape(parent)),
        });
    }

    let inputs = match (max_depth, worktrees) {
        (Some(max_depth), Some(worktrees)) => Some(Inputs {
            roots,
            adds,
            max_depth,
            worktrees,
        }),
        _ => None,
    };
    match (version_ok && !malformed, inputs) {
        (true, Some(inputs)) => Loaded::Ok(Index { inputs, projects }),
        (_, inputs) => Loaded::Stale(inputs),
    }
}

pub fn render(index: &Index) -> String {
    let mut out = format!("{MAGIC} {VERSION}\n");
    for root in &index.inputs.roots {
        out.push_str(&format!("# root={}\n", escape(root)));
    }
    for add in &index.inputs.adds {
        out.push_str(&format!("# add={}\n", escape(add)));
    }
    out.push_str(&format!("# max-depth={}\n", index.inputs.max_depth));
    out.push_str(&format!("# worktrees={}\n", index.inputs.worktrees));
    for project in &index.projects {
        let parent = project
            .parent
            .as_ref()
            .map_or_else(|| "-".to_string(), |p| escape(p));
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            escape(&project.path),
            escape(&project.realpath),
            project.kind.as_str(),
            parent,
        ));
    }
    out
}

/// Writes to a temporary file in the same directory, then renames over the index.
pub fn write_atomic(path: &Path, index: &Index) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp.{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let result = (|| -> io::Result<()> {
        let mut file = File::create(&tmp)?;
        file.write_all(render(index).as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|e| Error::io(path, e))
}

/// Escapes a path to ASCII-safe text: `\\`, tab and newline as usual, and any byte
/// that isn't valid UTF-8 as `\xHH`. Every Unix path round-trips exactly.
fn escape(path: &Path) -> String {
    let bytes = path.as_os_str().as_bytes();
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '\t' => out.push_str("\\t"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\x0d"),
                c => out.push(c),
            }
        }
        for b in chunk.invalid() {
            out.push_str(&format!("\\x{b:02x}"));
        }
    }
    out
}

fn unescape(s: &str) -> PathBuf {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        match bytes.get(i + 1) {
            Some(b't') => out.push(b'\t'),
            Some(b'n') => out.push(b'\n'),
            Some(b'\\') => out.push(b'\\'),
            Some(b'x')
                if bytes
                    .get(i + 2..i + 4)
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .map(|b| out.push(b))
                    .is_some() =>
            {
                i += 4;
                continue;
            }
            Some(other) => {
                out.push(b'\\');
                out.push(*other);
            }
            None => out.push(b'\\'),
        }
        i += 2;
    }
    PathBuf::from(OsString::from_vec(out))
}

/// An exclusive lock on `<index>.lock`. The file is never deleted, so a crashed
/// refresh cannot leave a stale lock. Dropping the guard releases it.
pub struct Lock {
    _file: File,
}

impl Lock {
    pub fn acquire(index: &Path, timeout: Duration) -> Result<Lock> {
        let mut name = index.as_os_str().to_owned();
        name.push(".lock");
        let lock_path = PathBuf::from(name);
        if let Some(dir) = lock_path.parent() {
            fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|e| Error::io(&lock_path, e))?;
        let deadline = Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Lock { _file: file }),
                Err(fs::TryLockError::WouldBlock) => {}
                Err(fs::TryLockError::Error(e)) => return Err(Error::io(&lock_path, e)),
            }
            if Instant::now() >= deadline {
                return Err(Error::new(
                    "timed out waiting for another refresh to finish",
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Index {
        Index {
            inputs: Inputs {
                roots: vec![PathBuf::from("/r/one"), PathBuf::from("/r/two")],
                adds: vec![PathBuf::from("/a/extras")],
                max_depth: 12,
                worktrees: true,
            },
            projects: vec![
                Project {
                    path: PathBuf::from("/r/one/a"),
                    realpath: PathBuf::from("/real/one/a"),
                    kind: Kind::Repo,
                    parent: None,
                },
                Project {
                    path: PathBuf::from("/r/one/a-wt"),
                    realpath: PathBuf::from("/real/one/a-wt"),
                    kind: Kind::Worktree,
                    parent: Some(PathBuf::from("/r/one/a")),
                },
                Project {
                    path: PathBuf::from("/odd/tab\there\nand\\slash"),
                    realpath: PathBuf::from("/odd/tab\there\nand\\slash"),
                    kind: Kind::Extra,
                    parent: None,
                },
            ],
        }
    }

    #[test]
    fn round_trips_including_awkward_paths() {
        let index = sample();
        assert_eq!(parse(&render(&index)), Loaded::Ok(index));
    }

    #[test]
    fn non_utf8_paths_round_trip_byte_for_byte() {
        let odd = PathBuf::from(OsString::from_vec(b"/r/caf\xe9/\xff\\x41/ok".to_vec()));
        let mut index = sample();
        index.projects.push(Project {
            path: odd.clone(),
            realpath: odd.clone(),
            kind: Kind::Repo,
            parent: Some(odd.clone()),
        });
        index.inputs.roots.push(odd.clone());
        let text = render(&index);
        assert!(text.is_ascii() || String::from_utf8(text.clone().into_bytes()).is_ok());
        assert_eq!(parse(&text), Loaded::Ok(index));
    }

    #[test]
    fn carriage_returns_in_paths_round_trip() {
        let mut index = sample();
        for odd in ["/r/projects\r", "/r/a\rb", "\r/r/c\r\r"] {
            let odd = PathBuf::from(odd);
            index.inputs.roots.push(odd.clone());
            index.projects.push(Project {
                path: odd.clone(),
                realpath: odd.clone(),
                kind: Kind::Extra,
                parent: Some(odd),
            });
        }
        let text = render(&index);
        assert!(!text.contains('\r'), "no raw carriage return in the file");
        assert_eq!(parse(&text), Loaded::Ok(index));
    }

    #[test]
    fn an_index_that_is_not_utf8_is_stale_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("projects");
        fs::write(&path, b"zop-index 1\n\xff\xfe\n").unwrap();
        assert_eq!(load(&path).unwrap(), Loaded::Stale(None));
    }

    #[test]
    fn an_unreadable_index_is_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("projects");
        fs::write(&path, "x").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read(&path).is_ok() {
            return; // running as root
        }
        let err = load(&path).unwrap_err();
        assert_eq!(err.io_kind(), Some(io::ErrorKind::PermissionDenied));
    }

    #[test]
    fn other_version_is_stale_but_keeps_inputs() {
        let text = render(&sample()).replacen("zop-index 1", "zop-index 99", 1);
        match parse(&text) {
            Loaded::Stale(Some(inputs)) => assert_eq!(inputs, sample().inputs),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn malformed_line_is_stale() {
        let text = format!("{}not\ta\tvalid\n", render(&sample()));
        assert!(matches!(parse(&text), Loaded::Stale(Some(_))));
    }

    #[test]
    fn missing_file_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(&dir.path().join("nope")).unwrap(), Loaded::Missing);
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("projects");
        write_atomic(&path, &sample()).unwrap();
        assert_eq!(load(&path).unwrap(), Loaded::Ok(sample()));
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["projects".to_string()]);
    }

    #[test]
    fn second_lock_times_out_until_the_first_drops() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("projects");
        let first = Lock::acquire(&path, Duration::from_secs(1)).unwrap();
        assert!(Lock::acquire(&path, Duration::from_millis(150)).is_err());
        drop(first);
        assert!(Lock::acquire(&path, Duration::from_millis(150)).is_ok());
    }
}
