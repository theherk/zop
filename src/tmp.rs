//! Per-project tmp paths: `<private dir>/zop/<hash16>[/NAME]`, or `/tmp/zop-<uid>/...`.

use std::ffi::OsStr;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

const MARKER: &str = "path";
const MAX_PATH_BYTES: usize = 100;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(200);
const RECHECK_DELAY: Duration = Duration::from_millis(100);

fn uid() -> u32 {
    crate::sys::effective_uid()
}

/// Bytes that a project's directory and a socket name add to the base:
/// `/` + 16 hex + `/` + a name of up to 16 bytes (`nvim.sock` is 9).
const BASE_ROOM: usize = 1 + 16 + 1 + 16;

fn choose_base(private: Option<PathBuf>, uid: u32) -> PathBuf {
    match private {
        Some(base) if base.as_os_str().len() + BASE_ROOM <= MAX_PATH_BYTES => base,
        _ => PathBuf::from(format!("/tmp/zop-{uid}")),
    }
}

#[cfg(target_os = "macos")]
fn private_dir() -> Option<PathBuf> {
    crate::sys::darwin_user_temp_dir()
}

#[cfg(not(target_os = "macos"))]
fn private_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

pub fn hash16(real: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let digest = Sha256::digest(real.as_os_str().as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// A caller's file name inside a project directory. It must be a single component, and
/// it can't be `path`, which is zop's own marker: overwriting it would make a live
/// project look gone to the next prune.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(Error::new(format!(
            "invalid name {name:?}: it must be a single file name"
        )));
    }
    if name == MARKER {
        return Err(Error::new(format!(
            "invalid name {name:?}: it is reserved for zop's own marker"
        )));
    }
    Ok(())
}

/// A per-project tmp area rooted at one base directory, with the ages that gate pruning.
pub struct Area {
    base: PathBuf,
    pub socket_min_age: Duration,
    pub empty_min_age: Duration,
    pub temp_min_age: Duration,
}

impl Area {
    /// The first of these that leaves room for a socket path:
    /// 1. `$ZOP_TMP_ROOT/zop-<uid>`, an override for tests.
    /// 2. `<user temp dir>/zop`: on macOS the per-user `confstr` directory, on other
    ///    systems `$XDG_RUNTIME_DIR`. Both are private to the user.
    /// 3. `/tmp/zop-<uid>`.
    pub fn from_env() -> Area {
        if let Some(root) = std::env::var_os("ZOP_TMP_ROOT") {
            return Area::at(PathBuf::from(root).join(format!("zop-{}", uid())));
        }
        let private = private_dir().map(|dir| dir.join("zop"));
        Area::at(choose_base(private, uid()))
    }

