//! What a branch's menu does beyond the graph's actions, against sandboxed repositories:
//! branches fast-forwarded (the current one, another from its remote, another from a ref
//! here; refused when they diverged, or checked out), upstreams set and forgotten, work trees
//! added (a branch, a new branch tracking a remote's, a commit), and a commit's patch, which
//! `git am` applies elsewhere.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::{Path, PathBuf};

use bstr::BStr;
use egit_git::testing::Sandbox;
use egit_git::{
    CancelToken, ForwardFrom, Forwarded, ObjectId, Progress, Repository, WorktreeFrom,
    add_worktree, fast_forward_branch, fast_forward_current, format_patch, set_upstream,
};

fn open(sandbox: &Sandbox, path: &Path) -> Repository {
    Repository::discover(sandbox.git(), path).unwrap()
}

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> ObjectId {
    let out = sandbox.run(repo, "rev-parse", &[name]).unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

fn config(sandbox: &Sandbox, repo: &Path, key: &str) -> Option<String> {
    sandbox
        .run(repo, "config", &["--get", key])
        .ok()
        .map(|out| String::from_utf8(out.stdout).unwrap().trim().to_owned())
}

/// An origin with `main` (two commits) and a clone of it, whose `behind` branch is at the
/// first commit; then the origin's `main` gains a third.
fn cloned(sandbox: &Sandbox) -> (PathBuf, PathBuf) {
    let origin = sandbox.init("origin").unwrap();
    sandbox.commit_file(&origin, "f", "one\n", "One").unwrap();
    sandbox.commit_file(&origin, "f", "two\n", "Two").unwrap();
    let repo = sandbox.path("repo");
    sandbox
        .run(
            sandbox.home(),
            "clone",
            &["--quiet", origin.to_str().unwrap(), repo.to_str().unwrap()],
        )
        .unwrap();
    sandbox.run(&repo, "branch", &["behind", "HEAD~1"]).unwrap();
    sandbox
        .commit_file(&origin, "f", "three\n", "Three")
        .unwrap();
    (origin, repo)
}

#[test]
fn branches_move_forward_only() {
    let sandbox = Sandbox::new().unwrap();
    let (origin, repo) = cloned(&sandbox);
    let repository = open(&sandbox, &repo);
    let (git, progress, cancel) = (sandbox.git(), Progress::new(), CancelToken::new());
    let forward = |branch: &str, from| {
        fast_forward_branch(
            git,
            &repository,
            BStr::new(branch),
            from,
            &progress,
            &cancel,
        )
    };
    let origin_main = ForwardFrom::Remote {
        remote: BStr::new("origin"),
        branch: BStr::new("main"),
    };

    // A branch that isn't checked out, from the remote: to its third commit.
    assert_eq!(forward("behind", origin_main).unwrap(), Forwarded::Moved);
    assert_eq!(
        rev(&sandbox, &repo, "behind"),
        rev(&sandbox, &origin, "main")
    );
    assert_eq!(
        forward("behind", origin_main).unwrap(),
        Forwarded::AlreadyThere
    );
    // The remote-tracking branch came along.
    assert_eq!(
        rev(&sandbox, &repo, "origin/main"),
        rev(&sandbox, &origin, "main")
    );

    // The current branch, to a ref here; then a branch that isn't, from a ref here.
    assert_eq!(
        fast_forward_current(
            git,
            &repository,
            BStr::new("refs/remotes/origin/main"),
            &cancel
        )
        .unwrap(),
        Forwarded::Moved
    );
    assert_eq!(rev(&sandbox, &repo, "main"), rev(&sandbox, &origin, "main"));
    assert_eq!(
        fast_forward_current(git, &repository, BStr::new("refs/heads/behind"), &cancel).unwrap(),
        Forwarded::AlreadyThere
    );
    sandbox
        .run(&repo, "branch", &["-f", "behind", "HEAD~2"])
        .unwrap();
    assert_eq!(
        forward("behind", ForwardFrom::Here(BStr::new("refs/heads/main"))).unwrap(),
        Forwarded::Moved
    );

    // Diverged: nothing moves.
    sandbox
        .run(&repo, "switch", &["--quiet", "behind"])
        .unwrap();
    sandbox.commit_file(&repo, "g", "mine\n", "Mine").unwrap();
    let mine = rev(&sandbox, &repo, "behind");
    sandbox.run(&repo, "switch", &["--quiet", "main"]).unwrap();
    sandbox.commit_file(&origin, "f", "four\n", "Four").unwrap();
    assert_eq!(
        forward("behind", origin_main).unwrap(),
        Forwarded::NotFastForward
    );
    assert_eq!(rev(&sandbox, &repo, "behind"), mine);
    // The current branch, once it has a commit of its own too.
    sandbox.commit_file(&repo, "h", "ours\n", "Ours").unwrap();
    let ours = rev(&sandbox, &repo, "main");
    assert_eq!(
        fast_forward_current(git, &repository, BStr::new("refs/heads/behind"), &cancel).unwrap(),
        Forwarded::NotFastForward
    );
    assert_eq!(rev(&sandbox, &repo, "main"), ours);
    // The branch checked out is git's to refuse.
    assert!(forward("main", origin_main).is_err());
}

#[test]
fn upstreams_are_set_and_forgotten() {
    let sandbox = Sandbox::new().unwrap();
    let (_, repo) = cloned(&sandbox);
    let repository = open(&sandbox, &repo);
    let (git, cancel) = (sandbox.git(), CancelToken::new());
    assert_eq!(config(&sandbox, &repo, "branch.behind.merge"), None);
    set_upstream(
        git,
        &repository,
        BStr::new("behind"),
        Some(BStr::new("origin/main")),
        &cancel,
    )
    .unwrap();
    assert_eq!(
        config(&sandbox, &repo, "branch.behind.remote").as_deref(),
        Some("origin")
    );
    assert_eq!(
        config(&sandbox, &repo, "branch.behind.merge").as_deref(),
        Some("refs/heads/main")
    );
    set_upstream(git, &repository, BStr::new("behind"), None, &cancel).unwrap();
    assert_eq!(config(&sandbox, &repo, "branch.behind.merge"), None);
    // Only a branch git has.
    assert!(
        set_upstream(
            git,
            &repository,
            BStr::new("behind"),
            Some(BStr::new("origin/nowhere")),
            &cancel
        )
        .is_err()
    );
}

#[test]
fn work_trees_check_out_a_branch_a_new_branch_or_a_commit() {
    let sandbox = Sandbox::new().unwrap();
    let (_, repo) = cloned(&sandbox);
    let repository = open(&sandbox, &repo);
    let (git, cancel) = (sandbox.git(), CancelToken::new());
    let head = |path: &Path| {
        let out = sandbox
            .run(path, "rev-parse", &["--abbrev-ref", "HEAD"])
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };

    let behind = sandbox.path("behind tree");
    add_worktree(
        git,
        &repository,
        &behind,
        WorktreeFrom::Branch(BStr::new("behind")),
        &cancel,
    )
    .unwrap();
    assert_eq!(head(&behind), "behind");
    assert!(behind.join("f").exists());

    let tracking = sandbox.path("tracking");
    add_worktree(
        git,
        &repository,
        &tracking,
        WorktreeFrom::NewBranch {
            name: BStr::new("topic"),
            at: BStr::new("refs/remotes/origin/main"),
            track: true,
        },
        &cancel,
    )
    .unwrap();
    assert_eq!(head(&tracking), "topic");
    assert_eq!(
        config(&sandbox, &repo, "branch.topic.merge").as_deref(),
        Some("refs/heads/main")
    );

    let detached = sandbox.path("detached");
    let first = rev(&sandbox, &repo, "main~1");
    add_worktree(
        git,
        &repository,
        &detached,
        WorktreeFrom::Detached(first),
        &cancel,
    )
    .unwrap();
    assert_eq!(head(&detached), "HEAD");
    assert_eq!(rev(&sandbox, &detached, "HEAD"), first);

    // A branch checked out elsewhere is refused.
    assert!(
        add_worktree(
            git,
            &repository,
            &sandbox.path("again"),
            WorktreeFrom::Branch(BStr::new("behind")),
            &cancel,
        )
        .is_err()
    );
}

#[test]
fn a_commit_s_patch_applies_elsewhere() {
    let sandbox = Sandbox::new().unwrap();
    let (_, repo) = cloned(&sandbox);
    // `diff.noprefix` doesn't change the patch.
    sandbox
        .run(&repo, "config", &["diff.noprefix", "true"])
        .unwrap();
    sandbox
        .commit_file(&repo, "dir/new", "new\n", "Add a file\n\nIn a folder.")
        .unwrap();
    let repository = open(&sandbox, &repo);
    let commit = rev(&sandbox, &repo, "HEAD");
    let patch = format_patch(sandbox.git(), &repository, commit, &CancelToken::new()).unwrap();
    let text = String::from_utf8(patch.clone()).unwrap();
    assert!(text.contains("Subject: [PATCH] Add a file"), "{text}");
    assert!(text.contains("+++ b/dir/new"), "{text}");

    // Applied to a clone at the commit's parent, it makes the same tree.
    let other = sandbox.path("other");
    sandbox
        .run(
            sandbox.home(),
            "clone",
            &["--quiet", repo.to_str().unwrap(), other.to_str().unwrap()],
        )
        .unwrap();
    sandbox
        .run(&other, "reset", &["--quiet", "--hard", "HEAD~1"])
        .unwrap();
    let file = sandbox.path("change.patch");
    std::fs::write(&file, &patch).unwrap();
    sandbox
        .run(&other, "am", &["--quiet", file.to_str().unwrap()])
        .unwrap();
    assert_eq!(
        rev(&sandbox, &other, "HEAD^{tree}"),
        rev(&sandbox, &repo, "HEAD^{tree}")
    );

    // The first commit too.
    let root = rev(&sandbox, &repo, "HEAD~2");
    let patch = format_patch(sandbox.git(), &repository, root, &CancelToken::new()).unwrap();
    assert!(
        String::from_utf8(patch)
            .unwrap()
            .contains("Subject: [PATCH] One")
    );
}
