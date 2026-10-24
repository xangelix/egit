//! Submodules against sandboxes: added on a branch, where each stands (nested ones too), moved
//! to their branch's latest and the commits that brings, recorded again, addresses changed and
//! synced, branches set and unset, deinit and set up again, removed.

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
    CancelToken, Checkout, ObjectId, Progress, Repository, SubmoduleChange, UpdateTo, Wait as _,
    add_submodule, change_submodules, remove_submodule, set_submodule_branch, set_submodule_url,
    submodule_commits, submodule_details, submodule_status, submodules, update_submodules_to,
};

fn rev(sandbox: &Sandbox, repo: &Path, name: &str) -> ObjectId {
    let out = sandbox.run(repo, "rev-parse", &[name]).unwrap();
    ObjectId::from_hex(out.stdout.trim_ascii()).unwrap()
}

/// A library with a submodule of its own (`inner`), and an app.
fn libraries(sandbox: &Sandbox) -> (PathBuf, PathBuf) {
    // Submodules over `file://` are refused unless allowed (git 2.38.1 on).
    sandbox
        .run(
            sandbox.home(),
            "config",
            &["--global", "protocol.file.allow", "always"],
        )
        .unwrap();
    let inner = sandbox.init("inner").unwrap();
    sandbox.commit_file(&inner, "i", "i\n", "Inner").unwrap();
    let library = sandbox.init("library").unwrap();
    sandbox
        .commit_file(&library, "lib", "one\n", "One")
        .unwrap();
    let url = format!("file://{}", inner.display());
    sandbox
        .run(&library, "submodule", &["--quiet", "add", &url, "inner"])
        .unwrap();
    sandbox.run(&library, "commit", &["-qm", "Inner"]).unwrap();
    let app = sandbox.init("app").unwrap();
    sandbox.commit_file(&app, "f", "f\n", "Start").unwrap();
    (library, app)
}

/// The app with the library added as a submodule at `libs/lib` (following `main`), its own
/// submodule set up, committed; and the commit recorded.
fn added(sandbox: &Sandbox) -> (PathBuf, PathBuf, Repository, ObjectId) {
    let (library, app) = libraries(sandbox);
    let git = sandbox.git();
    let cancel = CancelToken::new();
    let repository = Repository::discover(git, &app).wait().unwrap();
    let path = BStr::new("libs/lib");
    let url = format!("file://{}", library.display());
    add_submodule(
        git,
        &repository,
        &url,
        path,
        Some("main"),
        &Progress::new(),
        &cancel,
    )
    .wait()
    .unwrap();
    // Its own submodule set up too, as cloning one doesn't.
    update_submodules_to(
        git,
        &repository,
        &[],
        UpdateTo::Recorded,
        &Progress::new(),
        &cancel,
    )
    .wait()
    .unwrap();
    sandbox
        .run(&app, "commit", &["-qm", "Add the library"])
        .unwrap();
    let listed = submodules(git, &repository).wait().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].branch, Some(BString::from("main")));
    assert_eq!(listed[0].configured_url.as_ref(), listed[0].url.as_ref());
    let recorded = listed[0].recorded;

    (library, app, repository, recorded)
}

