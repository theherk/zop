//! Finding projects: walk roots, classify markers structurally, read worktree records.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};
use crate::expand;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Repo,
    Worktree,
    Extra,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Repo => "repo",
            Kind::Worktree => "worktree",
            Kind::Extra => "extra",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "repo" => Some(Kind::Repo),
            "worktree" => Some(Kind::Worktree),
            "extra" => Some(Kind::Extra),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    /// The path as discovered, in the spelling of the root or `--add` argument.
    pub path: PathBuf,
    pub realpath: PathBuf,
    pub kind: Kind,
    /// For worktrees: the `path` of the main checkout, only if it is in the index.
    pub parent: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub roots: Vec<PathBuf>,
    pub adds: Vec<PathBuf>,
    pub worktrees: bool,
    pub max_depth: usize,
}

pub struct Outcome {
    pub projects: Vec<Project>,
    pub warnings: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Marker {
    Repo,
    Worktree(Option<PathBuf>),
    Skip(&'static str),
    None,
}

const SKIP_DIRS: [&str; 3] = [".git", ".jj", "node_modules"];

pub fn discover(opts: &Options) -> Result<Outcome> {
    let mut walker = Walker {
        opts,
        cands: Vec::new(),
        warnings: Vec::new(),
    };
    for root in &opts.roots {
        let meta =
            fs::metadata(root).map_err(|e| Error::new(format!("root {}: {e}", root.display())))?;
        if !meta.is_dir() {
            return Err(Error::new(format!(
                "root {} is not a directory",
                root.display()
            )));
        }
        fs::read_dir(root).map_err(|e| Error::new(format!("root {}: {e}", root.display())))?;
        walker.walk(root, 0);
    }
    for add in &opts.adds {
        walker.add(add);
    }
    let outcome = walker.finish();
    if outcome.projects.is_empty() && opts.roots.is_empty() && !opts.adds.is_empty() {
        return Err(Error::new("none of the --add paths could be used"));
    }
    Ok(outcome)
}

struct Candidate {
    project: Project,
    parent_real: Option<PathBuf>,
    rank: u8,
}

struct Walker<'a> {
    opts: &'a Options,
    cands: Vec<Candidate>,
    warnings: Vec<String>,
}

impl Walker<'_> {
    fn walk(&mut self, dir: &Path, depth: usize) {
        match classify(dir) {
            Marker::Repo => {
                self.record(dir, Kind::Repo, None, true);
                self.scan_records(dir);
            }
            Marker::Worktree(parent) => {
                if self.opts.worktrees {
                    self.record(dir, Kind::Worktree, parent, true);
                }
            }
            Marker::Skip(_) => {}
            Marker::None => {
                if depth >= self.opts.max_depth {
                    return;
                }
                let entries = match fs::read_dir(dir) {
                    Ok(entries) => entries,
                    Err(e) => {
                        self.warn(dir, &e);
                        return;
                    }
                };
                let mut children: Vec<PathBuf> = Vec::new();
                for entry in entries {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(e) => {
                            self.warn(dir, &e);
                            continue;
                        }
                    };
                    let name = entry.file_name();
                    if SKIP_DIRS.iter().any(|skip| name == *skip) {
                        continue;
                    }
                    match entry.file_type() {
                        Ok(t) if t.is_dir() => children.push(entry.path()),
                        Ok(_) => {}
                        Err(e) => self.warn(&entry.path(), &e),
                    }
                }
                children.sort();
                for child in children {
                    self.walk(&child, depth + 1);
                }
            }
        }
    }

    /// A problem with one path. A path that vanished mid-walk is normal and not reported.
    fn warn(&mut self, path: &Path, e: &std::io::Error) {
        if e.kind() != std::io::ErrorKind::NotFound {
            self.warnings.push(format!("{}: {e}", path.display()));
        }
    }

    fn add(&mut self, path: &Path) {
        match fs::metadata(path) {
            Ok(meta) if meta.is_file() => {
                let text = match fs::read_to_string(path) {
                    Ok(text) => text,
                    Err(e) => {
                        self.warnings.push(format!("{}: {e}", path.display()));
                        return;
                    }
                };
                let base = path.parent().unwrap_or(Path::new("."));
                for line in text.lines().map(str::trim) {
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    match expand::expand(std::ffi::OsStr::new(line)) {
                        Ok(expanded) => {
                            let dir = resolve(base, &expanded);
                            self.add_dir(&dir);
                        }
                        Err(e) => self
                            .warnings
                            .push(format!("{}: {line}: {e}", path.display())),
                    }
                }
            }
            Ok(meta) if meta.is_dir() => self.add_dir(path),
            _ => self.warnings.push(format!(
                "--add {}: no such file or directory",
                path.display()
            )),
        }
    }

    fn add_dir(&mut self, dir: &Path) {
        if !dir.is_dir() {
            self.warnings
                .push(format!("{}: no such directory", dir.display()));
            return;
        }
        match classify(dir) {
            Marker::Repo => {
                self.record(dir, Kind::Repo, None, false);
                self.scan_records(dir);
            }
            Marker::Worktree(parent) => {
                if self.opts.worktrees {
                    self.record(dir, Kind::Worktree, parent, false);
                }
            }
            Marker::Skip(_) | Marker::None => self.record(dir, Kind::Extra, None, false),
        }
    }

    fn record(&mut self, dir: &Path, kind: Kind, parent_real: Option<PathBuf>, from_root: bool) {
        let realpath = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        let rank = u8::from(kind != Kind::Extra) * 2 + u8::from(from_root);
        self.cands.push(Candidate {
            project: Project {
                path: dir.to_path_buf(),
                realpath,
                kind,
                parent: None,
            },
            parent_real,
            rank,
        });
    }

    /// Worktrees recorded under `<repo>/.git/worktrees/*`, wherever they live.
    fn scan_records(&mut self, repo: &Path) {
        if !self.opts.worktrees {
            return;
        }
        let records = repo.join(".git").join("worktrees");
        let entries = match fs::read_dir(&records) {
            Ok(entries) => entries,
            Err(e) => return self.warn(&records, &e),
        };
        let parent_real = fs::canonicalize(repo).ok();
        let mut dirs: Vec<PathBuf> = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => dirs.push(entry.path()),
                Err(e) => self.warn(&records, &e),
            }
        }
        dirs.sort();
        for record in dirs {
            let text = match fs::read_to_string(record.join("gitdir")) {
                Ok(text) => text,
                Err(e) => {
                    self.warn(&record, &e);
                    continue;
                }
            };
            let git_file = resolve(&record, text.trim());
            if !git_file.is_file() || !git_file_points_to(&git_file, &record) {
                continue;
            }
            let Some(worktree) = git_file.parent() else {
                continue;
            };
            self.record(worktree, Kind::Worktree, parent_real.clone(), false);
        }
    }

    fn finish(self) -> Outcome {
        let mut order: Vec<PathBuf> = Vec::new();
        let mut best: HashMap<PathBuf, Candidate> = HashMap::new();
        for cand in self.cands {
            let key = cand.project.realpath.clone();
            match best.get(&key) {
                None => {
                    order.push(key.clone());
                    best.insert(key, cand);
                }
                Some(old) if cand.rank > old.rank => {
                    best.insert(key, cand);
                }
                Some(_) => {}
            }
        }
        let by_real: HashMap<&PathBuf, &PathBuf> = best
            .iter()
            .map(|(real, cand)| (real, &cand.project.path))
            .collect();
        let projects = order
            .iter()
            .map(|key| {
                let cand = &best[key];
                let mut project = cand.project.clone();
                if project.kind == Kind::Worktree {
                    project.parent = cand
                        .parent_real
                        .as_ref()
                        .and_then(|real| by_real.get(real))
                        .map(|path| (*path).clone());
                }
                project
            })
            .collect();
        Outcome {
            projects,
            warnings: self.warnings,
        }
    }
}

