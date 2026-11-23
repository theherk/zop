//! The command line: argv rewrite for the config file, clap definitions, and the commands.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{ArgAction, Args, Parser, Subcommand};
use serde::Serialize;

use crate::config::{self, Sections};
use crate::discover::{self, Options, Project};
use crate::error::{Error, Result};
use crate::expand;
use crate::ids;
use crate::index::{self, Index, Inputs, Loaded, Lock};
use crate::rank;
use crate::tmp::{self, Area};
use crate::zoxide::{self, Status};

const DEFAULT_MAX_DEPTH: usize = 12;
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Parser, Debug)]
#[command(
    name = "zop",
    version,
    about = "Every project, ranked by use",
    args_override_self = true
)]
pub struct Cli {
    /// Config file (also ZOP_CONFIG_PATH)
    #[arg(long, value_name = "FILE", value_parser = expand::PathParser)]
    config: Option<PathBuf>,

    /// Ignore the config file
    #[arg(long)]
    no_config: bool,

    /// Index file
    #[arg(long, value_name = "FILE", env = "ZOP_INDEX", value_parser = expand::PathParser)]
    index: Option<PathBuf>,

    /// The zoxide binary
    #[arg(long, value_name = "FILE", env = "ZOP_ZOXIDE", value_parser = expand::PathParser)]
    zoxide_path: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print every project, ranked
    List(ListArgs),
    /// Rescan roots, rewrite the index, prune stale tmp state
    Refresh(RefreshArgs),
    /// Print a per-project tmp path
    Tmp(TmpArgs),
    /// Tell zoxide a project was opened
    Visit(VisitArgs),
    /// Print a shell completion script
    Completions(CompletionsArgs),
}

#[derive(Args, Debug)]
struct CompletionsArgs {
    shell: CompletionShell,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum CompletionShell {
    Bash,
    Elvish,
    Fish,
    Nushell,
    Powershell,
    Zsh,
}

#[derive(Args, Debug)]
#[command(args_override_self = true)]
struct ListArgs {
    /// Collapse $HOME to ~ in plain output
    #[arg(long, overrides_with = "no_tilde")]
    tilde: bool,
    /// Print absolute paths (default)
    #[arg(long = "no-tilde", overrides_with = "tilde")]
    no_tilde: bool,

    /// Print JSON instead of paths
    #[arg(long, overrides_with = "no_json")]
    json: bool,
    /// Print one path per line (default)
    #[arg(long = "no-json", overrides_with = "json")]
    no_json: bool,

    /// Rank with zoxide (default)
    #[arg(long, overrides_with = "no_zoxide")]
    zoxide: bool,
    /// Don't rank with zoxide
    #[arg(long = "no-zoxide", overrides_with = "zoxide")]
    no_zoxide: bool,

    /// Keep only projects whose tmp socket NAME accepts connections
    #[arg(long, value_name = "NAME")]
    live: Option<String>,

    /// Refresh first if the index is older than DUR (30m, 12h, 1d)
    #[arg(long, value_name = "DUR", overrides_with = "no_refresh_if_older")]
    refresh_if_older: Option<String>,
    /// Never refresh on age
    #[arg(long = "no-refresh-if-older", overrides_with = "refresh_if_older")]
    no_refresh_if_older: bool,
}

#[derive(Args, Debug, Default)]
struct RefreshArgs {
    /// Walk DIR for projects (repeatable)
    #[arg(long, value_name = "DIR", action = ArgAction::Append, value_parser = expand::PathParser)]
    root: Vec<PathBuf>,

    /// Add PATH; a file is read as one path per line (repeatable)
    #[arg(long, value_name = "PATH", action = ArgAction::Append, value_parser = expand::PathParser)]
    add: Vec<PathBuf>,

    /// Include linked worktrees and jj workspaces (default)
    #[arg(long, overrides_with = "no_worktrees")]
    worktrees: bool,
    /// Leave out worktrees and jj workspaces
    #[arg(long = "no-worktrees", overrides_with = "worktrees")]
    no_worktrees: bool,

