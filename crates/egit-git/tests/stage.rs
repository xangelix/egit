//! Staging and unstaging whole files, against the real git.

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
use egit_git::{CancelToken, Repository, Status, stage, unstage};

fn sandbox() -> Sandbox {
    Sandbox::new().expect("a sandbox")
}

/// Each path in the status, with its `XY` as `git status --short` would show it.
fn short(sandbox: &Sandbox, repo: &Path) -> Vec<String> {
    let out = sandbox
        .run(
            repo,
            "status",
            &["--porcelain=v1", "-z", "--untracked-files=all"],
        )
        .unwrap();
    out.stdout
        .split(|&byte| byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| String::from_utf8_lossy(entry).into_owned())
        .collect()
}

fn paths(paths: &[&'static str]) -> Vec<&'static BStr> {
    paths.iter().map(|&path| BStr::new(path)).collect()
}

/// A repository with a modified file, a deleted one, and an untracked one with odd names.
fn changed(sandbox: &Sandbox) -> std::path::PathBuf {
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "kept", "one\n", "Kept").unwrap();
    sandbox.commit_file(&repo, "gone", "one\n", "Gone").unwrap();
    sandbox.commit_file(&repo, "*", "one\n", "A star").unwrap();
    std::fs::write(repo.join("kept"), "two\n").unwrap();
    std::fs::remove_file(repo.join("gone")).unwrap();
    std::fs::write(repo.join("new\tone [x]"), "x\n").unwrap();
    std::fs::write(repo.join("star-shaped"), "x\n").unwrap();
    repo
}

#[test]
fn stages_changed_deleted_and_new_files_by_their_names() {
    let sandbox = sandbox();
    let repo = changed(&sandbox);
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let cancel = CancelToken::new();

    stage(
        sandbox.git(),
        &repository,
        &paths(&["kept", "gone", "new\tone [x]"]),
        &cancel,
    )
    .unwrap();
    assert_eq!(
        short(&sandbox, &repo),
        ["D  gone", "M  kept", "A  new\tone [x]", "?? star-shaped"]
    );

    // A path is never a pattern: `*` names the file called `*`, untouched here.
    stage(sandbox.git(), &repository, &paths(&["*"]), &cancel).unwrap();
    assert_eq!(short(&sandbox, &repo).len(), 4);
    // And no paths stage nothing.
    stage(sandbox.git(), &repository, &[], &cancel).unwrap();
    assert_eq!(short(&sandbox, &repo).len(), 4);
}

#[test]
fn unstages_back_to_head_leaving_the_work_tree() {
    let sandbox = sandbox();
    let repo = changed(&sandbox);
    // Committed before the rest is staged: a commit takes the whole index.
    sandbox
        .commit_file(
            &repo,
            "old name",
            "long enough to be seen as renamed\n",
            "Old",
        )
        .unwrap();
    sandbox.run(&repo, "add", &["--all"]).unwrap();
    sandbox.run(&repo, "mv", &["old name", "new name"]).unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let cancel = CancelToken::new();

    // No paths would otherwise be everything: they unstage nothing.
    unstage(sandbox.git(), &repository, &[], &cancel).unwrap();
    assert_eq!(
        short(&sandbox, &repo),
        [
            "D  gone",
            "M  kept",
            "A  new\tone [x]",
            "R  new name",
            "old name",
            "A  star-shaped",
        ]
    );

    unstage(
        sandbox.git(),
        &repository,
        &paths(&["kept", "gone", "new\tone [x]", "new name", "old name"]),
        &cancel,
    )
    .unwrap();
    assert_eq!(
        short(&sandbox, &repo),
        [
            " D gone",
            " M kept",
            " D old name",
            "A  star-shaped",
            "?? new\tone [x]",
            "?? new name",
        ]
    );
    let status = Status::read(sandbox.git(), &repository, &cancel).unwrap();
    assert_eq!(
        status.entries().filter(|entry| entry.is_staged()).count(),
        1
    );
}

#[test]
fn unstages_in_a_repository_without_commits() {
    let sandbox = sandbox();
    let repo = sandbox.init("repo").unwrap();
    std::fs::write(repo.join("first"), "x\n").unwrap();
    sandbox.run(&repo, "add", &["first"]).unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();

    unstage(
        sandbox.git(),
        &repository,
        &paths(&["first"]),
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(short(&sandbox, &repo), ["?? first"]);
}
