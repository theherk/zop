//! The binary, end to end, with an isolated index, config, tmp area and fake zoxide.

mod common;

use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use common::{canon, git, init_repo};
use serde_json::Value;

struct Zop {
    _guard: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    index: PathBuf,
    tmp: PathBuf,
    config: PathBuf,
    zoxide: Option<PathBuf>,
}

impl Zop {
    fn new() -> Zop {
        let guard = tempfile::tempdir().unwrap();
        let home = guard.path().join("home");
        let root = home.join("projects");
        let tmp = guard.path().join("tmp");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&tmp).unwrap();
        Zop {
            index: guard.path().join("state/zop/projects"),
            config: guard.path().join("config"),
            zoxide: None,
            home,
            root,
            tmp,
            _guard: guard,
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_zop"));
        cmd.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self._guard.path().join("xdg-config"))
            .env("XDG_STATE_HOME", self._guard.path().join("xdg-state"))
            .env("ZOP_INDEX", &self.index)
            .env("ZOP_TMP_ROOT", &self.tmp)
            .env(
                "ZOP_ZOXIDE",
                self.zoxide
                    .as_deref()
                    .unwrap_or(Path::new("/nonexistent/zoxide")),
            );
        cmd
    }

    fn with_config(&self) -> Command {
        let mut cmd = self.command();
        cmd.env("ZOP_CONFIG_PATH", &self.config);
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "zop {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn repo(&self, rel: &str) -> PathBuf {
        let path = self.root.join(rel);
        init_repo(&path);
        path
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut full = vec!["list", "--json", "--no-zoxide"];
        full.extend_from_slice(args);
        serde_json::from_str(&self.ok(&full)).unwrap()
    }

    fn fake_zoxide(&mut self, body: &str) {
        let path = self._guard.path().join("zoxide");
        let mut file = fs::File::create(&path).unwrap();
        write!(file, "#!/bin/sh\n{body}\n").unwrap();
        drop(file);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        self.zoxide = Some(path);
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn lines(s: &str) -> Vec<String> {
    s.lines().map(str::to_owned).collect()
}

fn names(v: &Value) -> Vec<String> {
    v["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_owned())
        .collect()
}

fn age_index(z: &Zop) {
    let status = Command::new("touch")
        .args(["-t", "200001010000"])
        .arg(&z.index)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn refresh_then_list_without_zoxide_is_sorted() {
    let z = Zop::new();
    z.repo("b");
    z.repo("a");
    z.repo("c");
    z.ok(&["refresh", "--root", z.root.to_str().unwrap()]);
    let out = z.ok(&["list", "--no-zoxide"]);
    assert_eq!(
        lines(&out),
        [
            z.root.join("a").display().to_string(),
            z.root.join("b").display().to_string(),
            z.root.join("c").display().to_string()
        ]
    );
}

#[test]
fn tilde_collapses_home() {
    let z = Zop::new();
    z.repo("a");
    z.ok(&["refresh", "--root", z.root.to_str().unwrap()]);
    assert_eq!(
        lines(&z.ok(&["list", "--no-zoxide", "--tilde"])),
        ["~/projects/a"]
    );
    assert_eq!(
        lines(&z.ok(&["list", "--no-zoxide", "--tilde", "--no-tilde"])),
        [z.root.join("a").display().to_string()]
    );
}

#[test]
fn zoxide_order_marks_visited_and_ranks() {
    let mut z = Zop::new();
    for name in ["a", "b", "c", "d"] {
        z.repo(name);
    }
    z.ok(&["refresh", "--root", z.root.to_str().unwrap()]);
    let root = z.root.display();
    z.fake_zoxide(&format!(
        "printf '%s\\n' {root}/c /somewhere/else {root}/b/sub/dir"
    ));

    let v: Value = serde_json::from_str(&z.ok(&["list", "--json"])).unwrap();
    assert_eq!(v["version"], 1);
    assert_eq!(v["zoxide"], "ok");
    assert_eq!(names(&v), ["c", "b", "a", "d"]);
    let flags: Vec<bool> = v["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["visited"].as_bool().unwrap())
        .collect();
    assert_eq!(flags, [true, true, false, false]);
    let ranks: Vec<u64> = v["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["rank"].as_u64().unwrap())
        .collect();
    assert_eq!(ranks, [0, 1, 2, 3]);
}

#[test]
fn zoxide_problems_degrade_to_the_sorted_list_and_say_so() {
    let mut z = Zop::new();
    z.repo("a");
    z.repo("b");
    z.ok(&["refresh", "--root", z.root.to_str().unwrap()]);
    let sorted = vec!["a", "b"];

    let missing: Value = serde_json::from_str(&z.ok(&["list", "--json"])).unwrap();
    assert_eq!(missing["zoxide"], "missing");
    assert_eq!(names(&missing), sorted);

    z.fake_zoxide("exit 1");
    let failed: Value = serde_json::from_str(&z.ok(&["list", "--json"])).unwrap();
    assert_eq!(failed["zoxide"], "failed");
    assert_eq!(names(&failed), sorted);

    z.fake_zoxide("sleep 5");
    let started = Instant::now();
    let slow: Value = serde_json::from_str(&z.ok(&["list", "--json"])).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(slow["zoxide"], "failed");

    z.fake_zoxide(&format!("echo {}", z.root.join("b").display()));
    let disabled: Value = serde_json::from_str(&z.ok(&["list", "--json", "--no-zoxide"])).unwrap();
    assert_eq!(disabled["zoxide"], "disabled");
    assert_eq!(names(&disabled), sorted);
    let enabled: Value =
        serde_json::from_str(&z.ok(&["list", "--json", "--no-zoxide", "--zoxide"])).unwrap();
    assert_eq!(enabled["zoxide"], "ok");
    assert_eq!(names(&enabled), ["b", "a"]);
}

#[test]
fn list_with_no_index_and_no_inputs_is_an_error_that_writes_nothing() {
    let z = Zop::new();
    let out = z.run(&["list"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("zop refresh"), "{}", stderr(&out));
    assert!(!z.index.exists());
}

#[test]
fn a_bare_refresh_repeats_the_stored_inputs() {
    let z = Zop::new();
    z.repo("a");
    z.ok(&["refresh", "--root", z.root.to_str().unwrap()]);
    z.repo("b");
    assert_eq!(names(&z.json(&[])), ["a"]);
    z.ok(&["refresh"]);
    assert_eq!(names(&z.json(&[])), ["a", "b"]);
}

#[test]
fn a_refresh_with_nothing_to_go_on_is_a_usage_error() {
    let z = Zop::new();
    let out = z.run(&["refresh"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!z.index.exists());
}

#[test]
fn a_missing_root_is_fatal_and_keeps_the_old_index() {
    let z = Zop::new();
    z.repo("a");
    z.ok(&["refresh", "--root", z.root.to_str().unwrap()]);
    let before = fs::read(&z.index).unwrap();

    let out = z.run(&[
        "refresh",
        "--root",
        z.root.to_str().unwrap(),
        "--root",
        "/no/such/root",
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("/no/such/root"), "{}", stderr(&out));
    assert_eq!(fs::read(&z.index).unwrap(), before);
    assert_eq!(names(&z.json(&[])), ["a"]);
}

#[test]
fn an_unmounted_root_cannot_silently_shrink_the_index() {
    let z = Zop::new();
    z.repo("a");
    let other = z._guard.path().join("drive");
    init_repo(&other.join("b"));
    z.ok(&[
        "refresh",
        "--root",
        z.root.to_str().unwrap(),
        "--root",
        other.to_str().unwrap(),
    ]);
    let before = fs::read(&z.index).unwrap();

    fs::remove_dir_all(&other).unwrap();
    let out = z.run(&["refresh"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("drive"), "{}", stderr(&out));
    assert_eq!(
        fs::read(&z.index).unwrap(),
        before,
        "the old index, including b, is kept"
    );
}

#[test]
fn list_drops_projects_that_have_vanished_since_the_refresh() {
    let z = Zop::new();
    z.repo("a");
    let b = z.repo("b");
    z.ok(&["refresh", "--root", z.root.to_str().unwrap()]);
    fs::remove_dir_all(b).unwrap();
    assert_eq!(names(&z.json(&[])), ["a"]);
}

#[test]
fn implicit_refresh_uses_the_configs_refresh_section() {
    let z = Zop::new();
    z.repo("a");
    fs::write(
        &z.config,
        format!("[refresh]\n--root={}\n", z.root.display()),
    )
    .unwrap();
    let out = z
        .with_config()
        .args(["list", "--no-zoxide"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&stdout(&out)),
        [z.root.join("a").display().to_string()]
    );
    assert!(z.index.exists());
}

#[test]
fn implicit_refresh_with_a_stale_index_uses_the_stored_inputs() {
    let z = Zop::new();
    z.repo("a");
    z.ok(&["refresh", "--root", z.root.to_str().unwrap()]);
    z.repo("b");
    age_index(&z);

    assert_eq!(names(&z.json(&[])), ["a"], "no staleness flag, no refresh");
    assert_eq!(names(&z.json(&["--refresh-if-older", "1h"])), ["a", "b"]);
}

#[test]
fn no_refresh_if_older_cancels_the_configs_setting() {
    let z = Zop::new();
    z.repo("a");
    z.ok(&["refresh", "--root", z.root.to_str().unwrap()]);
    z.repo("b");
    fs::write(&z.config, "[list]\n--refresh-if-older=1h\n").unwrap();

    age_index(&z);
    let cancelled = z
        .with_config()
        .args(["list", "--json", "--no-zoxide", "--no-refresh-if-older"])
        .output()
        .unwrap();
    assert!(cancelled.status.success());
    let v: Value = serde_json::from_str(&stdout(&cancelled)).unwrap();
    assert_eq!(names(&v), ["a"]);

    let applied = z
        .with_config()
        .args(["list", "--json", "--no-zoxide"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_str(&stdout(&applied)).unwrap();
    assert_eq!(names(&v), ["a", "b"]);
}

#[test]
fn config_list_section_applies_and_the_command_line_overrides_it() {
    let z = Zop::new();
    z.repo("a");
    z.ok(&["refresh", "--root", z.root.to_str().unwrap()]);
    fs::write(&z.config, "[list]\n--json\n--no-zoxide\n").unwrap();

    let json = z.with_config().arg("list").output().unwrap();
    assert!(stdout(&json).starts_with('{'), "{}", stdout(&json));
    let bare = z.with_config().output().unwrap();
    assert!(
        stdout(&bare).starts_with('{'),
        "a bare zop is list, with its section"
    );
    let plain = z
        .with_config()
        .args(["list", "--no-json"])
        .output()
        .unwrap();
    assert!(stdout(&plain).starts_with('/'), "{}", stdout(&plain));
    let ignored = z
        .with_config()
        .args(["--no-config", "list", "--no-zoxide"])
        .output()
        .unwrap();
    assert!(stdout(&ignored).starts_with('/'));
}

#[test]
fn a_missing_explicit_config_is_an_error_and_a_bad_one_names_the_line() {
    let z = Zop::new();
    let out = z.run(&["--config", "/no/such/config", "list"]);
    assert_eq!(out.status.code(), Some(2));
    fs::write(&z.config, "[list]\n--json\n[global]\n").unwrap();
    let out = z.with_config().arg("list").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("line 3"), "{}", stderr(&out));
}

#[test]
fn ids_disambiguate_collisions_and_label_worktrees() {
    let z = Zop::new();
    z.repo("a/docs");
    z.repo("b/docs");
    let zop = z.repo("zop");
    git(&zop, &["worktree", "add", "-q", "../zop-fix"]);
    z.ok(&["refresh", "--root", z.root.to_str().unwrap()]);

    let v = z.json(&[]);
    let by_name = |name: &str| {
        v["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["path"].as_str().unwrap().ends_with(name))
            .unwrap()
            .clone()
    };
    assert_eq!(by_name("a/docs")["id"], "a~docs");
    assert_eq!(by_name("b/docs")["id"], "b~docs");
    assert_eq!(by_name("/zop")["id"], "zop");
    let wt = by_name("zop-fix");
    assert_eq!(wt["id"], "zop@zop-fix");
    assert_eq!(wt["kind"], "worktree");
    assert_eq!(wt["parent"], zop.display().to_string());
}

#[test]
fn json_tmp_field_matches_the_tmp_command() {
    let z = Zop::new();
    let repo = z.repo("a");
    z.ok(&["refresh", "--root", z.root.to_str().unwrap()]);
    let v = z.json(&[]);
    let from_json = v["projects"][0]["tmp"].as_str().unwrap().to_owned();
    let from_cmd = z.ok(&["tmp", repo.to_str().unwrap()]);
    assert_eq!(from_json, from_cmd.trim());
}

#[test]
fn no_worktrees_flag_and_the_stored_setting() {
    let z = Zop::new();
    let main = z.repo("main");
    git(&main, &["worktree", "add", "-q", "../wt"]);
    z.ok(&[
        "refresh",
        "--root",
        z.root.to_str().unwrap(),
        "--no-worktrees",
    ]);
    assert_eq!(names(&z.json(&[])), ["main"]);
    z.ok(&["refresh"]);
    assert_eq!(
        names(&z.json(&[])),
        ["main"],
        "a bare refresh keeps the stored setting"
    );
    z.ok(&["refresh", "--worktrees"]);
    assert_eq!(names(&z.json(&[])), ["main", "wt"]);
}

#[test]
fn add_file_works_end_to_end() {
    let z = Zop::new();
    z.repo("a");
    let vault = z.home.join("vault");
    fs::create_dir_all(&vault).unwrap();
    let extras = z.home.join("extras");
    fs::write(&extras, "# vaults\n$HOME/vault\n").unwrap();
    let out = z.run(&[
        "refresh",
        "--root",
        z.root.to_str().unwrap(),
        "--add",
        extras.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let v = z.json(&[]);
    let kinds: Vec<(&str, &str)> = v["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["name"].as_str().unwrap(), p["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(kinds, [("a", "repo"), ("vault", "extra")]);
}

#[test]
fn tmp_path_create_and_validation() {
    let z = Zop::new();
    let repo = z.repo("a");
    let real = canon(&repo);

    let printed = z.ok(&["tmp", repo.to_str().unwrap(), "nvim.sock"]);
    let uid = unsafe { libc::geteuid() };
    let expected = z
        .tmp
        .join(format!("zop-{uid}"))
        .join(zop::tmp::hash16(&real))
        .join("nvim.sock");
    assert_eq!(printed.trim(), expected.display().to_string());
    assert!(
        !expected.parent().unwrap().exists(),
        "printing creates nothing"
    );

    z.ok(&["tmp", repo.to_str().unwrap(), "nvim.sock", "--create"]);
    let dir = expected.parent().unwrap();
    assert_eq!(
        fs::read_to_string(dir.join("path")).unwrap(),
        real.display().to_string()
    );
    assert_eq!(
        fs::metadata(dir).unwrap().permissions().mode() & 0o777,
        0o700
    );

    for bad in ["../x", "a/b", ".."] {
        assert_eq!(
            z.run(&["tmp", repo.to_str().unwrap(), bad]).status.code(),
            Some(2),
            "{bad}"
        );
    }
    let long = "x".repeat(120);
    assert_eq!(
        z.run(&["tmp", repo.to_str().unwrap(), &long]).status.code(),
        Some(2)
    );
    assert_eq!(
        z.run(&["tmp", repo.to_str().unwrap(), "n", "--create", "--live"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        z.run(&["tmp", "/no/such/project", "n"]).status.code(),
        Some(2)
    );
}

#[test]
fn tmp_live_exit_codes_follow_the_socket() {
    let z = Zop::new();
    let repo = z.repo("a");
    let sock = PathBuf::from(
        z.ok(&["tmp", repo.to_str().unwrap(), "nvim.sock", "--create"])
            .trim(),
    );

    let live = |z: &Zop| {
        z.run(&["tmp", repo.to_str().unwrap(), "nvim.sock", "--live"])
            .status
            .code()
    };
    assert_eq!(live(&z), Some(1), "no socket yet");

    let listener = UnixListener::bind(&sock).unwrap();
    assert_eq!(live(&z), Some(0));
    drop(listener);
    assert_eq!(live(&z), Some(1), "dead socket");
    assert!(sock.exists(), "--live removes nothing");

    assert_eq!(
        z.run(&["tmp", repo.to_str().unwrap(), "--live"])
            .status
            .code(),
        Some(2),
        "--live needs a name"
    );
}

#[test]
fn tmp_all_and_list_live_find_live_sockets() {
    let z = Zop::new();
    let a = z.repo("a");
    let b = z.repo("b");
    z.ok(&["refresh", "--root", z.root.to_str().unwrap()]);

    let sock_a = PathBuf::from(
        z.ok(&["tmp", a.to_str().unwrap(), "nvim.sock", "--create"])
            .trim(),
    );
    let sock_b = PathBuf::from(
        z.ok(&["tmp", b.to_str().unwrap(), "nvim.sock", "--create"])
            .trim(),
    );
    let _live = UnixListener::bind(&sock_a).unwrap();
    drop(UnixListener::bind(&sock_b).unwrap());

    assert_eq!(
        lines(&z.ok(&["tmp", "--all", "nvim.sock", "--live"])),
        [sock_a.display().to_string()]
    );
    assert_eq!(lines(&z.ok(&["tmp", "--all", "nvim.sock"])).len(), 2);
    assert_eq!(names(&z.json(&["--live", "nvim.sock"])), ["a"]);
    assert_eq!(names(&z.json(&[])), ["a", "b"]);
    assert_eq!(z.run(&["list", "--live", "../x"]).status.code(), Some(2));
}

#[test]
fn explicit_refresh_prunes_orphans_and_no_prune_and_implicit_do_not() {
    let z = Zop::new();
    z.repo("a");
    let gone = z._guard.path().join("gone-project");
    fs::create_dir(&gone).unwrap();
    let dir = PathBuf::from(z.ok(&["tmp", gone.to_str().unwrap(), "--create"]).trim());
    assert!(dir.exists());
    fs::remove_dir(&gone).unwrap();
    let root = z.root.to_str().unwrap();

    z.ok(&["refresh", "--root", root, "--no-prune"]);
    assert!(dir.exists(), "--no-prune keeps it");
    age_index(&z);
    z.ok(&["list", "--no-zoxide", "--refresh-if-older", "1h"]);
    assert!(dir.exists(), "an implicit refresh does not prune");
    z.ok(&["refresh"]);
    assert!(dir.exists(), "a young orphan is kept");
    let touched = std::process::Command::new("touch")
        .args(["-t", "200001010000"])
        .arg(dir.join("path"))
        .status()
        .unwrap();
    assert!(touched.success());
    z.ok(&["refresh"]);
    assert!(!dir.exists(), "an explicit refresh prunes the old orphan");
}

#[test]
fn visit_runs_zoxide_add_and_never_fails() {
    let mut z = Zop::new();
    let log = z._guard.path().join("log");
    z.fake_zoxide(&format!("echo \"$@\" >> {}", log.display()));
    let project = z.repo("a");
    let out = z.run(&["visit", project.to_str().unwrap()]);
    assert!(out.status.success());
    assert_eq!(
        fs::read_to_string(&log).unwrap().trim(),
        format!("add -- {}", project.display())
    );

    z.fake_zoxide("echo boom >&2; exit 3");
    let out = z.run(&["visit", project.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    assert!(stderr(&out).contains("boom"), "{}", stderr(&out));

    z.zoxide = None;
    let out = z.run(&["visit", project.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stderr(&out).contains("zoxide not found"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn version_help_and_usage_errors() {
    let z = Zop::new();
    let out = z.run(&["--version"]);
    assert!(stdout(&out).starts_with("zop "), "{}", stdout(&out));
    assert!(z.run(&["--help"]).status.success());
    assert!(z.run(&["list", "--help"]).status.success());
    assert_eq!(z.run(&["bogus"]).status.code(), Some(2));
    assert_eq!(
        z.run(&["list", "--refresh-if-older", "1w"]).status.code(),
        Some(2)
    );
}

#[test]
fn a_reader_that_closes_early_does_not_make_zop_panic() {
    let z = Zop::new();
    for name in ["a", "b", "c", "d", "e"] {
        z.repo(name);
    }
    z.ok(&["refresh", "--root", z.root.to_str().unwrap()]);

    let mut child = z
        .command()
        .args(["list", "--no-zoxide"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    assert!(!stderr(&out).contains("panicked"), "{}", stderr(&out));
    assert!(!stderr(&out).contains("Broken pipe"), "{}", stderr(&out));
}

#[test]
fn concurrent_implicit_refreshes_all_succeed() {
    let z = Zop::new();
    for name in ["a", "b", "c"] {
        z.repo(name);
    }
    fs::write(
        &z.config,
        format!("[refresh]\n--root={}\n", z.root.display()),
    )
    .unwrap();

    let children: Vec<_> = (0..6)
        .map(|_| {
            z.with_config()
                .args(["list", "--no-zoxide"])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
        assert_eq!(lines(&stdout(&out)).len(), 3);
    }
}

#[test]
fn completions_print_a_script() {
    let z = Zop::new();
    let out = z.ok(&["completions", "bash"]);
    assert!(out.contains("_zop"), "{out}");
}

#[test]
fn bad_durations_error_instead_of_panicking() {
    let z = Zop::new();
    for bad in ["1µ", "99999999999999999d", "", "d"] {
        let out = z.run(&["list", "--no-zoxide", "--refresh-if-older", bad]);
        assert_eq!(out.status.code(), Some(2), "{bad}");
    }
}

#[test]
fn tmp_override_and_xdg_runtime_dir() {
    let z = Zop::new();
    let project = z.repo("a");
    let p = project.to_str().unwrap();

    let out = z.ok(&["tmp", p, "nvim.sock"]);
    assert!(
        out.trim().starts_with(&format!("{}/zop-", z.tmp.display())),
        "{out}"
    );

    let run = z._guard.path().join("run");
    fs::create_dir(&run).unwrap();
    fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut cmd = z.command();
    cmd.env_remove("ZOP_TMP_ROOT")
        .env("XDG_RUNTIME_DIR", &run)
        .args(["tmp", p, "nvim.sock"]);
    let out = String::from_utf8(cmd.output().unwrap().stdout).unwrap();
    if cfg!(target_os = "macos") {
        assert!(!out.starts_with(run.to_str().unwrap()), "{out}");
    } else {
        assert!(out.starts_with(&format!("{}/zop/", run.display())), "{out}");
    }
}

fn run_with_stdin(z: &Zop, args: &[&str], input: &[u8]) -> Output {
    use std::process::Stdio;
    let mut child = z
        .command()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn an_index_with_invalid_utf8_is_repaired_by_an_explicit_refresh() {
    let z = Zop::new();
    z.repo("a");
    fs::create_dir_all(z.index.parent().unwrap()).unwrap();
    fs::write(&z.index, b"zop-index 1\n\xff\xfe\n").unwrap();

    z.ok(&["refresh", "--root", z.root.to_str().unwrap()]);

    assert_eq!(names(&z.json(&[])), ["a"]);
}

#[test]
fn an_unreadable_index_is_reported_with_its_path_and_cause() {
    let z = Zop::new();
    z.repo("a");
    z.ok(&["refresh", "--root", z.root.to_str().unwrap()]);
    fs::set_permissions(&z.index, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&z.index).is_ok() {
        return; // running as root
    }
    let out = z.run(&["list", "--no-zoxide"]);
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.contains("Permission denied"), "{err}");
    assert!(err.contains(z.index.to_str().unwrap()), "{err}");
}

#[test]
fn visit_from_stdin_records_every_path_in_order() {
    let mut z = Zop::new();
    let log = z._guard.path().join("log");
    z.fake_zoxide(&format!("echo \"$@\" >> {}", log.display()));
    let out = run_with_stdin(&z, &["visit", "-"], b"/one\n\n  /two  \n/three");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "add -- /one\nadd -- /two\nadd -- /three\n"
    );
}

#[test]
fn visit_from_stdin_with_a_bad_line_records_nothing_and_fails() {
    let mut z = Zop::new();
    let log = z._guard.path().join("log");
    z.fake_zoxide(&format!("echo \"$@\" >> {}", log.display()));
    let out = run_with_stdin(&z, &["visit", "-"], b"/one\n\xff\xfe\n/three\n");
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.contains("stdin line 2"), "{err}");
    assert!(!log.exists(), "nothing may be recorded");
}

#[test]
fn prune_leaves_lookalike_directories_alone_and_says_so() {
    let z = Zop::new();
    let project = z.repo("a");
    z.ok(&["refresh", "--root", z.root.to_str().unwrap()]);
    let dir = PathBuf::from(z.ok(&["tmp", project.to_str().unwrap(), "--create"]).trim());
    let base = dir.parent().unwrap().to_path_buf();
    let lookalike = base.join("backup.tmp.old");
    fs::create_dir(&lookalike).unwrap();
    fs::set_permissions(&lookalike, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(lookalike.join("precious"), "x").unwrap();
    let touched = Command::new("touch")
        .args(["-t", "200001010000"])
        .arg(&lookalike)
        .status()
        .unwrap();
    assert!(touched.success());

    z.ok(&["refresh"]);

    assert!(lookalike.join("precious").exists());
    assert!(dir.exists());
}

#[test]
fn a_root_ending_in_a_carriage_return_survives_repeated_refreshes() {
    let z = Zop::new();
    let root = z._guard.path().join("projects\r");
    fs::create_dir(&root).unwrap();
    common::init_repo(&root.join("a"));

    z.ok(&["refresh", "--root", root.to_str().unwrap()]);
    z.ok(&["refresh"]);
    z.ok(&["refresh"]);

    assert_eq!(names(&z.json(&[])), ["a"]);
}

#[test]
fn a_reader_that_closes_the_pipe_early_is_not_an_error() {
    use std::process::Stdio;
    let z = Zop::new();
    for _ in 0..20 {
        let mut child = z
            .command()
            .args(["completions", "zsh"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        drop(child.stdout.take());
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        assert!(stderr(&out).is_empty(), "{}", stderr(&out));
    }
}

#[test]
fn the_marker_name_is_rejected_and_the_marker_is_left_alone() {
    let z = Zop::new();
    let project = z.repo("a");
    let p = project.to_str().unwrap();
    let dir = PathBuf::from(z.ok(&["tmp", p, "--create"]).trim());

    for args in [
        vec!["tmp", p, "path", "--create"],
        vec!["tmp", p, "path"],
        vec!["tmp", p, "path", "--live"],
        vec!["tmp", "--all", "path"],
    ] {
        let out = z.run(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(stderr(&out).contains("reserved"), "{}", stderr(&out));
    }
    let marker = fs::read(dir.join("path")).unwrap();
    assert_eq!(marker, canon(&project).as_os_str().as_bytes());
}

#[test]
fn live_on_an_overlong_path_never_exits_zero() {
    let z = Zop::new();
    let project = z.repo("a");
    let p = project.to_str().unwrap();
    z.ok(&["tmp", p, "--create"]);
    let long = "n".repeat(120);
    assert_eq!(z.run(&["tmp", p, &long]).status.code(), Some(2));
    assert_eq!(z.run(&["tmp", p, &long, "--live"]).status.code(), Some(2));
}

#[test]
fn a_path_argument_that_is_not_utf8_is_not_a_panic() {
    use std::ffi::OsStr;
    let z = Zop::new();
    let odd = OsStr::from_bytes(b"/nonexistent/caf\xe9");
    for sub in ["tmp", "visit"] {
        let out = z.command().arg(sub).arg(odd).output().unwrap();
        let code = out.status.code();
        assert_ne!(code, Some(101), "{sub}: {}", stderr(&out));
        assert!(code.is_some(), "{sub} must exit, not die on a signal");
        assert!(!stderr(&out).contains("panicked"), "{}", stderr(&out));
    }
    let out = z.command().arg("tmp").arg(odd).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("caf"), "{}", stderr(&out));
}

#[test]
fn a_non_utf8_config_path_is_used_not_dropped() {
    use std::ffi::OsStr;
    let z = Zop::new();
    let odd = OsStr::from_bytes(b"/nonexistent/cfg\xe9");
    let out = z
        .command()
        .arg("--config")
        .arg(odd)
        .arg("list")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "an explicit missing config is an error"
    );
    assert!(stderr(&out).contains("cfg"), "{}", stderr(&out));
}