    /// Walk depth limit [default: 12]
    #[arg(long, value_name = "N", overrides_with = "max_depth")]
    max_depth: Option<usize>,

    /// Prune stale tmp state afterwards (default)
    #[arg(long, overrides_with = "no_prune")]
    prune: bool,
    /// Don't prune stale tmp state
    #[arg(long = "no-prune", overrides_with = "prune")]
    no_prune: bool,
}

#[derive(Args, Debug)]
struct TmpArgs {
    /// The project root
    #[arg(value_name = "PATH", required_unless_present = "all", value_parser = expand::PathParser)]
    path: Option<PathBuf>,

    /// A file name inside the project's tmp directory
    #[arg(value_name = "NAME")]
    name: Option<String>,

    /// Create the directory (mode 0700) and its path marker
    #[arg(long, conflicts_with_all = ["live", "all"])]
    create: bool,

    /// Exit 0 only if NAME is a socket that accepts a connection
    #[arg(long)]
    live: bool,

    /// Print every existing tmp/*/NAME
    #[arg(long, value_name = "NAME", conflicts_with_all = ["path", "name", "create"])]
    all: Option<String>,
}

#[derive(Args, Debug)]
struct VisitArgs {
    /// A project path, or `-` to read one path per line from stdin
    #[arg(value_name = "PATH", value_parser = expand::PathParser)]
    path: PathBuf,
}

// ---------------------------------------------------------------------------------------
// argv rewrite

const SUBCOMMANDS: [&str; 5] = ["list", "refresh", "tmp", "visit", "completions"];
const VALUE_GLOBALS: [&str; 3] = ["--config", "--index", "--zoxide-path"];

#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// Leave argv alone (help, version, the `help` subcommand).
    Pass,
    /// The subcommand is at `at`; its section's arguments go right after it.
    Sub { name: String, at: usize },
    /// No subcommand was given; insert `list` at `at`.
    InsertList { at: usize },
}

#[derive(Debug, PartialEq, Eq)]
pub struct Scan {
    pub config: Option<OsString>,
    pub no_config: bool,
    pub plan: Plan,
}

/// Reads the global flags (with their known arity) up to the subcommand, and decides
/// where the subcommand is or should be. Arguments stay `OsString`: a path that isn't
/// valid UTF-8 is an ordinary argument.
pub fn scan(args: &[OsString]) -> Scan {
    let mut config = None;
    let mut no_config = false;
    let mut i = 1;
    let plan = loop {
        let Some(arg) = args.get(i) else {
            break Plan::InsertList { at: args.len() };
        };
        let tok = arg.as_bytes();
        if tok == b"--" {
            break Plan::InsertList { at: i };
        }
        if VALUE_GLOBALS.iter().any(|g| g.as_bytes() == tok) {
            if i + 1 >= args.len() {
                break Plan::InsertList { at: args.len() };
            }
            if tok == b"--config" {
                config = args.get(i + 1).cloned();
            }
            i += 2;
            continue;
        }
        if let Some(value) = tok.strip_prefix(b"--config=") {
            config = Some(OsStr::from_bytes(value).to_owned());
            i += 1;
            continue;
        }
        if tok.starts_with(b"--index=") || tok.starts_with(b"--zoxide-path=") {
            i += 1;
            continue;
        }
        if tok == b"--no-config" {
            no_config = true;
            i += 1;
            continue;
        }
        if [&b"-h"[..], b"--help", b"-V", b"--version", b"help"].contains(&tok) {
            break Plan::Pass;
        }
        if let Some(name) = SUBCOMMANDS.iter().find(|n| n.as_bytes() == tok) {
            break Plan::Sub {
                name: (*name).to_string(),
                at: i,
            };
        }
        break Plan::InsertList { at: i };
    };
    Scan {
        config,
        no_config,
        plan,
    }
}

