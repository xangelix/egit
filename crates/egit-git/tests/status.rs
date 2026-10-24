//! The working tree's status, from the real git.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::Path;

use egit_git::testing::Sandbox;
use egit_git::{
    CancelToken, Change, Conflict, EntryKind, ObjectId, Repository, Status, StatusEntry, Wait as _,
};

fn sandbox() -> Sandbox {
    Sandbox::new().expect("a sandbox")
}

fn status(sandbox: &Sandbox, repo: &Path) -> Status {
    let repository = Repository::discover(sandbox.git(), repo).wait().unwrap();
    Status::read(sandbox.git(), &repository, &CancelToken::new())
        .wait()
        .unwrap()
}

/// The entry for `path`.
fn entry<'s>(status: &'s Status, path: &str) -> StatusEntry<'s> {
    status
        .entries()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("{path} isn't in the status"))
}

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> ObjectId {
    let out = sandbox.run(repo, "rev-parse", &[name]).unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

#[test]
fn reads_what_changed_in_the_index_and_the_work_tree() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    for name in ["kept", "staged", "both", "gone"] {
        sandbox.commit_file(&repo, name, "one\n", name).unwrap();
    }
    sandbox
        .commit_file(
            &repo,
            "old name",
            "a long enough file to be recognised when renamed\n",
            "Rename me",
        )
        .unwrap();
    std::fs::write(repo.join("kept"), "two\n").unwrap();
    std::fs::write(repo.join("staged"), "two\n").unwrap();
    std::fs::write(repo.join("both"), "two\n").unwrap();
    sandbox.run(&repo, "add", &["staged", "both"]).unwrap();
    std::fs::write(repo.join("both"), "three\n").unwrap();
    sandbox.run(&repo, "rm", &["--quiet", "gone"]).unwrap();
    sandbox.run(&repo, "mv", &["old name", "new name"]).unwrap();
    std::fs::create_dir(repo.join("new dir")).unwrap();
    std::fs::write(repo.join("new dir/a\tfile"), "x\n").unwrap();
    let status = status(&sandbox, &repo);

    let kept = entry(&status, "kept");
    assert_eq!(
        (kept.kind, kept.staged(), kept.unstaged()),
        (EntryKind::Changed, Change::Unmodified, Change::Modified)
    );
    assert_eq!(
        kept.index().unwrap().id,
        rev(&sandbox, &repo, ":kept"),
        "the index's blob, to stage lines against"
    );

    let staged = entry(&status, "staged");
    assert!(staged.is_staged() && !staged.is_unstaged());
    assert_eq!(
        staged.head().unwrap().id,
        rev(&sandbox, &repo, "HEAD:staged")
    );

    let both = entry(&status, "both");
    assert_eq!(
        (both.staged(), both.unstaged()),
        (Change::Modified, Change::Modified)
    );

    let gone = entry(&status, "gone");
    assert_eq!(gone.staged(), Change::Deleted);
    assert_eq!(gone.index(), None);

    let renamed = entry(&status, "new name");
    assert_eq!(renamed.kind, EntryKind::Renamed);
    assert_eq!(renamed.from.unwrap(), "old name");
    assert_eq!(renamed.similarity(), Some(100));

    // Every untracked file is listed on its own, its name as it is.
    let untracked = entry(&status, "new dir/a\tfile");
    assert_eq!(untracked.kind, EntryKind::Untracked);
    assert_eq!(status.len(), 6);
}

#[test]
fn reads_a_conflict() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, "shared", "base\n", "Base")
        .unwrap();
    sandbox
        .run(&repo, "switch", &["--quiet", "--create", "theirs"])
        .unwrap();
    sandbox
        .commit_file(&repo, "shared", "theirs\n", "Theirs")
        .unwrap();
    sandbox.run(&repo, "switch", &["--quiet", "-"]).unwrap();
    sandbox
        .commit_file(&repo, "shared", "ours\n", "Ours")
        .unwrap();
    assert!(sandbox.run(&repo, "merge", &["--quiet", "theirs"]).is_err());

    let status = status(&sandbox, &repo);
    let shared = entry(&status, "shared");
    assert_eq!(shared.kind, EntryKind::Unmerged);
    assert_eq!(shared.conflict(), Some(Conflict::BothModified));
    assert!(shared.is_unstaged() && !shared.is_staged());
}

#[test]
fn reads_a_repository_without_commits() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    std::fs::write(repo.join("added"), "x\n").unwrap();
    std::fs::write(repo.join("untracked"), "y\n").unwrap();
    sandbox.run(&repo, "add", &["added"]).unwrap();

    let status = status(&sandbox, &repo);
    let added = entry(&status, "added");
    assert_eq!(added.staged(), Change::Added);
    assert_eq!(added.head(), None);
    assert_eq!(entry(&status, "untracked").kind, EntryKind::Untracked);
}

#[test]
fn a_clean_work_tree_has_no_entries() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "file", "x\n", "Add").unwrap();
    assert!(status(&sandbox, &repo).is_empty());
}
