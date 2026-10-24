//! Where refs have been, and taking moves back, against sandboxed repositories: `HEAD`'s and a
//! branch's reflog (newest first, what moved them, when), none before the first commit; where
//! `HEAD` and a ref are now; a reset keeping uncommitted changes (refused when they're in the
//! way); a dropped stash stored again; refs made and removed only where they were expected.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::Path;

use bstr::BStr;
use egit_git::testing::Sandbox;
use egit_git::{
    CancelToken, HeadRef, ObjectId, Repository, ResetMode, Wait as _, create_ref, delete_ref,
    ref_target, reflog, reset, stash_drop, stash_push, stash_store, stashes,
};

fn open(sandbox: &Sandbox, path: &Path) -> Repository {
    Repository::discover(sandbox.git(), path).wait().unwrap()
}

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> ObjectId {
    let out = sandbox.run(repo, "rev-parse", &[name]).unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

#[test]
fn reflogs_say_where_refs_have_been_newest_first() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    let repository = open(&sandbox, &repo);
    let cancel = CancelToken::new();
    let read = |reference: &str, limit| {
        reflog(
            sandbox.git(),
            &repository,
            BStr::new(reference),
            limit,
            &cancel,
        )
        .wait()
        .unwrap()
    };
    assert_eq!(read("HEAD", 100), Vec::new(), "no commit yet");

    sandbox.commit_file(&repo, "f", "one\n", "One").unwrap();
    sandbox.commit_file(&repo, "f", "two\n", "Two").unwrap();
    sandbox.run(&repo, "branch", &["topic"]).unwrap();
    sandbox
        .run(&repo, "reset", &["--quiet", "--hard", "HEAD~1"])
        .unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "topic"]).unwrap();

    let head = read("HEAD", 100);
    let actions: Vec<String> = head.iter().map(|entry| entry.action.to_string()).collect();
    assert_eq!(actions.len(), 4);
    assert_eq!(actions[0], "checkout: moving from main to topic");
    assert!(
        actions[1].starts_with("reset: moving to HEAD~1"),
        "{actions:?}"
    );
    assert_eq!(actions[2], "commit: Two");
    assert!(
        actions[3].starts_with("commit (initial): One"),
        "{actions:?}"
    );
    assert_eq!(head[2].commit, rev(&sandbox, &repo, "topic"));
    assert_eq!(head[2].subject, "Two");
    assert_eq!(
        head.iter().map(|entry| entry.index).collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    assert!(head.iter().all(|entry| entry.time > 1_000_000_000));

    // A branch's own; at most as many as asked.
    let main = read("refs/heads/main", 100);
    assert_eq!(main.len(), 3, "{main:?}");
    assert_eq!(main[0].commit, rev(&sandbox, &repo, "main"));
    assert_eq!(read("HEAD", 2).len(), 2);
    assert_eq!(read("refs/heads/nowhere", 10), Vec::new());
    assert_eq!(read("--all", 10), Vec::new(), "never an option");
}

#[test]
fn head_and_refs_are_read_where_they_are() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    let repository = open(&sandbox, &repo);
    let head = || repository.head_ref(sandbox.git()).wait().unwrap();
    let main = || Some(bstr::BString::from("refs/heads/main"));
    assert_eq!(
        head(),
        HeadRef {
            reference: main(),
            commit: None
        },
        "no commit yet"
    );
    assert_eq!(head().branch(), Some(BStr::new("main")));

    sandbox.commit_file(&repo, "f", "one\n", "One").unwrap();
    let one = rev(&sandbox, &repo, "HEAD");
    assert_eq!(
        head(),
        HeadRef {
            reference: main(),
            commit: Some(one)
        }
    );
    sandbox
        .run(&repo, "switch", &["--quiet", "--detach", "HEAD"])
        .unwrap();
    assert_eq!(
        head(),
        HeadRef {
            reference: None,
            commit: Some(one)
        }
    );
    assert_eq!(head().branch(), None);

    // A ref's own object: an annotated tag's, not its commit's.
    sandbox
        .run(&repo, "tag", &["--annotate", "--message", "First", "v1"])
        .unwrap();
    let target = |name: &str| {
        ref_target(sandbox.git(), &repository, BStr::new(name))
            .wait()
            .unwrap()
    };
    assert_eq!(
        target("refs/tags/v1"),
        Some(rev(&sandbox, &repo, "refs/tags/v1"))
    );
    assert_ne!(target("refs/tags/v1"), Some(one));
    assert_eq!(target("refs/heads/main"), Some(one));
    assert_eq!(target("HEAD"), Some(one));
    assert_eq!(target("refs/heads/nowhere"), None);
    assert_eq!(target("--all"), None, "never an option");
}

