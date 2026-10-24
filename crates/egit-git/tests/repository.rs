//! Discovering repositories, reading `HEAD`, and noticing operations in progress.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;

use egit_git::testing::Sandbox;
use egit_git::{
    DiscoverError, Head, ObjectFormat, Operation, Repository, Tracking, Wait as _, init_repository,
    trust_directory,
};

fn sandbox() -> Sandbox {
    Sandbox::new().expect("a sandbox")
}

#[test]
fn discovers_a_repository_from_anywhere_inside_it() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    let deep = repo.join("a/b/c");
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("file.txt"), "text").unwrap();

    for start in [repo.clone(), deep.clone(), deep.join("file.txt")] {
        let found = Repository::discover(sandbox.git(), &start).wait().unwrap();
        assert_eq!(found.work_tree(), repo);
        assert_eq!(found.git_dir(), repo.join(".git"));
        assert_eq!(found.common_dir(), repo.join(".git"));
        assert_eq!(found.object_format(), ObjectFormat::Sha1);
        assert!(!found.is_linked_worktree());
    }
}

#[test]
fn a_linked_worktree_has_its_own_git_dir() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "file", "one\n", "first")
        .unwrap();
    let linked = sandbox.path("linked");
    sandbox
        .run(
            &repo,
            "worktree",
            &["add", "--quiet", linked.to_str().unwrap()],
        )
        .unwrap();

    let found = Repository::discover(sandbox.git(), &linked).wait().unwrap();
    assert_eq!(found.work_tree(), linked);
    assert_eq!(found.common_dir(), repo.join(".git"));
    assert_eq!(found.git_dir(), repo.join(".git/worktrees/linked"));
    assert!(found.is_linked_worktree());
}

#[test]
fn reads_sha256_repositories() {
    let sandbox = sandbox();
    let repo = sandbox.path("repo");
    fs::create_dir(&repo).unwrap();
    sandbox
        .run(&repo, "init", &["--quiet", "--object-format=sha256"])
        .unwrap();
    sandbox
        .commit_file(&repo, "file", "one\n", "first")
        .unwrap();

    let found = Repository::discover(sandbox.git(), &repo).wait().unwrap();
    assert_eq!(found.object_format(), ObjectFormat::Sha256);
    let Head::Branch(branch) = found.head(sandbox.git()).wait().unwrap() else {
        panic!("not on a branch");
    };
    assert_eq!(branch.commit.unwrap().id.format(), ObjectFormat::Sha256);
}

#[test]
fn outside_any_repository() {
    let sandbox = sandbox();
    let elsewhere = sandbox.path("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    let error = Repository::discover(sandbox.git(), &elsewhere)
        .wait()
        .unwrap_err();
    assert!(
        matches!(&error, DiscoverError::NotARepository { path } if *path == elsewhere),
        "{error:?}"
    );
}

#[test]
fn a_folder_outside_any_repository_is_made_one() {
    let sandbox = sandbox();
    let folder = sandbox.path("new project");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("notes.txt"), "kept\n").unwrap();
    init_repository(sandbox.git(), &folder).wait().unwrap();
    let found = Repository::discover(sandbox.git(), &folder).wait().unwrap();
    assert_eq!(found.work_tree(), folder);
    let head = found.head_ref(sandbox.git()).wait().unwrap();
    assert_eq!(head.commit, None);
    assert!(head.reference.is_some());
    assert_eq!(
        fs::read_to_string(folder.join("notes.txt")).unwrap(),
        "kept\n"
    );
    // A file isn't a folder to make one in.
    assert!(
        init_repository(sandbox.git(), &folder.join("notes.txt"))
            .wait()
            .is_err()
    );
}

#[test]
fn refuses_bare_repositories_and_git_dirs() {
    let sandbox = sandbox();
    let bare = sandbox.path("bare.git");
    fs::create_dir(&bare).unwrap();
    sandbox.run(&bare, "init", &["--quiet", "--bare"]).unwrap();
    let error = Repository::discover(sandbox.git(), &bare)
        .wait()
        .unwrap_err();
    assert!(matches!(error, DiscoverError::Bare { .. }), "{error:?}");

    let repo = sandbox.init("repo").unwrap();
    let error = Repository::discover(sandbox.git(), &repo.join(".git"))
        .wait()
        .unwrap_err();
    assert!(
        matches!(error, DiscoverError::InsideGitDir { .. }),
        "{error:?}"
    );
}

#[test]
fn a_repository_another_user_owns_opens_once_trusted() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    let suspicious = sandbox
        .git_with([("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1")])
        .unwrap();

    let error = Repository::discover(&suspicious, &repo).wait().unwrap_err();
    let DiscoverError::DubiousOwnership { path } = error else {
        panic!("{error:?}");
    };
    assert_eq!(path, repo);

    trust_directory(sandbox.git(), &path).wait().unwrap();
    let found = Repository::discover(&suspicious, &repo).wait().unwrap();
    assert_eq!(found.work_tree(), repo);
}

#[test]
fn head_on_a_branch_without_commits() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    let found = Repository::discover(sandbox.git(), &repo).wait().unwrap();
    let Head::Branch(branch) = found.head(sandbox.git()).wait().unwrap() else {
        panic!("not on a branch");
    };
    assert_eq!(branch.name, "main");
    assert_eq!(branch.reference, "refs/heads/main");
    assert_eq!(branch.commit, None);
    assert_eq!(branch.upstream, None);
}

