//! Discovery against real git and jj repositories.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{canon, git, init_repo, jj, jj_available};
use zop::discover::{Kind, Marker, Options, Outcome, classify, discover};

fn opts(roots: &[&Path], adds: &[&Path]) -> Options {
    Options {
        roots: roots.iter().map(|p| p.to_path_buf()).collect(),
        adds: adds.iter().map(|p| p.to_path_buf()).collect(),
        worktrees: true,
        max_depth: 12,
    }
}

/// Paths relative to `base`, with kinds, sorted.
fn listing(out: &Outcome, base: &Path) -> Vec<(String, Kind)> {
    let mut rows: Vec<(String, Kind)> = out
        .projects
        .iter()
        .map(|p| {
            let rel = p.path.strip_prefix(base).unwrap_or(&p.path);
            (rel.display().to_string(), p.kind)
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

fn root() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    fs::create_dir(&root).unwrap();
    (dir, root)
}

#[test]
fn walk_stops_at_the_first_marker() {
    let (_guard, root) = root();
    init_repo(&root.join("a"));
    init_repo(&root.join("a/vendor/dep"));
    init_repo(&root.join("plain/b"));

    let out = discover(&opts(&[&root], &[])).unwrap();
    assert_eq!(
        listing(&out, &root),
        [("a".into(), Kind::Repo), ("plain/b".into(), Kind::Repo)]
    );
}

#[test]
fn a_root_that_is_itself_a_project_is_included_and_not_walked() {
    let (_guard, root) = root();
    init_repo(&root);
    init_repo(&root.join("inner"));
    let out = discover(&opts(&[&root], &[])).unwrap();
    assert_eq!(out.projects.len(), 1);
    assert_eq!(out.projects[0].path, root);
}

#[test]
fn hidden_directories_are_walked_and_node_modules_is_not() {
    let (_guard, root) = root();
    init_repo(&root.join(".archive/old"));
    init_repo(&root.join("node_modules/pkg"));
    let out = discover(&opts(&[&root], &[])).unwrap();
    assert_eq!(listing(&out, &root), [(".archive/old".into(), Kind::Repo)]);
}

#[test]
fn max_depth_bounds_the_walk() {
    let (_guard, root) = root();
    init_repo(&root.join("a/b/c/deep"));
    init_repo(&root.join("shallow"));
    let mut o = opts(&[&root], &[]);
    o.max_depth = 2;
    let out = discover(&o).unwrap();
    assert_eq!(listing(&out, &root), [("shallow".into(), Kind::Repo)]);
    o.max_depth = 4;
    assert_eq!(discover(&o).unwrap().projects.len(), 2);
}

#[test]
fn a_missing_or_non_directory_root_is_fatal() {
    let (_guard, root) = root();
    assert!(discover(&opts(&[&root.join("nope")], &[])).is_err());
    let file = root.join("file");
    fs::write(&file, "x").unwrap();
    assert!(discover(&opts(&[&file], &[])).is_err());
}

#[test]
fn an_unreadable_subdirectory_only_warns() {
    let (_guard, root) = root();
    init_repo(&root.join("ok"));
    let locked = root.join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let result = discover(&opts(&[&root], &[]));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

    let out = result.unwrap();
    assert_eq!(listing(&out, &root), [("ok".into(), Kind::Repo)]);
    assert!(
        out.warnings.iter().any(|w| w.contains("locked")),
        "{:?}",
        out.warnings
    );
}

#[test]
fn linked_worktree_under_a_root_gets_its_parent() {
    let (_guard, root) = root();
    init_repo(&root.join("main"));
    git(&root.join("main"), &["worktree", "add", "-q", "../wt"]);

    let out = discover(&opts(&[&root], &[])).unwrap();
    assert_eq!(
        listing(&out, &root),
        [("main".into(), Kind::Repo), ("wt".into(), Kind::Worktree)]
    );
    let wt = out
        .projects
        .iter()
        .find(|p| p.kind == Kind::Worktree)
        .unwrap();
    assert_eq!(wt.parent.as_deref(), Some(root.join("main").as_path()));
}

#[test]
fn worktree_outside_every_root_is_found_through_the_records() {
    let (guard, root) = root();
    init_repo(&root.join("main"));
    let elsewhere = guard.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    git(
        &root.join("main"),
        &["worktree", "add", "-q", "../../elsewhere/far"],
    );

    let out = discover(&opts(&[&root], &[])).unwrap();
    let far = out
        .projects
        .iter()
        .find(|p| p.kind == Kind::Worktree)
        .expect("found via records");
    assert_eq!(far.realpath, canon(&elsewhere.join("far")));
    assert_eq!(far.parent.as_deref(), Some(root.join("main").as_path()));
}

#[test]
fn no_worktrees_drops_them_everywhere() {
    let (_guard, root) = root();
    init_repo(&root.join("main"));
    git(&root.join("main"), &["worktree", "add", "-q", "../wt"]);
    let mut o = opts(&[&root], &[]);
    o.worktrees = false;
    let out = discover(&o).unwrap();
    assert_eq!(listing(&out, &root), [("main".into(), Kind::Repo)]);
}

#[test]
fn a_worktree_whose_main_repo_is_not_indexed_has_no_parent() {
    let (guard, root) = root();
    let main = guard.path().join("outside-main");
    init_repo(&main);
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            &root.join("wt").display().to_string(),
        ],
    );

    let out = discover(&opts(&[&root], &[])).unwrap();
    assert_eq!(listing(&out, &root), [("wt".into(), Kind::Worktree)]);
    assert_eq!(out.projects[0].parent, None);
}

