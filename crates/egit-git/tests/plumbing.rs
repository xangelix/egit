//! A repository's plumbing against sandboxes: work trees listed (the main one, a branch's, a
//! detached one, locked, prunable), removed (refused with changes or while locked), locked,
//! unlocked, and pruned; remotes added (a taken name refused), renamed (their branches and
//! followers along), pointed elsewhere for fetching and pushing, fetched alone, and removed;
//! submodules listed (set up here or not) and brought to their recorded commits; and where a
//! walk starts with some branches hidden.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::{Path, PathBuf};

use bstr::{BStr, BString};
use egit_git::testing::Sandbox;
use egit_git::{
    CancelToken, ObjectId, Progress, RemoveWorktreeError, Repository, add_remote, fetch_remote,
    history_tips, lock_worktree, prune_worktrees, remove_remote, remove_worktree, rename_remote,
    set_remote_url, submodules, unlock_worktree, update_submodules, worktrees,
};

fn open(sandbox: &Sandbox, path: &Path) -> Repository {
    Repository::discover(sandbox.git(), path).unwrap()
}

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> Option<ObjectId> {
    let out = sandbox
        .run(repo, "rev-parse", &["--verify", "--quiet", name])
        .ok()?;
    ObjectId::from_hex(out.stdout.trim_ascii())
}

fn says(sandbox: &Sandbox, repo: &Path, subcommand: &'static str, args: &[&str]) -> String {
    let out = sandbox.run(repo, subcommand, args).unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// Git's own name for a folder (the sandbox's temporary folder may be reached by a link).
fn real(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}

#[test]
fn worktrees_are_listed_locked_removed_and_pruned() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "one\n", "One").unwrap();
    sandbox.run(&repo, "branch", &["side"]).unwrap();
    let (beside, detached) = (sandbox.path("beside"), sandbox.path("detached"));
    let path = |path: &Path| path.to_str().unwrap().to_owned();
    sandbox
        .run(
            &repo,
            "worktree",
            &["add", "--quiet", &path(&beside), "side"],
        )
        .unwrap();
    sandbox
        .run(
            &repo,
            "worktree",
            &["add", "--quiet", "--detach", &path(&detached), "HEAD"],
        )
        .unwrap();
    let repository = open(&sandbox, &repo);
    let cancel = CancelToken::new();
    let list = || worktrees(sandbox.git(), &repository).unwrap();

    let listed = list();
    assert_eq!(listed.len(), 3);
    assert!(listed[0].main && !listed[1].main && !listed[2].main);
    assert_eq!(real(&listed[0].path), real(&repo));
    assert_eq!(listed[0].branch, Some(BString::from("refs/heads/main")));
    assert_eq!(real(&listed[1].path), real(&beside));
    assert_eq!(listed[1].branch, Some(BString::from("refs/heads/side")));
    assert_eq!(listed[2].branch, None, "detached");
    assert_eq!(listed[2].head, rev(&sandbox, &repo, "HEAD"));
    assert!(listed.iter().all(|worktree| !worktree.bare));

    // Locked: not removed, until unlocked.
    lock_worktree(sandbox.git(), &repository, &beside, "on a drive", &cancel).unwrap();
    assert_eq!(list()[1].locked, Some(BString::from("on a drive")));
    assert!(matches!(
        remove_worktree(sandbox.git(), &repository, &beside, false, &cancel),
        Err(RemoveWorktreeError::Locked)
    ));
    unlock_worktree(sandbox.git(), &repository, &beside, &cancel).unwrap();
    assert_eq!(list()[1].locked, None);

    // Changes in it: removed only when forced.
    std::fs::write(beside.join("f"), "changed\n").unwrap();
    assert!(matches!(
        remove_worktree(sandbox.git(), &repository, &beside, false, &cancel),
        Err(RemoveWorktreeError::Changes)
    ));
    assert!(beside.exists());
    remove_worktree(sandbox.git(), &repository, &beside, true, &cancel).unwrap();
    assert!(!beside.exists());
    assert_eq!(list().len(), 2);

    // Its folder gone: prunable, then pruned.
    std::fs::remove_dir_all(&detached).unwrap();
    assert!(list()[1].prunable.is_some());
    prune_worktrees(sandbox.git(), &repository, &cancel).unwrap();
    assert_eq!(list().len(), 1);
}