#[test]
fn head_on_a_branch_with_commits() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "file", "one\n", "First commit")
        .unwrap();
    sandbox
        .commit_file(&repo, "file", "two\n", "Second commit\n\nWith a body.")
        .unwrap();
    // A branch whose name extends HEAD's must not be mistaken for it.
    sandbox.run(&repo, "branch", &["main-other"]).unwrap();

    let found = Repository::discover(sandbox.git(), &repo).wait().unwrap();
    let Head::Branch(branch) = found.head(sandbox.git()).wait().unwrap() else {
        panic!("not on a branch");
    };
    assert_eq!(branch.name, "main");
    let commit = branch.commit.unwrap();
    assert_eq!(commit.subject, "Second commit");
    let rev = sandbox.run(&repo, "rev-parse", &["HEAD"]).unwrap();
    assert_eq!(format!("{}\n", commit.id).as_bytes(), rev.stdout);
}

#[test]
fn head_detached() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "file", "one\n", "First")
        .unwrap();
    sandbox
        .commit_file(&repo, "file", "two\n", "Second")
        .unwrap();
    sandbox
        .run(&repo, "checkout", &["--quiet", "--detach", "HEAD~1"])
        .unwrap();

    let found = Repository::discover(sandbox.git(), &repo).wait().unwrap();
    let Head::Detached(commit) = found.head(sandbox.git()).wait().unwrap() else {
        panic!("not detached");
    };
    assert_eq!(commit.subject, "First");
}

#[test]
fn head_tracks_its_upstream() {
    let sandbox = sandbox();
    let remote = sandbox.path("remote.git");
    fs::create_dir(&remote).unwrap();
    sandbox
        .run(&remote, "init", &["--quiet", "--bare"])
        .unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "file", "one\n", "First")
        .unwrap();
    sandbox
        .run(
            &repo,
            "remote",
            &["add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
    sandbox
        .run(
            &repo,
            "push",
            &["--quiet", "--set-upstream", "origin", "main"],
        )
        .unwrap();
    let found = Repository::discover(sandbox.git(), &repo).wait().unwrap();

    let upstream_of = |found: &Repository| match found.head(sandbox.git()).wait().unwrap() {
        Head::Branch(branch) => branch.upstream.unwrap(),
        Head::Detached(_) => panic!("not on a branch"),
    };
    let upstream = upstream_of(&found);
    assert_eq!(upstream.name, "origin/main");
    assert_eq!(
        upstream.tracking,
        Tracking::Diverged {
            ahead: 0,
            behind: 0
        }
    );

    sandbox
        .commit_file(&repo, "file", "two\n", "Second")
        .unwrap();
    sandbox
        .commit_file(&repo, "file", "three\n", "Third")
        .unwrap();
    assert_eq!(
        upstream_of(&found).tracking,
        Tracking::Diverged {
            ahead: 2,
            behind: 0
        }
    );

    sandbox
        .run(&repo, "update-ref", &["-d", "refs/remotes/origin/main"])
        .unwrap();
    assert_eq!(upstream_of(&found).tracking, Tracking::Gone);
}

#[test]
fn notices_operations_stopped_on_conflicts() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "file", "base\n", "Base")
        .unwrap();
    sandbox.run(&repo, "branch", &["other"]).unwrap();
    sandbox
        .commit_file(&repo, "file", "ours\n", "Ours")
        .unwrap();
    sandbox
        .run(&repo, "checkout", &["--quiet", "other"])
        .unwrap();
    sandbox
        .commit_file(&repo, "file", "theirs\n", "Theirs")
        .unwrap();
    let found = Repository::discover(sandbox.git(), &repo).wait().unwrap();
    assert_eq!(found.operations(sandbox.git()).wait().unwrap(), []);

    let merge = sandbox
        .git()
        .write("merge")
        .arg("main")
        .current_dir(&repo)
        .output();
    assert!(merge.wait().is_err(), "the merge should stop on a conflict");
    assert_eq!(
        found.operations(sandbox.git()).wait().unwrap(),
        [Operation::Merge]
    );
    sandbox.run(&repo, "merge", &["--abort"]).unwrap();

    let pick = sandbox
        .git()
        .write("cherry-pick")
        .arg("main")
        .current_dir(&repo)
        .output();
    assert!(
        pick.wait().is_err(),
        "the cherry-pick should stop on a conflict"
    );
    assert_eq!(
        found.operations(sandbox.git()).wait().unwrap(),
        [Operation::CherryPick]
    );
    sandbox.run(&repo, "cherry-pick", &["--abort"]).unwrap();

    let rebase = sandbox
        .git()
        .write("rebase")
        .arg("main")
        .current_dir(&repo)
        .output();
    assert!(
        rebase.wait().is_err(),
        "the rebase should stop on a conflict"
    );
    assert_eq!(
        found.operations(sandbox.git()).wait().unwrap(),
        [Operation::Rebase]
    );
    sandbox.run(&repo, "rebase", &["--abort"]).unwrap();
    assert_eq!(found.operations(sandbox.git()).wait().unwrap(), []);
}
