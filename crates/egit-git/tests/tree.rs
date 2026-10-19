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

#[test]
fn lists_the_folders_inside_new_ones_up_to_a_limit() {
    let sandbox = Sandbox::new().unwrap();
    let repo = sandbox.init("repo").unwrap();
    for folder in ["new/a b/deep", "new/c", "new/.git/objects", "other"] {
        std::fs::create_dir_all(repo.join(folder)).unwrap();
    }
    std::fs::write(repo.join("new/file"), "x\n").unwrap();
    let files = sandbox.git().files();
    let roots = [repo.join("new"), repo.join("gone"), repo.join("new/file")];
    let roots: Vec<&std::path::Path> = roots.iter().map(AsRef::as_ref).collect();

    let mut found = files.folders_within(&roots, 100).unwrap();
    found.sort();
    let expected: Vec<_> = ["new", "new/a b", "new/a b/deep", "new/c"]
        .iter()
        .map(|folder| repo.join(folder))
        .collect();
    // Neither git's own folder, nor what isn't a folder, nor what isn't there.
    assert_eq!(found, expected);
    assert_eq!(files.folders_within(&roots, 2).unwrap().len(), 2);
    assert_eq!(
        files.folders_within(&[], 10).unwrap(),
        Vec::<std::path::PathBuf>::new()
    );
}
