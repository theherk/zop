//! The only contact with zoxide: `zoxide query -l` to read, `zoxide add -- PATH` to visit.
//! zop never opens zoxide's database file.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    Disabled,
    Missing,
    Failed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Disabled => "disabled",
            Status::Missing => "missing",
            Status::Failed => "failed",
        }
    }
}

pub const QUERY_TIMEOUT: Duration = Duration::from_millis(500);

/// An explicit path wins. Otherwise `zoxide` on `PATH`, then the usual install locations,
/// because GUI apps often start with a minimal `PATH`.
pub fn locate(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return path.is_file().then(|| path.to_path_buf());
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("zoxide");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut fallbacks = vec![
        PathBuf::from("/opt/homebrew/bin/zoxide"),
        PathBuf::from("/usr/local/bin/zoxide"),
    ];
    if let Some(home) = home {
        fallbacks.push(home.join(".local/share/mise/shims/zoxide"));
    }
    fallbacks.into_iter().find(|p| p.is_file())
}

/// `zoxide query -l`, most used first. Fails on a non-zero exit or after `timeout`.
///
/// zoxide runs in its own process group, so a wrapper script's children are killed
/// with it on a timeout instead of holding the output pipe open.
pub fn query(bin: &Path, timeout: Duration) -> Option<Vec<String>> {
    let mut child = Command::new(bin)
        .args(["query", "-l"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut text = Vec::new();
        let _ = tx.send(stdout.read_to_end(&mut text).map(|_| text));
    });

    // One deadline covers the child exiting and its output reaching EOF. A wrapper that
    // exits while a descendant still holds the pipe open must not outlive it.
    let deadline = Instant::now() + timeout;
    let outcome = wait_for_output(&mut child, &rx, deadline);
    if outcome.is_none() {
        // The child leads its own process group (`process_group(0)` above).
        let _ = crate::sys::kill_process_group(child.id());
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    let text = outcome?;
    Some(
        text.split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| String::from_utf8_lossy(l).into_owned())
            .collect(),
    )
}

/// The output of a child that exited successfully and closed stdout before `deadline`.
/// `None` for a failure, a read error, or a timeout.
fn wait_for_output(
    child: &mut std::process::Child,
    rx: &mpsc::Receiver<std::io::Result<Vec<u8>>>,
    deadline: Instant,
) -> Option<Vec<u8>> {
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            _ => return None,
        }
    };
    if !status.success() {
        return None;
    }
    let left = deadline.saturating_duration_since(Instant::now());
    rx.recv_timeout(left).ok()?.ok()
}

pub fn add(bin: &Path, path: &Path) -> Result<(), String> {
    let output = Command::new(bin)
        .arg("add")
        .arg("--")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("zoxide");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn query_returns_the_lines_of_a_clean_run() {
        let dir = tempfile::tempdir().unwrap();
        let bin = script(dir.path(), "printf '/a\\n/b\\n'");
        assert_eq!(
            query(&bin, Duration::from_secs(5)),
            Some(vec!["/a".into(), "/b".into()])
        );
    }

    #[test]
    fn a_wrapper_that_exits_while_a_child_holds_stdout_is_cut_off_and_cleaned_up() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let bin = script(
            dir.path(),
            &format!(
                "sleep 30 & echo $! > {}; printf '/a\\n'; exit 0",
                pidfile.display()
            ),
        );
        let started = Instant::now();
        assert_eq!(query(&bin, Duration::from_millis(800)), None);
        assert!(started.elapsed() < Duration::from_secs(5));

        let pid: i32 = fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let gone = (0..50).any(|_| {
            thread::sleep(Duration::from_millis(50));
            !Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        });
        assert!(gone, "the descendant must be killed with the process group");
    }

    #[test]
    fn a_failing_query_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let bin = script(dir.path(), "printf '/a\\n'; exit 3");
        assert_eq!(query(&bin, Duration::from_secs(5)), None);
    }
}