/// Classifies one directory by reading its markers. Never runs `git` or `jj`.
pub fn classify(dir: &Path) -> Marker {
    let jj = dir.join(".jj");
    if jj.is_dir() {
        let repo = jj.join("repo");
        return match fs::symlink_metadata(&repo) {
            Ok(meta) if meta.is_file() => Marker::Worktree(jj_parent(&jj, &repo)),
            _ => Marker::Repo,
        };
    }

    let git = dir.join(".git");
    let Ok(meta) = fs::symlink_metadata(&git) else {
        return Marker::None;
    };
    if meta.is_dir() {
        return if git.join("HEAD").exists() {
            Marker::Repo
        } else {
            Marker::None
        };
    }
    if meta.is_file() {
        return classify_git_file(dir, &git);
    }
    Marker::None
}

fn classify_git_file(dir: &Path, git_file: &Path) -> Marker {
    let Ok(text) = fs::read_to_string(git_file) else {
        return Marker::Skip("unreadable .git file");
    };
    let Some(line) = text.lines().find_map(|l| l.strip_prefix("gitdir:")) else {
        return Marker::Skip("malformed .git file");
    };
    let Ok(gitdir) = fs::canonicalize(resolve(dir, line.trim())) else {
        return Marker::Skip("broken .git file");
    };

    let is_worktree_record = gitdir
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "worktrees")
        && gitdir.join("commondir").is_file()
        && record_points_to(&gitdir, git_file);
    if is_worktree_record {
        let parent = fs::read_to_string(gitdir.join("commondir"))
            .ok()
            .and_then(|text| fs::canonicalize(resolve(&gitdir, text.trim())).ok())
            .filter(|common| common.file_name().is_some_and(|name| name == ".git"))
            .and_then(|common| common.parent().map(Path::to_path_buf));
        return Marker::Worktree(parent);
    }

    if is_submodule_gitdir(&gitdir) {
        return Marker::Skip("submodule");
    }
    Marker::Repo
}

/// A worktree's `.git` file names a record directory in the main repo.
fn git_file_points_to(git_file: &Path, record: &Path) -> bool {
    let Ok(text) = fs::read_to_string(git_file) else {
        return false;
    };
    let Some(line) = text.lines().find_map(|l| l.strip_prefix("gitdir:")) else {
        return false;
    };
    let base = git_file.parent().unwrap_or(Path::new("."));
    same_file(&resolve(base, line.trim()), record)
}

/// The record's `gitdir` file names the worktree's `.git` file. Together with
/// `git_file_points_to` this is the reciprocal check.
fn record_points_to(record: &Path, git_file: &Path) -> bool {
    let Ok(text) = fs::read_to_string(record.join("gitdir")) else {
        return false;
    };
    same_file(&resolve(record, text.trim()), git_file)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// A `modules` segment after a `.git` segment marks a submodule's git dir.
fn is_submodule_gitdir(gitdir: &Path) -> bool {
    let names: Vec<_> = gitdir
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    let Some(last_git) = names.iter().rposition(|name| *name == ".git") else {
        return false;
    };
    names[last_git + 1..].iter().any(|name| *name == "modules")
}

fn jj_parent(jj: &Path, repo_file: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(repo_file).ok()?;
    let main_repo = fs::canonicalize(resolve(jj, text.trim())).ok()?;
    Some(main_repo.parent()?.parent()?.to_path_buf())
}

fn resolve(base: &Path, s: impl AsRef<Path>) -> PathBuf {
    let path = s.as_ref();
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}