#[test]
fn a_reset_keeping_changes_moves_only_when_they_are_not_in_the_way() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "one\n", "One").unwrap();
    sandbox.commit_file(&repo, "f", "two\n", "Two").unwrap();
    sandbox.commit_file(&repo, "g", "g\n", "G").unwrap();
    let repository = open(&sandbox, &repo);
    let cancel = CancelToken::new();
    // A change to `g`, which the last commit made: in the way of going back past it.
    std::fs::write(repo.join("g"), "changed\n").unwrap();
    let first = rev(&sandbox, &repo, "HEAD~2");
    assert!(
        reset(sandbox.git(), &repository, first, ResetMode::Keep, &cancel)
            .wait()
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("g")).unwrap(),
        "changed\n"
    );
    // A change to a file the move doesn't touch stays.
    sandbox
        .run(&repo, "checkout", &["--quiet", "--", "g"])
        .unwrap();
    std::fs::write(repo.join("h"), "untracked\n").unwrap();
    let second = rev(&sandbox, &repo, "HEAD~1");
    reset(sandbox.git(), &repository, second, ResetMode::Keep, &cancel)
        .wait()
        .unwrap();
    assert_eq!(rev(&sandbox, &repo, "HEAD"), second);
    assert!(!repo.join("g").exists(), "the commit's file went with it");
    assert!(repo.join("h").exists());
}

#[test]
fn a_dropped_stash_is_stored_again() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "one\n", "One").unwrap();
    let repository = open(&sandbox, &repo);
    let cancel = CancelToken::new();
    std::fs::write(repo.join("f"), "changed\n").unwrap();
    assert!(
        stash_push(sandbox.git(), &repository, ("Work", false), &[], &cancel)
            .wait()
            .unwrap()
    );
    let stash = stashes(sandbox.git(), &repository, &cancel)
        .wait()
        .unwrap()
        .remove(0);
    stash_drop(sandbox.git(), &repository, &stash, &cancel)
        .wait()
        .unwrap();
    assert_eq!(
        stashes(sandbox.git(), &repository, &cancel).wait().unwrap(),
        Vec::new()
    );
    stash_store(
        sandbox.git(),
        &repository,
        stash.id,
        "On main: Work",
        &cancel,
    )
    .wait()
    .unwrap();
    let again = stashes(sandbox.git(), &repository, &cancel).wait().unwrap();
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].id, stash.id);
    assert_eq!(again[0].message, "On main: Work");
}

#[test]
fn refs_are_made_and_removed_only_where_expected() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "f", "one\n", "One").unwrap();
    sandbox.commit_file(&repo, "f", "two\n", "Two").unwrap();
    let repository = open(&sandbox, &repo);
    let cancel = CancelToken::new();
    let (one, two) = (rev(&sandbox, &repo, "HEAD~1"), rev(&sandbox, &repo, "HEAD"));
    let name = BStr::new("refs/tags/kept");
    create_ref(sandbox.git(), &repository, name, one, &cancel)
        .wait()
        .unwrap();
    assert_eq!(rev(&sandbox, &repo, "kept"), one);
    assert!(
        create_ref(sandbox.git(), &repository, name, two, &cancel)
            .wait()
            .is_err(),
        "it exists"
    );
    assert!(
        delete_ref(sandbox.git(), &repository, name, two, &cancel)
            .wait()
            .is_err(),
        "it isn't there"
    );
    delete_ref(sandbox.git(), &repository, name, one, &cancel)
        .wait()
        .unwrap();
    let gone = sandbox.run(&repo, "rev-parse", &["--verify", "--quiet", "kept"]);
    assert!(gone.is_err());
}