#[test]
fn submodules_are_added_followed_and_moved() {
    let sandbox = Sandbox::new().unwrap();
    let (library, app, repository, recorded) = added(&sandbox);
    let git = sandbox.git();
    let cancel = CancelToken::new();
    let path = BStr::new("libs/lib");
    // Where each stands, nested ones too.
    let states = submodule_status(git, &repository, true, &cancel)
        .wait()
        .unwrap();
    let paths: Vec<String> = states.iter().map(|state| state.path.to_string()).collect();
    assert_eq!(paths, ["libs/lib", "libs/lib/inner"]);
    assert!(
        states
            .iter()
            .all(|state| state.checkout == Checkout::AtRecorded)
    );
    let details = submodule_details(git, &repository, path, recorded)
        .wait()
        .unwrap();
    assert_eq!(details.head, Some(recorded));
    assert!(details.has_recorded);
    assert_eq!((details.ahead, details.behind), (0, 0));

    // Its branch moves on; updated to the latest, it is ahead of the commit recorded.
    sandbox
        .commit_file(&library, "lib", "two\n", "Two")
        .unwrap();
    update_submodules_to(
        git,
        &repository,
        &[path],
        UpdateTo::Latest,
        &Progress::new(),
        &cancel,
    )
    .wait()
    .unwrap();
    let states = submodule_status(git, &repository, false, &cancel)
        .wait()
        .unwrap();
    assert_eq!(states[0].checkout, Checkout::Elsewhere);
    let details = submodule_details(git, &repository, path, recorded)
        .wait()
        .unwrap();
    assert_eq!((details.ahead, details.behind), (1, 0));
    let head = details.head.unwrap();
    assert_eq!(head, rev(&sandbox, &library, "HEAD"));
    let brought = submodule_commits(git, &repository, path, (Some(recorded), head), 10)
        .wait()
        .unwrap()
        .unwrap();
    assert_eq!(brought.len(), 1);
    assert_eq!(brought[0].1, "Two");
    // Recorded again: at it.
    sandbox.run(&app, "add", &["libs/lib"]).unwrap();
    sandbox
        .run(&app, "commit", &["-qm", "Library two"])
        .unwrap();
    let states = submodule_status(git, &repository, false, &cancel)
        .wait()
        .unwrap();
    assert_eq!(states[0].checkout, Checkout::AtRecorded);
}

#[test]
fn submodules_addresses_branches_and_setup_are_mended() {
    let sandbox = Sandbox::new().unwrap();
    let (library, app, repository, _) = added(&sandbox);
    let git = sandbox.git();
    let cancel = CancelToken::new();
    let path = BStr::new("libs/lib");
    let url = format!("file://{}", library.display());
    // An address changed in .gitmodules (as a pull would), out of step until synced.
    let moved = sandbox.path("moved-library");
    std::fs::rename(&library, &moved).unwrap();
    let new_url = format!("file://{}", moved.display());
    sandbox
        .run(
            &app,
            "config",
            &["--file", ".gitmodules", "submodule.libs/lib.url", &new_url],
        )
        .unwrap();
    let listed = submodules(git, &repository).wait().unwrap();
    assert_ne!(listed[0].url, listed[0].configured_url);
    change_submodules(git, &repository, &[], &SubmoduleChange::Sync)
        .wait()
        .unwrap();
    let listed = submodules(git, &repository).wait().unwrap();
    assert_eq!(listed[0].configured_url.as_ref(), Some(&new_url.into()));
    // Changed through egit: both at once.
    set_submodule_url(
        git,
        &repository,
        path,
        &url.replace("library", "moved-library"),
    )
    .wait()
    .unwrap();
    let listed = submodules(git, &repository).wait().unwrap();
    assert_eq!(listed[0].url, listed[0].configured_url);

    // A branch set, then the remote's default again.
    set_submodule_branch(git, &repository, path, Some("release"))
        .wait()
        .unwrap();
    assert_eq!(
        submodules(git, &repository).wait().unwrap()[0].branch,
        Some(BString::from("release"))
    );
    set_submodule_branch(git, &repository, path, None)
        .wait()
        .unwrap();
    assert_eq!(submodules(git, &repository).wait().unwrap()[0].branch, None);

    // Forgotten here, then set up again.
    change_submodules(
        git,
        &repository,
        &[path],
        &SubmoduleChange::Deinit { force: true },
    )
    .wait()
    .unwrap();
    let states = submodule_status(git, &repository, false, &cancel)
        .wait()
        .unwrap();
    assert_eq!(states[0].checkout, Checkout::NotInitialized);
    assert!(!submodules(git, &repository).wait().unwrap()[0].initialized);
    update_submodules_to(
        git,
        &repository,
        &[path],
        UpdateTo::Recorded,
        &Progress::new(),
        &cancel,
    )
    .wait()
    .unwrap();
    let states = submodule_status(git, &repository, false, &cancel)
        .wait()
        .unwrap();
    assert_eq!(states[0].checkout, Checkout::AtRecorded);

    // Removed: gone from the index and .gitmodules, to commit.
    remove_submodule(git, &repository, path, false)
        .wait()
        .unwrap();
    assert_eq!(submodules(git, &repository).wait().unwrap(), []);
    let gitmodules = std::fs::read_to_string(app.join(".gitmodules")).unwrap_or_default();
    assert!(!gitmodules.contains("libs/lib"), "{gitmodules}");
}
