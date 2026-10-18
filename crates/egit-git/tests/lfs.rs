//! Git LFS against sandboxes: set up, tracking and untracking, the files it holds, pushed to a
//! bare repository through `file://` and pulled into a clone that had only their pointers,
//! pruned; locks, which need a server, none.
//!
//! git-lfs must be installed: without it the tests are skipped, unless `EGIT_REQUIRE_LFS` is set
//! (as CI sets it).

// The relaxations `clippy.toml` gives `#[test]` functions, for this test-only crate's helpers;
// a skipped test says so.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::print_stderr
)]

use std::path::Path;

use egit_git::testing::Sandbox;
use egit_git::{
    CancelToken, ConfigPlace, ConfigSnapshot, LfsTransfer, Progress, Repository, lfs_files,
    lfs_install, lfs_locks, lfs_patterns, lfs_prune, lfs_set_up, lfs_track, lfs_transfer,
    lfs_untrack, lfs_version,
};

/// Whether git-lfs is installed; without it a test is skipped, unless it is required.
fn available(sandbox: &Sandbox) -> bool {
    let found = lfs_version(sandbox.git()).unwrap().is_some();
    if !found {
        assert!(
            std::env::var_os("EGIT_REQUIRE_LFS").is_none(),
            "git-lfs is needed to test Git LFS"
        );
        eprintln!("skipped: git-lfs isn't installed");
    }
    found
}

fn set_up(sandbox: &Sandbox, repo: &Path) -> bool {
    let config = ConfigSnapshot::read(sandbox.git(), ConfigPlace::Repository(repo)).unwrap();
    lfs_set_up(&config)
}

#[test]
fn lfs_tracks_holds_pushes_and_pulls_files() {
    let sandbox = Sandbox::new().unwrap();
    if !available(&sandbox) {
        return;
    }
    let git = sandbox.git();
    let cancel = CancelToken::new();
    let repo = sandbox.init("repo").unwrap();
    let repository = Repository::discover(git, &repo).unwrap();
    assert!(!set_up(&sandbox, &repo));
    lfs_install(git, &repository).unwrap();
    assert!(set_up(&sandbox, &repo));

    // Tracking, lockable or not, and untracking.
    lfs_track(git, &repository, "*.bin", false).unwrap();
    lfs_track(git, &repository, "art/*.psd", true).unwrap();
    lfs_track(git, &repository, "*.tmp", false).unwrap();
    lfs_untrack(git, &repository, "*.tmp").unwrap();
    let patterns = lfs_patterns(git, &repository, &cancel).unwrap();
    let names: Vec<String> = patterns
        .iter()
        .map(|pattern| pattern.pattern.to_string())
        .collect();
    assert_eq!(names, ["*.bin", "art/*.psd"]);
    assert!(!patterns[0].lockable && patterns[1].lockable);
    assert!(patterns.iter().all(|pattern| pattern.tracked));
    assert_eq!(patterns[0].source, ".gitattributes");

    // Nothing held before a commit; then the file, here whole.
    assert_eq!(lfs_files(git, &repository, &cancel).unwrap(), []);
    let contents: Vec<u8> = (0..4096_u32).map(|n| n.to_le_bytes()[0] ^ 0x5a).collect();
    std::fs::write(repo.join("data.bin"), &contents).unwrap();
    sandbox
        .run(&repo, "add", &[".gitattributes", "data.bin"])
        .unwrap();
    sandbox.run(&repo, "commit", &["-m", "Data"]).unwrap();
    let files = lfs_files(git, &repository, &cancel).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "data.bin");
    assert_eq!(files[0].size, 4096);
    assert!(files[0].checked_out && files[0].downloaded);

    // Pushed to a bare repository's store, then pulled into a clone that had pointers.
    let bare = sandbox.path("origin.git");
    sandbox
        .run(
            sandbox.home(),
            "init",
            &["--quiet", "--bare", "-b", "main", bare.to_str().unwrap()],
        )
        .unwrap();
    let url = format!("file://{}", bare.display());
    sandbox
        .run(&repo, "remote", &["add", "origin", &url])
        .unwrap();
    sandbox
        .run(&repo, "push", &["--quiet", "origin", "main"])
        .unwrap();
    let push = LfsTransfer::Push {
        remote: "origin".into(),
    };
    lfs_transfer(git, &repository, &push, &Progress::new(), &cancel).unwrap();
    let clone = sandbox.path("clone");
    sandbox
        .run(
            sandbox.home(),
            "clone",
            &["--quiet", &url, clone.to_str().unwrap()],
        )
        .unwrap();
    let cloned = Repository::discover(git, &clone).unwrap();
    // No filters in the clone yet (the sandbox has no global ones): a pointer.
    assert_ne!(std::fs::read(clone.join("data.bin")).unwrap(), contents);
    lfs_install(git, &cloned).unwrap();
    let pointer = &lfs_files(git, &cloned, &cancel).unwrap()[0];
    assert!(!pointer.downloaded && !pointer.checked_out);
    lfs_transfer(git, &cloned, &LfsTransfer::Pull, &Progress::new(), &cancel).unwrap();
    assert_eq!(std::fs::read(clone.join("data.bin")).unwrap(), contents);
    let pulled = &lfs_files(git, &cloned, &cancel).unwrap()[0];
    assert!(pulled.downloaded && pulled.checked_out);

    // Pruning keeps what the checkout needs.
    lfs_prune(git, &cloned, &cancel).unwrap();
    assert!(lfs_files(git, &cloned, &cancel).unwrap()[0].downloaded);

    // Locks need an LFS server: a file:// remote has none, so nothing is locked there.
    assert_eq!(lfs_locks(git, &cloned, &cancel).unwrap(), []);
}