    pub fn at(base: PathBuf) -> Area {
        Area {
            base,
            socket_min_age: Duration::from_secs(10),
            empty_min_age: Duration::from_secs(10),
            temp_min_age: Duration::from_secs(60),
        }
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    /// The project's tmp directory. Computed only; nothing is created or checked.
    pub fn dir_for(&self, real: &Path) -> PathBuf {
        self.base.join(hash16(real))
    }

    /// The project's tmp path with `name` appended. Errors if it would not fit a socket path.
    pub fn path_for(&self, real: &Path, name: Option<&str>) -> Result<PathBuf> {
        let mut path = self.dir_for(real);
        if let Some(name) = name {
            validate_name(name)?;
            path.push(name);
        }
        if path.as_os_str().len() > MAX_PATH_BYTES {
            return Err(Error::new(format!(
                "{} is longer than {MAX_PATH_BYTES} bytes, too long for a unix socket",
                path.display()
            )));
        }
        Ok(path)
    }

    fn ensure_base(&self) -> Result<()> {
        match fs::symlink_metadata(&self.base) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // Never recursive: a missing parent is an error, not something to create.
                if let Err(e) = DirBuilder::new().mode(0o700).create(&self.base)
                    && e.kind() != io::ErrorKind::AlreadyExists
                {
                    return Err(Error::io(&self.base, e));
                }
            }
            Err(e) => return Err(Error::io(&self.base, e)),
        }
        verify_dir(&self.base)
    }

    /// Creates the project's directory atomically: a temporary directory holding the
    /// `path` marker is renamed into place, so a prune never sees it empty.
    pub fn create(&self, real: &Path) -> Result<PathBuf> {
        self.ensure_base()?;
        let dir = self.dir_for(real);
        match fs::symlink_metadata(&dir) {
            Ok(_) => {
                verify_dir(&dir)?;
                return Ok(dir);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(&dir, e)),
        }
        let temp = self
            .base
            .join(format!("{}.tmp.{}", hash16(real), std::process::id()));
        DirBuilder::new()
            .mode(0o700)
            .create(&temp)
            .map_err(|e| Error::io(&temp, e))?;
        let placed = fs::write(temp.join(MARKER), real.as_os_str().as_bytes())
            .and_then(|()| fs::rename(&temp, &dir));
        if let Err(e) = placed {
            let _ = remove_marker_dir(&temp);
            match fs::symlink_metadata(&dir) {
                Ok(_) => {}
                Err(_) => return Err(Error::io(&dir, e)),
            }
        }
        verify_dir(&dir)?;
        Ok(dir)
    }

    /// Whether the project's `name` is a live socket. Read-only. A missing or unsafe
    /// directory simply isn't live; any other failure to inspect it is an error.
    pub fn is_live_for(&self, real: &Path, name: &str) -> Result<bool> {
        let path = self.path_for(real, Some(name))?;
        let dir = self.dir_for(real);
        for checked in [&self.base, &dir] {
            match verify_dir(checked) {
                Ok(()) => {}
                Err(e) => match e.io_kind() {
                    None | Some(io::ErrorKind::NotFound) => return Ok(false),
                    Some(_) => return Err(e),
                },
            }
        }
        Ok(is_live(&path))
    }

    /// Every existing `<hash>/<name>` under the base directory, optionally only live sockets.
    /// Entries that are not zop's hash directories, or are unsafe, are skipped.
    pub fn all(&self, name: &str, live: bool) -> Result<Vec<PathBuf>> {
        validate_name(name)?;
        match fs::symlink_metadata(&self.base) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::io(&self.base, e)),
        }
        verify_dir(&self.base)?;
        let mut found = Vec::new();
        for entry in fs::read_dir(&self.base).map_err(|e| Error::io(&self.base, e))? {
            let entry = entry.map_err(|e| Error::io(&self.base, e))?;
            if entry_kind(&entry.file_name()) != Some(EntryKind::Hash)
                || verify_dir(&entry.path()).is_err()
            {
                continue;
            }
            let candidate = entry.path().join(name);
            match fs::symlink_metadata(&candidate) {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(Error::io(&candidate, e)),
            }
            if !live || is_live(&candidate) {
                found.push(candidate);
            }
        }
        found.sort();
        Ok(found)
    }

    /// Housekeeping for `zop refresh`. Removes dead sockets, orphaned files of projects
    /// that are gone, and stale empty or temporary directories. Only entries named the
    /// way zop names them are touched. A problem with one entry is reported in
    /// `warnings` and does not stop the rest.
    pub fn prune(&self) -> Result<Pruned> {
        let mut report = Pruned::default();
        match fs::symlink_metadata(&self.base) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(report),
            Err(e) => return Err(Error::io(&self.base, e)),
        }
        verify_dir(&self.base)?;

        let entries = fs::read_dir(&self.base).map_err(|e| Error::io(&self.base, e))?;
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    report.warn(&self.base, &e);
                    continue;
                }
            };
            let dir = entry.path();
            let Some(kind) = entry_kind(&entry.file_name()) else {
                continue;
            };
            if let Err(e) = verify_dir(&dir) {
                report.warnings.push(format!("skipping {dir:?}: {e}"));
                continue;
            }
            match kind {
                EntryKind::Temp => self.prune_temp(&dir, &mut report),
                EntryKind::Hash => self.prune_hash(&dir, &mut report),
            }
        }
        Ok(report)
    }

    fn prune_temp(&self, dir: &Path, report: &mut Pruned) {
        if !older_than(dir, self.temp_min_age) {
            return;
        }
        match remove_marker_dir(dir) {
            Ok(()) => report.dirs += 1,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => report.warn(dir, &e),
        }
    }

    fn prune_hash(&self, dir: &Path, report: &mut Pruned) {
        let files = match list_dir(dir) {
            Ok(files) => files,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(e) => return report.warn(dir, &e),
        };
        for file in &files {
            match self.remove_if_dead_socket(file) {
                Ok(true) => report.sockets += 1,
                Ok(false) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => report.warn(file, &e),
            }
        }

        let marker = dir.join(MARKER);
        let orphan = match fs::read(&marker) {
            Ok(bytes) => match Path::new(OsStr::from_bytes(&bytes)).try_exists() {
                Ok(gone) => !gone,
                Err(e) => {
                    report.warn(dir, &e);
                    false
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => {
                report.warn(dir, &e);
                false
            }
        };
        if orphan {
            // Everything but the marker goes first. The marker is what tells a later pass
            // that this directory is an orphan, so it stays while anything else remains.
            for file in files.iter().filter(|f| **f != marker) {
                let plain = fs::symlink_metadata(file).is_ok_and(|m| m.file_type().is_file());
                if plain && older_than(file, self.empty_min_age) {
                    match fs::remove_file(file) {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                        Err(e) => report.warn(file, &e),
                    }
                }
            }
            match list_dir(dir) {
                Ok(rest) if rest == [marker.clone()] && older_than(&marker, self.empty_min_age) => {
                    match fs::remove_file(&marker) {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                        Err(e) => report.warn(&marker, &e),
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => report.warn(dir, &e),
            }
        }

        match list_dir(dir) {
            Ok(rest) if rest.is_empty() && (orphan || older_than(dir, self.empty_min_age)) => {
                match fs::remove_dir(dir) {
                    Ok(()) => report.dirs += 1,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => report.warn(dir, &e),
                }
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => report.warn(dir, &e),
        }
    }

    /// Removes a socket only if it is ours, old enough, refused twice, and unchanged
    /// between the decision and the unlink. Only an explicit connection refusal counts;
    /// any other outcome, such as a timeout or a socket that can't be probed with a
    /// stream connect, leaves the file alone.
    fn remove_if_dead_socket(&self, path: &Path) -> io::Result<bool> {
        let first = fs::symlink_metadata(path)?;
        if !first.file_type().is_socket()
            || first.uid() != uid()
            || !older_than(path, self.socket_min_age)
        {
            return Ok(false);
        }
        if probe(path) != Probe::Refused {
            return Ok(false);
        }
        thread::sleep(RECHECK_DELAY);
        if probe(path) != Probe::Refused {
            return Ok(false);
        }
        let again = fs::symlink_metadata(path)?;
        if again.ino() != first.ino() || again.dev() != first.dev() {
            return Ok(false);
        }
        fs::remove_file(path)?;
        Ok(true)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    pub sockets: usize,
    pub dirs: usize,
    pub warnings: Vec<String>,
}

impl Pruned {
    fn warn(&mut self, path: &Path, e: &io::Error) {
        self.warnings.push(format!("{}: {e}", path.display()));
    }
}

#[derive(Debug, PartialEq, Eq)]
enum EntryKind {
    /// `<hash16>`
    Hash,
    /// `<hash16>.tmp.<pid>`
    Temp,
}

/// Recognises exactly the two names zop creates. Anything else is not ours.
fn entry_kind(name: &OsStr) -> Option<EntryKind> {
    let bytes = name.as_bytes();
    let is_hash =
        |b: &[u8]| b.len() == 16 && b.iter().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'));
    if is_hash(bytes) {
        return Some(EntryKind::Hash);
    }
    let (hash, pid) = (bytes.get(..16)?, bytes.get(16..)?.strip_prefix(b".tmp.")?);
    (is_hash(hash) && !pid.is_empty() && pid.iter().all(u8::is_ascii_digit))
        .then_some(EntryKind::Temp)
}

fn list_dir(dir: &Path) -> io::Result<Vec<PathBuf>> {
    fs::read_dir(dir)?.map(|e| e.map(|e| e.path())).collect()
}

/// Removes a directory that holds nothing but zop's marker file. It is not recursive:
/// anything else inside makes it fail and the directory stays.
fn remove_marker_dir(dir: &Path) -> io::Result<()> {
    for file in list_dir(dir)? {
        let plain = fs::symlink_metadata(&file)?.file_type().is_file();
        if !plain || file.file_name() != Some(OsStr::new(MARKER)) {
            return Err(io::Error::other(format!(
                "{} holds something zop did not create",
                file.display()
            )));
        }
        fs::remove_file(&file)?;
    }
    fs::remove_dir(dir)
}

/// A real directory, not a symlink, owned by us, closed to group and others.
pub fn verify_dir(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path).map_err(|e| Error::io(path, e))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(Error::new(format!(
            "{} is not a plain directory",
            path.display()
        )));
    }
    if meta.uid() != uid() {
        return Err(Error::new(format!(
            "{} is not owned by you",
            path.display()
        )));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(Error::new(format!(
            "{} is accessible to others",
            path.display()
        )));
    }
    Ok(())
}

/// What a stream connect to a socket path told us.
#[derive(Debug, PartialEq, Eq)]
pub enum Probe {
    /// Something accepted the connection.
    Connected,
    /// The kernel said no one is listening: `ECONNREFUSED`.
    Refused,
    /// The path is gone.
    Absent,
    /// No answer in time, or an error that doesn't show whether anyone is listening
    /// (a datagram socket, a permission problem, ...).
    Indeterminate,
}

pub fn probe(socket: &Path) -> Probe {
    let (tx, rx) = mpsc::channel();
    let path = socket.to_path_buf();
    thread::spawn(move || {
        let _ = tx.send(UnixStream::connect(&path).map(|_| ()));
    });
    match rx.recv_timeout(CONNECT_TIMEOUT) {
        Ok(Ok(())) => Probe::Connected,
        Ok(Err(e)) => match e.kind() {
            io::ErrorKind::ConnectionRefused => Probe::Refused,
            io::ErrorKind::NotFound => Probe::Absent,
            _ => Probe::Indeterminate,
        },
        Err(_) => Probe::Indeterminate,
    }
}

/// Whether a socket might be in use. Anything but a clear refusal or a missing path
/// counts as live, so a busy, stopped or unprobeable process is never mistaken for a
/// dead one.
pub fn is_live(socket: &Path) -> bool {
    matches!(probe(socket), Probe::Connected | Probe::Indeterminate)
}

fn older_than(path: &Path, age: Duration) -> bool {
    fs::symlink_metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|elapsed| elapsed >= age)
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use super::*;

    fn area() -> (tempfile::TempDir, Area) {
        let dir = tempfile::tempdir().unwrap();
        let area = Area::at(dir.path().join(format!("zop-{}", uid())));
        (dir, area)
    }

    fn zero_ages(area: &mut Area) {
        area.socket_min_age = Duration::ZERO;
        area.empty_min_age = Duration::ZERO;
        area.temp_min_age = Duration::ZERO;
    }

    #[test]
    fn base_prefers_the_private_dir_when_it_fits() {
        let private = PathBuf::from("/var/folders/ab/cdef/T/zop");
        assert_eq!(choose_base(Some(private.clone()), 501), private);
    }

    #[test]
    fn base_falls_back_to_tmp_when_missing_or_too_long() {
        assert_eq!(choose_base(None, 501), PathBuf::from("/tmp/zop-501"));
        let long = PathBuf::from(format!("/{}/zop", "x".repeat(80)));
        assert_eq!(choose_base(Some(long), 7), PathBuf::from("/tmp/zop-7"));
    }

    #[test]
    fn a_private_dir_is_absolute_and_not_world_accessible() {
        if let Some(dir) = private_dir() {
            assert!(dir.is_absolute());
            let mode = fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "{}", dir.display());
        }
    }

    #[test]
    fn hash_is_16_hex_and_stable() {
        let h = hash16(Path::new("/p/zop"));
        assert_eq!(h.len(), 16);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(h, hash16(Path::new("/p/zop")));
        assert_ne!(h, hash16(Path::new("/p/zop2")));
    }

    #[test]
    fn names_must_be_one_component() {
        for bad in ["", ".", "..", "a/b", "../x", "a\0b"] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
        assert!(validate_name("nvim.sock").is_ok());
    }

    #[test]
    fn path_for_guards_the_socket_length() {
        let (_dir, area) = area();
        let real = Path::new("/p/zop");
        assert!(area.path_for(real, Some("nvim.sock")).is_ok());
        let long = "x".repeat(120);
        assert!(area.path_for(real, Some(&long)).is_err());
        assert!(area.path_for(real, Some("../escape")).is_err());
    }

    #[test]
    fn create_is_atomic_idempotent_and_writes_the_marker() {
        let (_dir, area) = area();
        let real = Path::new("/p/zop");
        let dir = area.create(real).unwrap();
        assert_eq!(dir, area.dir_for(real));
        assert_eq!(fs::read_to_string(dir.join(MARKER)).unwrap(), "/p/zop");
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(area.base()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(area.create(real).unwrap(), dir);
        let leftovers = fs::read_dir(area.base())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".tmp.")
            })
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn a_symlinked_base_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        let base = dir.path().join("zop");
        std::os::unix::fs::symlink(&elsewhere, &base).unwrap();
        let area = Area::at(base);
        assert!(area.create(Path::new("/p/zop")).is_err());
    }

    #[test]
    fn a_base_open_to_others_is_refused() {
        let (_dir, area) = area();
        area.create(Path::new("/p/zop")).unwrap();
        fs::set_permissions(area.base(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(area.create(Path::new("/p/other")).is_err());
        assert!(area.all("nvim.sock", false).is_err());
    }

    #[test]
    fn liveness_tells_listening_from_dead_from_missing() {
        let (_dir, area) = area();
        let real = Path::new("/p/zop");
        let dir = area.create(real).unwrap();
        let sock = dir.join("nvim.sock");
        assert!(!area.is_live_for(real, "nvim.sock").unwrap(), "missing");

        let listener = UnixListener::bind(&sock).unwrap();
        assert!(
            area.is_live_for(real, "nvim.sock").unwrap(),
            "listening, never accepted"
        );
        drop(listener);
        assert!(sock.exists());
        assert!(
            !area.is_live_for(real, "nvim.sock").unwrap(),
            "dead socket file"
        );
    }

    #[test]
    fn live_check_never_removes_anything() {
        let (_dir, area) = area();
        let real = Path::new("/p/zop");
        let dir = area.create(real).unwrap();
        let sock = dir.join("nvim.sock");
        drop(UnixListener::bind(&sock).unwrap());
        assert!(!area.is_live_for(real, "nvim.sock").unwrap());
        assert!(sock.exists());
    }

    #[test]
    fn all_finds_sockets_and_filters_live() {
        let (_dir, area) = area();
        let a = area.create(Path::new("/p/a")).unwrap();
        let b = area.create(Path::new("/p/b")).unwrap();
        let _live = UnixListener::bind(a.join("nvim.sock")).unwrap();
        drop(UnixListener::bind(b.join("nvim.sock")).unwrap());

        assert_eq!(area.all("nvim.sock", false).unwrap().len(), 2);
        assert_eq!(
            area.all("nvim.sock", true).unwrap(),
            vec![a.join("nvim.sock")]
        );
        assert!(area.all("other", false).unwrap().is_empty());
        assert!(area.all("../x", false).is_err());
    }

    #[test]
    fn prune_removes_a_dead_socket_only_when_old_enough() {
        let (_dir, mut area) = area();
        let real = tempfile::tempdir().unwrap();
        let dir = area.create(real.path()).unwrap();
        let sock = dir.join("nvim.sock");
        drop(UnixListener::bind(&sock).unwrap());

        area.socket_min_age = Duration::from_secs(3600);
        assert_eq!(area.prune().unwrap().sockets, 0);
        assert!(sock.exists());

        area.socket_min_age = Duration::ZERO;
        assert_eq!(area.prune().unwrap().sockets, 1);
        assert!(!sock.exists());
        assert!(
            dir.exists(),
            "the project still exists, so its directory stays"
        );
    }

    #[test]
    fn prune_keeps_a_live_socket() {
        let (_dir, mut area) = area();
        zero_ages(&mut area);
        let real = tempfile::tempdir().unwrap();
        let dir = area.create(real.path()).unwrap();
        let sock = dir.join("nvim.sock");
        let _live = UnixListener::bind(&sock).unwrap();
        assert_eq!(area.prune().unwrap(), Pruned::default());
        assert!(sock.exists());
    }

    #[test]
    fn prune_removes_the_directory_of_a_project_that_is_gone() {
        let (_dir, mut area) = area();
        zero_ages(&mut area);
        let gone = Path::new("/definitely/not/here");
        let dir = area.create(gone).unwrap();
        fs::write(dir.join("server.log"), "x").unwrap();
        let report = area.prune().unwrap();
        assert_eq!(report.dirs, 1);
        assert!(!dir.exists());
    }

    #[test]
    fn prune_keeps_an_orphan_that_still_has_a_live_socket() {
        let (_dir, mut area) = area();
        zero_ages(&mut area);
        let dir = area.create(Path::new("/definitely/not/here")).unwrap();
        let _live = UnixListener::bind(dir.join("nvim.sock")).unwrap();
        fs::write(dir.join("server.log"), "x").unwrap();
        area.prune().unwrap();
        assert!(dir.join("nvim.sock").exists());
        assert!(
            !dir.join("server.log").exists(),
            "non-live files in an orphan are cleared"
        );
    }

    #[test]
    fn prune_clears_stale_temp_and_empty_directories() {
        let (_dir, mut area) = area();
        zero_ages(&mut area);
        area.create(Path::new("/p/zop")).unwrap();
        let temp = area.base().join("0123456789abcdef.tmp.99999");
        fs::create_dir(&temp).unwrap();
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o700)).unwrap();
        let empty = area.base().join("fedcba9876543210");
        fs::create_dir(&empty).unwrap();
        fs::set_permissions(&empty, fs::Permissions::from_mode(0o700)).unwrap();
        area.prune().unwrap();
        assert!(!temp.exists());
        assert!(!empty.exists());
    }

    #[test]
    fn prune_with_no_base_is_a_no_op() {
        let (_dir, area) = area();
        assert_eq!(area.prune().unwrap(), Pruned::default());
    }

    const HASH: &str = "0123456789abcdef";

    #[test]
    fn entry_names_are_exactly_zops_two_schemes() {
        let kind = |n: &str| entry_kind(OsStr::new(n));
        assert_eq!(kind(HASH), Some(EntryKind::Hash));
        assert_eq!(kind(&format!("{HASH}.tmp.123")), Some(EntryKind::Temp));
        for bad in [
            "backup.tmp.old",
            "x.tmp.1",
            &format!("{HASH}.tmp."),
            &format!("{HASH}.tmp.12a"),
            &format!("{HASH}.tmp.1.2"),
            &format!("{HASH}0"),
            &format!("{HASH}.bak"),
            "0123456789ABCDEF",
            "0123456789abcde",
            "nvim.sock",
            "",
        ] {
            assert_eq!(kind(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn prune_removes_only_a_valid_stale_temp_directory() {
        let (_guard, mut area) = area();
        zero_ages(&mut area);
        area.ensure_base().unwrap();
        let mk = |name: &str, file: Option<&str>| {
            let dir = area.base().join(name);
            DirBuilder::new().mode(0o700).create(&dir).unwrap();
            if let Some(file) = file {
                fs::write(dir.join(file), "x").unwrap();
            }
            dir
        };
        let valid = mk(&format!("{HASH}.tmp.4242"), Some(MARKER));
        let lookalike = mk("backup.tmp.old", Some("precious"));
        let lookalike_empty = mk("data.tmp.1", None);
        let holds_foreign = mk(&format!("{}.tmp.7", "fedcba9876543210"), Some("precious"));
        let plain_other = mk("notes", Some("precious"));

        let report = area.prune().unwrap();

        assert!(!valid.exists(), "a stale zop temp directory goes");
        assert_eq!(report.dirs, 1);
        assert!(lookalike.join("precious").exists());
        assert!(lookalike_empty.exists());
        assert!(
            holds_foreign.join("precious").exists(),
            "a temp directory holding anything but the marker is kept"
        );
        assert!(plain_other.join("precious").exists());
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("fedcba9876543210")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn prune_keeps_a_live_datagram_socket_and_other_unprobeable_ones() {
        use std::os::unix::net::UnixDatagram;
        let (_guard, mut area) = area();
        zero_ages(&mut area);
        let real = Path::new("/p/dgram");
        let dir = area.create(real).unwrap();
        let sock = dir.join("dgram.sock");
        let _live = UnixDatagram::bind(&sock).unwrap();
        assert_eq!(probe(&sock), Probe::Indeterminate);

        let report = area.prune().unwrap();

        assert!(sock.exists(), "a live datagram socket must survive");
        assert_eq!(report.sockets, 0);
    }

    #[test]
    fn prune_removes_a_socket_only_after_an_explicit_refusal() {
        let (_guard, mut area) = area();
        zero_ages(&mut area);
        let dir = area.create(Path::new("/p/dead")).unwrap();
        let sock = dir.join("nvim.sock");
        drop(UnixListener::bind(&sock).unwrap());
        assert_eq!(probe(&sock), Probe::Refused);

        let report = area.prune().unwrap();

        assert!(!sock.exists());
        assert_eq!(report.sockets, 1);
    }

    #[test]
    fn probe_tells_apart_connected_refused_and_absent() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("s");
        assert_eq!(probe(&sock), Probe::Absent);
        let listener = UnixListener::bind(&sock).unwrap();
        assert_eq!(probe(&sock), Probe::Connected);
        drop(listener);
        assert_eq!(probe(&sock), Probe::Refused);
    }

    #[test]
    fn the_marker_keeps_the_exact_bytes_of_a_non_utf8_path() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let (_guard, area) = area();
        let project = PathBuf::from(OsString::from_vec(b"/p/caf\xe9".to_vec()));
        let dir = area.create(&project).unwrap();
        assert_eq!(fs::read(dir.join(MARKER)).unwrap(), b"/p/caf\xe9");
    }

    fn backdate(path: &Path) {
        let status = std::process::Command::new("touch")
            .args(["-t", "200001010000"])
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn the_marker_name_is_reserved() {
        assert!(validate_name(MARKER).is_err());
        assert!(validate_name("nvim.sock").is_ok());
        assert!(validate_name("paths").is_ok());
        let (_guard, area) = area();
        assert!(area.path_for(Path::new("/p/a"), Some(MARKER)).is_err());
        assert!(area.is_live_for(Path::new("/p/a"), MARKER).is_err());
        assert!(area.all(MARKER, false).is_err());
    }

    #[test]
    fn a_live_check_on_an_overlong_path_is_an_error_not_a_yes() {
        let (_guard, area) = area();
        area.create(Path::new("/p/a")).unwrap();
        let long = "n".repeat(120);
        assert!(area.path_for(Path::new("/p/a"), Some(&long)).is_err());
        assert!(area.is_live_for(Path::new("/p/a"), &long).is_err());
    }

    #[test]
    fn the_marker_outlives_a_young_file_so_a_later_pass_still_cleans_up() {
        let (_guard, mut area) = area();
        zero_ages(&mut area);
        area.empty_min_age = Duration::from_secs(3600);
        let gone = _guard.path().join("moved-away");
        fs::create_dir(&gone).unwrap();
        let dir = area.create(&gone).unwrap();
        fs::remove_dir(&gone).unwrap();
        let log = dir.join("server.log");
        fs::write(&log, "x").unwrap();
        backdate(&dir.join(MARKER));

        area.prune().unwrap();
        assert!(
            dir.join(MARKER).exists(),
            "marker kept while a young log remains"
        );
        assert!(log.exists());

        area.prune().unwrap();
        assert!(dir.join(MARKER).exists(), "still kept on the second pass");

        backdate(&log);
        area.prune().unwrap();
        assert!(!log.exists(), "the old log goes");
        assert!(!dir.exists(), "then the marker, then the directory");
    }
}