#[test]
fn relative_worktree_paths_resolve() {
    let (_guard, root) = root();
    let main = root.join("main");
    init_repo(&main);
    git(&main, &["config", "worktree.useRelativePaths", "true"]);
    git(&main, &["worktree", "add", "-q", "../rel"]);
    let text = fs::read_to_string(root.join("rel/.git")).unwrap();
    assert!(
        text.starts_with("gitdir: ../"),
        "expected a relative gitdir, got {text:?}"
    );

    let out = discover(&opts(&[&root], &[])).unwrap();
    let rel = out
        .projects
        .iter()
        .find(|p| p.path.ends_with("rel"))
        .unwrap();
    assert_eq!(rel.kind, Kind::Worktree);
    assert_eq!(rel.parent.as_deref(), Some(main.as_path()));
}

#[test]
fn a_separate_git_dir_is_a_repo() {
    let (_guard, root) = root();
    let work = root.join("work");
    fs::create_dir(&work).unwrap();
    git(&work, &["init", "-q", "--separate-git-dir", "../sgd.git"]);
    assert_eq!(classify(&work), Marker::Repo);
    let out = discover(&opts(&[&root], &[])).unwrap();
    assert_eq!(listing(&out, &root), [("work".into(), Kind::Repo)]);
}

#[test]
fn a_submodule_is_skipped_and_its_worktree_is_not_a_repo() {
    let (guard, root) = root();
    let sub = guard.path().join("sub");
    init_repo(&sub);
    let main = root.join("main");
    init_repo(&main);
    git(
        &main,
        &[
            "submodule",
            "add",
            "-q",
            &sub.display().to_string(),
            "deps/sm",
        ],
    );

    assert_eq!(classify(&main.join("deps/sm")), Marker::Skip("submodule"));
    let out = discover(&opts(&[&root], &[])).unwrap();
    assert_eq!(listing(&out, &root), [("main".into(), Kind::Repo)]);

    let smwt = guard.path().join("smwt");
    git(
        &main.join("deps/sm"),
        &["worktree", "add", "-q", &smwt.display().to_string()],
    );
    assert_eq!(classify(&smwt), Marker::Worktree(None));
}

#[test]
fn a_bare_repos_worktree_has_no_parent() {
    let (guard, root) = root();
    let seed = guard.path().join("seed");
    init_repo(&seed);
    git(
        guard.path(),
        &[
            "clone",
            "-q",
            "--bare",
            &seed.display().to_string(),
            "root/bare.git",
        ],
    );
    git(&root.join("bare.git"), &["worktree", "add", "-q", "../bwt"]);

    assert_eq!(classify(&root.join("bwt")), Marker::Worktree(None));
    let out = discover(&opts(&[&root], &[])).unwrap();
    assert_eq!(listing(&out, &root), [("bwt".into(), Kind::Worktree)]);
}

#[test]
fn a_moved_worktree_no_longer_points_back_and_is_treated_as_a_repo() {
    let (_guard, root) = root();
    let main = root.join("main");
    init_repo(&main);
    git(&main, &["worktree", "add", "-q", "../before"]);
    fs::rename(root.join("before"), root.join("after")).unwrap();

    assert_eq!(classify(&root.join("after")), Marker::Repo);
    let out = discover(&opts(&[&root], &[])).unwrap();
    assert_eq!(
        listing(&out, &root),
        [("after".into(), Kind::Repo), ("main".into(), Kind::Repo)]
    );
}

#[test]
fn a_broken_git_file_is_skipped() {
    let (_guard, root) = root();
    let dir = root.join("broken");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join(".git"), "gitdir: /nonexistent/place\n").unwrap();
    assert_eq!(classify(&dir), Marker::Skip("broken .git file"));
    assert!(discover(&opts(&[&root], &[])).unwrap().projects.is_empty());
}

#[test]
fn a_git_dir_without_head_is_not_a_marker() {
    let (_guard, root) = root();
    fs::create_dir_all(root.join("fake/.git")).unwrap();
    init_repo(&root.join("fake/real"));
    let out = discover(&opts(&[&root], &[])).unwrap();
    assert_eq!(listing(&out, &root), [("fake/real".into(), Kind::Repo)]);
}

