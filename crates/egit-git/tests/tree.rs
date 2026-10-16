//! A commit's folders and which paths git ignores, from the real git.

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use bstr::BStr;
use egit_git::testing::Sandbox;
use egit_git::{CancelToken, ObjectId, Repository, ignored_paths, tree_folders};

#[test]
fn lists_every_folder_of_a_commit() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox.commit_file(&repo, "top", "x\n", "Top").unwrap();
    sandbox
        .commit_file(&repo, "a/b c/deep", "x\n", "Deep")
        .unwrap();
    sandbox.commit_file(&repo, "d/file", "x\n", "D").unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let head = sandbox.run(&repo, "rev-parse", &["HEAD"]).unwrap();
    let head = ObjectId::from_hex(head.stdout.trim_ascii()).unwrap();

    let mut folders = tree_folders(sandbox.git(), &repository, head, &CancelToken::new()).unwrap();
    folders.sort();
    assert_eq!(folders, ["a", "a/b c", "d"]);
}

#[test]
fn says_which_paths_are_ignored_but_not_tracked_ones() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    sandbox
        .commit_file(&repo, ".gitignore", "build/\n*.log\n", "Ignore")
        .unwrap();
    std::fs::create_dir(repo.join("kept.log")).unwrap();
    std::fs::write(repo.join("kept.log/inside"), "x\n").unwrap();
    sandbox
        .run(&repo, "add", &["--force", "kept.log/inside"])
        .unwrap();
    let repository = Repository::discover(sandbox.git(), &repo).unwrap();
    let cancel = CancelToken::new();

    // Folders end with `/`, so that folder-only patterns match them.
    let paths = ["build/", "src/", "debug.log", "kept.log/inside"].map(BStr::new);
    let ignored = ignored_paths(sandbox.git(), &repository, &paths, &cancel).unwrap();
    assert_eq!(ignored, ["build/", "debug.log"]);
    assert_eq!(
        ignored_paths(sandbox.git(), &repository, &[BStr::new("src")], &cancel).unwrap(),
        Vec::<bstr::BString>::new()
    );
}