#[test]
fn remotes_are_added_renamed_pointed_elsewhere_fetched_and_removed() {
    let sandbox = Sandbox::new().unwrap();
    let upstream = sandbox.init("upstream").unwrap();
    sandbox.commit_file(&upstream, "f", "one\n", "One").unwrap();
    let other = sandbox.init("other").unwrap();
    sandbox.commit_file(&other, "g", "g\n", "G").unwrap();
    let repo = sandbox.init("repo").unwrap();
    let repository = open(&sandbox, &repo);
    let cancel = CancelToken::new();
    let url = |path: &Path| format!("file://{}", path.display());
    let config = |key: &str| {
        sandbox
            .run(&repo, "config", &["--get", key])
            .ok()
            .map(|out| String::from_utf8(out.stdout).unwrap().trim().to_owned())
    };

    add_remote(
        sandbox.git(),
        &repository,
        "origin",
        &url(&upstream),
        &cancel,
    )
    .unwrap();
    assert!(
        add_remote(sandbox.git(), &repository, "origin", &url(&other), &cancel).is_err(),
        "taken"
    );
    add_remote(sandbox.git(), &repository, "other", &url(&other), &cancel).unwrap();
    assert_eq!(says(&sandbox, &repo, "remote", &[]), "origin\nother");

    // Fetched alone: the other remote's branches stay unfetched.
    let progress = Progress::new();
    let origin = BStr::new("origin");
    fetch_remote(
        sandbox.git(),
        &repository,
        origin,
        false,
        &progress,
        &cancel,
    )
    .unwrap();
    assert_eq!(
        rev(&sandbox, &repo, "refs/remotes/origin/main"),
        rev(&sandbox, &upstream, "main")
    );
    assert_eq!(rev(&sandbox, &repo, "refs/remotes/other/main"), None);

    // A branch following it follows it renamed; its branches go along.
    sandbox
        .run(&repo, "switch", &["--quiet", "--track", "origin/main"])
        .unwrap();
    rename_remote(sandbox.git(), &repository, origin, "upstream", &cancel).unwrap();
    assert_eq!(rev(&sandbox, &repo, "refs/remotes/origin/main"), None);
    assert_eq!(
        rev(&sandbox, &repo, "refs/remotes/upstream/main"),
        rev(&sandbox, &upstream, "main")
    );
    assert_eq!(config("branch.main.remote").as_deref(), Some("upstream"));

    // Pointed elsewhere, for fetching and for pushing.
    let renamed = BStr::new("upstream");
    set_remote_url(
        sandbox.git(),
        &repository,
        renamed,
        &url(&other),
        false,
        &cancel,
    )
    .unwrap();
    assert_eq!(config("remote.upstream.url"), Some(url(&other)));
    set_remote_url(
        sandbox.git(),
        &repository,
        renamed,
        &url(&upstream),
        true,
        &cancel,
    )
    .unwrap();
    assert_eq!(config("remote.upstream.pushurl"), Some(url(&upstream)));
    assert_eq!(config("remote.upstream.url"), Some(url(&other)));

    // Removed, with its branches.
    let removed = BStr::new("other");
    fetch_remote(
        sandbox.git(),
        &repository,
        removed,
        false,
        &progress,
        &cancel,
    )
    .unwrap();
    remove_remote(sandbox.git(), &repository, removed, &cancel).unwrap();
    assert_eq!(rev(&sandbox, &repo, "refs/remotes/other/main"), None);
    assert_eq!(says(&sandbox, &repo, "remote", &[]), "upstream");
    assert!(remove_remote(sandbox.git(), &repository, removed, &cancel).is_err());
}

#[test]
fn a_remote_fetched_alone_is_pruned_when_asked() {
    let sandbox = Sandbox::new().unwrap();
    let other = sandbox.init("other").unwrap();
    sandbox.commit_file(&other, "g", "g\n", "G").unwrap();
    sandbox.run(&other, "branch", &["gone"]).unwrap();
    let repo = sandbox.init("repo").unwrap();
    let repository = open(&sandbox, &repo);
    let (cancel, progress) = (CancelToken::new(), Progress::new());
    let url = format!("file://{}", other.display());
    add_remote(sandbox.git(), &repository, "other", &url, &cancel).unwrap();
    let remote = BStr::new("other");
    fetch_remote(
        sandbox.git(),
        &repository,
        remote,
        false,
        &progress,
        &cancel,
    )
    .unwrap();
    assert!(rev(&sandbox, &repo, "refs/remotes/other/gone").is_some());
    sandbox.run(&other, "branch", &["-D", "gone"]).unwrap();
    fetch_remote(
        sandbox.git(),
        &repository,
        remote,
        false,
        &progress,
        &cancel,
    )
    .unwrap();
    assert!(
        rev(&sandbox, &repo, "refs/remotes/other/gone").is_some(),
        "kept unless pruned"
    );
    fetch_remote(sandbox.git(), &repository, remote, true, &progress, &cancel).unwrap();
    assert_eq!(rev(&sandbox, &repo, "refs/remotes/other/gone"), None);
    assert!(rev(&sandbox, &repo, "refs/remotes/other/main").is_some());
}

