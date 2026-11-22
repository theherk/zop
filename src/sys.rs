//! The crate's only `unsafe` code: three small libc calls behind safe signatures.
//! The library denies `unsafe_code` everywhere else, and the binary forbids it.
#![allow(unsafe_code)]

use std::io;
#[cfg(target_os = "macos")]
use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::PathBuf};

/// The effective user id.
pub fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Sends SIGKILL to the process group `pgid`. Refuses `0` and any id that doesn't fit a
/// positive `pid_t`, because `kill(-0)` and `kill(-1)` would reach far more than one group.
pub fn kill_process_group(pgid: u32) -> io::Result<()> {
    let pgid = i32::try_from(pgid)
        .ok()
        .filter(|id| *id > 1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid process group"))?;
    // SAFETY: kill takes plain integers and has no memory preconditions. The id was
    // checked above, so the negated value names exactly one process group.
    if unsafe { libc::kill(-pgid, libc::SIGKILL) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The per-user temporary directory that macOS creates for this user, from
/// `confstr(_CS_DARWIN_USER_TEMP_DIR)`. Unlike `$TMPDIR`, it is the same for every
/// process of the user, whatever environment it was started with.
#[cfg(target_os = "macos")]
pub fn darwin_user_temp_dir() -> Option<PathBuf> {
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: `buf` is valid for writes of `buf.len()` bytes, which is what confstr is told.
    let n = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            buf.as_mut_ptr().cast(),
            buf.len(),
        )
    };
    // `n` counts the terminating NUL. Zero is an error; more than `buf.len()` is truncation.
    if n == 0 || n > buf.len() {
        return None;
    }
    buf.truncate(n - 1);
    let path = PathBuf::from(OsStr::from_bytes(&buf));
    path.is_absolute().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_matches_the_owner_of_a_file_we_create() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            std::fs::metadata(dir.path()).unwrap().uid(),
            effective_uid()
        );
    }

    #[test]
    fn kill_refuses_ids_that_would_reach_too_much() {
        for bad in [0, 1, u32::MAX, i32::MAX as u32 + 1] {
            let err = kill_process_group(bad).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{bad}");
        }
    }

    #[test]
    fn kill_ends_a_process_group() {
        use std::os::unix::process::CommandExt;
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        kill_process_group(child.id()).unwrap();
        assert!(!child.wait().unwrap().success());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_darwin_temp_dir_is_absolute_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = darwin_user_temp_dir().expect("confstr works on macOS");
        assert!(dir.is_absolute());
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "{}", dir.display());
    }
}
