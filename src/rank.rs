//! Ranking: projects zoxide has seen, in zoxide's order, then the rest by path.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path};

use crate::discover::{Kind, Project};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ranked {
    pub index: usize,
    pub visited: bool,
}

/// `zoxide` lists directories, most used first. Each is mapped to the project that
/// contains it, and a project ranks by its best entry.
pub fn rank(projects: &[Project], zoxide: &[String]) -> Vec<Ranked> {
    let keys: Vec<[Vec<OsString>; 2]> = projects
        .iter()
        .map(|p| [components(&p.path), components(&p.realpath)])
        .collect();

    let mut seen = vec![false; projects.len()];
    let mut head: Vec<Ranked> = Vec::new();
    for entry in zoxide {
        let path = Path::new(entry);
        let found = enclosing(&keys, &components(path)).or_else(|| {
            let real = fs::canonicalize(path).ok()?;
            enclosing(&keys, &components(&real))
        });
        if let Some(index) = found
            && !seen[index]
        {
            seen[index] = true;
            head.push(Ranked {
                index,
                visited: true,
            });
        }
    }

    let tail: Vec<usize> = (0..projects.len()).filter(|&i| !seen[i]).collect();
    let in_tail: HashSet<&Path> = tail.iter().map(|&i| projects[i].path.as_path()).collect();

    let mut children: HashMap<&Path, Vec<usize>> = HashMap::new();
    let mut top: Vec<usize> = Vec::new();
    for &i in &tail {
        let project = &projects[i];
        match project.parent.as_deref() {
            Some(parent) if project.kind == Kind::Worktree && in_tail.contains(parent) => {
                children.entry(parent).or_default().push(i);
            }
            _ => top.push(i),
        }
    }
    top.sort_by(|&a, &b| path_bytes(&projects[a].path).cmp(path_bytes(&projects[b].path)));
    for kids in children.values_mut() {
        kids.sort_by(|&a, &b| path_bytes(&projects[a].path).cmp(path_bytes(&projects[b].path)));
    }

    for i in top {
        head.push(Ranked {
            index: i,
            visited: false,
        });
        if let Some(kids) = children.get(projects[i].path.as_path()) {
            head.extend(kids.iter().map(|&k| Ranked {
                index: k,
                visited: false,
            }));
        }
    }
    head
}

/// The project whose path (or real path) is the longest whole-component prefix of `target`.
fn enclosing(keys: &[[Vec<OsString>; 2]], target: &[OsString]) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for (index, pair) in keys.iter().enumerate() {
        for key in pair {
            if !key.is_empty()
                && target.starts_with(key)
                && best.is_none_or(|(len, _)| key.len() > len)
            {
                best = Some((key.len(), index));
            }
        }
    }
    best.map(|(_, index)| index)
}

fn components(path: &Path) -> Vec<OsString> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name.to_os_string()),
            _ => None,
        })
        .collect()
}

fn path_bytes(path: &Path) -> &[u8] {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn repo(path: &str) -> Project {
        Project {
            path: PathBuf::from(path),
            realpath: PathBuf::from(path),
            kind: Kind::Repo,
            parent: None,
        }
    }

    fn order(projects: &[Project], zoxide: &[&str]) -> Vec<(String, bool)> {
        let zoxide: Vec<String> = zoxide.iter().map(|s| s.to_string()).collect();
        rank(projects, &zoxide)
            .into_iter()
            .map(|r| (projects[r.index].path.display().to_string(), r.visited))
            .collect()
    }

    #[test]
    fn visited_first_in_zoxide_order_then_the_rest_sorted() {
        let projects = [repo("/p/b"), repo("/p/a"), repo("/p/c"), repo("/p/d")];
        let got = order(&projects, &["/p/c", "/elsewhere", "/p/b"]);
        assert_eq!(
            got,
            [
                ("/p/c".into(), true),
                ("/p/b".into(), true),
                ("/p/a".into(), false),
                ("/p/d".into(), false)
            ]
        );
    }

    #[test]
    fn a_subdirectory_visit_counts_for_its_project() {
        let projects = [repo("/p/zop"), repo("/p/other")];
        let got = order(&projects, &["/p/zop/src/deep"]);
        assert_eq!(got[0], ("/p/zop".into(), true));
    }

    #[test]
    fn matching_is_by_whole_components() {
        let projects = [repo("/p/foo"), repo("/p/foobar")];
        let got = order(&projects, &["/p/foobar/x"]);
        assert_eq!(got[0], ("/p/foobar".into(), true));
        assert_eq!(got[1], ("/p/foo".into(), false));
    }

    #[test]
    fn a_nested_project_wins_over_its_container() {
        let projects = [repo("/p"), repo("/p/inner")];
        let got = order(&projects, &["/p/inner/x"]);
        assert_eq!(got[0], ("/p/inner".into(), true));
    }

    #[test]
    fn without_zoxide_everything_is_sorted_by_path() {
        let projects = [repo("/p/b"), repo("/p/a")];
        let got = order(&projects, &[]);
        assert_eq!(got, [("/p/a".into(), false), ("/p/b".into(), false)]);
    }

    #[test]
    fn worktrees_follow_their_parent_in_the_tail() {
        let mut wt = repo("/p/a-wt");
        wt.kind = Kind::Worktree;
        wt.parent = Some(PathBuf::from("/p/a"));
        let projects = [repo("/p/b"), wt, repo("/p/a"), repo("/p/c")];
        let got: Vec<String> = order(&projects, &[]).into_iter().map(|(p, _)| p).collect();
        assert_eq!(got, ["/p/a", "/p/a-wt", "/p/b", "/p/c"]);
    }

    #[test]
    fn a_visited_worktree_ranks_on_its_own() {
        let mut wt = repo("/p/a-wt");
        wt.kind = Kind::Worktree;
        wt.parent = Some(PathBuf::from("/p/a"));
        let projects = [repo("/p/a"), wt];
        let got = order(&projects, &["/p/a-wt"]);
        assert_eq!(got, [("/p/a-wt".into(), true), ("/p/a".into(), false)]);
    }
}