/// Builds the final argv: the subcommand's config section goes right after the
/// subcommand, so anything typed on the command line is later and wins.
pub fn apply(args: &[OsString], plan: &Plan, sections: &Sections) -> Vec<OsString> {
    let os = |v: &[String]| -> Vec<OsString> { v.iter().map(OsString::from).collect() };
    match plan {
        Plan::Pass => args.to_vec(),
        Plan::Sub { name, at } => {
            let section = match name.as_str() {
                "list" => os(&sections.list),
                "refresh" => os(&sections.refresh),
                _ => Vec::new(),
            };
            let mut out = args[..=*at].to_vec();
            out.extend(section);
            out.extend_from_slice(&args[at + 1..]);
            out
        }
        Plan::InsertList { at } => {
            let mut out = args[..*at].to_vec();
            out.push(OsString::from("list"));
            out.extend(os(&sections.list));
            out.extend_from_slice(&args[*at..]);
            out
        }
    }
}

// ---------------------------------------------------------------------------------------
// running

struct Ctx {
    index: PathBuf,
    zoxide_path: Option<PathBuf>,
    sections: Sections,
}

/// Writes to stdout. Rust ignores SIGPIPE, so a reader that closes early
/// (`zop list | head`, or `fzf` after Esc) shows up as a `BrokenPipe` error here
/// instead of ending the process. That is the reader's choice, not a failure.
fn emit(bytes: &[u8]) -> Result<()> {
    let mut out = std::io::stdout().lock();
    out.write_all(bytes)
        .and_then(|()| out.flush())
        .map_err(|e| Error::io_context("stdout", e))
}

fn is_broken_pipe(e: &Error) -> bool {
    e.io_kind() == Some(std::io::ErrorKind::BrokenPipe)
}

pub fn run<I, S>(args: I) -> ExitCode
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    match run_inner(&args) {
        Ok(code) => ExitCode::from(code),
        Err(e) if is_broken_pipe(&e) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("zop: {e}");
            ExitCode::from(2)
        }
    }
}

fn run_inner(args: &[OsString]) -> Result<u8> {
    let scan = scan(args);
    let sections = if scan.no_config || scan.plan == Plan::Pass {
        Sections::default()
    } else {
        let chosen = scan
            .config
            .clone()
            .or_else(|| std::env::var_os("ZOP_CONFIG_PATH"));
        let (path, explicit) = match chosen {
            Some(path) => (PathBuf::from(expand::expand(&path)?), true),
            None => (config::default_path(), false),
        };
        config::load(&path, explicit)?
    };

    let argv = apply(args, &scan.plan, &sections);
    let cli = Cli::parse_from(argv);

    let ctx = Ctx {
        index: cli.index.clone().unwrap_or_else(default_index_path),
        zoxide_path: cli.zoxide_path.clone(),
        sections,
    };
    match &cli.command {
        Command::List(args) => cmd_list(&ctx, args),
        Command::Refresh(args) => cmd_refresh(&ctx, args),
        Command::Tmp(args) => cmd_tmp(args),
        Command::Visit(args) => cmd_visit(&ctx, args),
        Command::Completions(args) => {
            let mut cmd = <Cli as clap::CommandFactory>::command();
            let mut script = Vec::new();
            match args.shell {
                CompletionShell::Bash => clap_complete::generate(
                    clap_complete::Shell::Bash,
                    &mut cmd,
                    "zop",
                    &mut script,
                ),
                CompletionShell::Elvish => clap_complete::generate(
                    clap_complete::Shell::Elvish,
                    &mut cmd,
                    "zop",
                    &mut script,
                ),
                CompletionShell::Fish => clap_complete::generate(
                    clap_complete::Shell::Fish,
                    &mut cmd,
                    "zop",
                    &mut script,
                ),
                CompletionShell::Nushell => clap_complete::generate(
                    clap_complete_nushell::Nushell,
                    &mut cmd,
                    "zop",
                    &mut script,
                ),
                CompletionShell::Powershell => clap_complete::generate(
                    clap_complete::Shell::PowerShell,
                    &mut cmd,
                    "zop",
                    &mut script,
                ),
                CompletionShell::Zsh => {
                    clap_complete::generate(clap_complete::Shell::Zsh, &mut cmd, "zop", &mut script)
                }
            }
            emit(&script)?;
            Ok(0)
        }
    }
}