#[test]
fn jj_colocated_repo_and_workspace() {
    if !jj_available() {
        assert!(
            std::env::var_os("ZOP_REQUIRE_JJ").is_none(),
            "ZOP_REQUIRE_JJ is set but jj is not installed"
        );
        eprintln!("jj not installed; skipping");
        return;
    }
    let (_guard, root) = root();
    jj(&root, &["git", "init", "--colocate", "jmain"]);
    jj(&root.join("jmain"), &["workspace", "add", "../jws"]);

    assert_eq!(classify(&root.join("jmain")), Marker::Repo);
    assert_eq!(
        classify(&root.join("jws")),
        Marker::Worktree(Some(canon(&root.join("jmain"))))
    );

    let out = discover(&opts(&[&root], &[])).unwrap();
    assert_eq!(
        listing(&out, &root),
        [("jmain".into(), Kind::Repo), ("jws".into(), Kind::Worktree)]
    );
    let ws = out
        .projects
        .iter()
        .find(|p| p.kind == Kind::Worktree)
        .unwrap();
    assert_eq!(ws.parent.as_deref(), Some(root.join("jmain").as_path()));
}

#[test]
fn jj_repo_that_is_not_colocated() {
    if !jj_available() {
        assert!(
            std::env::var_os("ZOP_REQUIRE_JJ").is_none(),
            "ZOP_REQUIRE_JJ is set but jj is not installed"
        );
        eprintln!("jj not installed; skipping");
        return;
    }
    let (_guard, root) = root();
    jj(&root, &["git", "init", "--no-colocate", "plain"]);
    jj(&root.join("plain"), &["workspace", "add", "../plainws"]);
    assert_eq!(classify(&root.join("plain")), Marker::Repo);
    assert_eq!(
        classify(&root.join("plainws")),
        Marker::Worktree(Some(canon(&root.join("plain"))))
    );
}

#[test]
fn a_symlinked_root_keeps_its_spelling_and_dedupes_by_real_path() {
    let (guard, root) = root();
    init_repo(&root.join("proj"));
    let link = guard.path().join("link");
    std::os::unix::fs::symlink(&root, &link).unwrap();

    let out = discover(&opts(&[&link], &[])).unwrap();
    assert_eq!(out.projects.len(), 1);
    assert_eq!(out.projects[0].path, link.join("proj"));
    assert_eq!(out.projects[0].realpath, canon(&root.join("proj")));

    let both = discover(&opts(&[&link, &root], &[])).unwrap();
    assert_eq!(both.projects.len(), 1, "same real path from two roots");
    assert_eq!(
        both.projects[0].path,
        link.join("proj"),
        "the first spelling wins a tie"
    );
}

#[test]
fn symlinks_inside_a_root_are_not_followed() {
    let (guard, root) = root();
    let outside = guard.path().join("outside");
    init_repo(&outside.join("secret"));
    std::os::unix::fs::symlink(&outside, root.join("portal")).unwrap();
    assert!(discover(&opts(&[&root], &[])).unwrap().projects.is_empty());
}

#[test]
fn add_directory_with_a_marker_is_classified_and_without_is_extra() {
    let (guard, root) = root();
    let repo = guard.path().join("somerepo");
    init_repo(&repo);
    git(&repo, &["worktree", "add", "-q", "../somewt"]);
    let vault = guard.path().join("vault");
    fs::create_dir(&vault).unwrap();

    let out = discover(&opts(&[&root], &[&repo, &vault])).unwrap();
    // The worktree comes from git's own record, so it keeps git's spelling of the path.
    let base = canon(guard.path());
    let mut rows: Vec<(String, Kind)> = out
        .projects
        .iter()
        .map(|p| {
            (
                p.realpath
                    .strip_prefix(&base)
                    .unwrap()
                    .display()
                    .to_string(),
                p.kind,
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        rows,
        [
            ("somerepo".into(), Kind::Repo),
            ("somewt".into(), Kind::Worktree),
            ("vault".into(), Kind::Extra),
        ]
    );
}

#[test]
fn add_file_reads_lines_and_warns_about_bad_ones() {
    let (guard, root) = root();
    let dir_a = guard.path().join("a");
    fs::create_dir(&dir_a).unwrap();
    let list = guard.path().join("extras.txt");
    fs::write(
        &list,
        "# a comment\n\n./a\n/definitely/not/here\n$ZOP_SURELY_UNSET_VAR/x\n",
    )
    .unwrap();

    let out = discover(&opts(&[&root], &[&list])).unwrap();
    assert_eq!(listing(&out, guard.path()), [("a".into(), Kind::Extra)]);
    assert_eq!(out.warnings.len(), 2, "{:?}", out.warnings);
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("ZOP_SURELY_UNSET_VAR"))
    );
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("/definitely/not/here"))
    );
}

#[test]
fn a_root_find_beats_an_add_of_the_same_directory() {
    let (_guard, root) = root();
    let repo = root.join("proj");
    init_repo(&repo);
    let out = discover(&opts(&[&root], &[&repo])).unwrap();
    assert_eq!(out.projects.len(), 1);
    assert_eq!(out.projects[0].kind, Kind::Repo);
}

#[test]
fn a_missing_add_path_warns_and_is_skipped() {
    let (guard, root) = root();
    let out = discover(&opts(&[&root], &[&guard.path().join("nope")])).unwrap();
    assert!(out.projects.is_empty());
    assert_eq!(out.warnings.len(), 1);
}