#[test]
fn submodules_are_listed_and_brought_to_their_commits() {
    let sandbox = Sandbox::new().unwrap();
    // Submodules over `file://` are refused unless allowed (git 2.38.1 on).
    sandbox
        .run(
            sandbox.home(),
            "config",
            &["--global", "protocol.file.allow", "always"],
        )
        .unwrap();
    let library = sandbox.init("library").unwrap();
    sandbox
        .commit_file(&library, "lib", "one\n", "One")
        .unwrap();
    let first = rev(&sandbox, &library, "HEAD").unwrap();
    sandbox
        .commit_file(&library, "lib", "two\n", "Two")
        .unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "f\n", "Start").unwrap();
    let url = format!("file://{}", library.display());
    sandbox
        .run(
            &repo,
            "submodule",
            &["--quiet", "add", "--name", "lib", &url, "vendor/lib one"],
        )
        .unwrap();
    // Recorded at the first commit.
    let inside = repo.join("vendor/lib one");
    sandbox
        .run(&inside, "checkout", &["--quiet", &first.to_string()])
        .unwrap();
    sandbox.run(&repo, "add", &["vendor/lib one"]).unwrap();
    sandbox
        .run(
            &repo,
            "commit",
            &["--quiet", "--message", "Add the library"],
        )
        .unwrap();
    let repository = open(&sandbox, &repo);
    let listed = submodules(sandbox.git(), &repository).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "lib");
    assert_eq!(listed[0].path, "vendor/lib one");
    assert_eq!(listed[0].url.as_ref().map(ToString::to_string), Some(url));
    assert_eq!(listed[0].recorded, first);
    assert!(listed[0].initialized);

    // A clone has it, not set up yet; brought to the commit recorded.
    let clone = sandbox.path("clone");
    sandbox
        .run(
            sandbox.home(),
            "clone",
            &["--quiet", repo.to_str().unwrap(), clone.to_str().unwrap()],
        )
        .unwrap();
    let cloned = open(&sandbox, &clone);
    let listed = submodules(sandbox.git(), &cloned).unwrap();
    assert!(!listed[0].initialized);
    assert!(!clone.join("vendor/lib one/lib").exists());
    let cancel = CancelToken::new();
    update_submodules(
        sandbox.git(),
        &cloned,
        &[listed[0].path.as_ref()],
        &Progress::new(),
        &cancel,
    )
    .unwrap();
    assert!(submodules(sandbox.git(), &cloned).unwrap()[0].initialized);
    assert_eq!(
        std::fs::read_to_string(clone.join("vendor/lib one/lib")).unwrap(),
        "one\n"
    );

    // None at all.
    let plain = sandbox.init("plain").unwrap();
    sandbox.commit_file(&plain, "f", "f\n", "Start").unwrap();
    assert_eq!(
        submodules(sandbox.git(), &open(&sandbox, &plain)).unwrap(),
        Vec::new()
    );
}

#[test]
fn a_walk_with_branches_hidden_starts_from_the_others() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "one\n", "One").unwrap();
    for branch in ["left", "right"] {
        sandbox
            .run(&repo, "switch", &["--quiet", "--create", branch, "main"])
            .unwrap();
        sandbox.commit_file(&repo, branch, "x\n", branch).unwrap();
    }
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    let repository = open(&sandbox, &repo);
    let cancel = CancelToken::new();
    let tips = |hidden: &[&str]| {
        let hidden: Vec<BString> = hidden.iter().map(|name| BString::from(*name)).collect();
        let mut tips = history_tips(sandbox.git(), &repository, &hidden, &cancel).unwrap();
        tips.sort_unstable();
        tips
    };
    let ids = |names: &[&str]| {
        let mut ids: Vec<ObjectId> = names
            .iter()
            .map(|name| rev(&sandbox, &repo, name).unwrap())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    };
    assert_eq!(tips(&[]), ids(&["main", "left", "right"]));
    assert_eq!(tips(&["refs/heads/left"]), ids(&["main", "right"]));
    // `HEAD` stays, whatever its branch.
    assert_eq!(
        tips(&["refs/heads/main", "refs/heads/right"]),
        ids(&["main", "left"])
    );
}
