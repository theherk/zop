//! Collision-safe labels for projects. An id is a label; `path` is the key.

use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};

use sha2::{Digest, Sha256};

use crate::discover::{Kind, Project};

const SEP: &str = "~";
const MAX_LEN: usize = 64;

/// One id per project, in the order given. Computed over the whole slice.
pub fn assign(projects: &[Project]) -> Vec<String> {
    let by_path: HashMap<&Path, usize> = projects
        .iter()
        .enumerate()
        .map(|(i, p)| (p.path.as_path(), i))
        .collect();

    let attached = |i: usize| -> Option<usize> {
        let project = &projects[i];
        if project.kind != Kind::Worktree {
            return None;
        }
        let parent = *by_path.get(project.parent.as_deref()?)?;
        (!(projects[parent].kind == Kind::Worktree && projects[parent].parent.is_some()))
            .then_some(parent)
    };

    let mut ids: Vec<Option<String>> = vec![None; projects.len()];

    let free: Vec<usize> = (0..projects.len())
        .filter(|&i| attached(i).is_none())
        .collect();
    let free_segments: Vec<Vec<String>> =
        free.iter().map(|&i| segments(&projects[i].path)).collect();
    for (slot, id) in free.iter().zip(shortest_unique(&free_segments)) {
        ids[*slot] = Some(id);
    }

    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..projects.len() {
        if let Some(parent) = attached(i) {
            children.entry(parent).or_default().push(i);
        }
    }
    for (parent, kids) in children {
        let parent_id = ids[parent].clone().unwrap_or_default();
        let kid_segments: Vec<Vec<String>> =
            kids.iter().map(|&i| segments(&projects[i].path)).collect();
        for (slot, suffix) in kids.iter().zip(shortest_unique(&kid_segments)) {
            ids[*slot] = Some(format!("{parent_id}@{suffix}"));
        }
    }

    let mut ids: Vec<String> = ids.into_iter().map(Option::unwrap_or_default).collect();

    let mut seen: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, id) in ids.iter().enumerate() {
        seen.entry(id.clone()).or_default().push(i);
    }
    for group in seen.values().filter(|group| group.len() > 1) {
        for &i in group {
            ids[i] = format!("{}{SEP}{}", ids[i], hash6(&projects[i].realpath));
        }
    }

    for (i, id) in ids.iter_mut().enumerate() {
        if id.len() > MAX_LEN {
            let keep = MAX_LEN - 1 - 6;
            *id = format!("{}{SEP}{}", &id[..keep], hash6(&projects[i].realpath));
        }
    }
    ids
}

/// For each item, the shortest suffix of its segments that no other item shares.
fn shortest_unique(items: &[Vec<String>]) -> Vec<String> {
    let suffix = |segs: &[String], k: usize| -> String {
        let k = k.min(segs.len());
        segs[segs.len() - k..].join(SEP)
    };
    items
        .iter()
        .map(|segs| {
            for k in 1..=segs.len() {
                let mine = suffix(segs, k);
                let shared = items
                    .iter()
                    .filter(|other| suffix(other, k) == mine)
                    .count();
                if shared == 1 {
                    return mine;
                }
            }
            suffix(segs, segs.len())
        })
        .collect()
}

fn segments(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(sanitize(&name.to_string_lossy())),
            _ => None,
        })
        .collect()
}

fn sanitize(segment: &str) -> String {
    segment
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._@~+-".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn hash6(real: &Path) -> String {
    let digest = Sha256::digest(real.as_os_str().as_bytes());
    digest.iter().take(3).map(|b| format!("{b:02x}")).collect()
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

    fn worktree(path: &str, parent: Option<&str>) -> Project {
        Project {
            path: PathBuf::from(path),
            realpath: PathBuf::from(path),
            kind: Kind::Worktree,
            parent: parent.map(PathBuf::from),
        }
    }

    #[test]
    fn unique_names_stay_short() {
        let ids = assign(&[repo("/p/github.com/o/zop"), repo("/p/github.com/o/other")]);
        assert_eq!(ids, ["zop", "other"]);
    }

    #[test]
    fn collisions_extend_only_as_far_as_needed() {
        let ids = assign(&[
            repo("/home/me/commons"),
            repo("/p/github.com/example/commons"),
            repo("/p/github.com/example/zop"),
        ]);
        assert_eq!(ids, ["me~commons", "example~commons", "zop"]);
    }

    #[test]
    fn three_way_collision_deepens_the_shared_suffix() {
        let ids = assign(&[repo("/a/x/docs"), repo("/b/x/docs"), repo("/c/docs")]);
        assert_eq!(ids, ["a~x~docs", "b~x~docs", "c~docs"]);
    }

    #[test]
    fn worktree_takes_its_parents_id() {
        let ids = assign(&[
            repo("/p/o/zop"),
            worktree("/p/o/zop-fix-login", Some("/p/o/zop")),
        ]);
        assert_eq!(ids, ["zop", "zop@zop-fix-login"]);
    }

    #[test]
    fn worktrees_sharing_a_directory_name_extend_within_the_parent() {
        let ids = assign(&[
            repo("/p/zop"),
            worktree("/wt/a/fix", Some("/p/zop")),
            worktree("/wt/b/fix", Some("/p/zop")),
        ]);
        assert_eq!(ids, ["zop", "zop@a~fix", "zop@b~fix"]);
    }

    #[test]
    fn worktree_without_a_parent_in_the_index_follows_the_repo_rule() {
        let ids = assign(&[worktree("/wt/loose", None), repo("/p/zop")]);
        assert_eq!(ids, ["loose", "zop"]);
    }

    #[test]
    fn awkward_characters_are_sanitised() {
        let ids = assign(&[repo("/p/my project:v2")]);
        assert_eq!(ids, ["my-project-v2"]);
    }

    #[test]
    fn leftover_collisions_get_a_hash_suffix() {
        // A worktree `x` of `zop` is `zop@x`, and a repo directory literally named `zop@x` is too.
        let ids = assign(&[
            worktree("/w/x", Some("/p/zop")),
            repo("/p/zop"),
            repo("/q/zop@x"),
        ]);
        assert_ne!(ids[0], ids[2]);
        assert!(
            ids[0].starts_with("zop@x~") && ids[2].starts_with("zop@x~"),
            "{ids:?}"
        );
        assert_eq!(ids[1], "zop");
    }

    #[test]
    fn long_ids_are_bounded() {
        let name = "a".repeat(100);
        let ids = assign(&[repo(&format!("/p/{name}"))]);
        assert_eq!(ids[0].len(), MAX_LEN);
        assert!(ids[0].contains('~'));
    }

    #[test]
    fn independent_of_input_order() {
        let a = [repo("/a/docs"), repo("/b/docs"), repo("/p/zop")];
        let b = [repo("/p/zop"), repo("/b/docs"), repo("/a/docs")];
        let mut ids_a: Vec<_> = a
            .iter()
            .zip(assign(&a))
            .map(|(p, id)| (p.path.clone(), id))
            .collect();
        let mut ids_b: Vec<_> = b
            .iter()
            .zip(assign(&b))
            .map(|(p, id)| (p.path.clone(), id))
            .collect();
        ids_a.sort();
        ids_b.sort();
        assert_eq!(ids_a, ids_b);
    }
}