fn default_index_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".local/state"));
    base.join("zop").join("projects")
}

// ---------------------------------------------------------------------------------------
// list

#[derive(Serialize)]
struct Envelope<'a> {
    version: u32,
    zoxide: &'a str,
    projects: Vec<JsonProject>,
}

#[derive(Serialize)]
struct JsonProject {
    path: String,
    realpath: String,
    name: String,
    id: String,
    kind: &'static str,
    parent: Option<String>,
    visited: bool,
    rank: usize,
    tmp: String,
}

fn cmd_list(ctx: &Ctx, args: &ListArgs) -> Result<u8> {
    let tilde = args.tilde && !args.no_tilde;
    let json = args.json && !args.no_json;
    let max_age = match (&args.refresh_if_older, args.no_refresh_if_older) {
        (Some(dur), false) => Some(parse_duration(dur)?),
        _ => None,
    };
    if let Some(name) = &args.live {
        tmp::validate_name(name)?;
    }
    let area = Area::from_env();

    if let Err(e) = ensure_fresh(ctx, max_age) {
        if matches!(index::load(&ctx.index), Ok(Loaded::Ok(_))) {
            eprintln!("zop: warning: refresh failed, using the existing index: {e}");
        } else {
            return Err(e);
        }
    }
    let Loaded::Ok(index) = index::load(&ctx.index)? else {
        return Err(Error::new("the index could not be read; run `zop refresh`"));
    };

    let mut projects: Vec<Project> = index
        .projects
        .into_iter()
        .filter(|p| p.path.exists())
        .collect();
    let present: HashSet<PathBuf> = projects.iter().map(|p| p.path.clone()).collect();
    for project in &mut projects {
        if project
            .parent
            .as_ref()
            .is_some_and(|parent| !present.contains(parent))
        {
            project.parent = None;
        }
    }
    let ids = ids::assign(&projects);

    let (status, entries) = if args.no_zoxide && !args.zoxide {
        (Status::Disabled, Vec::new())
    } else {
        match zoxide::locate(ctx.zoxide_path.as_deref()) {
            None => (Status::Missing, Vec::new()),
            Some(bin) => match zoxide::query(&bin, zoxide::QUERY_TIMEOUT) {
                Some(entries) => (Status::Ok, entries),
                None => (Status::Failed, Vec::new()),
            },
        }
    };
    let mut ranked = rank::rank(&projects, &entries);

    if let Some(name) = &args.live {
        let mut kept = Vec::new();
        for r in ranked {
            if area.is_live_for(&projects[r.index].realpath, name)? {
                kept.push(r);
            }
        }
        ranked = kept;
    }

    if json {
        let out = Envelope {
            version: index::VERSION,
            zoxide: status.as_str(),
            projects: ranked
                .iter()
                .enumerate()
                .map(|(position, r)| {
                    let p = &projects[r.index];
                    JsonProject {
                        path: lossy(&p.path),
                        realpath: lossy(&p.realpath),
                        name: p
                            .path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        id: ids[r.index].clone(),
                        kind: p.kind.as_str(),
                        parent: p.parent.as_deref().map(lossy),
                        visited: r.visited,
                        rank: position,
                        tmp: lossy(&area.dir_for(&p.realpath)),
                    }
                })
                .collect(),
        };
        let mut text = serde_json::to_vec(&out).map_err(|e| Error::new(e.to_string()))?;
        text.push(b'\n');
        emit(&text)?;
    } else {
        let mut text = Vec::new();
        for r in &ranked {
            let path = &projects[r.index].path;
            let shown = if tilde { tildify(path) } else { path.clone() };
            text.extend_from_slice(shown.as_os_str().as_bytes());
            text.push(b'\n');
        }
        emit(&text)?;
    }
    Ok(0)
}

fn lossy(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn tildify(path: &Path) -> PathBuf {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return path.to_path_buf();
    };
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => PathBuf::from("~"),
        Ok(rest) => Path::new("~").join(rest),
        Err(_) => path.to_path_buf(),
    }
}

pub fn parse_duration(s: &str) -> Result<Duration> {
    let bad = || {
        Error::new(format!(
            "invalid duration {s:?}; use a number and s, m, h or d, like 30m or 1d"
        ))
    };
    let (at, unit) = s.char_indices().next_back().ok_or_else(bad)?;
    let n: u64 = s[..at].parse().map_err(|_| bad())?;
    let factor: u64 = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86400,
        _ => return Err(bad()),
    };
    let secs = n.checked_mul(factor).ok_or_else(bad)?;
    Ok(Duration::from_secs(secs))
}

// ---------------------------------------------------------------------------------------
// refresh

enum Mode {
    Explicit { prune: bool },
    Implicit { max_age: Option<Duration> },
}

fn needs_refresh(loaded: &Loaded, index: &Path, max_age: Option<Duration>) -> bool {
    match loaded {
        Loaded::Missing | Loaded::Stale(_) => true,
        Loaded::Ok(_) => max_age.is_some_and(|max| index::age(index).is_none_or(|age| age > max)),
    }
}

fn stored_inputs(loaded: &Loaded) -> Option<Inputs> {
    match loaded {
        Loaded::Ok(index) => Some(index.inputs.clone()),
        Loaded::Stale(inputs) => inputs.clone(),
        Loaded::Missing => None,
    }
}

/// An implicit refresh from `list`: missing, older format, or stale by `--refresh-if-older`.
/// Inputs come from the config's `[refresh]` section, else from the index itself.
fn ensure_fresh(ctx: &Ctx, max_age: Option<Duration>) -> Result<()> {
    let loaded = index::load(&ctx.index)?;
    if !needs_refresh(&loaded, &ctx.index, max_age) {
        return Ok(());
    }
    let from_config = if ctx.sections.refresh.is_empty() {
        None
    } else {
        let mut argv = vec![OsString::from("zop"), OsString::from("refresh")];
        argv.extend(ctx.sections.refresh.iter().map(OsString::from));
        match Cli::try_parse_from(argv) {
            Ok(Cli {
                command: Command::Refresh(args),
                ..
            }) => inputs_from(&args, stored_inputs(&loaded).as_ref()),
            Ok(_) => None,
            Err(e) => return Err(Error::new(format!("config [refresh]: {e}"))),
        }
    };
    let Some(inputs) = from_config.or_else(|| stored_inputs(&loaded)) else {
        return Err(Error::new(
            "no index yet and nothing to build one from; run `zop refresh --root DIR`, \
             or put roots in the [refresh] section of the config",
        ));
    };
    do_refresh(ctx, &inputs, &Mode::Implicit { max_age }).map(|_| ())
}

/// Inputs from `refresh` flags. With none given and a stored set, reuse the stored set,
/// changed only by flags that were actually typed.
fn inputs_from(args: &RefreshArgs, stored: Option<&Inputs>) -> Option<Inputs> {
    let explicit = !args.root.is_empty() || !args.add.is_empty();
    let base = if explicit {
        Inputs {
            roots: args.root.iter().map(|p| absolute(p)).collect(),
            adds: args.add.iter().map(|p| absolute(p)).collect(),
            max_depth: DEFAULT_MAX_DEPTH,
            worktrees: true,
        }
    } else {
        stored?.clone()
    };
    Some(Inputs {
        max_depth: args.max_depth.unwrap_or(base.max_depth),
        worktrees: if args.no_worktrees {
            false
        } else if args.worktrees {
            true
        } else {
            base.worktrees
        },
        ..base
    })
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn cmd_refresh(ctx: &Ctx, args: &RefreshArgs) -> Result<u8> {
    let stored = stored_inputs(&index::load(&ctx.index)?);
    let Some(inputs) = inputs_from(args, stored.as_ref()) else {
        return Err(Error::new(
            "no roots or adds: pass --root DIR or --add PATH, or put them in the [refresh] \
             section of the config",
        ));
    };
    let summary = do_refresh(
        ctx,
        &inputs,
        &Mode::Explicit {
            prune: !args.no_prune,
        },
    )?;
    if let Some(summary) = summary {
        eprintln!("zop: {summary}");
    }
    Ok(0)
}

/// What a refresh found. Printed to stderr by an explicit `zop refresh`.
struct Summary {
    projects: usize,
    worktrees: usize,
    roots: usize,
    adds: usize,
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        write!(f, "{}", plural(self.projects, "project", "projects"))?;
        if self.worktrees > 0 {
            write!(f, " ({})", plural(self.worktrees, "worktree", "worktrees"))?;
        }
        let mut from = Vec::new();
        if self.roots > 0 {
            from.push(plural(self.roots, "root", "roots"));
        }
        if self.adds > 0 {
            from.push(plural(self.adds, "extra", "extras"));
        }
        write!(f, " from {}", from.join(" and "))
    }
}

fn do_refresh(ctx: &Ctx, inputs: &Inputs, mode: &Mode) -> Result<Option<Summary>> {
    let lock = Lock::acquire(&ctx.index, LOCK_TIMEOUT)?;

    if let Mode::Implicit { max_age } = mode {
        let loaded = index::load(&ctx.index)?;
        if !needs_refresh(&loaded, &ctx.index, *max_age) {
            return Ok(None);
        }
    }

    let outcome = discover::discover(&Options {
        roots: inputs.roots.clone(),
        adds: inputs.adds.clone(),
        worktrees: inputs.worktrees,
        max_depth: inputs.max_depth,
    })?;
    for warning in &outcome.warnings {
        eprintln!("zop: warning: {warning}");
    }
    let summary = Summary {
        projects: outcome.projects.len(),
        worktrees: outcome
            .projects
            .iter()
            .filter(|p| p.kind == discover::Kind::Worktree)
            .count(),
        roots: inputs.roots.len(),
        adds: inputs.adds.len(),
    };
    index::write_atomic(
        &ctx.index,
        &Index {
            inputs: inputs.clone(),
            projects: outcome.projects,
        },
    )?;
    drop(lock);

    if let Mode::Explicit { prune: true } = mode {
        match Area::from_env().prune() {
            Ok(report) => {
                for warning in &report.warnings {
                    eprintln!("zop: warning: prune: {warning}");
                }
            }
            Err(e) => eprintln!("zop: warning: prune: {e}"),
        }
    }
    Ok(Some(summary))
}

// ---------------------------------------------------------------------------------------
// tmp and visit

fn cmd_tmp(args: &TmpArgs) -> Result<u8> {
    let area = Area::from_env();
    if let Some(name) = &args.all {
        let mut text = Vec::new();
        for path in area.all(name, args.live)? {
            text.extend_from_slice(path.as_os_str().as_bytes());
            text.push(b'\n');
        }
        emit(&text)?;
        return Ok(0);
    }

    let Some(path) = &args.path else {
        return Err(Error::new("tmp needs a PATH, or --all NAME"));
    };
    let real = fs::canonicalize(path).map_err(|e| Error::io(path, e))?;

    if args.live {
        let Some(name) = &args.name else {
            return Err(Error::new("--live needs a NAME"));
        };
        return Ok(if area.is_live_for(&real, name)? { 0 } else { 1 });
    }

    let out = area.path_for(&real, args.name.as_deref())?;
    if args.create {
        area.create(&real)?;
    }
    let mut text = out.as_os_str().as_bytes().to_vec();
    text.push(b'\n');
    emit(&text)?;
    Ok(0)
}

/// One path per line. A blank line is skipped. Any read or decoding error, or a path
/// that can't be expanded, fails the whole command before anything is recorded.
fn read_visit_paths(input: impl std::io::BufRead) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for (n, line) in input.lines().enumerate() {
        let n = n + 1;
        let line = line.map_err(|e| Error::io_context(format!("stdin line {n}"), e))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let path = expand::parse_path(OsStr::new(line))
            .map_err(|e| Error::new(format!("stdin line {n}: {e}")))?;
        paths.push(absolute(&path));
    }
    Ok(paths)
}

fn cmd_visit(ctx: &Ctx, args: &VisitArgs) -> Result<u8> {
    let paths: Vec<PathBuf> = if args.path.as_os_str() == "-" {
        read_visit_paths(std::io::stdin().lock())?
    } else {
        vec![absolute(&args.path)]
    };
    match zoxide::locate(ctx.zoxide_path.as_deref()) {
        None => eprintln!("zop: warning: zoxide not found; visit not recorded"),
        Some(bin) => {
            for path in &paths {
                if let Err(e) = zoxide::add(&bin, path) {
                    eprintln!("zop: warning: zoxide add failed: {e}");
                }
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_broken_pipe_is_a_quiet_exit() {
        let pipe = Error::io_context("stdout", std::io::ErrorKind::BrokenPipe.into());
        let other = Error::io_context("stdout", std::io::ErrorKind::PermissionDenied.into());
        assert!(is_broken_pipe(&pipe));
        assert!(!is_broken_pipe(&other));
        assert!(!is_broken_pipe(&Error::new("stdout: Broken pipe")));
    }

    #[test]
    fn visit_paths_skip_blanks_and_number_lines_in_errors() {
        let ok = read_visit_paths("/a\n\n  /b  \n".as_bytes()).unwrap();
        assert_eq!(ok, [PathBuf::from("/a"), PathBuf::from("/b")]);

        let err = read_visit_paths(&b"/a\n\xff\n/b\n"[..]).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("stdin line 2"), "{text}");
        assert_eq!(err.io_kind(), Some(std::io::ErrorKind::InvalidData));

        let err = read_visit_paths("/a\n$ZOP_UNDEFINED_VAR_XYZ\n".as_bytes()).unwrap_err();
        assert!(err.to_string().contains("stdin line 2"), "{err}");
    }

    fn argv(s: &str) -> Vec<OsString> {
        std::iter::once("zop")
            .chain(s.split_whitespace())
            .map(OsString::from)
            .collect()
    }

    fn sections() -> Sections {
        Sections {
            list: vec!["--refresh-if-older=1d".into()],
            refresh: vec!["--root=~/p".into()],
        }
    }

    fn rewritten(input: &str) -> String {
        let args = argv(input);
        let scan = scan(&args);
        apply(&args, &scan.plan, &sections())
            .iter()
            .map(|a| a.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn bare_zop_is_list_with_its_section() {
        assert_eq!(rewritten(""), "zop list --refresh-if-older=1d");
    }

    #[test]
    fn list_is_inserted_before_the_first_non_global_flag() {
        assert_eq!(rewritten("--json"), "zop list --refresh-if-older=1d --json");
        assert_eq!(
            rewritten("--live nvim.sock"),
            "zop list --refresh-if-older=1d --live nvim.sock"
        );
    }

    #[test]
    fn a_global_flags_value_is_never_taken_for_the_subcommand() {
        assert_eq!(
            rewritten("--index /x"),
            "zop --index /x list --refresh-if-older=1d"
        );
        assert_eq!(
            rewritten("--index=/x"),
            "zop --index=/x list --refresh-if-older=1d"
        );
        assert_eq!(
            rewritten("--index /x list --json"),
            "zop --index /x list --refresh-if-older=1d --json"
        );
        assert_eq!(
            rewritten("--zoxide-path /z --json"),
            "zop --zoxide-path /z list --refresh-if-older=1d --json"
        );
    }

    #[test]
    fn subcommand_section_goes_right_after_the_subcommand() {
        assert_eq!(
            rewritten("--config F refresh --root R"),
            "zop --config F refresh --root=~/p --root R"
        );
    }

    #[test]
    fn subcommands_without_a_section_are_untouched() {
        assert_eq!(
            rewritten("tmp . nvim.sock --live"),
            "zop tmp . nvim.sock --live"
        );
        assert_eq!(rewritten("visit /p"), "zop visit /p");
    }

    #[test]
    fn help_and_version_pass_through() {
        for input in ["--help", "-h", "-V", "--version", "help", "help list"] {
            assert_eq!(rewritten(input), format!("zop {input}"));
        }
        assert_eq!(
            rewritten("list --help"),
            "zop list --refresh-if-older=1d --help"
        );
    }

    #[test]
    fn scanning_stops_at_double_dash() {
        assert_eq!(rewritten("-- foo"), "zop list --refresh-if-older=1d -- foo");
    }

    #[test]
    fn config_flags_are_read_by_the_scan() {
        let s = scan(&argv("--config /c/file list"));
        assert_eq!(s.config.as_deref(), Some(OsStr::new("/c/file")));
        let s = scan(&argv("--config=/c/file"));
        assert_eq!(s.config.as_deref(), Some(OsStr::new("/c/file")));
        assert!(scan(&argv("--no-config list")).no_config);
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(parse_duration("1d").unwrap(), Duration::from_secs(86400));
        assert_eq!(parse_duration("12h").unwrap(), Duration::from_secs(43200));
        assert!(parse_duration("1w").is_err());
        assert!(parse_duration("d").is_err());
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn tilde_collapses_only_at_a_component_boundary() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            tildify(Path::new(&format!("{home}/a/b"))),
            Path::new("~/a/b")
        );
        assert_eq!(tildify(Path::new(&home)), Path::new("~"));
        assert_eq!(
            tildify(Path::new(&format!("{home}x/a"))),
            format!("{home}x/a")
        );
    }

    #[test]
    fn later_flag_wins_for_pairs_and_values() {
        let cli = Cli::try_parse_from([
            "zop",
            "list",
            "--refresh-if-older=1d",
            "--no-refresh-if-older",
        ])
        .unwrap();
        let Command::List(a) = cli.command else {
            panic!()
        };
        assert!(a.no_refresh_if_older && a.refresh_if_older.is_none());

        let cli = Cli::try_parse_from(["zop", "list", "--json", "--no-json", "--json"]).unwrap();
        let Command::List(a) = cli.command else {
            panic!()
        };
        assert!(a.json && !a.no_json);

        let cli = Cli::try_parse_from(["zop", "list", "--live", "a", "--live", "b"]).unwrap();
        let Command::List(a) = cli.command else {
            panic!()
        };
        assert_eq!(a.live.as_deref(), Some("b"));
    }

    #[test]
    fn repeatable_flags_accumulate() {
        let cli = Cli::try_parse_from([
            "zop", "refresh", "--root", "/a", "--root", "/b", "--add", "/c",
        ])
        .unwrap();
        let Command::Refresh(a) = cli.command else {
            panic!()
        };
        assert_eq!(a.root, [PathBuf::from("/a"), PathBuf::from("/b")]);
        assert_eq!(a.add, [PathBuf::from("/c")]);
    }

    #[test]
    fn tmp_flag_conflicts() {
        assert!(Cli::try_parse_from(["zop", "tmp", ".", "n", "--create", "--live"]).is_err());
        assert!(Cli::try_parse_from(["zop", "tmp", "--all", "n", "."]).is_err());
        assert!(Cli::try_parse_from(["zop", "tmp"]).is_err());
        assert!(Cli::try_parse_from(["zop", "tmp", "--all", "n", "--live"]).is_ok());
    }

    #[test]
    fn stored_inputs_are_reused_and_changed_only_by_typed_flags() {
        let stored = Inputs {
            roots: vec![PathBuf::from("/r")],
            adds: vec![],
            max_depth: 7,
            worktrees: true,
        };
        let none = RefreshArgs::default();
        assert_eq!(inputs_from(&none, Some(&stored)), Some(stored.clone()));
        assert_eq!(inputs_from(&none, None), None);

        let no_wt = RefreshArgs {
            no_worktrees: true,
            ..Default::default()
        };
        assert!(!inputs_from(&no_wt, Some(&stored)).unwrap().worktrees);

        let fresh = RefreshArgs {
            root: vec![PathBuf::from("/new")],
            ..Default::default()
        };
        let got = inputs_from(&fresh, Some(&stored)).unwrap();
        assert_eq!(got.roots, [PathBuf::from("/new")]);
        assert_eq!(got.max_depth, DEFAULT_MAX_DEPTH);
    }
}
